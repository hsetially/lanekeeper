//! The identity module in the composed hub (plan 18, T4; S1, S2, S3, S4, S9): sign-in, sessions, users and roles, and the audit
//! chain's own jobs.
//!
//! `hub-identity` is a library of parts, and this is the order they are put together in, from the settings and the edges:
//!
//! 1. [`Sessions`] over the pool, with its sweeper and its `last_seen_at` batcher, and the [`RevocationListener`] that evicts a
//!    user's cached sessions on this replica when a role or status change publishes `UserSessionsRevoked`.
//! 2. [`UserCache`] and its listener (the same event), for the profile the platform's guard wants of a user.
//! 3. [`OidcLogin`] (the Entra sign-in; the cookie key is read now, the tenant's discovery document lazily), after a check that the
//!    client secret is there, so a name that is wrong is an exit at start and not a failed first sign-in.
//! 4. [`PgUsers`] and [`UserAdmin`], and the routes: sign-in, callback, sign-out, `me`, access requests and user administration.
//!    The routes are identity's own router with its own guard and its own CSRF layer; the hub carries it as a [`MountedRouter`]
//!    with its fallback taken off, and its route table converted to the platform's, so the one table has every route (S4).
//! 5. [`HubTokenVerifier`] (Entra access tokens for MCP, Google ID tokens), for the modules that take a bearer token.
//! 6. The audit chain's three jobs: the hourly checkpoint (KMS signs the head, the locked bucket keeps a copy), the nightly
//!    verification (the whole chain and every checkpoint against the bucket and the key), and the shipper that copies committed
//!    events to Cloud Logging. Each takes its lease through the `Leases` port, so one replica runs it. They stop with the hub.
//!
//! The pieces that come out are a [`SessionEdge`] (what the hub's guard asks, and the two layers over the whole router) and a
//! [`MountedRouter`]; [`Identity`] is what has to stay alive for them to keep working.
//!
//! **It never starts on less.** The client secret, the cookie key, the checkpoint keys and the identity routes are each checked
//! before the hub serves; each failure is an exit with a fixed message that names the variable and never a value.

use std::sync::Arc;
use std::time::Duration;

use http::Method;
use hub_identity::Clock as IdentityClockTrait;
use hub_identity::audit::{
    AuditLogShipper, ChainVerifier, CheckpointDeps, CheckpointJob, EcdsaP256Verifier, JobHandle,
    NightlyVerifyJob,
};
use hub_identity::config::EntraConfig;
use hub_identity::guard::{Access, GuardedRouter as IdentityGuard};
use hub_identity::metrics::IdentityMetrics;
use hub_identity::oidc::login::OidcLogin;
use hub_identity::routes::auth::AuthState;
use hub_identity::routes::{IdentityState, identity_routes};
use hub_identity::session::{RevocationListener, SessionPolicy, Sessions};
use hub_identity::tokens::{EntraAccessTokens, GoogleIdTokens, HubTokenVerifier};
use hub_identity::users::onboarding::PgGithubTokenPresence;
use hub_identity::users::{AccessNotifier, PgUsers, UserAdmin, UserServices};
use hub_platform::{RouteGuard, RouteSpec};
use ports::{AuditLog, EventBus, Leases, Notifier, TokenVerifier, Users};
use sqlx::PgPool;
use tokio::sync::watch;

use crate::compose::{MountedRouter, SessionEdge};
use crate::config::{CheckpointSettings, IdentitySettings};
use crate::edges::{Edges, IdentityClock};
use crate::session_auth::CookieAuth;
use crate::user_cache::{self, USER_CACHE_ENTRIES, USER_CACHE_TTL, UserCache};
use crate::wiring::Tasks;

/// How often the audit head is signed and a checkpoint written (S9: every hour).
pub const CHECKPOINT_EVERY: Duration = Duration::from_secs(60 * 60);
/// How long after start the first nightly verification runs, and how often the next ones do. The lease keeps it to one replica.
pub const VERIFY_FIRST_DELAY: Duration = Duration::from_secs(15 * 60);
pub const VERIFY_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// When the audit chain's jobs run. The defaults are S9's; a test shortens them, because it cannot wait an hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditSchedule {
    pub checkpoint_every: Duration,
    pub verify_first_delay: Duration,
    pub verify_every: Duration,
}

impl Default for AuditSchedule {
    fn default() -> Self {
        Self {
            checkpoint_every: CHECKPOINT_EVERY,
            verify_first_delay: VERIFY_FIRST_DELAY,
            verify_every: VERIFY_EVERY,
        }
    }
}
/// How long the shipper may take to give its lease up when the hub stops.
const SHUTDOWN_STEP: Duration = Duration::from_secs(2);

/// Why the identity module could not be set up. Every message is fixed text that names a variable or a component, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityBuildError {
    #[error("the Entra client secret cannot be read (LK_ENTRA_CLIENT_AUTH_REF)")]
    ClientSecret,
    #[error("the key that seals the login cookie cannot be read (LK_ENTRA_COOKIE_KEY_REF)")]
    CookieKeyUnavailable,
    #[error("the key that seals the login cookie is not 32 bytes of base64url (LK_ENTRA_COOKIE_KEY_REF)")]
    CookieKeyMalformed,
    #[error("the Entra sign-in cannot be set up (the LK_ENTRA_* settings)")]
    SignIn,
    #[error(
        "the public half of an audit checkpoint key cannot be read from KMS (LK_AUDIT_CHECKPOINT_KMS_REF)"
    )]
    CheckpointKeyUnavailable,
    #[error("an audit checkpoint key is not a P-256 signing key (LK_AUDIT_CHECKPOINT_KMS_REF)")]
    CheckpointKeyUnusable,
    #[error("the identity routes were refused")]
    Routes,
    #[error("setting up the identity module took too long")]
    TimedOut,
}

/// What [`Identity::build`] is made from.
pub struct IdentityInputs<'a> {
    pub settings: &'a IdentitySettings,
    pub pool: PgPool,
    pub edges: &'a Edges,
    pub audit: Arc<dyn AuditLog>,
    pub leases: Arc<dyn Leases>,
    pub bus: Arc<dyn EventBus>,
    /// Who an access request and a failed verification are told to.
    pub notifier: Arc<dyn Notifier>,
    /// Turns true when the hub stops: every task here ends then.
    pub closing: watch::Receiver<bool>,
    /// The longest any one step of starting may take.
    pub start_timeout: Duration,
    /// When the audit chain's three jobs run. [`AuditSchedule::default`] in production.
    pub schedule: AuditSchedule,
}

impl std::fmt::Debug for IdentityInputs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityInputs").finish_non_exhaustive()
    }
}

/// What the composition takes from the module: the guard's side and the routes.
pub struct IdentityMounts {
    pub session: SessionEdge,
    pub mounted: MountedRouter,
}

impl std::fmt::Debug for IdentityMounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityMounts").finish_non_exhaustive()
    }
}

/// The identity module, running. Dropping it stops the session sweeper, the `last_seen_at` batcher and the revocation listener,
/// so it is held for as long as the hub serves.
pub struct Identity {
    /// The cookie sessions: the platform's [`SessionAuth`](hub_platform::SessionAuth) for streams and for the guard.
    pub auth: Arc<CookieAuth>,
    pub sessions: Arc<Sessions>,
    /// The user directory, as the other modules take it (MCP's bearer check, the write paths).
    pub users: Arc<dyn Users>,
    /// Entra access tokens and Google ID tokens. The module that takes a bearer token (MCP, T6) is given this one.
    pub tokens: Arc<dyn TokenVerifier>,
    /// Counters of the audit jobs. `/metrics` reads them through [`crate::metrics_bridge::SourceCollector`].
    pub metrics: Arc<IdentityMetrics>,
    /// The users in memory, for the tests that look at what is kept.
    pub user_cache: Arc<UserCache>,
    /// The Entra settings (the origin, the audiences) the modules after this one are made from.
    pub entra: EntraConfig,
    _revocation: RevocationListener,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity").finish_non_exhaustive()
    }
}

/// The route table of identity's router in the platform's terms, so that the hub's one table (the S4 test's input) has every route.
fn specs_of(table: &[hub_identity::guard::RouteEntry]) -> Vec<RouteSpec> {
    table
        .iter()
        .filter_map(|entry| {
            let method = Method::from_bytes(entry.method.as_bytes()).ok()?;
            let guard = match &entry.access {
                Access::Role(role) => RouteGuard::Role(*role),
                Access::Public { reason } => RouteGuard::Public {
                    reason: (*reason).to_owned(),
                },
            };
            Some(RouteSpec::new(&method, &entry.path, guard))
        })
        .collect()
}

/// The hub's clock as the identity crate's.
fn clock_of(edges: &Edges) -> Arc<dyn IdentityClockTrait> {
    Arc::new(IdentityClock(Arc::clone(&edges.clock)))
}

/// `step`, or `TimedOut` when `limit` passes first.
async fn bounded<T>(
    limit: Duration,
    step: impl std::future::Future<Output = Result<T, IdentityBuildError>>,
) -> Result<T, IdentityBuildError> {
    tokio::time::timeout(limit, step)
        .await
        .map_err(|_| IdentityBuildError::TimedOut)?
}

/// A verifier that holds the public key of the key that signs checkpoints and of every older version named for verifying.
async fn checkpoint_verifier(
    edges: &Edges,
    checkpoint: &CheckpointSettings,
) -> Result<EcdsaP256Verifier, IdentityBuildError> {
    let mut verifier = EcdsaP256Verifier::new();
    for key in std::iter::once(&checkpoint.key).chain(&checkpoint.older_keys) {
        let der = edges
            .public_keys
            .spki_der(key)
            .await
            .map_err(|_| IdentityBuildError::CheckpointKeyUnavailable)?;
        verifier = verifier
            .with_spki_der(key.as_str(), &der)
            .map_err(|_| IdentityBuildError::CheckpointKeyUnusable)?;
    }
    Ok(verifier)
}

/// The three jobs of the audit chain, and what it takes to stop them: dropping a handle ends its job.
struct AuditJobs {
    _checkpoint: JobHandle,
    _verify: JobHandle,
    _ship: JobHandle,
    shipper: Arc<AuditLogShipper>,
}

/// What the audit chain's jobs are made from.
struct AuditInputs<'a> {
    settings: &'a IdentitySettings,
    pool: PgPool,
    edges: &'a Edges,
    clock: Arc<dyn IdentityClockTrait>,
    leases: Arc<dyn Leases>,
    notifier: Arc<dyn Notifier>,
    metrics: Arc<IdentityMetrics>,
    schedule: AuditSchedule,
    start_timeout: Duration,
}

impl AuditJobs {
    /// Start the checkpoint, the nightly verification and the shipper. Each takes its lease through `leases`, so one replica runs it.
    async fn spawn(inputs: AuditInputs<'_>) -> Result<Self, IdentityBuildError> {
        let AuditInputs {
            settings,
            pool,
            edges,
            clock,
            leases,
            notifier,
            metrics,
            schedule,
            start_timeout,
        } = inputs;
        let verifier = bounded(start_timeout, checkpoint_verifier(edges, &settings.checkpoint)).await?;
        let checkpoint = Arc::new(CheckpointJob::new(CheckpointDeps {
            pool: pool.clone(),
            leases: Arc::clone(&leases),
            signer: Arc::clone(&edges.signer),
            key: settings.checkpoint.key.clone(),
            bucket: Arc::clone(&edges.identity.checkpoints),
            clock,
            metrics: Arc::clone(&metrics),
        }));
        let verify = Arc::new(NightlyVerifyJob::new(
            ChainVerifier::new(
                pool.clone(),
                Arc::clone(&edges.identity.checkpoints),
                Arc::new(verifier),
            ),
            Arc::clone(&leases),
            notifier,
            Arc::clone(&metrics),
        ));
        let shipper = Arc::new(AuditLogShipper::new(pool, leases, metrics));
        Ok(Self {
            _checkpoint: checkpoint.spawn(schedule.checkpoint_every),
            _verify: verify.spawn(schedule.verify_first_delay, schedule.verify_every),
            _ship: Arc::clone(&shipper).spawn(),
            shipper,
        })
    }

    /// Hand the jobs to a task that ends them when the hub stops, after giving the shipper's lease up so that another replica takes
    /// the log over at once.
    fn adopt(self, tasks: &mut Tasks, mut closing: watch::Receiver<bool>) {
        tasks.adopt(
            "audit-jobs",
            tokio::spawn(async move {
                // The whole of `self` moves into the task, not only the field that is named below: a closure captures the fields it
                // uses, and the handles it left out would drop when `adopt` returns, which ends the jobs at once.
                let jobs = self;
                let _ = closing.wait_for(|stopping| *stopping).await;
                let _ = tokio::time::timeout(SHUTDOWN_STEP, jobs.shipper.shutdown()).await;
                // The handles drop with `jobs`, and the jobs end with them.
                drop(jobs);
            }),
        );
    }
}

/// The Entra sign-in. The client secret is read once now (and again, from the secret source's cache, at each exchange) so that a
/// wrong name stops the start and not the first person to try. The cookie key is read by `OidcLogin::new`.
async fn sign_in(
    entra: &EntraConfig,
    edges: &Edges,
    clock: &Arc<dyn IdentityClockTrait>,
    start_timeout: Duration,
) -> Result<OidcLogin, IdentityBuildError> {
    let client_secret = match &entra.client_auth {
        hub_identity::config::ClientAuth::Secret { name } => name.clone(),
        hub_identity::config::ClientAuth::Federated { .. } => return Err(IdentityBuildError::SignIn),
    };
    let (login, secret) = bounded(start_timeout, async {
        let login = OidcLogin::new(
            entra.clone(),
            Arc::clone(&edges.identity.entra),
            Arc::clone(&edges.secrets),
            Arc::clone(clock),
        );
        let secret = edges.secrets.get(&client_secret);
        Ok(tokio::join!(login, secret))
    })
    .await?;
    let login = login.map_err(|error| match error {
        hub_identity::IdentityError::Secret(_) => IdentityBuildError::CookieKeyUnavailable,
        hub_identity::IdentityError::Crypto => IdentityBuildError::CookieKeyMalformed,
        _ => IdentityBuildError::SignIn,
    })?;
    secret.map_err(|_| IdentityBuildError::ClientSecret)?;
    Ok(login)
}

/// What the users and the routes are made from.
struct RouteInputs<'a> {
    settings: &'a IdentitySettings,
    pool: PgPool,
    audit: Arc<dyn AuditLog>,
    bus: Arc<dyn EventBus>,
    notifier: Arc<dyn Notifier>,
    clock: Arc<dyn IdentityClockTrait>,
    sessions: Arc<Sessions>,
    login: OidcLogin,
    origin: &'a str,
}

/// The users, and the routes: sign-in, sign-out, `me`, access requests and user administration, behind identity's own guard. Returns
/// the router with its table in the platform's terms.
fn routes(inputs: RouteInputs<'_>) -> Result<(PgUsers, axum::Router, Vec<RouteSpec>), IdentityBuildError> {
    let RouteInputs {
        settings,
        pool,
        audit,
        bus,
        notifier,
        clock,
        sessions,
        login,
        origin,
    } = inputs;
    let services = UserServices {
        pool: pool.clone(),
        audit: Arc::clone(&audit),
        bus: Arc::clone(&bus),
        notifier: AccessNotifier::new(notifier),
        clock: Arc::clone(&clock),
    };
    let users = PgUsers::new(services.clone(), settings.config.bootstrap_set());
    let admin = UserAdmin::new(services);
    let auth_state = AuthState::new(
        login,
        users.clone(),
        Arc::clone(&sessions),
        pool.clone(),
        audit,
        bus,
        clock,
    );
    let state = IdentityState::new(users.clone(), admin, Arc::new(PgGithubTokenPresence::new(pool)));
    let guarded = identity_routes(IdentityGuard::new(sessions, origin), auth_state, state);
    let specs = specs_of(guarded.route_table());
    if specs.len() != guarded.route_table().len() {
        return Err(IdentityBuildError::Routes);
    }
    // Identity's router answers an unmatched path itself (401, then 404). In the hub that is the hub's to answer, once, for every
    // module, so the fallback goes before the merge (`Router::merge` adopts a custom fallback, or panics on two).
    let router = guarded
        .into_router()
        .map_err(|_| IdentityBuildError::Routes)?
        .reset_fallback();
    Ok((users, router, specs))
}

impl Identity {
    /// Set the module up. It adopts the tasks it needs into `tasks`, so that they stop with the hub and a panic in one is seen.
    pub async fn build(
        inputs: IdentityInputs<'_>,
        tasks: &mut Tasks,
    ) -> Result<(Self, IdentityMounts), IdentityBuildError> {
        let IdentityInputs {
            settings,
            pool,
            edges,
            audit,
            leases,
            bus,
            notifier,
            closing,
            start_timeout,
            schedule,
        } = inputs;
        let entra = settings.config.entra.clone();
        let origin = settings.public_origin();
        let clock = clock_of(edges);

        // Sessions, and the two listeners that keep the caches in step with role changes made on any replica.
        let sessions = Arc::new(Sessions::new(
            pool.clone(),
            Arc::clone(&clock),
            &SessionPolicy::default(),
        ));
        let revocation = RevocationListener::spawn(Arc::clone(&bus), Arc::clone(sessions.cache()));
        let user_cache = Arc::new(UserCache::new(USER_CACHE_ENTRIES, USER_CACHE_TTL));
        tasks.adopt(
            "user-cache-listener",
            user_cache::spawn_listener(Arc::clone(&bus), Arc::clone(&user_cache), closing.clone()),
        );

        let login = sign_in(&entra, edges, &clock, start_timeout).await?;
        let verifier = login.verifier();

        // Users, and the routes.
        let (users, router, specs) = routes(RouteInputs {
            settings,
            pool: pool.clone(),
            audit,
            bus,
            notifier: Arc::clone(&notifier),
            clock: Arc::clone(&clock),
            sessions: Arc::clone(&sessions),
            login,
            origin: &origin,
        })?;

        let users: Arc<dyn Users> = Arc::new(users);
        let tokens: Arc<dyn TokenVerifier> = Arc::new(HubTokenVerifier::new(
            EntraAccessTokens::new(&entra, verifier),
            GoogleIdTokens::new(Arc::clone(&edges.identity.google_keys), Arc::clone(&clock)),
        ));

        // The audit chain's jobs.
        let metrics = Arc::new(IdentityMetrics::new());
        AuditJobs::spawn(AuditInputs {
            settings,
            pool,
            edges,
            clock: Arc::clone(&clock),
            leases,
            notifier,
            metrics: Arc::clone(&metrics),
            schedule,
            start_timeout,
        })
        .await?
        .adopt(tasks, closing);

        let auth = Arc::new(CookieAuth::new(
            Arc::clone(&sessions),
            Arc::clone(&users),
            Arc::clone(&user_cache),
            clock,
        ));
        let session = SessionEdge::cookie(Arc::clone(&sessions), &origin, Arc::clone(&auth) as _);
        Ok((
            Self {
                auth,
                sessions,
                users,
                tokens,
                metrics,
                user_cache,
                entra,
                _revocation: revocation,
            },
            IdentityMounts {
                session,
                mounted: MountedRouter {
                    name: "identity",
                    router,
                    specs,
                },
            },
        ))
    }
}
