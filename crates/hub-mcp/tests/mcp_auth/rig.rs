//! A server built from fakes, and a client for it. One rig is one deployment: OAuth mode or personal-token mode.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, header};
use domain::{Guid, Role, ShortText, Timestamp, User, UserId, UserStatus};
use hub_mcp::auth::oauth::OAuthSettings;
use hub_mcp::auth::pat::{PersonalTokens, TokenSettings};
use hub_mcp::auth::store::MemoryTokenStore;
use hub_mcp::auth::token::HmacKey;
use hub_mcp::clock::{Clock, ManualClock, SystemClock};
use hub_mcp::config::{AuthMode, McpConfig, PublicOrigin};
use hub_mcp::{Credentials, McpDeps, McpSettings, McpState, mount};
use hub_platform::{FakeSessionAuth, GuardedRouter, RouteSpec, TEST_USER_HEADER};
use ports::fakes::{FakeAuditLog, FakeUsers};
use ports::{DocSearch, RegistryRead, WriteService};

use crate::spies::{SpyDocs, SpyRegistry, SpyWrites};
use ports::{AuthError, GoogleIdentity, TokenVerifier, VerifiedUser};
use tower::ServiceExt;

use crate::support::Client;

pub const HOST: &str = "hub.example.test";
pub const ORIGIN: &str = "https://hub.example.test";
pub const TENANT: &str = "11111111-2222-4333-8444-555555555555";
pub const AUDIENCE: &str = "api://hub-api";

/// The user id the fake session auth derives from a key. The same id is used everywhere in a rig, so a user is one user whether
/// it comes in with a cookie stand-in (the REST routes) or with a bearer token (`/mcp`).
pub fn uid(key: &str) -> UserId {
    hub_platform::session_auth::user_id(key)
}

pub fn short(text: &str) -> ShortText {
    ShortText::parse(text).unwrap()
}

pub fn user(key: &str, role: Option<Role>, status: UserStatus) -> User {
    User {
        id: uid(key),
        email: short(&format!("{key}@example.test")),
        display_name: short(key),
        role,
        status,
        requires_approval: false,
        github_login: None,
        last_seen_at: None,
    }
}

pub fn oauth_settings(mirror: bool) -> OAuthSettings {
    OAuthSettings {
        tenant_id: Guid::parse(TENANT).unwrap(),
        api_audience: AUDIENCE.to_owned(),
        authority_host: OAuthSettings::DEFAULT_AUTHORITY_HOST.to_owned(),
        authorization_server_mirror: mirror,
    }
}

pub fn settings(mode: AuthMode, mirror: bool) -> McpSettings {
    let config = McpConfig {
        auth_mode: mode,
        ..McpConfig::default()
    };
    let oauth = (mode == AuthMode::Oauth).then(|| oauth_settings(mirror));
    let mut settings = McpSettings::new(config, PublicOrigin::parse(ORIGIN).unwrap(), oauth);
    // No keep-alive timer: the tests run on a paused clock, and the timer of the SSE crate is not a tokio one.
    settings.sse_keep_alive = None;
    // The authentication tests open a session with nearly every request; the cap on sessions has tests of its own.
    settings.limits.sessions_per_user = 1_000;
    settings
}

/// A verifier that answers what it was told to, and counts its calls.
#[derive(Default)]
pub struct ScriptedVerifier {
    script: Mutex<HashMap<String, Result<VerifiedUser, AuthError>>>,
    otherwise: Mutex<Option<AuthError>>,
    calls: AtomicUsize,
}

impl std::fmt::Debug for ScriptedVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedVerifier").finish_non_exhaustive()
    }
}

impl ScriptedVerifier {
    pub fn accept(&self, token: &str, key: &str) {
        self.script.lock().unwrap().insert(
            token.to_owned(),
            Ok(VerifiedUser {
                id: uid(key),
                email: Some(short(&format!("{key}@example.test"))),
                display_name: Some(short(key)),
            }),
        );
    }

    pub fn refuse(&self, token: &str, error: AuthError) {
        self.script.lock().unwrap().insert(token.to_owned(), Err(error));
    }

    /// What every token that was not scripted gets (the default is `InvalidToken`).
    pub fn otherwise(&self, error: AuthError) {
        *self.otherwise.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl TokenVerifier for ScriptedVerifier {
    async fn entra_access_token(&self, jwt: &str) -> Result<VerifiedUser, AuthError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(answer) = self.script.lock().unwrap().get(jwt) {
            return answer.clone();
        }
        Err(self
            .otherwise
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(AuthError::InvalidToken))
    }

    async fn google_id_token(&self, _jwt: &str, _aud: &str) -> Result<GoogleIdentity, AuthError> {
        Err(AuthError::InvalidToken)
    }
}

pub struct Rig {
    pub state: Arc<McpState>,
    pub router: Router,
    pub routes: Vec<RouteSpec>,
    pub session_auth: Arc<FakeSessionAuth>,
    pub users: FakeUsers,
    /// The ports the server stands on: the fakes, with a window into them.
    pub registry: SpyRegistry,
    pub docs: SpyDocs,
    pub writes: SpyWrites,
    pub verifier: Arc<ScriptedVerifier>,
    pub clock: Arc<ManualClock>,
    pub audit: FakeAuditLog,
    pub store: Option<Arc<MemoryTokenStore>>,
    pub tokens: Option<Arc<PersonalTokens>>,
}

impl Rig {
    pub fn oauth() -> Self {
        Self::build(AuthMode::Oauth, false, TokenSettings::default(), |s| s)
    }

    pub fn oauth_with_mirror() -> Self {
        Self::build(AuthMode::Oauth, true, TokenSettings::default(), |s| s)
    }

    pub fn personal() -> Self {
        Self::build(AuthMode::PersonalToken, false, TokenSettings::default(), |s| s)
    }

    pub fn personal_with(tokens: TokenSettings) -> Self {
        Self::build(AuthMode::PersonalToken, false, tokens, |s| s)
    }

    pub fn build(
        mode: AuthMode,
        mirror: bool,
        token_settings: TokenSettings,
        tune: impl FnOnce(McpSettings) -> McpSettings,
    ) -> Self {
        let clock = Arc::new(ManualClock::at(SystemClock.now()));
        let users = FakeUsers::new();
        let verifier = Arc::new(ScriptedVerifier::default());
        let audit = FakeAuditLog::new();
        let (credentials, store, tokens) = match mode {
            AuthMode::Oauth => (
                Credentials::Oauth(Arc::clone(&verifier) as Arc<dyn TokenVerifier>),
                None,
                None,
            ),
            AuthMode::PersonalToken => {
                let store = Arc::new(MemoryTokenStore::new(Arc::clone(&clock) as Arc<dyn Clock>));
                let tokens = Arc::new(PersonalTokens::new(
                    Arc::clone(&store) as Arc<_>,
                    key(),
                    Arc::new(audit.clone()),
                    Arc::clone(&clock) as Arc<dyn Clock>,
                    token_settings,
                ));
                (
                    Credentials::PersonalToken(Arc::clone(&tokens)),
                    Some(store),
                    Some(tokens),
                )
            }
        };
        let registry = SpyRegistry::new();
        let docs = SpyDocs::new();
        let writes = SpyWrites::new();
        let state = McpState::new(
            McpDeps::new(
                credentials,
                Arc::new(users.clone()),
                Arc::new(registry.clone()) as Arc<dyn RegistryRead>,
                Arc::new(docs.clone()) as Arc<dyn DocSearch>,
                Arc::new(writes.clone()) as Arc<dyn WriteService>,
            )
            .with_clock(Arc::clone(&clock) as Arc<dyn Clock>),
            tune(settings(mode, mirror)),
        )
        .unwrap();
        let session_auth = Arc::new(FakeSessionAuth::new());
        let (router, routes) = mount(
            GuardedRouter::new(Arc::clone(&session_auth) as Arc<_>, None),
            &state,
        )
        .into_parts()
        .unwrap();
        Self {
            state,
            router,
            routes,
            session_auth,
            users,
            registry,
            docs,
            writes,
            verifier,
            clock,
            audit,
            store,
            tokens,
        }
    }

    /// Register a user with the bearer side (`Users`) and the cookie stand-in (the REST routes) at once.
    pub fn add_user(&self, key: &str, role: Option<Role>, status: UserStatus) -> UserId {
        self.users.insert(user(key, role, status));
        self.session_auth.set(key, role, status);
        uid(key)
    }

    /// A client that sends `bearer`, at the deployment's host.
    pub fn client(&self, bearer: Option<&str>) -> Client {
        Client::new(self.router.clone(), bearer.map(str::to_owned)).at_host(HOST)
    }

    /// One request to a REST route as the signed-in user `key`.
    pub async fn rest(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        origin: Option<&str>,
        body: Option<&str>,
    ) -> Response<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, HOST);
        if let Some(key) = key {
            builder = builder.header(TEST_USER_HEADER, key);
        }
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        let request = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned())),
            None => builder.body(Body::empty()),
        }
        .unwrap();
        self.router.clone().oneshot(request).await.unwrap()
    }
}

pub fn key() -> HmacKey {
    HmacKey::from_bytes(&[9_u8; 32]).unwrap()
}

/// How far past a token's end the tests move the clock.
pub const EIGHT_DAYS_SECS: i64 = 8 * 24 * 60 * 60;
pub const FIVE_SECONDS: Duration = Duration::from_secs(5);

pub fn ts(ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(ms)
}
