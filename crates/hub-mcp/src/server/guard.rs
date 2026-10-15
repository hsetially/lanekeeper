//! The gate in front of `/mcp` (S4, S11, S14): host and origin, the bearer token, the user, and the session's owner.
//!
//! **Why a gate and not the hub's role guard.** The hub's `GuardedRouter` turns a failed `SessionAuth` into a plain 401, and an
//! OAuth client (Cursor) starts its sign-in from the `WWW-Authenticate` header of that 401, which only a layer *outside* the guard
//! can add. `GuardedRouter` has no such layer, so `/mcp` is registered as a route that proves its caller itself (`public`, with
//! the reason in the route table, as the GitHub webhook is), and this gate is that proof: it runs before the handler on every
//! method, and a request that does not pass it never reaches rmcp. `tests/route_table.rs` holds that line.
//!
//! Order, cheapest first: the declared body length, `Host`, `Origin`, the token and the user (401, 403 or 503, never a 200 for a
//! caller that is not allowed), then the session id against its owner.
//!
//! **Sessions.** rmcp keeps a session, and the channel an elicitation answer returns on, in one replica's memory. A session id is
//! never an authenticator: this gate binds each id to the user who opened it and answers a different user, or an id it did not
//! see created, as rmcp answers an unknown id (404), which does not reveal that the id exists. The binding covers `GET` (the
//! server-to-client stream) and `DELETE` (end the session), which never reach a handler (D91, rmcp 3.5.0). The handler holds the
//! binding too, as the second line.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use domain::{User, UserId};
use http::{HeaderValue, Method, StatusCode, header};
use hub_platform::{ApiError, SessionAuth};
use moka::sync::Cache;
use ports::AuthError;

use super::session::{SessionCounts, SessionLimit};
use crate::auth::BearerAuth;
use crate::auth::oauth::Challenge;
use crate::error::McpError;

/// The header that names an MCP session.
pub const SESSION_HEADER: &str = "mcp-session-id";
/// A session id longer than this is not one rmcp made (its ids are 36 characters).
const MAX_SESSION_ID_LEN: usize = 128;

/// The user behind an authenticated request, put in the request's extensions for the handler. Only this gate creates one.
#[derive(Debug, Clone)]
pub struct McpCaller(pub User);

/// Who opened each live session. Bounded, and idle entries expire a little after rmcp closes the session itself.
pub(crate) type SessionOwners = Cache<String, UserId>;

/// The memory of who opened which session. When an entry goes (the session was deleted, rmcp closed it, or the entry expired) the
/// owner's session count falls, so the count is the number of entries.
pub(crate) fn session_owners(capacity: u64, idle: Duration, counts: Arc<SessionCounts>) -> SessionOwners {
    Cache::builder()
        .max_capacity(capacity)
        .time_to_idle(idle)
        .eviction_listener(move |_key, owner: UserId, _cause| counts.release(&owner))
        .build()
}

/// Everything the gate needs, fixed at start-up.
pub(crate) struct GateState {
    pub auth: Arc<BearerAuth>,
    pub challenge: Challenge,
    /// Values `Host` may have, lower case.
    pub hosts: Vec<String>,
    /// Values `Origin` may have when it is sent at all.
    pub origins: Vec<String>,
    pub owners: SessionOwners,
    /// Sessions open or being opened, per user and in all.
    pub sessions: Arc<SessionCounts>,
    /// The largest request body, by its declared length (rmcp counts the bytes as they arrive).
    pub max_body: usize,
}

impl std::fmt::Debug for GateState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GateState").finish_non_exhaustive()
    }
}

fn problem(error: McpError) -> Response {
    error.api_error().into_response()
}

/// The answer to a request that did not authenticate: 401 with the challenge, 403, or 503.
fn refusal(state: &GateState, error: AuthError, token_presented: bool) -> Response {
    let mapped = McpError::from(error);
    let mut response = problem(mapped);
    if mapped == McpError::Unauthenticated
        && let Ok(value) = HeaderValue::from_str(state.challenge.value(token_presented))
    {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The `Host` of a request: the header, or the authority of the URI (HTTP/2 has no `Host` header).
fn host_of(request: &Request) -> Option<String> {
    let from_header = request
        .headers()
        .get(header::HOST)
        .map(|v| v.to_str().map(str::to_owned));
    match from_header {
        Some(Ok(host)) => Some(host),
        Some(Err(_)) => None,
        None => request.uri().authority().map(|a| a.as_str().to_owned()),
    }
}

fn declared_length(request: &Request) -> Option<u64> {
    request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
}

/// The gate. A [`Handler::layer`](axum::handler::Handler::layer) over the rmcp service, for every method of `/mcp`.
pub(crate) async fn gate(State(state): State<Arc<GateState>>, request: Request, next: Next) -> Response {
    // 1. Size, by what the request says it holds.
    if declared_length(&request).is_some_and(|len| len > state.max_body as u64) {
        return problem(McpError::PayloadTooLarge);
    }
    // 2. Host and Origin (DNS rebinding and cross-site requests). A request with another host, or an origin that is not ours,
    // is refused before its token is looked at.
    let host_ok =
        host_of(&request).is_some_and(|host| state.hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)));
    if !host_ok {
        return problem(McpError::Forbidden);
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let allowed = origin
            .to_str()
            .ok()
            .is_some_and(|origin| state.origins.iter().any(|o| o.eq_ignore_ascii_case(origin)));
        if !allowed {
            return problem(McpError::Forbidden);
        }
    }
    // 3. The token, then the user. `authenticate` re-reads the user's record on every request.
    let token_presented = request.headers().contains_key(header::AUTHORIZATION);
    let (parts, body) = request.into_parts();
    let user = match state.auth.authenticate(&parts).await {
        Ok(user) => user,
        Err(error) => return refusal(&state, error, token_presented),
    };
    let mut request = Request::from_parts(parts, body);
    // 4. The session id against its owner.
    let presented = match request.headers().get(SESSION_HEADER) {
        None => None,
        Some(value) => match value.to_str() {
            Ok(text) if !text.is_empty() && text.len() <= MAX_SESSION_ID_LEN => Some(text.to_owned()),
            // A session id that is not one rmcp could have made names no session.
            _ => return session_not_found(),
        },
    };
    if let Some(id) = &presented {
        match state.owners.get(id) {
            Some(owner) if owner == user.id => {}
            // Another user's session, or one this layer did not see opened (it ended, expired, or never existed).
            _ => return session_not_found(),
        }
    }
    let method = request.method().clone();
    // 5. A request that may open a session counts against the user's and the replica's limits. The count is taken before the session
    // exists, so two requests at the limit cannot both pass.
    let opens_session = method == Method::POST && presented.is_none();
    if opens_session && let Err(limit) = state.sessions.reserve(&user.id) {
        let mut response = problem(match limit {
            SessionLimit::User => McpError::TooManySessions,
            SessionLimit::Replica => McpError::Unavailable,
        });
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
        return response;
    }
    request.extensions_mut().insert(McpCaller(user.clone()));
    let response = next.run(request).await;
    match (&method, &presented) {
        (&Method::POST, None) => {
            let id = response
                .headers()
                .get(SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
                .filter(|_| response.status().is_success())
                .map(str::to_owned);
            match id {
                // The owner's entry now stands for the reserved count: it is released when the entry goes.
                Some(id) => state.owners.insert(id, user.id),
                None => state.sessions.release(&user.id),
            }
        }
        (&Method::DELETE, Some(id)) if response.status().is_success() => state.owners.invalidate(id),
        _ => {}
    }
    response
}

/// What rmcp answers for a session id it does not know.
fn session_not_found() -> Response {
    let mut response = ApiError::NotFound.into_response();
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}
