//! The MCP module (prompts 07 and 18, T6; S1, S3, S4, S14, S14b, S15): `/mcp`, the OAuth discovery documents, and the personal-token
//! routes, built from the settings and mounted in the composed hub.
//!
//! The server itself, its gate and its authentication are in `hub_mcp`. [`Mcp::build`] is the hub's half: it chooses the credential
//! the deployment runs, hands the server the ports it stands on, and keeps it stopped with the hub. [`McpModule`] is the line that
//! puts its routes in the hub's route table, so that they pass through the same layers as every other route (headers, limits,
//! metrics, drain) and are checked by the S4 route-table test. `/mcp` answers a long-lived stream, which the limits layer allows: it
//! times out the response *head*, not the body (`/api/v1/events` is the precedent). `crates/hub/tests/mcp_module.rs` proves both, and
//! that the hub's log filter keeps `rmcp` at `warn`, since rmcp logs whole requests and results at `debug` (S10).
//!
//! **One credential, never both (D52).** `LK_MCP_AUTH` is `oauth` or `personal_token`.
//! - *OAuth* (the default): the credential is the identity module's `HubTokenVerifier`, Entra access tokens whose audience, scope
//!   `mcp.access`, tenant and issuer it checks (S1). No secret is read.
//! - *Personal token*: `PersonalTokens` over the `mcp_tokens` table (`PgTokenStore`, as `lanekeeper_app`) and the key that hashes
//!   them, the Secret Manager secret `mcp-token-hmac-key`, read once now. A hub without a usable key does not start in this mode
//!   (exit 1, naming the secret): the alternative is a hub that accepts no token and says nothing.
//!
//! **Every call checks the user again** (S4): the server's gate asks the identity module's `Users::require_active` on each request,
//! so a role change or a disabling bites on the next call. The three personal-token routes ride the hub's cookie sessions, and so
//! they are behind the hub-wide CSRF layer (D109's condition for S3, `mcp_module::the_personal_token_routes_need_the_csrf_token`);
//! `/mcp` itself is a route that authenticates by bearer token, with no session, and so has no ambient authority for CSRF to protect.
//!
//! **What the tools stand on.** The registry's read API and the write path's service are the real ones (the process passes
//! `RegistryService` and `WriteHub`). The document search is [`crate::docs::UnavailableDocs`] until prompt 14: the five docs tools
//! answer a typed `unavailable` and never data (S15 has nothing to defend there, and an empty list would be a false answer).
//! Config and doc text in every result is data, labelled and scanned for secrets by the crate (S14b, S15).

use std::sync::Arc;
use std::time::Duration;

use hub_mcp::McpState;
use hub_mcp::StartError as McpStartError;
use hub_mcp::auth::pat::{PersonalTokens, TokenSettings};
use hub_mcp::auth::pg_store::PgTokenStore;
use hub_mcp::config::{AuthMode, ConfigError as McpConfigError};
use hub_mcp::{Credentials, McpDeps};
use hub_platform::GuardedRouter;
use hub_platform::clock::Clock;
use ports::{AuditLog, DocSearch, RegistryRead, SecretSource, TokenVerifier, Users, WriteService};
use prometheus_client::registry::Registry;
use sqlx::PgPool;
use tokio::sync::watch;
use tracing::info;

use crate::compose::Module;
use crate::edges::McpClock;
use crate::wiring::Tasks;

/// Why the MCP server could not be set up. Every message is fixed text that names a setting or a secret, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum McpBuildError {
    /// Personal-token mode needs the key that hashes the tokens. Without it no token could be made or accepted.
    #[error(
        "the key for personal MCP tokens cannot be read or is not usable (the secret mcp-token-hmac-key)"
    )]
    TokenKey,
    /// The library refused a setting. The name is one of the library's own constants.
    #[error("the MCP server refused a setting ({0})")]
    Settings(&'static str),
    /// The credentials are for the other mode than the settings name, or the tool contract does not match the handlers: a defect in
    /// the build and not in the deployment.
    #[error("the MCP server could not be assembled")]
    Assembly,
    #[error("setting up the MCP server took too long")]
    TimedOut,
}

impl From<McpStartError> for McpBuildError {
    fn from(error: McpStartError) -> Self {
        match error {
            McpStartError::Config(McpConfigError::Invalid(name)) => Self::Settings(name),
            McpStartError::Config(McpConfigError::BothAuthModes) => Self::Settings("LK_MCP_AUTH"),
            McpStartError::Tokens => Self::Settings("personal-token settings"),
            McpStartError::ModeMismatch | McpStartError::Tools => Self::Assembly,
        }
    }
}

/// What [`Mcp::build`] is made from: the ports the tools stand on, and the settings.
pub struct McpInputs {
    /// Built from the settings and the identity group by `McpSettings::for_identity`.
    pub settings: hub_mcp::McpSettings,
    /// For the personal-token table. Not touched in OAuth mode.
    pub pool: PgPool,
    /// OAuth mode: the identity module's verifier of Entra access tokens.
    pub tokens: Arc<dyn TokenVerifier>,
    /// The user directory, asked again on every call.
    pub users: Arc<dyn Users>,
    pub registry: Arc<dyn RegistryRead>,
    pub writes: Arc<dyn WriteService>,
    pub docs: Arc<dyn DocSearch>,
    /// Personal-token changes (a token made, a token revoked) are audited in the transaction that changes them.
    pub audit: Arc<dyn AuditLog>,
    /// For the token key, in personal-token mode.
    pub secrets: Arc<dyn SecretSource>,
    pub clock: Arc<dyn Clock>,
    /// Turns true when the hub stops: open streams and open questions end then.
    pub closing: watch::Receiver<bool>,
    /// The longest the key may take to read.
    pub start_timeout: Duration,
}

impl std::fmt::Debug for McpInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpInputs")
            .field("mode", &self.settings.config.auth_mode)
            .finish_non_exhaustive()
    }
}

/// The MCP server, built. The router holds what it needs; this is for the module, the metrics and the tests.
pub struct Mcp {
    state: Arc<McpState>,
}

impl std::fmt::Debug for Mcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mcp").field("state", &self.state).finish()
    }
}

/// Resolves when `closing` is true (or its sender is gone).
async fn closed(mut closing: watch::Receiver<bool>) {
    let _ = closing.wait_for(|stopping| *stopping).await;
}

impl Mcp {
    /// Choose the credential, build the server, and adopt the task that stops it with the hub into `tasks`.
    pub async fn build(inputs: McpInputs, tasks: &mut Tasks) -> Result<Self, McpBuildError> {
        let McpInputs {
            settings,
            pool,
            tokens,
            users,
            registry,
            writes,
            docs,
            audit,
            secrets,
            clock,
            closing,
            start_timeout,
        } = inputs;
        let clock: Arc<dyn hub_mcp::clock::Clock> = Arc::new(McpClock(clock));
        let credentials = match settings.config.auth_mode {
            AuthMode::Oauth => Credentials::Oauth(tokens),
            AuthMode::PersonalToken => {
                let store = Arc::new(PgTokenStore::new(pool));
                let loaded = tokio::time::timeout(
                    start_timeout,
                    PersonalTokens::load(
                        store,
                        secrets.as_ref(),
                        audit,
                        Arc::clone(&clock),
                        TokenSettings::default(),
                    ),
                )
                .await
                .map_err(|_| McpBuildError::TimedOut)?
                .map_err(|_| McpBuildError::TokenKey)?;
                Credentials::PersonalToken(Arc::new(loaded))
            }
        };
        let deps = McpDeps::new(credentials, users, registry, docs, writes).with_clock(clock);
        // Once, at start: the tool contract is parsed and checked against the handlers.
        let mode = settings.config.auth_mode;
        let begun = std::time::Instant::now();
        let state = McpState::new(deps, settings)?;
        info!(
            %mode,
            elapsed_ms = u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX),
            "the MCP server is set up"
        );
        // Open streams and open questions end when the hub starts to stop, and not when the connections have drained: the server's
        // own stop signal wakes a question that is waiting for an answer (rmcp's token does not, D91). Required: if this task ended
        // early nothing would stop the server, and readiness says so.
        let stopping = Arc::clone(&state);
        tasks.adopt(
            "mcp-stop",
            tokio::spawn(async move {
                closed(closing).await;
                stopping.stop();
            }),
        );
        Ok(Self { state })
    }

    /// The server, for the tests and the module.
    #[must_use]
    pub fn state(&self) -> &Arc<McpState> {
        &self.state
    }

    /// `/mcp`, the discovery documents and the three personal-token routes.
    #[must_use]
    pub fn module(&self) -> Arc<dyn Module> {
        Arc::new(McpModule::new(Arc::clone(&self.state)))
    }

    /// The counters of the tools, on the hub's metrics.
    pub fn register_metrics(&self, registry: &mut Registry) {
        self.state.metrics().register(registry);
    }
}

/// Mounts the MCP server.
#[derive(Debug, Clone)]
pub struct McpModule {
    state: Arc<McpState>,
}

impl McpModule {
    #[must_use]
    pub fn new(state: Arc<McpState>) -> Self {
        Self { state }
    }
}

impl Module for McpModule {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        hub_mcp::mount(router, &self.state)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use domain::{Guid, Role, ShortText, User, UserId, UserStatus};
    use hub_mcp::auth::oauth::OAuthSettings;
    use hub_mcp::config::{AuthMode, McpConfig, PublicOrigin};
    use hub_platform::clock::SystemClock;
    use ports::fakes::{
        FakeAuditLog, FakeDocSearch, FakeRegistry, FakeSecretSource, FakeTokenVerifier, FakeUsers,
        FakeWriteService,
    };
    use tokio::sync::watch;

    use super::*;
    use crate::wiring::Tasks;

    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

    fn settings(mode: AuthMode) -> hub_mcp::McpSettings {
        let oauth = OAuthSettings {
            tenant_id: Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
            api_audience: "api://hub-api".to_owned(),
            authority_host: OAuthSettings::DEFAULT_AUTHORITY_HOST.to_owned(),
            authorization_server_mirror: false,
        };
        hub_mcp::McpSettings::new(
            McpConfig {
                auth_mode: mode,
                ..McpConfig::default()
            },
            PublicOrigin::parse("https://hub.example.test").unwrap(),
            (mode == AuthMode::Oauth).then_some(oauth),
        )
    }

    fn inputs(mode: AuthMode, secrets: FakeSecretSource, closing: watch::Receiver<bool>) -> McpInputs {
        let users = FakeUsers::new();
        users.insert(User {
            id: UserId::new(
                Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
                Guid::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
            ),
            email: ShortText::parse("alice@example.test").unwrap(),
            display_name: ShortText::parse("Alice").unwrap(),
            role: Some(Role::Viewer),
            status: UserStatus::Active,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        });
        McpInputs {
            settings: settings(mode),
            // Never connected to: personal tokens are only read when a request presents one.
            pool: sqlx::PgPool::connect_lazy("postgres://nobody@127.0.0.1:9/none").unwrap(),
            tokens: Arc::new(FakeTokenVerifier::new()),
            users: Arc::new(users),
            registry: Arc::new(FakeRegistry::new()),
            writes: Arc::new(FakeWriteService::new()),
            docs: Arc::new(FakeDocSearch::new()),
            audit: Arc::new(FakeAuditLog::new()),
            secrets: Arc::new(secrets),
            clock: Arc::new(SystemClock),
            closing,
            start_timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn oauth_mode_is_built_from_the_verifier_and_reads_no_secret() {
        let (_closing, rx) = watch::channel(false);
        let mut tasks = Tasks::new(rx.clone());
        // No secret at all: OAuth mode has no key of its own.
        let mcp = Mcp::build(inputs(AuthMode::Oauth, FakeSecretSource::new(), rx), &mut tasks)
            .await
            .unwrap();
        assert_eq!(mcp.state().settings().config.auth_mode, AuthMode::Oauth);
        assert_eq!(McpModule::new(Arc::clone(mcp.state())).name(), "mcp");
        assert!(tasks.names().contains(&"mcp-stop"), "{:?}", tasks.names());
    }

    #[tokio::test]
    async fn personal_token_mode_reads_its_key_and_is_built() {
        let (_closing, rx) = watch::channel(false);
        let mut tasks = Tasks::new(rx.clone());
        let secrets = FakeSecretSource::new().with(hub_mcp::auth::pat::KEY_SECRET_NAME, KEY);
        let mcp = Mcp::build(inputs(AuthMode::PersonalToken, secrets, rx), &mut tasks)
            .await
            .unwrap();
        assert_eq!(mcp.state().settings().config.auth_mode, AuthMode::PersonalToken);
    }

    #[tokio::test]
    async fn personal_token_mode_without_a_usable_key_does_not_start_and_names_the_secret() {
        for (what, secrets) in [
            ("no secret", FakeSecretSource::new()),
            (
                "a key that is not base64url",
                FakeSecretSource::new().with(hub_mcp::auth::pat::KEY_SECRET_NAME, "planted key!"),
            ),
            (
                "a key that is too short",
                FakeSecretSource::new().with(hub_mcp::auth::pat::KEY_SECRET_NAME, "AAEC"),
            ),
        ] {
            let (_closing, rx) = watch::channel(false);
            let mut tasks = Tasks::new(rx.clone());
            let error = Mcp::build(inputs(AuthMode::PersonalToken, secrets, rx), &mut tasks)
                .await
                .unwrap_err();
            assert_eq!(error, McpBuildError::TokenKey, "{what}");
            let shown = error.to_string();
            assert!(shown.contains(hub_mcp::auth::pat::KEY_SECRET_NAME), "{shown}");
            assert!(!shown.contains("planted"), "{shown}");
        }
    }

    #[tokio::test]
    async fn settings_for_the_other_mode_than_the_credentials_are_refused() {
        // OAuth settings in personal-token mode (D52: never both) are refused by the library; the hub says which part, in fixed words.
        let (_closing, rx) = watch::channel(false);
        let mut tasks = Tasks::new(rx.clone());
        let mut input = inputs(
            AuthMode::PersonalToken,
            FakeSecretSource::new().with(hub_mcp::auth::pat::KEY_SECRET_NAME, KEY),
            rx,
        );
        input.settings.oauth = settings(AuthMode::Oauth).oauth;
        let error = Mcp::build(input, &mut tasks).await.unwrap_err();
        assert!(matches!(error, McpBuildError::Settings(_)), "{error:?}");
    }

    #[tokio::test]
    async fn the_stop_task_ends_with_the_hub_and_stops_the_server() {
        let (closing, rx) = watch::channel(false);
        let mut tasks = Tasks::new(rx.clone());
        let mcp = Mcp::build(inputs(AuthMode::Oauth, FakeSecretSource::new(), rx), &mut tasks)
            .await
            .unwrap();
        // Until the hub closes, the task is waiting: a required task that had ended would turn readiness off.
        assert!(tasks.alive().ended().is_empty());
        closing.send_replace(true);
        let overran = tasks.finish(Duration::from_secs(5)).await;
        assert!(overran.is_empty(), "{overran:?}");
        drop(mcp);
    }
}
