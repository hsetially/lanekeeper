//! `/auth/login`, `/auth/callback` and `/auth/logout` (T3; S1, S2, S3, S9).
//!
//! - **login** starts the Entra sign-in: a redirect with state, nonce and PKCE, and the sealed login cookie (T1).
//! - **callback** finishes it: the id token is verified (T1), the user is created pending on first sign-in or refreshed (T2),
//!   and a new session is made. Any session the browser presented is deleted in the same transaction, and nothing the browser
//!   sent is ever adopted as the new session id, so a session id planted before sign-in is worthless (fixation). The sign-in
//!   writes one `SessionSignedIn` audit event as the last statement before the commit. The answer is a redirect with the
//!   session and CSRF cookies; a pending or disabled user still gets a session, and is sent to the page that explains it.
//! - **logout** ends the session the request carries. It is idempotent: with no session, or one that is gone, it still answers
//!   204 and clears the cookies. With a session it must have passed the CSRF layer (compose it behind
//!   [`crate::csrf::CsrfLayer`]; the guard in T4 does), and it writes one `SessionSignedOut` audit event and publishes the
//!   revocation, so that no replica keeps serving the session from its cache.
//!
//! Every failure is a generic `application/problem+json` (S21); the reason is logged by variant. The three routes are open
//! by design (S1): no session exists before sign-in, and logout has nothing to end without one. T4 registers them as such.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use domain::{RequestId, UserId, UserStatus};
use ports::{AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, EventBus, Tx};
use sqlx::PgPool;

use crate::clock::Clock;
use crate::error::IdentityError;
use crate::middleware::AuthContext;
use crate::oidc::login::{CallbackQuery, LOGIN_COOKIE_NAME, LOGIN_LIFETIME_SECS, OidcLogin};
use crate::problem::problem;
use crate::session::cookie::{
    CSRF_COOKIE, RandomToken, RequestCookies, SESSION_COOKIE, TokenHash, clear_cookie, csrf_set_cookie,
    login_set_cookie, session_set_cookie,
};
use crate::session::revocation::publish_revoked;
use crate::session::store::NewSession;
use crate::session::{SessionError, Sessions};
use crate::users::{AccessError, PgUsers};

/// Where a user who is waiting for a role is sent after sign-in (prompt 08 builds the page).
pub const ACCESS_PENDING_PATH: &str = "/access-requested";
/// Where a disabled user is sent after sign-in (prompt 08 builds the page).
pub const ACCESS_DISABLED_PATH: &str = "/access-disabled";

/// The longest `/auth/login` query read.
const MAX_LOGIN_QUERY_BYTES: usize = 2048;
/// A sign-in or logout transaction is cut off here; a dropped future rolls back.
const TX_TIMEOUT: Duration = Duration::from_secs(10);

/// What the routes need from the rest of the hub.
#[derive(Clone)]
pub struct AuthState {
    inner: Arc<Inner>,
}

struct Inner {
    login: OidcLogin,
    users: PgUsers,
    sessions: Arc<Sessions>,
    pool: PgPool,
    audit: Arc<dyn AuditLog>,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState").finish_non_exhaustive()
    }
}

impl AuthState {
    pub fn new(
        login: OidcLogin,
        users: PgUsers,
        sessions: Arc<Sessions>,
        pool: PgPool,
        audit: Arc<dyn AuditLog>,
        bus: Arc<dyn EventBus>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                login,
                users,
                sessions,
                pool,
                audit,
                bus,
                clock,
            }),
        }
    }
}

/// The three routes. Compose them behind `AuthLayer` and `CsrfLayer` (the guard in T4 does).
pub fn auth_routes(state: AuthState) -> Router {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
        .with_state(state)
}

fn request_id(req: &Request) -> Option<RequestId> {
    req.extensions().get::<RequestId>().cloned()
}

/// The request's cookies, or none if they cannot be read.
fn cookies(req: &Request) -> RequestCookies {
    RequestCookies::parse(
        req.headers()
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()),
    )
    .unwrap_or_default()
}

fn set_cookie(response: &mut Response, line: &str) {
    if let Ok(mut value) = HeaderValue::from_str(line) {
        value.set_sensitive(true);
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn no_store(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn redirect(to: &str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    match HeaderValue::from_str(to) {
        Ok(v) => {
            response.headers_mut().insert(header::LOCATION, v);
        }
        Err(_) => *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR,
    }
    no_store(&mut response);
    response
}

fn login_failure(e: IdentityError, id: Option<&RequestId>) -> Response {
    tracing::warn!(error = %e, request_id = id.map(ToString::to_string), "sign-in failed");
    match e {
        IdentityError::BadRequest
        | IdentityError::LoginStateInvalid
        | IdentityError::LoginStateExpired
        | IdentityError::StateMismatch => problem(
            StatusCode::BAD_REQUEST,
            "sign-in-invalid",
            "Sign-in request is not valid",
            id,
        ),
        IdentityError::Token(_) => problem(
            StatusCode::FORBIDDEN,
            "sign-in-refused",
            "Sign-in was refused",
            id,
        ),
        IdentityError::TokenEndpoint
        | IdentityError::Http(_)
        | IdentityError::Discovery(_)
        | IdentityError::Jwks => problem(
            StatusCode::BAD_GATEWAY,
            "identity-provider",
            "Sign-in service is unavailable",
            id,
        ),
        IdentityError::Config(_)
        | IdentityError::Secret(_)
        | IdentityError::ClientAssertion
        | IdentityError::Crypto => problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Something went wrong",
            id,
        ),
    }
}

fn unavailable(what: &str, id: Option<&RequestId>) -> Response {
    tracing::warn!(
        what,
        request_id = id.map(ToString::to_string),
        "sign-in step failed"
    );
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Service temporarily unavailable",
        id,
    )
}

// ---------------------------------------------------------------------------------------------------------------
// login

async fn login(State(state): State<AuthState>, req: Request) -> Response {
    let id = request_id(&req);
    let query = req.uri().query().unwrap_or("").to_owned();
    drop(req);
    if query.len() > MAX_LOGIN_QUERY_BYTES {
        return login_failure(IdentityError::BadRequest, id.as_ref());
    }
    let return_to = url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "return_to")
        .map(|(_, v)| v.into_owned());
    match state.inner.login.begin(return_to.as_deref()).await {
        Ok(start) => {
            let mut response = redirect(start.redirect_to.as_str());
            set_cookie(
                &mut response,
                &login_set_cookie(start.cookie.as_str(), LOGIN_LIFETIME_SECS),
            );
            response
        }
        Err(e) => login_failure(e, id.as_ref()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// callback

async fn callback(State(state): State<AuthState>, req: Request) -> Response {
    let id = request_id(&req);
    let presented = cookies(&req);
    // A request body is not `Sync`: take what is needed before the first await.
    let query = req.uri().query().unwrap_or("").to_owned();
    drop(req);
    let mut response = finish_sign_in(&state, &query, &presented, id.as_ref()).await;
    // The login state is single use whatever happened.
    set_cookie(&mut response, &clear_cookie(LOGIN_COOKIE_NAME));
    no_store(&mut response);
    response
}

async fn finish_sign_in(
    state: &AuthState,
    raw_query: &str,
    presented: &RequestCookies,
    id: Option<&RequestId>,
) -> Response {
    let s = &state.inner;
    let query = match CallbackQuery::parse(raw_query) {
        Ok(q) => q,
        Err(e) => return login_failure(e, id),
    };
    let signed = match s.login.complete(&query, presented.login.as_deref()).await {
        Ok(signed) => signed,
        Err(e) => return login_failure(e, id),
    };
    let known = match s.users.first_sign_in(&signed, id).await {
        Ok(known) => known,
        Err(e) => return unavailable(access_error_name(e), id),
    };
    // Nothing the browser sent becomes the new session; the one it presented, if any, is deleted.
    let old = presented
        .session
        .as_deref()
        .and_then(RandomToken::from_text)
        .map(|t| t.hash());
    let session = match tokio::time::timeout(TX_TIMEOUT, establish(s, &known.user.id, old, id)).await {
        Ok(Ok(session)) => session,
        Ok(Err(e)) => return unavailable(session_error_name(e), id),
        Err(_) => return unavailable("timeout", id),
    };
    let to = match known.user.status {
        UserStatus::Active => signed.return_to.as_str(),
        UserStatus::Pending => ACCESS_PENDING_PATH,
        UserStatus::Disabled => ACCESS_DISABLED_PATH,
    };
    let mut response = redirect(to);
    let max_age = i64::try_from(s.sessions.store_lifetime().as_secs()).unwrap_or(i64::MAX);
    set_cookie(&mut response, &session_set_cookie(&session.session, max_age));
    set_cookie(&mut response, &csrf_set_cookie(&session.csrf, max_age));
    response
}

/// The sign-in transaction: delete the presented session, create the new one, publish the revocation of the old one, and
/// record `SessionSignedIn` last.
async fn establish(
    s: &Inner,
    user: &UserId,
    old: Option<TokenHash>,
    id: Option<&RequestId>,
) -> Result<NewSession, EstablishError> {
    let now = s.clock.now();
    let mut tx = s
        .pool
        .begin()
        .await
        .map_err(|_| EstablishError::Session(SessionError::Unavailable))?;
    let store = s.sessions.store();
    let replaced = match old {
        Some(hash) => store.delete_in(&mut tx, &hash).await?,
        None => false,
    };
    let session = store.create_in(&mut tx, user).await?;
    if replaced {
        // The session that was replaced may be cached on other replicas.
        publish_revoked(s.bus.as_ref(), &mut Tx::from_pg(&mut tx), user)
            .await
            .map_err(|_| EstablishError::Bus)?;
    }
    let mut event = AuditEvent::new(
        now,
        AuditActor::User { user: user.clone() },
        AuditAction::SessionSignedIn,
        AuditVia::Ui,
    );
    event.request_id = id.cloned();
    if replaced {
        event.diff = Some("replaced the session the browser presented".to_owned());
    }
    // The last statement before the commit: the audit chain lock is held from here to there.
    s.audit
        .record(&mut Tx::from_pg(&mut tx), event)
        .await
        .map_err(|_| EstablishError::Audit)?;
    tx.commit()
        .await
        .map_err(|_| EstablishError::Session(SessionError::Unavailable))?;
    if replaced {
        // This replica need not wait for the event it just published; the others get it from the bus.
        s.sessions.cache().evict_user(user);
    }
    Ok(session)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EstablishError {
    Session(SessionError),
    Bus,
    Audit,
}

impl From<SessionError> for EstablishError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}

fn access_error_name(e: AccessError) -> &'static str {
    match e {
        AccessError::Audit(_) => "user audit",
        AccessError::Bus(_) => "user event",
        _ => "user store",
    }
}

fn session_error_name(e: EstablishError) -> &'static str {
    match e {
        EstablishError::Session(_) => "session store",
        EstablishError::Bus => "session event",
        EstablishError::Audit => "session audit",
    }
}

// ---------------------------------------------------------------------------------------------------------------
// logout

async fn logout(State(state): State<AuthState>, req: Request) -> Response {
    let s = &state.inner;
    let id = request_id(&req);
    // Only a context the auth layer made counts: a cookie that names no live session ends nothing and is just cleared.
    let context = req.extensions().get::<AuthContext>().cloned();
    drop(req);
    let ended = match context {
        None => Ok(false),
        Some(ctx) => match tokio::time::timeout(TX_TIMEOUT, end(s, &ctx, id.as_ref())).await {
            Ok(r) => r,
            Err(_) => Err(EstablishError::Session(SessionError::Unavailable)),
        },
    };
    if let Err(e) = ended {
        return unavailable(session_error_name(e), id.as_ref());
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    set_cookie(&mut response, &clear_cookie(SESSION_COOKIE));
    set_cookie(&mut response, &clear_cookie(CSRF_COOKIE));
    no_store(&mut response);
    response
}

/// End the session in `ctx`. True when this call deleted it. Writes one `SessionSignedOut` event only if it did.
async fn end(s: &Inner, ctx: &AuthContext, id: Option<&RequestId>) -> Result<bool, EstablishError> {
    let now = s.clock.now();
    let mut tx = s
        .pool
        .begin()
        .await
        .map_err(|_| EstablishError::Session(SessionError::Unavailable))?;
    if !s.sessions.store().delete_in(&mut tx, &ctx.session_hash).await? {
        // Gone already (another tab, another replica): nothing changed, nothing to audit.
        return Ok(false);
    }
    publish_revoked(s.bus.as_ref(), &mut Tx::from_pg(&mut tx), &ctx.user)
        .await
        .map_err(|_| EstablishError::Bus)?;
    let mut event = AuditEvent::new(
        now,
        AuditActor::User {
            user: ctx.user.clone(),
        },
        AuditAction::SessionSignedOut,
        AuditVia::Ui,
    );
    event.request_id = id.cloned();
    s.audit
        .record(&mut Tx::from_pg(&mut tx), event)
        .await
        .map_err(|_| EstablishError::Audit)?;
    tx.commit()
        .await
        .map_err(|_| EstablishError::Session(SessionError::Unavailable))?;
    // This replica need not wait for the event it just published; the others get it from the bus.
    s.sessions.cache().evict_user(&ctx.user);
    Ok(true)
}
