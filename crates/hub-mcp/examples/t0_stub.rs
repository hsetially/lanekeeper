//! The T0 compatibility stub (plan 07, T0; A4, A19; D52).
//!
//! A tiny MCP server on rmcp's Streamable HTTP transport with exactly two tools, `t0_read` (read only) and `t0_write` (asks the
//! user through MCP elicitation, and writes nothing whatever the answer). A human points Cursor 3.22.12 at it, on Windows and on
//! macOS, and fills in the results table of `crates/hub-mcp/t0/README.md` and D91 in `docs/decisions.md`.
//!
//! **This is a test tool, not a server.** In OAuth mode it reads the claims of the bearer token without verifying the token,
//! because its job is to show what Cursor and Entra put in it. It therefore listens on loopback only and refuses any other
//! address unless `--allow-non-loopback` is given. It lives in `examples/` and not in the library, so that nothing which
//! trusts an unverified token can reach the hub. `T1` replaces the token reading with `TokenVerifier::entra_access_token`.
//!
//! Settings (environment, `LK_*`; `crates/hub-mcp/t0/README.md` has copy-and-paste blocks):
//!
//! | Variable | Meaning |
//! |---|---|
//! | `LK_MCP_AUTH`, `LK_MCP_CONFIRM`, `LK_MCP_CONTENT` | the switches of `hub_mcp::config` |
//! | `LK_T0_BIND` | listen address, default `127.0.0.1:8799` |
//! | `LK_T0_PUBLIC_URL` | the URL Cursor uses, default `http://127.0.0.1:8799` (no path) |
//! | `LK_T0_TENANT_ID`, `LK_T0_API_AUDIENCE` | OAuth mode: the Entra tenant and `api://<hub-api>` |
//! | `LK_T0_AS_MIRROR` | OAuth mode: `true` also serves `/.well-known/oauth-authorization-server` |
//! | `LK_T0_PAT` | personal-token mode: the one token the stub accepts (16 characters or more) |
//! | `LK_T0_ELICIT_TIMEOUT_SECS` | how long to wait for an answer, default 120 |
//! | `LK_T0_SESSION_CAP` | most live sessions, default 8 |
//!
//! The test files `tests/stub_protocol.rs` and `tests/rmcp_spike.rs` include this file as a module (`#[path]`) and drive it in
//! process, so what the human runs is what the tests prove.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::{HashMap, VecDeque};
use std::fmt::{self, Write as _};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::Secret;
use hub_mcp::config::{AuthMode, ConfigError, ConfirmMode, McpConfig};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelledNotificationParam,
    ClientJsonRpcMessage, ClientResult, ContentBlock, ElicitRequest, ElicitRequestParams, ElicitResult,
    ElicitationAction, ElicitationSchema, Implementation, InitializeRequestParams, InitializeResult,
    JsonObject, ListToolsResult, PaginatedRequestParams, PrimitiveSchemaDefinition, ServerCapabilities,
    ServerConfig, ServerJsonRpcMessage, ServerRequest, Tool,
};
use rmcp::service::{ElicitationMode, PeerRequestOptions, RequestContext};
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{
    SessionId, SessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// Where the stub listens unless `LK_T0_BIND` says otherwise. Not 8787: Q21 gives Cursor's public client the redirect
/// `http://localhost:8787/callback`, and the stub must never sit on the port Cursor's sign-in comes back to.
pub const DEFAULT_BIND: &str = "127.0.0.1:8799";
/// The URL Cursor is given unless `LK_T0_PUBLIC_URL` says otherwise.
pub const DEFAULT_PUBLIC_URL: &str = "http://127.0.0.1:8799";
/// A bearer token longer than this is refused unread (S11).
const MAX_TOKEN_BYTES: usize = 16 * 1024;
/// A decoded JWT segment longer than this is refused unparsed.
const MAX_SEGMENT_BYTES: usize = 12 * 1024;
/// A claim value is cut to this many characters before it is logged.
const MAX_CLAIM_CHARS: usize = 256;
/// Largest request body the stub reads (S11).
const MAX_BODY_BYTES: usize = 64 * 1024;
/// How many outcomes the probe remembers. Older ones are dropped (rule 5).
const PROBE_CAPACITY: usize = 64;
/// The draft link the stub returns in draft mode.
const DRAFT_PATH: &str = "/drafts/t0-demo";

// ---------------------------------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------------------------------

/// What can stop the stub. No variant carries a token, a header or any other input text (S21).
#[derive(Debug, thiserror::Error)]
pub enum StubError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{0} is not valid")]
    Invalid(&'static str),
    #[error(transparent)]
    Mcp(#[from] ConfigError),
    #[error(
        "refusing to listen on {0}: the stub accepts unverified tokens, so it listens on loopback only; \
         pass --allow-non-loopback to override"
    )]
    NonLoopback(SocketAddr),
    #[error("could not listen on the bind address")]
    Listen,
    #[error("the HTTP server stopped with an error")]
    Serve,
    #[error("unknown argument; the options are --allow-non-loopback and --help")]
    Usage,
}

// ---------------------------------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------------------------------

/// Everything the stub needs, parsed once at start (rule 5: nothing is read later).
#[derive(Debug)]
pub struct StubConfig {
    pub mcp: McpConfig,
    /// `scheme://host[:port]`, no path and no trailing slash.
    pub public_url: String,
    pub tenant_id: String,
    /// `api://<hub-api>`; the scope is this plus `/mcp.access`.
    pub api_audience: String,
    /// OAuth mode only: also serve `/.well-known/oauth-authorization-server`, a static mirror of Entra's.
    pub authorization_server_mirror: bool,
    /// Personal-token mode only: the one token the stub accepts.
    pub personal_token: Option<Secret<String>>,
    pub elicit_timeout: Duration,
    /// Most live sessions at once (a capped session manager, plan A19).
    pub session_cap: usize,
    /// Host names that are accepted in `Host` besides the loopback names and the public URL's host.
    pub extra_allowed_hosts: Vec<String>,
    /// How often an idle event stream gets a comment line so that proxies keep it open. `None` for tests on a paused clock:
    /// the `sse-stream` crate re-arms its timer from `std::time::Instant::now()`, which a paused tokio clock does not move.
    pub sse_keep_alive: Option<Duration>,
}

impl Default for StubConfig {
    fn default() -> Self {
        Self {
            mcp: McpConfig::default(),
            public_url: DEFAULT_PUBLIC_URL.to_owned(),
            tenant_id: "00000000-0000-0000-0000-000000000000".to_owned(),
            api_audience: "api://hub-api".to_owned(),
            authorization_server_mirror: false,
            personal_token: None,
            elicit_timeout: Duration::from_secs(120),
            session_cap: 8,
            extra_allowed_hosts: Vec::new(),
            sse_keep_alive: Some(Duration::from_secs(15)),
        }
    }
}

impl StubConfig {
    /// The one scope the hub asks for. Cursor's `auth.scopes` also lists `offline_access`.
    pub fn scope(&self) -> String {
        format!("{}/mcp.access", self.api_audience)
    }

    /// Entra's issuer for this tenant, which is the authorization server of the protected-resource metadata.
    pub fn issuer(&self) -> String {
        format!("https://login.microsoftonline.com/{}/v2.0", self.tenant_id)
    }

    /// Read the settings through `lookup` (`std::env::var` in `main`, a map in tests). A malformed value is an error that
    /// names the variable and never prints the value. A setting that belongs to the other authentication mode is an error
    /// too: the two modes are never combined (D52), not even in a test tool.
    #[allow(clippy::too_many_lines)]
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, StubError> {
        let mcp = McpConfig::from_lookup(&lookup)?;
        let get = |name: &'static str| -> Result<Option<String>, StubError> {
            let Some(value) = lookup(name).filter(|v| !v.is_empty()) else {
                return Ok(None);
            };
            if value.chars().any(char::is_control) {
                return Err(StubError::Invalid(name));
            }
            Ok(Some(value))
        };
        let number = |name: &'static str, default: u64, min: u64, max: u64| -> Result<u64, StubError> {
            let Some(value) = get(name)? else {
                return Ok(default);
            };
            let n: u64 = value.parse().map_err(|_| StubError::Invalid(name))?;
            if (min..=max).contains(&n) {
                Ok(n)
            } else {
                Err(StubError::Invalid(name))
            }
        };
        let defaults = Self::default();
        let public_url = match get("LK_T0_PUBLIC_URL")? {
            Some(value) => normalize_public_url(&value)?,
            None => defaults.public_url,
        };
        let elicit_timeout = Duration::from_secs(number("LK_T0_ELICIT_TIMEOUT_SECS", 120, 1, 600)?);
        let session_cap = usize::try_from(number("LK_T0_SESSION_CAP", 8, 1, 64)?)
            .map_err(|_| StubError::Invalid("LK_T0_SESSION_CAP"))?;
        let tenant = get("LK_T0_TENANT_ID")?;
        let audience = get("LK_T0_API_AUDIENCE")?;
        let mirror = match get("LK_T0_AS_MIRROR")?.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => return Err(StubError::Invalid("LK_T0_AS_MIRROR")),
        };
        let pat = get("LK_T0_PAT")?;

        let (tenant_id, api_audience, personal_token) = match mcp.auth_mode {
            AuthMode::Oauth => {
                if pat.is_some() {
                    return Err(StubError::Invalid("LK_T0_PAT"));
                }
                let tenant = tenant.ok_or(StubError::Missing("LK_T0_TENANT_ID"))?;
                let audience = audience.ok_or(StubError::Missing("LK_T0_API_AUDIENCE"))?;
                if !is_valid_tenant(&tenant) {
                    return Err(StubError::Invalid("LK_T0_TENANT_ID"));
                }
                if !is_valid_audience(&audience) {
                    return Err(StubError::Invalid("LK_T0_API_AUDIENCE"));
                }
                (tenant, audience, None)
            }
            AuthMode::PersonalToken => {
                if mirror {
                    return Err(StubError::Invalid("LK_T0_AS_MIRROR"));
                }
                if tenant.is_some() {
                    return Err(StubError::Invalid("LK_T0_TENANT_ID"));
                }
                if audience.is_some() {
                    return Err(StubError::Invalid("LK_T0_API_AUDIENCE"));
                }
                let token = pat.ok_or(StubError::Missing("LK_T0_PAT"))?;
                if token.len() < 16 || token.len() > 256 || !token.bytes().all(|b| b.is_ascii_graphic()) {
                    return Err(StubError::Invalid("LK_T0_PAT"));
                }
                (
                    defaults.tenant_id,
                    defaults.api_audience,
                    Some(Secret::new(token)),
                )
            }
        };
        Ok(Self {
            mcp,
            public_url,
            tenant_id,
            api_audience,
            authorization_server_mirror: mirror,
            personal_token,
            elicit_timeout,
            session_cap,
            extra_allowed_hosts: Vec::new(),
            sse_keep_alive: defaults.sse_keep_alive,
        })
    }
}

/// `scheme://host[:port]` with no user info, path, query or fragment. The stub speaks plain HTTP on loopback; an `https`
/// public URL is for a human who has put a TLS-terminating proxy in front of it.
fn normalize_public_url(value: &str) -> Result<String, StubError> {
    const NAME: &str = "LK_T0_PUBLIC_URL";
    let uri: axum::http::Uri = value.parse().map_err(|_| StubError::Invalid(NAME))?;
    let scheme = uri.scheme_str().ok_or(StubError::Invalid(NAME))?;
    if scheme != "http" && scheme != "https" {
        return Err(StubError::Invalid(NAME));
    }
    let authority = uri.authority().ok_or(StubError::Invalid(NAME))?;
    if authority.as_str().contains('@') || uri.query().is_some() || !matches!(uri.path(), "" | "/") {
        return Err(StubError::Invalid(NAME));
    }
    Ok(format!("{scheme}://{authority}"))
}

fn is_valid_tenant(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with(['.', '-'])
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

fn is_valid_audience(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("api://") else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 128
        && !rest.starts_with('/')
        && !rest.ends_with('/')
        && !rest.contains("//")
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
}

/// Where to listen. Anything but a loopback address needs `allow_non_loopback`: the stub accepts unverified tokens.
pub fn resolve_bind(value: &str, allow_non_loopback: bool) -> Result<SocketAddr, StubError> {
    let addr: SocketAddr = value.parse().map_err(|_| StubError::Invalid("LK_T0_BIND"))?;
    if !allow_non_loopback && !addr.ip().is_loopback() {
        return Err(StubError::NonLoopback(addr));
    }
    Ok(addr)
}

// ---------------------------------------------------------------------------------------------------------------------
// Reading a bearer token without verifying it (OAuth mode)
// ---------------------------------------------------------------------------------------------------------------------

/// The claims T0 wants to see. Nothing else is read out of the token, and nothing here is ever a credential.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Claims {
    pub aud: Option<String>,
    pub scp: Option<String>,
    pub iss: Option<String>,
    pub tid: Option<String>,
    pub ver: Option<String>,
}

/// The token is not a JWT-shaped string. No detail, because every detail would be a piece of the input.
#[derive(Debug, PartialEq, Eq)]
pub struct NotAJwt;

/// Read `aud`, `scp`, `iss`, `tid` and `ver` from a JWT's payload. **The signature is not checked**, and neither is the expiry.
pub fn read_unverified_claims(token: &str) -> Result<Claims, NotAJwt> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(NotAJwt);
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(NotAJwt);
    };
    decode_json_object(header)?;
    let payload = decode_json_object(payload)?;
    let text = |name: &str| -> Option<String> {
        let value = payload.get(name)?;
        let text = match value {
            Value::String(s) => s.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(","),
            _ => return None,
        };
        Some(text.chars().take(MAX_CLAIM_CHARS).collect())
    };
    Ok(Claims {
        aud: text("aud"),
        scp: text("scp"),
        iss: text("iss"),
        tid: text("tid"),
        ver: text("ver"),
    })
}

fn decode_json_object(segment: &str) -> Result<serde_json::Map<String, Value>, NotAJwt> {
    if segment.is_empty() || segment.len() > MAX_SEGMENT_BYTES {
        return Err(NotAJwt);
    }
    let bytes = URL_SAFE_NO_PAD.decode(segment).map_err(|_| NotAJwt)?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(NotAJwt),
    }
}

/// A stand-in for the user that is stable for one token and is never logged: the first 16 hex characters of its SHA-256. `T1`
/// replaces it with the verified user id. Sessions are bound to it (plan A19, first spike).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StubCaller(String);

impl StubCaller {
    fn of(token: &str) -> Self {
        let digest = Sha256::digest(token.as_bytes());
        Self(hex::encode(&digest[..8]))
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Authentication layer
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct AuthState {
    mode: AuthMode,
    resource_metadata_url: String,
    scope: String,
    audience: String,
    tenant: String,
    personal_token: Option<Arc<Secret<String>>>,
}

/// `Authorization: Bearer <token>` with exactly one such header. Anything else is "no token".
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || token.contains(' ')
    {
        return None;
    }
    Some(token)
}

fn unauthorized(state: &AuthState, token_was_presented: bool) -> Response {
    let mut challenge = String::from("Bearer");
    match state.mode {
        AuthMode::Oauth => {
            // The resource-metadata pointer is what starts Cursor's OAuth flow (RFC 9728, MCP authorization).
            let _ = write!(
                challenge,
                " resource_metadata=\"{}\", scope=\"{}\"",
                state.resource_metadata_url, state.scope
            );
            if token_was_presented {
                challenge.push_str(", error=\"invalid_token\"");
            }
        }
        // Personal-token mode publishes no discovery metadata, so its challenge points at none.
        AuthMode::PersonalToken => {
            if token_was_presented {
                challenge.push_str(" error=\"invalid_token\"");
            }
        }
    }
    let mut response = Response::new(axum::body::Body::from(r#"{"error":"unauthorized"}"#));
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    let headers = response.headers_mut();
    headers.insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_str(&challenge).unwrap_or(HeaderValue::from_static("Bearer")),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

async fn authenticate(State(state): State<Arc<AuthState>>, mut request: Request, next: Next) -> Response {
    let Some(token) = bearer_token(request.headers()) else {
        return unauthorized(&state, false);
    };
    let caller = StubCaller::of(token);
    match state.mode {
        AuthMode::Oauth => {
            let Ok(claims) = read_unverified_claims(token) else {
                return unauthorized(&state, true);
            };
            tracing::info!(
                target: "t0::claims",
                aud = ?claims.aud,
                scp = ?claims.scp,
                iss = ?claims.iss,
                tid = ?claims.tid,
                ver = ?claims.ver,
                aud_is_configured_audience = claims.aud.as_deref() == Some(state.audience.as_str()),
                scp_has_mcp_access = claims.scp.as_deref().is_some_and(|s| s.split(' ').any(|scope| scope == "mcp.access")),
                tid_is_configured_tenant = claims.tid.as_deref() == Some(state.tenant.as_str()),
                "bearer token claims (the token is not verified)"
            );
        }
        AuthMode::PersonalToken => {
            let accepted = state
                .personal_token
                .as_ref()
                .is_some_and(|pat| bool::from(pat.ct_eq_bytes(token.as_bytes())));
            if !accepted {
                return unauthorized(&state, true);
            }
        }
    }
    request.extensions_mut().insert(caller);
    next.run(request).await
}

/// Who opened each live session, kept for as long as the session is in use and no longer than the idle limit below. Bounded.
type SessionOwners = moka::sync::Cache<String, StubCaller>;

const OWNERS_CAPACITY: u64 = 1024;
const OWNERS_IDLE: Duration = Duration::from_secs(30 * 60);

/// The HTTP-layer half of the first spike of plan A19. rmcp hands the handler the request's `Parts`, so the handler can compare
/// callers (`StubHandler::check_caller`), but a `GET` that opens the server-to-client stream and a `DELETE` that ends the session
/// never reach the handler. This layer binds a session id to the caller that opened it, for every method, and answers a different
/// caller as rmcp itself answers an unknown session: 404, which does not reveal that the id exists. `T2` builds the real one.
async fn bind_sessions(State(owners): State<SessionOwners>, request: Request, next: Next) -> Response {
    let caller = request.extensions().get::<StubCaller>().cloned();
    let presented = request
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let method = request.method().clone();
    if let (Some(id), Some(caller)) = (&presented, &caller)
        && owners.get(id).is_some_and(|owner| owner != *caller)
    {
        return (StatusCode::NOT_FOUND, "Not Found: Session not found").into_response();
    }
    let response = next.run(request).await;
    match (method, presented, caller) {
        (axum::http::Method::POST, None, Some(caller)) => {
            if let Some(id) = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                owners.insert(id.to_owned(), caller);
            }
        }
        (axum::http::Method::DELETE, Some(id), _) if response.status().is_success() => owners.invalidate(&id),
        _ => {}
    }
    response
}

/// One line per request: what Cursor sent, never the `Authorization` header. T0 reads `origin`, `user_agent` and
/// `mcp_protocol_version` from here.
async fn log_request(request: Request, next: Next) -> Response {
    // Everything is copied out of the request in a block of its own: the request body is `Send` but not `Sync`, so no borrow of the
    // request may be alive across the `await` below.
    let (method, path, origin, user_agent, protocol, has_session) = {
        let headers = request.headers();
        let text = |name: &str| -> Option<String> {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.chars().take(MAX_CLAIM_CHARS).collect())
        };
        (
            request.method().clone(),
            request.uri().path().chars().take(128).collect::<String>(),
            text("origin"),
            text("user-agent"),
            text("mcp-protocol-version"),
            headers.contains_key("mcp-session-id"),
        )
    };
    let response = next.run(request).await;
    tracing::info!(
        target: "t0::request",
        %method,
        path = ?path,
        status = response.status().as_u16(),
        origin = ?origin,
        user_agent = ?user_agent,
        mcp_protocol_version = ?protocol,
        has_session,
        "request"
    );
    response
}

// ---------------------------------------------------------------------------------------------------------------------
// Discovery metadata (OAuth mode only)
// ---------------------------------------------------------------------------------------------------------------------

fn protected_resource_metadata(config: &StubConfig) -> Value {
    json!({
        "resource": format!("{}/mcp", config.public_url),
        "authorization_servers": [config.issuer()],
        "scopes_supported": [config.scope(), "offline_access"],
        "bearer_methods_supported": ["header"],
        "resource_name": "Lanekeeper T0 stub",
    })
}

/// A static copy of the fields of Entra's v2 discovery document that a client needs to start the code flow. It is built from
/// the tenant id alone, never fetched, so the stub makes no outbound call. Compare it with the live document (runbook step).
fn authorization_server_metadata(config: &StubConfig) -> Value {
    let base = format!("https://login.microsoftonline.com/{}", config.tenant_id);
    json!({
        "issuer": config.issuer(),
        "authorization_endpoint": format!("{base}/oauth2/v2.0/authorize"),
        "token_endpoint": format!("{base}/oauth2/v2.0/token"),
        "jwks_uri": format!("{base}/discovery/v2.0/keys"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [config.scope(), "offline_access", "openid", "profile"],
    })
}

fn metadata_route(document: Value) -> axum::routing::MethodRouter {
    let body = Arc::new(document);
    get(move || {
        let body = Arc::clone(&body);
        async move {
            let mut response = axum::Json((*body).clone()).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// What the write tool did (a probe for the tests; the stub writes nothing anywhere)
// ---------------------------------------------------------------------------------------------------------------------

/// How a `t0_write` call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Accept with `confirm: true`. The only outcome on which a real server would write. The stub still writes nothing.
    Confirmed,
    Declined,
    Cancelled,
    TimedOut,
    /// The client did not declare elicitation, or draft mode is on: a link, no prompt.
    DraftLink,
    /// The call was cancelled by the client or the session ended while the question was open.
    Aborted,
}

/// A bounded record of outcomes, so tests can assert that nothing was applied and wait for an outcome without sleeping.
#[derive(Clone, Default)]
pub struct Probe {
    inner: Arc<Mutex<VecDeque<Outcome>>>,
    changed: Arc<Notify>,
}

impl fmt::Debug for Probe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Probe")
            .field("outcomes", &self.outcomes())
            .finish()
    }
}

impl Probe {
    fn record(&self, outcome: Outcome) {
        let mut queue = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.len() == PROBE_CAPACITY {
            queue.pop_front();
        }
        queue.push_back(outcome);
        drop(queue);
        self.changed.notify_one();
    }

    pub fn outcomes(&self) -> Vec<Outcome> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }

    /// Wait until at least `count` outcomes have been recorded, and return them all.
    pub async fn wait_for(&self, count: usize) -> Vec<Outcome> {
        loop {
            let outcomes = self.outcomes();
            if outcomes.len() >= count {
                return outcomes;
            }
            self.changed.notified().await;
        }
    }
}

/// What a client's answer to the question means. **Only an accept with exactly `{"confirm": true}` proceeds**; anything
/// else, including an answer that does not fit the schema, is a refusal. This is the rule `T3` builds the real gate on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Proceed,
    Declined,
    Cancelled,
}

pub fn decide(result: &ElicitResult) -> Decision {
    match result.action {
        ElicitationAction::Accept => {
            let confirm = result
                .content
                .as_ref()
                .and_then(Value::as_object)
                .filter(|o| o.len() == 1)
                .and_then(|o| o.get("confirm"));
            match confirm {
                Some(Value::Bool(true)) => Decision::Proceed,
                Some(Value::Bool(false)) => Decision::Declined,
                _ => Decision::Cancelled,
            }
        }
        ElicitationAction::Decline => Decision::Declined,
        // `Cancel`, and any action a later protocol revision adds: fail closed.
        _ => Decision::Cancelled,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The MCP handler
// ---------------------------------------------------------------------------------------------------------------------

struct Shared {
    probe: Probe,
    public_url: String,
    confirm_mode: ConfirmMode,
    elicit_timeout: Duration,
    /// The server's own stop signal. rmcp's `cancellation_token` ends the transport but does not wake a handler that is waiting
    /// for an answer (`tests/rmcp_spike.rs` pins that), so a handler selects on this as well.
    stopping: tokio::sync::watch::Receiver<bool>,
}

/// One handler per MCP session (rmcp calls the factory for each `initialize`).
struct StubHandler {
    shared: Arc<Shared>,
    /// Set by `initialize` from the request's `http::request::Parts`; every later call must come from the same caller.
    bound: OnceLock<StubCaller>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum ReadMode {
    #[default]
    Ok,
    Error,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    #[serde(default)]
    mode: ReadMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    #[serde(default)]
    note: String,
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

fn parse_args<T: serde::de::DeserializeOwned>(arguments: Option<JsonObject>) -> Result<T, McpError> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|_| McpError::invalid_params("invalid arguments", None))
}

/// Text from the assistant, made safe to put in a prompt: control characters dropped, quotes neutralised, cut to `max`.
fn quote_untrusted(text: &str, max: usize) -> String {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '"' { '\'' } else { c })
        .collect();
    if cleaned.chars().count() > max {
        let cut: String = cleaned.chars().take(max).collect();
        format!("\"{cut}...\"")
    } else {
        format!("\"{cleaned}\"")
    }
}

fn text_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text)])
}

impl StubHandler {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            bound: OnceLock::new(),
        }
    }

    /// The first spike of plan A19: the request's `Parts` reach the handler on every call, with what the layer in front added.
    /// The session binds to the first caller, and a request from anyone else is refused.
    fn check_caller(&self, context: &RequestContext<RoleServer>) -> Result<(), McpError> {
        let caller = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<StubCaller>())
            .ok_or_else(|| McpError::invalid_request("unauthenticated", None))?;
        match self.bound.get_or_init(|| caller.clone()) {
            bound if bound == caller => Ok(()),
            _ => Err(McpError::invalid_request(
                "this session belongs to another caller",
                None,
            )),
        }
    }

    fn t0_read(arguments: Option<JsonObject>) -> Result<CallToolResult, McpError> {
        let args: ReadArgs = parse_args(arguments)?;
        match args.mode {
            ReadMode::Ok => {
                // Two markers, one in `content` and one only in `structuredContent`: which of them the model repeats tells the
                // human which of the two Cursor shows it (plan A6).
                let mut result =
                    text_result("t0_read ok. CONTENT-MARKER=alpha-7f3a (this marker is in `content`).");
                result.structured_content =
                    Some(json!({ "marker": "bravo-91c2 (this marker is only in structuredContent)" }));
                Ok(result)
            }
            ReadMode::Error => {
                // The contract's error shape (`{error: ...}` in structuredContent) does not match the declared output schema.
                // Whether Cursor accepts it is one of the observations.
                let mut result = CallToolResult::error(vec![ContentBlock::text(
                    "t0_read error demo: structuredContent does not match outputSchema.",
                )]);
                result.structured_content =
                    Some(json!({ "error": { "code": "t0_demo", "message": "error demo" } }));
                Ok(result)
            }
        }
    }

    async fn t0_write(
        &self,
        arguments: Option<JsonObject>,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let args: WriteArgs = parse_args(arguments)?;
        let probe = &self.shared.probe;
        let supports_form = context
            .peer
            .supported_elicitation_modes()
            .contains(&ElicitationMode::Form);
        if self.shared.confirm_mode == ConfirmMode::Draft || !supports_form {
            probe.record(Outcome::DraftLink);
            tracing::info!(target: "t0::write", outcome = ?Outcome::DraftLink, supports_form, "t0_write finished; nothing was written");
            return Ok(text_result(format!(
                "DRAFT_LINK: nothing was done. Confirm in the web UI at {}{DRAFT_PATH}",
                self.shared.public_url
            )));
        }

        let schema = ElicitationSchema::builder()
            .required_property(
                "confirm",
                PrimitiveSchemaDefinition::Boolean(rmcp::model::BooleanSchema::new().title("Confirm")),
            )
            .build()
            .map_err(|_| McpError::internal_error("could not build the question", None))?;
        let message = format!(
            "Lanekeeper T0 stub: approve the test action {} (supplied by the assistant)? Nothing is written anywhere; this only \
             checks that the prompt appears and that accept, decline and cancel all complete.",
            quote_untrusted(&args.note, 80)
        );
        let params = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message,
            requested_schema: schema,
        };
        let handle = context
            .peer
            .send_request_with_option(
                ServerRequest::ElicitRequest(ElicitRequest::new(params)),
                PeerRequestOptions::with_timeout(self.shared.elicit_timeout),
            )
            .await
            .map_err(|_| McpError::internal_error("could not ask the question", None))?;
        let question_id = handle.id.clone();
        let answer = handle.await_response();
        tokio::pin!(answer);

        // The second spike of plan A19: a question that is open when the call is cancelled, or the session ends, is withdrawn
        // from the client, and nothing proceeds.
        let mut stopping = self.shared.stopping.clone();
        let outcome = tokio::select! {
            () = context.ct.cancelled() => {
                let withdrawn = CancelledNotificationParam::new(Some(question_id), Some("the tool call was cancelled".to_owned()));
                let _ = context.peer.notify_cancelled(withdrawn).await;
                Outcome::Aborted
            }
            // The server is stopping (or its sender is gone, which means the same): withdraw the question and do nothing.
            () = async { let _ = stopping.wait_for(|stop| *stop).await; } => {
                let withdrawn = CancelledNotificationParam::new(Some(question_id), Some("the server is stopping".to_owned()));
                let _ = context.peer.notify_cancelled(withdrawn).await;
                Outcome::Aborted
            }
            answer = &mut answer => match answer {
                Ok(ClientResult::ElicitResult(result)) => match decide(&result) {
                    Decision::Proceed => Outcome::Confirmed,
                    Decision::Declined => Outcome::Declined,
                    Decision::Cancelled => Outcome::Cancelled,
                },
                // A timeout (rmcp has already told the client the question is withdrawn), a closed transport, or an answer of
                // the wrong kind: all of them leave the action undone.
                Err(rmcp::ServiceError::Timeout { .. }) => Outcome::TimedOut,
                Ok(_) | Err(_) => Outcome::Cancelled,
            },
        };
        probe.record(outcome);
        // The model may paraphrase the tool's answer, so the human reads what happened here.
        tracing::info!(target: "t0::write", outcome = ?outcome, "t0_write finished; nothing was written");
        let text = match outcome {
            Outcome::Confirmed => {
                "ACCEPTED: the user confirmed. A real server would write now; the stub writes nothing."
            }
            Outcome::Declined => "DECLINED: the user said no. Nothing was done.",
            Outcome::Cancelled => {
                "CANCELLED: the question was cancelled or the answer was not a confirmation. Nothing was done."
            }
            Outcome::TimedOut => "TIMED_OUT: no answer arrived in time. Nothing was done.",
            Outcome::Aborted => {
                "ABORTED: the call was cancelled while the question was open. Nothing was done."
            }
            Outcome::DraftLink => "DRAFT_LINK",
        };
        Ok(text_result(text))
    }

    fn tools() -> Vec<Tool> {
        let read_input = object(json!({
            "type": "object",
            "properties": { "mode": { "type": "string", "enum": ["ok", "error"], "description": "`error` returns an error result." } },
            "additionalProperties": false
        }));
        let read_output = object(json!({
            "type": "object",
            "properties": { "marker": { "type": "string" } },
            "required": ["marker"],
            "additionalProperties": false
        }));
        let write_input = object(json!({
            "type": "object",
            "properties": { "note": { "type": "string", "maxLength": 200, "description": "Free text shown in the question." } },
            "additionalProperties": false
        }));
        vec![
            Tool::new(
                "t0_read",
                "Read-only T0 test tool. Returns two markers; say which of them you can see.",
                Arc::new(read_input),
            )
            .with_raw_output_schema(Arc::new(read_output))
            .annotate(
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                "t0_write",
                "T0 test tool that asks the user to confirm. It writes nothing.",
                Arc::new(write_input),
            )
            .annotate(
                rmcp::model::ToolAnnotations::new()
                    .read_only(false)
                    .destructive(true)
                    .idempotent(false)
                    .open_world(false),
            ),
        ]
    }
}

impl ServerHandler for StubHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "lanekeeper-t0-stub",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions("Lanekeeper T0 compatibility stub. It has two test tools and no real data.")
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        self.check_caller(&context)?;
        // What T0 records: who connected, which protocol version they asked for, and whether they declared elicitation.
        tracing::info!(
            target: "t0::initialize",
            client_name = ?request.client_info.name.chars().take(MAX_CLAIM_CHARS).collect::<String>(),
            client_version = ?request.client_info.version.chars().take(MAX_CLAIM_CHARS).collect::<String>(),
            requested_protocol_version = %request.protocol_version,
            declares_elicitation = request.capabilities.elicitation.is_some(),
            "initialize"
        );
        context.peer.set_peer_info(request.clone());
        let result = self.negotiate_initialize(&request)?;
        tracing::info!(target: "t0::initialize", negotiated_protocol_version = %result.protocol_version, "negotiated");
        Ok(result)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.check_caller(&context)?;
        Ok(ListToolsResult::with_all_items(Self::tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        self.check_caller(&context)?;
        match request.name.as_ref() {
            "t0_read" => Self::t0_read(request.arguments).map(Into::into),
            "t0_write" => self.t0_write(request.arguments, &context).await.map(Into::into),
            _ => Err(McpError::invalid_params("unknown tool", None)),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// A capped session manager (the third spike of plan A19)
// ---------------------------------------------------------------------------------------------------------------------

/// rmcp's session manager with a ceiling on live sessions. A session holds one permit from its creation until it is closed,
/// whether by `DELETE`, by the idle timeout (rmcp closes the session when its worker ends) or by shutdown. `T2` puts a per-user
/// count and a 503 in front of this; the manager is the defence in depth.
pub struct CappedSessionManager<M> {
    inner: M,
    permits: Arc<Semaphore>,
    held: Mutex<HashMap<SessionId, OwnedSemaphorePermit>>,
}

impl<M> CappedSessionManager<M> {
    pub fn new(inner: M, cap: usize) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(cap)),
            held: Mutex::new(HashMap::new()),
        }
    }

    /// How many sessions are live right now.
    pub fn live(&self) -> usize {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl<M> fmt::Debug for CappedSessionManager<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CappedSessionManager")
            .field("live", &self.live())
            .finish_non_exhaustive()
    }
}

/// The ceiling was reached, or the wrapped manager failed.
#[derive(Debug)]
pub enum CappedError<E> {
    Full,
    Inner(E),
}

impl<E: fmt::Display> fmt::Display for CappedError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("too many live sessions"),
            Self::Inner(error) => error.fmt(f),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CappedError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Full => None,
            Self::Inner(error) => Some(error),
        }
    }
}

impl<M: SessionManager> SessionManager for CappedSessionManager<M> {
    type Error = CappedError<M::Error>;
    type Transport = M::Transport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        // The permit is taken before the session exists, so two requests cannot both pass a check-then-create race.
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| CappedError::Full)?;
        let (id, transport) = self.inner.create_session().await.map_err(CappedError::Inner)?;
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), permit);
        Ok((id, transport))
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        self.inner
            .initialize_session(id, message)
            .await
            .map_err(CappedError::Inner)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        self.inner.has_session(id).await.map_err(CappedError::Inner)
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        // Dropping the permit frees the slot. rmcp calls this again when the worker ends; the second call finds nothing.
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        self.inner.close_session(id).await.map_err(CappedError::Inner)
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl futures::Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner
            .create_stream(id, message)
            .await
            .map_err(CappedError::Inner)
    }

    async fn accept_message(&self, id: &SessionId, message: ClientJsonRpcMessage) -> Result<(), Self::Error> {
        self.inner
            .accept_message(id, message)
            .await
            .map_err(CappedError::Inner)
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl futures::Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner
            .create_standalone_stream(id)
            .await
            .map_err(CappedError::Inner)
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl futures::Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner
            .resume(id, last_event_id)
            .await
            .map_err(CappedError::Inner)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------------------------------------------------

/// The built stub: the router to serve, the probe to read, and a way to end every session.
pub struct Stub {
    pub router: Router,
    pub probe: Probe,
    pub sessions: Arc<CappedSessionManager<LocalSessionManager>>,
    stop_transport: Arc<dyn Fn() + Send + Sync>,
    stopping: Arc<tokio::sync::watch::Sender<bool>>,
    owners: SessionOwners,
}

impl fmt::Debug for Stub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stub")
            .field("probe", &self.probe)
            .finish_non_exhaustive()
    }
}

impl Stub {
    /// End rmcp's streams and sessions. A handler that is waiting for an answer is **not** woken by this.
    pub fn stop_transport(&self) {
        (self.stop_transport)();
    }

    /// Drop what the HTTP layer remembers about who opened which session. That memory is bounded and expires, so the handler's own
    /// check (`StubHandler::check_caller`) is the second line; tests use this to reach it.
    pub fn forget_session_owners(&self) {
        self.owners.invalidate_all();
        self.owners.run_pending_tasks();
    }

    /// Stop the server: wake every handler that is waiting (they withdraw their question and do nothing), then end the transport.
    pub fn shutdown(&self) {
        let _ = self.stopping.send(true);
        self.stop_transport();
    }
}

/// Build the router for `config`. In personal-token mode it has no discovery route at all, so there is no code path that could
/// publish OAuth metadata in that mode.
pub fn build(config: &StubConfig) -> Stub {
    let probe = Probe::default();
    let (stopping_tx, stopping_rx) = tokio::sync::watch::channel(false);
    let shared = Arc::new(Shared {
        probe: probe.clone(),
        public_url: config.public_url.clone(),
        confirm_mode: config.mcp.confirm_mode,
        elicit_timeout: config.elicit_timeout,
        stopping: stopping_rx,
    });

    let authority = config.public_url.split_once("://").map_or("", |(_, rest)| rest);
    let mut allowed_hosts: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(str::to_owned).to_vec();
    allowed_hosts.push(authority.to_owned());
    allowed_hosts.extend(config.extra_allowed_hosts.iter().cloned());
    let http_config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_hosts)
        .with_max_request_body_bytes(MAX_BODY_BYTES)
        .with_sse_keep_alive(config.sse_keep_alive);
    let token = http_config.cancellation_token.clone();
    let sessions = Arc::new(CappedSessionManager::new(
        LocalSessionManager::default(),
        config.session_cap,
    ));
    let factory_shared = Arc::clone(&shared);
    let service = StreamableHttpService::new(
        move || Ok(StubHandler::new(Arc::clone(&factory_shared))),
        Arc::clone(&sessions),
        http_config,
    );

    let auth = Arc::new(AuthState {
        mode: config.mcp.auth_mode,
        resource_metadata_url: format!("{}/.well-known/oauth-protected-resource/mcp", config.public_url),
        scope: config.scope(),
        audience: config.api_audience.clone(),
        tenant: config.tenant_id.clone(),
        personal_token: None,
    });
    let auth = match &config.personal_token {
        Some(secret) if config.mcp.auth_mode == AuthMode::PersonalToken => Arc::new(AuthState {
            personal_token: Some(Arc::new(Secret::new(secret.expose().clone()))),
            ..(*auth).clone()
        }),
        _ => auth,
    };

    let owners: SessionOwners = moka::sync::Cache::builder()
        .max_capacity(OWNERS_CAPACITY)
        .time_to_idle(OWNERS_IDLE)
        .build();
    // `route_layer`, not `layer`: the checks wrap `/mcp` only, so a path that matches no route is a plain 404 for everybody. That
    // is what "this mode publishes no discovery metadata" looks like from outside; a 401 there would hide whether it exists.
    let mut router = Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(owners.clone(), bind_sessions))
        .route_layer(middleware::from_fn_with_state(auth, authenticate));
    if config.mcp.auth_mode == AuthMode::Oauth {
        let resource = protected_resource_metadata(config);
        router = router
            .route(
                "/.well-known/oauth-protected-resource",
                metadata_route(resource.clone()),
            )
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                metadata_route(resource),
            );
        if config.authorization_server_mirror {
            router = router.route(
                "/.well-known/oauth-authorization-server",
                metadata_route(authorization_server_metadata(config)),
            );
        }
    }
    let router = router.layer(middleware::from_fn(log_request));

    Stub {
        router,
        probe,
        sessions,
        stop_transport: Arc::new(move || token.cancel()),
        stopping: Arc::new(stopping_tx),
        owners,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The program
// ---------------------------------------------------------------------------------------------------------------------

/// Which events are logged: the stub's own `t0::*` targets at `info`, everything else at `warn`. The filter matters. rmcp 3.5.0
/// logs every request it receives at `debug` (`service.rs`: `tracing::debug!(%id, ?request, ..)`, which includes the tool
/// arguments), every result at `debug` (`?result`, which includes file text) and every notification at `info`. A hub that
/// lets `rmcp` log below `warn` writes file content into its logs (S10, S21); `tests/rmcp_spike.rs` proves both halves.
pub fn log_filter() -> tracing_subscriber::filter::Targets {
    tracing_subscriber::filter::Targets::new()
        .with_default(tracing::level_filters::LevelFilter::WARN)
        .with_target("t0", tracing::level_filters::LevelFilter::INFO)
}

/// A subscriber that writes one line per event to `writer`, filtered by [`log_filter`] or by `filter` when given.
pub fn log_subscriber<W>(
    writer: W,
    filter: tracing_subscriber::filter::Targets,
) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt as _;
    let output = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(writer)
        .with_filter(filter);
    tracing_subscriber::Registry::default().with(output)
}

fn init_logging() {
    // Logs go to stderr, one line per event, with the claim names and values of `t0::claims` and the request lines of
    // `t0::request`. The token and the `Authorization` header are never logged.
    let _ = tracing::subscriber::set_global_default(log_subscriber(std::io::stderr, log_filter()));
}

fn usage() -> &'static str {
    "t0_stub [--allow-non-loopback]\n\nSettings come from the LK_MCP_* and LK_T0_* environment variables; see crates/hub-mcp/t0/README.md.\n"
}

async fn run() -> Result<(), StubError> {
    let mut allow_non_loopback = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--allow-non-loopback" => allow_non_loopback = true,
            "--help" | "-h" => {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), usage().as_bytes());
                return Ok(());
            }
            _ => return Err(StubError::Usage),
        }
    }
    let config = StubConfig::from_lookup(|name| std::env::var(name).ok())?;
    let bind = resolve_bind(
        &std::env::var("LK_T0_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned()),
        allow_non_loopback,
    )?;
    let stub = build(&config);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|_| StubError::Listen)?;
    tracing::info!(
        %bind,
        public_url = %config.public_url,
        auth_mode = %config.mcp.auth_mode,
        confirm_mode = %config.mcp.confirm_mode,
        mirror = config.authorization_server_mirror,
        "t0 stub listening; this server does not verify tokens, stop it when the check is done"
    );
    let stopper = Arc::new(stub);
    let on_signal = Arc::clone(&stopper);
    axum::serve(listener, stopper.router.clone())
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            on_signal.shutdown();
        })
        .await
        .map_err(|_| StubError::Serve)
}

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "t0 stub stopped");
            ExitCode::FAILURE
        }
    }
}
