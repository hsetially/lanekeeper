//! The MCP server in the hub: rmcp's Streamable HTTP at `/mcp`, behind the gate of [`guard`] (S4, S14).
//!
//! [`McpState::new`] builds everything once from validated settings; [`mount`] puts the routes on the hub's router. T1 has
//! authentication, the session binding and the token routes; T2 adds the tools, their limits and the session caps; T3 adds the
//! confirmation of writes (`crate::confirm`).
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
use ports::{DocSearch, RegistryRead, TokenVerifier, Users, WriteService};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::sync::watch;

use crate::auth::oauth::{Challenge, OAuthSettings};
use crate::auth::pat::PersonalTokens;
use crate::auth::routes::{self, TokenRoutes};
use crate::auth::{BearerAuth, metadata};
use crate::clock::{Clock, SystemClock};
use crate::config::{AuthMode, ConfigError, McpConfig, OutputSettings, PublicOrigin, ToolSettings};
use crate::confirm::replay::Replay;
use crate::limits::{LimitSettings, Limits};
use crate::metrics::McpMetrics;
use crate::output::Redactor;
use crate::output::links::{LinkTemplates, WebLinks};
use crate::schemas::{Contract, SchemaError};
use crate::tools::cache::FileCache;
use crate::tools::{HostError, Listed, ToolHost};

pub mod guard;
pub mod handler;
pub mod instructions;
pub mod session;

use guard::{GateState, gate, session_owners};
use handler::McpHandler;
use session::{CappedSessionManager, SessionCounts};

/// Where the server is mounted.
pub const MCP_PATH: &str = "/mcp";

/// The reason the route table gives for `/mcp` being registered as a route that authenticates itself.
const MCP_REASON: &str = "bearer token (Entra access token or personal token) checked by the MCP gate before the handler on every \
                          method, with the user's status re-read on every request; a role is checked per tool";

/// The longest request body: the hub's cap (S11).
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// How long an owner is remembered beyond the idle time of a session, so that a session never outlives its owner's record.
const OWNERS_GRACE: Duration = Duration::from_secs(5 * 60);
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
    /// Rates, concurrency, byte budget, timeouts, session caps and cache size (rule 5, A15).
    pub limits: LimitSettings,
    /// The caps on what a result may weigh (D58).
    pub output: OutputSettings,
    /// Switches of single tools (`tools.notify_services.enabled`).
    pub tools: ToolSettings,
    /// Where the links of a result point in the web UI (A12).
    pub links: LinkTemplates,
    /// Put the result schema of each tool in `tools/list` as `outputSchema`. On by default; a deployment can turn it off if Cursor
    /// puts the schemas in the model's context or rejects results that do not match (T0 records what it does).
    pub advertise_output_schemas: bool,
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
            limits: LimitSettings::default(),
            output: OutputSettings::default(),
            tools: ToolSettings::default(),
            links: LinkTemplates::default(),
            advertise_output_schemas: true,
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
        self.limits.validate()?;
        self.output.validate()?;
        self.links.validate()?;
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

/// What the server consumes from the rest of the hub: who may call (`credentials`, `users`) and the three services the tools
/// stand on (`RegistryRead`, `DocSearch`, `WriteService`). The crate depends on those traits and on no sibling's types.
#[derive(Clone)]
pub struct McpDeps {
    pub credentials: Credentials,
    pub users: Arc<dyn Users>,
    pub registry: Arc<dyn RegistryRead>,
    pub docs: Arc<dyn DocSearch>,
    pub writes: Arc<dyn WriteService>,
    /// Time, for the rate limits. The real clock unless a test says otherwise ([`McpDeps::with_clock`]).
    pub clock: Arc<dyn Clock>,
}

impl McpDeps {
    #[must_use]
    pub fn new(
        credentials: Credentials,
        users: Arc<dyn Users>,
        registry: Arc<dyn RegistryRead>,
        docs: Arc<dyn DocSearch>,
        writes: Arc<dyn WriteService>,
    ) -> Self {
        Self {
            credentials,
            users,
            registry,
            docs,
            writes,
            clock: Arc::new(SystemClock),
        }
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
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
    #[error("the tool contract is not usable, or does not match the handlers")]
    Tools,
}

impl From<HostError> for StartError {
    fn from(_: HostError) -> Self {
        Self::Tools
    }
}

impl From<SchemaError> for StartError {
    fn from(_: SchemaError) -> Self {
        Self::Tools
    }
}

type Sessions = CappedSessionManager<LocalSessionManager>;
type Service = StreamableHttpService<McpHandler, Sessions>;

/// The MCP server, built. Cheap to share: [`mount`] and the tests take an `Arc` of it.
pub struct McpState {
    settings: McpSettings,
    auth: Arc<BearerAuth>,
    gate: Arc<GateState>,
    service: Service,
    sessions: Arc<Sessions>,
    session_counts: Arc<SessionCounts>,
    host: Arc<ToolHost>,
    listing: Arc<Vec<Listed>>,
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
    #[allow(clippy::too_many_lines)] // the one place that wires the limits, the caches, the host and the transport together
    pub fn new(deps: McpDeps, settings: McpSettings) -> Result<Arc<Self>, StartError> {
        settings.validate()?;
        let McpDeps {
            credentials,
            users,
            registry,
            docs,
            writes,
            clock,
        } = deps;
        if credentials.mode() != settings.config.auth_mode {
            return Err(StartError::ModeMismatch);
        }
        let (auth, challenge, tokens) = match (credentials, &settings.oauth) {
            (Credentials::Oauth(verifier), Some(oauth)) => (
                BearerAuth::oauth(verifier, Arc::clone(&users)),
                Challenge::oauth(&settings.public_origin, oauth),
                None,
            ),
            (Credentials::PersonalToken(tokens), None) => (
                BearerAuth::personal(Arc::clone(&tokens), Arc::clone(&users)),
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

        // The tools: the contract and the handlers must be the same set, or the server does not start.
        let contract = Contract::embedded()?;
        let listing = Arc::new(ToolHost::listing(&contract, settings.advertise_output_schemas)?);
        let metrics = McpMetrics::new();
        // The server's own stop signal, which a confirmation waits on (D91: rmcp's token does not wake it).
        let (stop_signal, stopping) = watch::channel(false);
        let replay = Replay::new(Arc::clone(&clock));
        let host = Arc::new(ToolHost {
            contract,
            registry,
            docs,
            writes,
            users: Arc::clone(&users),
            redactor: Redactor::new(),
            links: WebLinks::new(settings.public_origin.clone(), settings.links.clone()),
            limits: Limits::new(settings.limits.clone(), Arc::clone(&clock)),
            content_mode: settings.config.content_mode,
            confirm_mode: settings.config.confirm_mode,
            output: settings.output,
            tool_settings: settings.tools,
            files: FileCache::new(settings.limits.cache_bytes),
            metrics,
            clock,
            replay,
            stopping,
        });

        let mut hosts = settings.public_origin.host_values();
        hosts.iter_mut().for_each(|h| h.make_ascii_lowercase());
        let mut origins = vec![settings.public_origin.as_str().to_owned()];
        origins.extend(settings.allowed_origins.iter().cloned());
        let session_counts = Arc::new(SessionCounts::new(
            settings.limits.sessions_per_user,
            settings.limits.sessions_per_replica,
        ));
        let owners = session_owners(
            OWNERS_CAPACITY,
            settings.limits.session_idle + OWNERS_GRACE,
            Arc::clone(&session_counts),
        );
        let gate = Arc::new(GateState {
            auth: Arc::clone(&auth),
            challenge,
            hosts: hosts.clone(),
            origins: origins.clone(),
            owners: owners.clone(),
            sessions: Arc::clone(&session_counts),
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
        manager.session_config.keep_alive = Some(settings.limits.session_idle);
        // When rmcp closes a session (it was idle too long, or the owner ended it) the owner's record goes with it.
        let forget = owners.clone();
        let sessions = Arc::new(CappedSessionManager::new(
            manager,
            settings.limits.sessions_per_replica as usize,
            Arc::new(move |id: &str| forget.invalidate(id)),
        ));
        let factory_host = Arc::clone(&host);
        let factory_listing = Arc::clone(&listing);
        let service = StreamableHttpService::new(
            move || {
                Ok(McpHandler::new(
                    Arc::clone(&factory_host),
                    Arc::clone(&factory_listing),
                ))
            },
            Arc::clone(&sessions),
            http_config,
        );

        let token_routes = TokenRoutes::new(tokens, settings.public_origin.clone());
        Ok(Arc::new(Self {
            settings,
            auth,
            gate,
            service,
            sessions,
            session_counts,
            host,
            listing,
            token_routes,
            stop: Arc::new(move || {
                // Ours first, so that a question that is open sees it before rmcp tears its streams down.
                stop_signal.send_replace(true);
                token.cancel();
            }),
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
        self.sessions.inner().sessions.read().await.len()
    }

    /// How many sessions the gate counts (open, or being opened) for all users and for `user`.
    #[must_use]
    pub fn counted_sessions(&self, user: Option<&domain::UserId>) -> usize {
        user.map_or_else(|| self.session_counts.total(), |u| self.session_counts.of(u))
    }

    /// The metrics of the tools; the hub registers them with its own (`McpMetrics::register`).
    #[must_use]
    pub fn metrics(&self) -> &McpMetrics {
        &self.host.metrics
    }

    /// The tools and what they share (for tests and the benchmarks).
    #[must_use]
    pub fn host(&self) -> &Arc<ToolHost> {
        &self.host
    }

    /// The tools as `tools/list` shows them to a user with `role` or more, and how many bytes that is.
    #[must_use]
    pub fn tools_list_bytes(&self, user: &domain::User) -> usize {
        crate::output::cap::json_len(&self.host.visible(&self.listing, user))
    }

    /// Forget who opened which session, as the memory's own expiry would. Tests use it to show that a session whose owner is not
    /// remembered is not served.
    #[doc(hidden)]
    pub fn forget_session_owners(&self) {
        self.gate.owners.invalidate_all();
        self.gate.owners.run_pending_tasks();
    }

    /// End every session and stream, and tell every question that is open that the server is stopping: it is withdrawn and nothing
    /// is written. rmcp's own token does not wake a handler that waits for an answer (D91), so the signal is ours.
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
