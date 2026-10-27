//! Who the request is (plan 18, T4; S1, S2, S4, P7): the platform's [`SessionAuth`] over the identity module's cookie sessions.
//!
//! The platform's guard (`hub_platform::GuardedRouter`) is the one every module's routes are registered through, and it asks a
//! [`SessionAuth`] for the active user behind a request. Identity authenticates by cookie, in a layer: [`AuthLayer`] reads the
//! `__Host-lk_session` cookie once per request, asks the session cache, and puts an [`AuthContext`] in the request. So that a
//! request is looked up once and not twice, [`CookieAuth`] reads that context and does not read the cookie itself.
//!
//! **What the guard decides on.** The role and the status in the context, which come from the session row joined with its user
//! and are evicted by the same `UserSessionsRevoked` event that ends the user's sessions (30 seconds at most, a second in
//! practice). The rest of the [`User`] (the e-mail and the name that go into an audit trail) comes from [`UserCache`], which may be
//! behind by a moment and decides nothing. A pending or disabled user is a `403` on every route, a request with no session a `401`.
//!
//! **Fail closed.** Without an [`AuthContext`] (the layer did not run, or found no session) every call is [`AuthError::InvalidToken`].
//! Anything that cannot be answered in time is [`AuthError::Unavailable`], which the guard answers with `503`, never with a pass.
//!
//! **Streams (G2).** A long-lived stream asks, on its heartbeat, whether the *session* that opened it is still live
//! ([`SessionAuth::binding_valid`], by the SHA-256 of the session id, which is what the sessions table keeps), so that signing out
//! of one browser ends that browser's stream and not the user's other ones.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{Role, User, UserId, UserStatus};
use http::HeaderMap;
use http::request::Parts;
use hub_identity::Clock;
use hub_identity::middleware::AuthContext;
use hub_identity::session::Sessions;
use hub_identity::session::cache::CachedSession;
use hub_identity::session::cookie::{RandomToken, RequestCookies, TokenHash};
use hub_platform::SessionAuth;
use hub_platform::session_auth::StreamBinding;
use ports::{AuthError, Users};

use crate::user_cache::UserCache;

/// How long a question to the user directory or the session table may take before the answer is "unavailable".
const AUTH_TIMEOUT: Duration = Duration::from_secs(3);

/// `SessionAuth` over the identity module's cookie sessions.
pub struct CookieAuth {
    sessions: Arc<Sessions>,
    users: Arc<dyn Users>,
    cache: Arc<UserCache>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for CookieAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieAuth").finish_non_exhaustive()
    }
}

impl CookieAuth {
    #[must_use]
    pub fn new(
        sessions: Arc<Sessions>,
        users: Arc<dyn Users>,
        cache: Arc<UserCache>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            sessions,
            users,
            cache,
            clock,
        }
    }

    /// The user's profile, from memory or from the directory. `UnknownUser` when there is none.
    async fn profile(&self, id: &UserId) -> Result<Arc<User>, AuthError> {
        let found = tokio::time::timeout(
            AUTH_TIMEOUT,
            self.cache.get_or_load(id, || async {
                self.users.get(id).await.map_err(|_| AuthError::Unavailable)
            }),
        )
        .await
        .map_err(|_| AuthError::Unavailable)??;
        found.ok_or(AuthError::UnknownUser)
    }
}

#[async_trait]
impl SessionAuth for CookieAuth {
    async fn authenticate(&self, parts: &Parts) -> Result<User, AuthError> {
        let context = parts
            .extensions
            .get::<AuthContext>()
            .ok_or(AuthError::InvalidToken)?;
        // Pending and disabled users never pass, whatever their role (D36, S4).
        match context.status {
            UserStatus::Pending => return Err(AuthError::Pending),
            UserStatus::Disabled => return Err(AuthError::Disabled),
            UserStatus::Active => {}
        }
        if context.role.is_none() {
            return Err(AuthError::InsufficientRole);
        }
        let profile = self.profile(&context.user).await?;
        // The authorization is the session's, not the cache's.
        let mut user = User::clone(&profile);
        user.role = context.role;
        user.status = context.status;
        Ok(user)
    }

    async fn still_valid(&self, user: &UserId) -> bool {
        // A user the directory cannot vouch for in time is not valid: a stream is cut rather than kept open on a guess.
        self.profile(user)
            .await
            .is_ok_and(|profile| profile.allows(Role::Viewer))
    }

    fn stream_binding(&self, headers: &HeaderMap) -> Option<StreamBinding> {
        let cookies = RequestCookies::parse(
            headers
                .get_all(http::header::COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok()),
        )
        .ok()?;
        let token = RandomToken::from_text(&cookies.session?)?;
        Some(StreamBinding::new(*token.hash().as_bytes()))
    }

    async fn binding_valid(&self, binding: &StreamBinding) -> bool {
        let hash = TokenHash::from_bytes(*binding.as_bytes());
        // Through the session cache, so that the many streams of one session cost one read, and a logout (which evicts the user's
        // sessions on every replica) is seen here as the request path sees it.
        let found = tokio::time::timeout(
            AUTH_TIMEOUT,
            self.sessions.cache().get_or_load(hash, || async {
                let loaded_at = self.clock.now();
                Ok(self
                    .sessions
                    .store()
                    .lookup(&hash)
                    .await?
                    .map(|row| CachedSession {
                        user: row.user,
                        role: row.role,
                        status: row.status,
                        csrf_hash: row.csrf_hash,
                        last_seen_at: row.last_seen_at,
                        expires_at: row.expires_at,
                        loaded_at,
                    }))
            }),
        )
        .await;
        // No answer in time, a failed read and a session that is gone are all "no": a stream stays open on a positive answer only.
        // The idle limit is not judged here: it is 8 hours and a stream lives 30 minutes, and the request that reopens it is judged.
        let Ok(Ok(Some(session))) = found else {
            return false;
        };
        session.status == UserStatus::Active
            && session.role.is_some()
            && self.clock.now().unix_millis() < session.expires_at.unix_millis()
    }
}

/// Authenticates nobody. For the tests of routers that have no identity module behind them (the platform's cluster tests): every
/// request is "no valid session" and no stream stays open. Not constructible in a release build.
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Copy, Default)]
pub struct DenySessions;

#[cfg(feature = "testing")]
#[async_trait]
impl SessionAuth for DenySessions {
    async fn authenticate(&self, _parts: &Parts) -> Result<User, AuthError> {
        Err(AuthError::InvalidToken)
    }

    async fn still_valid(&self, _user: &UserId) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use domain::{Guid, ShortText};
    use hub_identity::ManualClock;
    use hub_identity::session::SessionPolicy;
    use ports::UserError;
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    fn id(n: u32) -> UserId {
        UserId::new(
            Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        )
    }

    fn user(n: u32, role: Option<Role>, status: UserStatus) -> User {
        User {
            id: id(n),
            email: ShortText::parse(&format!("user{n}@example.test")).unwrap(),
            display_name: ShortText::parse(&format!("User {n}")).unwrap(),
            role,
            status,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        }
    }

    /// A directory that answers from a table and counts the reads. Its answer for user 1 is `Editor`, whatever the session says.
    #[derive(Default)]
    struct Directory {
        reads: std::sync::atomic::AtomicUsize,
        broken: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl Users for Directory {
        async fn get(&self, user: &UserId) -> Result<Option<User>, UserError> {
            use std::sync::atomic::Ordering::SeqCst;
            self.reads.fetch_add(1, SeqCst);
            if self.broken.load(SeqCst) {
                return Err(UserError::Unavailable);
            }
            Ok((*user == id(1)).then(|| user_at(1)))
        }

        async fn require_active(&self, _id: &UserId, _min: Role) -> Result<User, AuthError> {
            Err(AuthError::Unavailable)
        }
    }

    fn user_at(n: u32) -> User {
        user(n, Some(Role::Editor), UserStatus::Active)
    }

    /// A `Sessions` that never touches the database: its pool never connects, and these tests never ask it anything.
    fn auth(directory: Arc<Directory>) -> CookieAuth {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::at(domain::Timestamp::from_unix_millis(
            1_800_000_000_000,
        )));
        let sessions = Arc::new(Sessions::new(pool, Arc::clone(&clock), &SessionPolicy::default()));
        CookieAuth::new(
            sessions,
            directory,
            Arc::new(UserCache::new(100, Duration::from_secs(30))),
            clock,
        )
    }

    fn parts_with(context: Option<AuthContext>) -> Parts {
        let (mut parts, ()) = http::Request::builder()
            .uri("/api/v1/anything")
            .body(())
            .unwrap()
            .into_parts();
        if let Some(context) = context {
            parts.extensions.insert(context);
        }
        parts
    }

    fn context(n: u32, role: Option<Role>, status: UserStatus) -> AuthContext {
        AuthContext {
            user: id(n),
            role,
            status,
            session_hash: TokenHash::from_bytes([1; 32]),
            csrf_hash: TokenHash::from_bytes([2; 32]),
            request_id: None,
        }
    }

    #[tokio::test]
    async fn a_request_the_layer_found_no_session_for_is_not_authenticated() {
        let auth = auth(Arc::default());
        // No cookie, a header that says anything, a bearer token: none of it is read here.
        let (mut parts, ()) = http::Request::builder()
            .uri("/api/v1/me")
            .header("cookie", "__Host-lk_session=anything")
            .header("authorization", "Bearer anything")
            .header("x-test-user", "alice")
            .body(())
            .unwrap()
            .into_parts();
        assert_eq!(
            auth.authenticate(&parts).await.unwrap_err(),
            AuthError::InvalidToken
        );
        parts.extensions.clear();
        assert_eq!(
            auth.authenticate(&parts).await.unwrap_err(),
            AuthError::InvalidToken
        );
    }

    #[tokio::test]
    async fn pending_disabled_and_roleless_users_are_refused_before_the_directory_is_asked() {
        let directory = Arc::new(Directory::default());
        let auth = auth(Arc::clone(&directory));
        for (role, status, want) in [
            (None, UserStatus::Pending, AuthError::Pending),
            (Some(Role::Admin), UserStatus::Pending, AuthError::Pending),
            (Some(Role::Admin), UserStatus::Disabled, AuthError::Disabled),
            (None, UserStatus::Disabled, AuthError::Disabled),
            (None, UserStatus::Active, AuthError::InsufficientRole),
        ] {
            let parts = parts_with(Some(context(1, role, status)));
            assert_eq!(
                auth.authenticate(&parts).await.unwrap_err(),
                want,
                "{role:?} {status:?}"
            );
        }
        assert_eq!(directory.reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_role_and_the_status_are_the_sessions_and_the_profile_is_the_directorys() {
        let directory = Arc::new(Directory::default());
        let auth = auth(Arc::clone(&directory));
        // The directory says Editor; the session, which was evicted at the last role change, says Viewer. The session decides.
        let parts = parts_with(Some(context(1, Some(Role::Viewer), UserStatus::Active)));
        let got = auth.authenticate(&parts).await.unwrap();
        assert_eq!(got.role, Some(Role::Viewer));
        assert_eq!(got.status, UserStatus::Active);
        assert_eq!(got.email.as_str(), "user1@example.test");
        assert_eq!(got.id, id(1));
    }

    #[tokio::test]
    async fn the_directory_is_read_once_for_many_requests() {
        let directory = Arc::new(Directory::default());
        let auth = auth(Arc::clone(&directory));
        let parts = parts_with(Some(context(1, Some(Role::Editor), UserStatus::Active)));
        for _ in 0..10 {
            auth.authenticate(&parts).await.unwrap();
        }
        assert_eq!(directory.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_session_whose_user_the_directory_does_not_know_is_unknown_and_a_broken_directory_is_unavailable()
     {
        let directory = Arc::new(Directory::default());
        let auth = auth(Arc::clone(&directory));
        let stranger = parts_with(Some(context(9, Some(Role::Viewer), UserStatus::Active)));
        assert_eq!(
            auth.authenticate(&stranger).await.unwrap_err(),
            AuthError::UnknownUser
        );
        directory.broken.store(true, std::sync::atomic::Ordering::SeqCst);
        let known = parts_with(Some(context(1, Some(Role::Viewer), UserStatus::Active)));
        assert_eq!(
            auth.authenticate(&known).await.unwrap_err(),
            AuthError::Unavailable
        );
    }

    #[tokio::test]
    async fn a_stream_stays_valid_only_for_a_user_the_directory_vouches_for() {
        let directory = Arc::new(Directory::default());
        let auth = auth(Arc::clone(&directory));
        assert!(auth.still_valid(&id(1)).await);
        assert!(!auth.still_valid(&id(9)).await, "an unknown user");
        directory.broken.store(true, std::sync::atomic::Ordering::SeqCst);
        auth.cache.flush();
        assert!(!auth.still_valid(&id(1)).await, "no answer is no");
    }

    #[tokio::test]
    async fn the_stream_binding_is_the_hash_of_the_session_cookie_and_nothing_else() {
        let auth = auth(Arc::default());
        let token = RandomToken::generate().unwrap();
        let text = token.to_text();
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            format!("other=1; __Host-lk_session={}; __Host-lk_csrf=zzz", text.expose())
                .parse()
                .unwrap(),
        );
        let binding = auth.stream_binding(&headers).unwrap();
        assert_eq!(binding.as_bytes(), token.hash().as_bytes());
        // Not a session cookie, a duplicated one, a malformed one, no cookie at all: no binding, so no stream.
        for bad in [
            "other=1",
            "__Host-lk_session=not-a-token",
            &format!("__Host-lk_session={0}; __Host-lk_session={0}", text.expose()),
            "__Host-lk_session=",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(http::header::COOKIE, bad.parse().unwrap());
            assert!(auth.stream_binding(&headers).is_none(), "{bad}");
        }
        assert!(auth.stream_binding(&HeaderMap::new()).is_none());
        // And it never prints.
        assert!(!format!("{binding:?}").contains(&format!("{:?}", binding.as_bytes())));
    }
}
