//! The MCP server in the hub: rmcp's Streamable HTTP at `/mcp`, behind the gate of [`guard`] (S4, S14).
//!
//! [`McpState::new`] builds everything once from validated settings; [`mount`] puts the routes on the hub's router. T1 has
//! authentication, the session binding and the token routes; the tools, limits and confirmation come with T2 and T3.
//!
//! **One mode per deployment (D52).** [`Credentials`] holds the verifier *or* the personal-token service, never both, and the
//! settings must name the same mode, so the router for one mode has no code path for the other. Personal-token mode registers
//! no OAuth discovery route.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::handler::Handler as _;
use axum::middleware::from_fn_with_state;
use axum::response::Response;
use http::Method;
use hub_platform::{GuardedRouter, SessionAuth};
use ports::{TokenVerifier, Users};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::auth::oauth::{Challenge, OAuthSettings};
use crate::auth::pat::PersonalTokens;
use crate::auth::routes::{self, TokenRoutes};
use crate::auth::{BearerAuth, metadata};
use crate::config::{AuthMode, ConfigError, McpConfig, PublicOrigin};

pub mod guard;
pub mod handler;

use guard::{GateState, gate, session_owners};
use handler::McpHandler;

/// Where the server is mounted.
pub const MCP_PATH: &str = "/mcp";

/// The reason the route table gives for `/mcp` being registered as a route that authenticates itself.
const MCP_REASON: &str = "bearer token (Entra access token or personal token) checked by the MCP gate before the handler on every \
                          method, with the user's status re-read on every request; a role is checked per tool";

/// The longest request body: the hub's cap (S11).
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// How long an idle MCP session lives, and how long its owner is remembered (a little longer, so a session never outlives it).
pub const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);
const OWNERS_IDLE: Duration = Duration::from_secs(35 * 60);
/// The most sessions whose owner is remembered at once. The per-replica session cap of T2 is far below it.
const OWNERS_CAPACITY: u64 = 4096;
/// The most extra origins a deployment may allow.
const MAX_EXTRA_ORIGINS: usize = 8;

/// How the server is deployed.
#[derive(Debug, Clone)]
pub struct McpSettings {
    pub config: McpConfig,
    /// The origin users and Cursor see. `Host` and `Origin` are checked against it, and the discovery links are built from it.
    pub public_origin: PublicOrigin,
    /// OAuth mode only: the tenant, audience and mirror. `None` in personal-token mode, which has no OAuth settings at all.
    pub oauth: Option<OAuthSettings>,
    /// `Origin` values accepted besides the public origin, when a client sends one (T0 records whether Cursor does).
    pub allowed_origins: Vec<String>,
    /// How often an idle event stream gets a comment line, so that proxies keep it open. `None` for tests on a paused clock.
    pub sse_keep_alive: Option<Duration>,
}

impl McpSettings {
    #[must_use]
    pub fn new(config: McpConfig, public_origin: PublicOrigin, oauth: Option<OAuthSettings>) -> Self {
        Self {
            config,
            public_origin,
            oauth,
            allowed_origins: Vec::new(),
            sse_keep_alive: Some(Duration::from_secs(15)),
        }
    }

    /// # Errors
    ///
    /// [`ConfigError`] naming the first setting that is not usable, or that belongs to the other authentication mode.
    pub fn validate(&self) -> Result<(), ConfigError> {
        match (self.config.auth_mode, &self.oauth) {
            (AuthMode::Oauth, Some(oauth)) => oauth.validate()?,
            // OAuth mode needs its settings; personal-token mode must not carry any (D52: the modes are never combined).
            (AuthMode::Oauth, None) => return Err(ConfigError::Invalid("oauth")),
            (AuthMode::PersonalToken, Some(_)) => return Err(ConfigError::BothAuthModes),
            (AuthMode::PersonalToken, None) => {}
        }
        if self.allowed_origins.len() > MAX_EXTRA_ORIGINS
            || self.allowed_origins.iter().any(|o| {
                o.is_empty()
                    || o.len() > 256
                    || !o.bytes().all(|b| b.is_ascii_graphic())
                    || o.contains(['*', ' '])
            })
        {
            return Err(ConfigError::Invalid("allowed_origins"));
        }
        Ok(())
    }
}

/// How a caller proves who it is: one of the two, chosen by the deployment.
#[derive(Clone)]
pub enum Credentials {
    /// OAuth mode: Entra access tokens, verified by the identity crate.
    Oauth(Arc<dyn TokenVerifier>),
    /// Personal-token mode: `lkpat_` tokens made through the REST routes.
    PersonalToken(Arc<PersonalTokens>),
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Oauth(_) => "Credentials::Oauth",
            Self::PersonalToken(_) => "Credentials::PersonalToken",
        })
    }
}

impl Credentials {
    #[must_use]
    pub fn mode(&self) -> AuthMode {
        match self {
            Self::Oauth(_) => AuthMode::Oauth,
            Self::PersonalToken(_) => AuthMode::PersonalToken,
        }
    }
}

/// What the server consumes from the rest of the hub.
#[derive(Clone)]
pub struct McpDeps {
    pub credentials: Credentials,
    pub users: Arc<dyn Users>,
}

impl std::fmt::Debug for McpDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpDeps")
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

/// Why the server could not be built. Names the setting, never its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("the credentials are for the other authentication mode than the settings name")]
    ModeMismatch,
    #[error("the personal-token settings are out of range")]
    Tokens,
}

type Service = StreamableHttpService<McpHandler, LocalSessionManager>;

/// The MCP server, built. Cheap to share: [`mount`] and the tests take an `Arc` of it.
pub struct McpState {
    settings: McpSettings,
    auth: Arc<BearerAuth>,
    gate: Arc<GateState>,
    service: Service,
    sessions: Arc<LocalSessionManager>,
    token_routes: TokenRoutes,
    stop: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for McpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpState")
            .field("mode", &self.settings.config.auth_mode)
            .finish_non_exhaustive()
    }
}

impl McpState {
    /// # Errors
    ///
    /// [`StartError`] when the settings are not usable, or the credentials are for the other mode than the settings name.
    pub fn new(deps: McpDeps, settings: McpSettings) -> Result<Arc<Self>, StartError> {
        settings.validate()?;
        let McpDeps { credentials, users } = deps;
        if credentials.mode() != settings.config.auth_mode {
            return Err(StartError::ModeMismatch);
        }
        let (auth, challenge, tokens) = match (credentials, &settings.oauth) {
            (Credentials::Oauth(verifier), Some(oauth)) => (
                BearerAuth::oauth(verifier, users),
                Challenge::oauth(&settings.public_origin, oauth),
                None,
            ),
            (Credentials::PersonalToken(tokens), None) => (
                BearerAuth::personal(Arc::clone(&tokens), users),
                Challenge::personal_token(),
                Some(tokens),
            ),
            // `validate` and the mode check above already ruled these out.
            _ => return Err(StartError::ModeMismatch),
        };
        if let Some(tokens) = &tokens {
            tokens.settings().validate().map_err(|_| StartError::Tokens)?;
        }
        let auth = Arc::new(auth);

        let mut hosts = settings.public_origin.host_values();
        hosts.iter_mut().for_each(|h| h.make_ascii_lowercase());
        let mut origins = vec![settings.public_origin.as_str().to_owned()];
        origins.extend(settings.allowed_origins.iter().cloned());
        let gate = Arc::new(GateState {
            auth: Arc::clone(&auth),
            challenge,
            hosts: hosts.clone(),
            origins: origins.clone(),
            owners: session_owners(OWNERS_CAPACITY, OWNERS_IDLE),
            max_body: MAX_BODY_BYTES,
        });

        // rmcp checks `Host` and `Origin` itself as well; the gate has already refused anything else, so this is the second line.
        let http_config = StreamableHttpServerConfig::default()
            .with_allowed_hosts(hosts)
            .with_allowed_origins(origins)
            .enforce_origin_validation()
            .with_max_request_body_bytes(MAX_BODY_BYTES)
            .with_sse_keep_alive(settings.sse_keep_alive);
        let token = http_config.cancellation_token.clone();
        let mut manager = LocalSessionManager::default();
        manager.session_config.keep_alive = Some(SESSION_IDLE);
        let sessions = Arc::new(manager);
        let service =
            StreamableHttpService::new(|| Ok(McpHandler::new()), Arc::clone(&sessions), http_config);

        let token_routes = TokenRoutes::new(tokens, settings.public_origin.clone());
        Ok(Arc::new(Self {
            settings,
            auth,
            gate,
            service,
            sessions,
            token_routes,
            stop: Arc::new(move || token.cancel()),
        }))
    }

    #[must_use]
    pub fn settings(&self) -> &McpSettings {
        &self.settings
    }

    /// The authenticator, as the hub's guard machinery sees it.
    #[must_use]
    pub fn auth(&self) -> Arc<dyn SessionAuth> {
        Arc::clone(&self.auth) as Arc<dyn SessionAuth>
    }

    /// How many MCP sessions are open on this replica.
    pub async fn live_sessions(&self) -> usize {
        self.sessions.sessions.read().await.len()
    }

    /// Forget who opened which session, as the memory's own expiry would. Tests use it to show that a session whose owner is not
    /// remembered is not served.
    #[doc(hidden)]
    pub fn forget_session_owners(&self) {
        self.gate.owners.invalidate_all();
        self.gate.owners.run_pending_tasks();
    }

    /// End every session and stream. A handler that waits for an answer is not woken by this (D91): T3 gives a confirmation a stop
    /// signal of its own.
    pub fn stop(&self) {
        (self.stop)();
    }
}

/// Put the server on the hub's router: the three personal-token routes on its session guard, and `/mcp` (and, in OAuth mode, the
/// discovery documents) as routes that authenticate themselves.
#[must_use]
pub fn mount(router: GuardedRouter, state: &Arc<McpState>) -> GuardedRouter {
    let mut router = routes::mount(router, &state.token_routes);
    if let Some(oauth) = &state.settings.oauth {
        router = metadata::mount(router, &state.settings.public_origin, oauth);
    }
    let service = state.service.clone();
    // The response of rmcp keeps its status and headers; only the body type is converted.
    let handler = move |request: Request| {
        let service = service.clone();
        async move {
            let (parts, body) = service.handle(request).await.into_parts();
            Response::from_parts(parts, axum::body::Body::new(body))
        }
    };
    let guarded = handler.layer(from_fn_with_state(Arc::clone(&state.gate), gate));
    for method in [Method::POST, Method::GET, Method::DELETE] {
        router = router.public(&method, MCP_PATH, MCP_REASON, guarded.clone());
    }
    router
}
