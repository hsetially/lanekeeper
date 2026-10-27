//! The hub process (plan 18, T1 and T3): starts it, serves until told to stop, drains, and says how it ended.
//!
//! What this assembles is the shell of the hub and the platform under it: the settings, the database pool, the web certificate,
//! the two certificate authorities, the web listener (TLS 1.3, the certificate reloaded on a timer), the operations listener
//! (`/healthz`, `/readyz`, `/metrics`), the agent and replica listeners (mTLS), the event bus, the Git mirrors, the leases and the
//! platform's jobs, the identity module (sign-in, sessions, CSRF, users, the audit chain's jobs; T4), the drain, and readiness. The
//! other modules come in later tasks and plug into the same `Deps`: registry and writes (T5), MCP (T6).
//!
//! **It never starts on a fake.** Every start step is a real component or an error with a fixed message that names the
//! component, never a value. `Edges` is the only place software can be substituted, and the binary fills it with Google.
//!
//! **Exit codes.** 0: stopped on request. 1: could not start, or stopped for a reason of its own, or was told twice. 2: the
//! command line or the settings were refused (nothing was started).
//!
//! **Stopping.** The first SIGTERM or SIGINT fails readiness, waits the grace period (so the load balancer notices), stops the
//! accept loops, gives connections in flight the drain period, stops the job runner (which releases its leases), waits for the
//! background tasks, and stops the operations listener last. A second signal at any point exits at once. A signal during start
//! abandons the start and exits 0: nothing was serving.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hub_identity::metrics::CounterSource;
use hub_platform::RouteSpec;
use hub_platform::jobs::JobRunnerHandle;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

use crate::ca_init;
use crate::cli::{self, CliError, Command};
use crate::compose::{BuildError, Deps, Hub, Module};
use crate::config::{
    ConfigError, DatabaseSettings, GitSettings, HubSettings, IdentitySettings, PlatformSettings,
    WebTlsSettings,
};
use crate::db::{self, DbError, PostgresReady};
use crate::edges::Edges;
use crate::identity::{AuditSchedule, Identity, IdentityBuildError, IdentityInputs, IdentityMounts};
use crate::metrics_bridge::SourceCollector;
use crate::migrate;
use crate::notify::UnavailableNotifier;
use crate::ready::{CertsReady, Expiring};
use crate::schema::{self, SchemaError, SchemaReady};
use crate::serve::{self, ServeLimits};
use crate::shutdown::{Drain, Stop};
use crate::static_files::{StaticError, StaticFiles};
use crate::telemetry;
use crate::web_tls::{WebCert, WebTlsError};
use crate::wiring::{Cas, Ingest, Platform, PlatformError, PlatformInputs, Tasks, prepare_cas};

/// How long the background tasks get to stop after the listeners have, before the rest are cut off.
const TASKS_GRACE: Duration = Duration::from_secs(5);
/// How long the runtime waits for blocking calls (a Git fetch) once the hub has drained, before the process leaves without them.
const RUNTIME_GRACE: Duration = Duration::from_secs(2);

/// Why the hub did not start. Every message is fixed text that names a component or a variable, never a value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    /// A setting is missing or malformed. Nothing was started.
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("the Google clients could not be set up")]
    Google,
    #[error("the database is not usable: {0}")]
    Database(DbError),
    /// The hub never migrates (T2, G4): a database that lacks a migration is for `hub migrate` to fix, not for the hub to start on.
    #[error("the database schema is not usable: {0}")]
    Schema(SchemaError),
    #[error("the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF): {0}")]
    WebCertificate(WebTlsError),
    #[error("the web listener's TLS configuration was refused")]
    Tls,
    #[error("the web app cannot be served from LK_WEB_DIST: {0}")]
    WebApp(StaticError),
    /// A CA, a listener or a job of the platform (T3).
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// Sign-in, sessions or the audit chain's jobs (T4).
    #[error(transparent)]
    Identity(#[from] IdentityBuildError),
    #[error("the hub could not be composed: {0}")]
    Compose(BuildError),
    #[error("could not listen on the {0} address")]
    Bind(&'static str),
    #[error("starting took too long: {0}")]
    TimedOut(&'static str),
    #[error("the {0} listener stopped by itself")]
    ListenerStopped(&'static str),
}

impl StartError {
    /// 2 for a refused setting, 1 for anything that went wrong while starting.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Config(_) => 2,
            _ => 1,
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Stopped on request, and drained.
    Stopped,
    /// Told to stop twice: it did not wait for the drain.
    Forced,
    /// Could not start, or a listener died.
    Failed(StartError),
}

impl Outcome {
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Stopped => 0,
            Self::Forced => 1,
            Self::Failed(error) => error.exit_code(),
        }
    }
}

/// The parts of the settings the listeners need once the settings have been taken apart.
struct Plan {
    web_addr: SocketAddr,
    ops_addr: SocketAddr,
    agent_addr: SocketAddr,
    replica_addr: SocketAddr,
    web_dist: PathBuf,
    max_connections: usize,
    grace: Duration,
    drain: Duration,
    start_timeout: Duration,
}

/// The four sockets, bound before anything is loaded: a taken address is found in a moment and not after a start's worth of work,
/// and the replica listener's address is known before the platform advertises it. Connections wait in the accept queue until the
/// listener starts accepting.
struct Sockets {
    web: TcpListener,
    ops: TcpListener,
    agent: TcpListener,
    replica: TcpListener,
}

impl Sockets {
    async fn bind(plan: &Plan) -> Result<Self, StartError> {
        let bind = |addr: SocketAddr, what: &'static str| async move {
            TcpListener::bind(addr).await.map_err(|_| StartError::Bind(what))
        };
        Ok(Self {
            web: bind(plan.web_addr, "web").await?,
            ops: bind(plan.ops_addr, "operations").await?,
            agent: bind(plan.agent_addr, "agent").await?,
            replica: bind(plan.replica_addr, "replica").await?,
        })
    }
}

/// A running hub: every listener is bound and serving.
pub struct Running {
    web_addr: SocketAddr,
    ops_addr: SocketAddr,
    agent_addr: SocketAddr,
    replica_addr: SocketAddr,
    drain: Drain,
    grace: Duration,
    web: JoinHandle<()>,
    ops: JoinHandle<()>,
    ops_stop: watch::Sender<bool>,
    /// The platform's tasks and the web certificate's reload. `None` once they have been waited for.
    tasks: Option<Tasks>,
    jobs: Option<JobRunnerHandle>,
    /// The identity module: its sweeper, batcher and listener stop when it drops, which is after the listeners have drained.
    identity: Option<Identity>,
    /// Every route of every module, with its guard: what the S4 tests check against `api/openapi.yaml`.
    routes: Vec<RouteSpec>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("web_addr", &self.web_addr)
            .field("ops_addr", &self.ops_addr)
            .field("agent_addr", &self.agent_addr)
            .field("replica_addr", &self.replica_addr)
            .finish_non_exhaustive()
    }
}

/// `what` as the step that timed out, if `limit` passes first.
async fn bounded<T>(
    limit: Duration,
    what: &'static str,
    step: impl std::future::Future<Output = Result<T, StartError>>,
) -> Result<T, StartError> {
    tokio::time::timeout(limit, step)
        .await
        .map_err(|_| StartError::TimedOut(what))?
}

/// What loading found: the pool, the certificate the web listener will serve with, and both authorities.
struct Loaded {
    pool: sqlx::PgPool,
    web_cert: Arc<WebCert>,
    cas: Cas,
}

/// `Some(value)`, or `None` after putting the failure in `errors`.
fn keep<T>(result: Result<T, StartError>, errors: &mut Vec<StartError>) -> Option<T> {
    result.map_err(|error| errors.push(error)).ok()
}

/// Connect to the database, check its schema, load the web certificate and set up both CAs, at the same time, each under the
/// start timeout.
///
/// All steps run to their end even when one fails, so that a start that fails on several things says all of them (a network policy
/// that blocks Postgres and Google at once is one cause with several symptoms, and the first symptom alone sends a person the
/// wrong way). The error returned is the first in this order: database, web certificate, CAs; the others are logged here.
async fn load(
    plan: &Plan,
    database: &DatabaseSettings,
    web_tls: &WebTlsSettings,
    platform: &PlatformSettings,
    edges: &Edges,
) -> Result<Loaded, StartError> {
    let begun = Instant::now();
    let (pool, web_cert, cas) = tokio::join!(
        bounded(plan.start_timeout, "database", async {
            let pool = db::connect(database).await.map_err(StartError::Database)?;
            // Not "migrate if it is behind": the hub holds no credential that could (G4). Behind is an exit that names the
            // migration, and the Job that runs `hub migrate` is what fixes it.
            schema::verify(&pool).await.map_err(StartError::Schema)?;
            Ok(pool)
        }),
        bounded(plan.start_timeout, "web certificate", async {
            WebCert::load(&*edges.secrets, web_tls, &*edges.clock)
                .await
                .map_err(StartError::WebCertificate)
        }),
        async {
            prepare_cas(platform, edges, plan.start_timeout)
                .await
                .map_err(StartError::Platform)
        },
    );
    let mut errors = Vec::new();
    let (pool, web_cert, cas) = (
        keep(pool, &mut errors),
        keep(web_cert, &mut errors),
        keep(cas, &mut errors),
    );
    let (Some(pool), Some(web_cert), Some(cas)) = (pool, web_cert, cas) else {
        let mut errors = errors.into_iter();
        let first = errors.next().unwrap_or(StartError::TimedOut("start"));
        for later in errors {
            error!(error = %later, "the hub could not start");
        }
        return Err(first);
    };
    info!(
        elapsed_ms = u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX),
        "the database, the web certificate and the CAs are ready"
    );
    Ok(Loaded { pool, web_cert, cas })
}

/// Compose the hub from what was loaded: the platform's routes and the identity module's, over the identity module's sessions.
#[allow(clippy::too_many_arguments)]
async fn compose(
    settings: HubSettings,
    plan: &Plan,
    pool: &sqlx::PgPool,
    web_cert: &Arc<WebCert>,
    edges: &Edges,
    platform: &Platform,
    identity: &Identity,
    mounts: IdentityMounts,
    events: Arc<dyn Module>,
    drain: Drain,
) -> Result<Hub, StartError> {
    let dist = plan.web_dist.clone();
    let files = tokio::task::spawn_blocking(move || StaticFiles::open(&dist))
        .await
        .map_err(|_| StartError::WebApp(StaticError::NoRoot))?
        .map_err(StartError::WebApp)?;
    let mut held: Vec<Arc<dyn Expiring>> = vec![Arc::clone(web_cert) as Arc<dyn Expiring>];
    held.extend(platform.certificates.iter().cloned());
    let mut checks: Vec<(&'static str, Arc<dyn crate::ops::ReadyCheck>)> = vec![
        ("postgres", Arc::new(PostgresReady(pool.clone()))),
        ("schema", Arc::new(SchemaReady::new(pool.clone()))),
        (
            "certificates",
            Arc::new(CertsReady::new(Arc::clone(&edges.clock), held)),
        ),
    ];
    checks.extend(platform.checks.iter().cloned());
    let mut modules = platform.modules.clone();
    modules.push(events);
    let IdentityMounts { session, mounted } = mounts;
    let hub = Hub::build_with(
        Deps {
            settings,
            session,
            modules,
            mounted: vec![mounted],
            static_files: Some(Arc::new(files)),
            checks,
        },
        drain,
    )
    .map_err(StartError::Compose)?;
    hub.metrics.register(|registry| {
        web_cert.metrics().register(registry);
        edges.google_metrics.register(registry);
        platform.register_metrics(registry);
        registry.register_collector(Box::new(SourceCollector::new(
            Arc::clone(&identity.metrics) as Arc<dyn CounterSource>
        )));
    });
    Ok(hub)
}

/// Start the hub: check the settings, bind the listeners, load the database, the web certificate and the CAs, build the platform,
/// compose the hub, and serve.
pub async fn start(mut settings: HubSettings, edges: Edges) -> Result<Running, StartError> {
    settings.require_serving()?;
    let database = settings
        .database
        .take()
        .ok_or(StartError::Config(ConfigError::Missing("LK_DATABASE_URL_FILE")))?;
    let web_tls = settings
        .web_tls
        .take()
        .ok_or(StartError::Config(ConfigError::Missing("LK_WEB_CERT_REF")))?;
    let platform = settings
        .platform
        .take()
        .ok_or(StartError::Config(ConfigError::Missing("LK_AGENT_CA_KMS_REF")))?;
    let identity = settings
        .identity
        .take()
        .ok_or(StartError::Config(ConfigError::Missing("LK_ENTRA_TENANT_ID")))?;
    let git = settings
        .git
        .take()
        .ok_or(StartError::Config(ConfigError::Missing("LK_GIT_BASE_URL")))?;
    let plan = Plan {
        web_addr: settings.web_addr,
        ops_addr: settings.ops_addr,
        agent_addr: platform.agent_addr,
        replica_addr: platform.replica_addr,
        web_dist: settings.web_dist.clone(),
        max_connections: settings.max_connections,
        grace: settings.shutdown_grace,
        drain: settings.shutdown_drain,
        start_timeout: settings.start_timeout,
    };
    info!(version = env!("CARGO_PKG_VERSION"), "starting");
    let drain = Drain::new();
    // Boxed: the start holds every step's state at once, and a future that large does not belong on the caller's stack.
    let started = Box::pin(start_with(
        settings,
        edges,
        &plan,
        database,
        web_tls,
        platform,
        identity,
        git,
        drain.clone(),
    ))
    .await;
    if started.is_err() {
        // Whatever was started before the step that failed ends with the process; this makes it end now, in order.
        drain.begin_stopping();
    }
    started
}

#[allow(clippy::too_many_arguments)]
async fn start_with(
    settings: HubSettings,
    edges: Edges,
    plan: &Plan,
    database: DatabaseSettings,
    web_tls: WebTlsSettings,
    platform_settings: PlatformSettings,
    identity_settings: IdentitySettings,
    git: GitSettings,
    drain: Drain,
) -> Result<Running, StartError> {
    let sockets = Sockets::bind(plan).await?;
    let loaded = load(plan, &database, &web_tls, &platform_settings, &edges).await?;
    // The connection URL held the password; the pool has what it needs.
    drop(database);
    let Loaded { pool, web_cert, cas } = loaded;
    // Bounded like every other step of starting: a bus whose first query hangs is an exit that names the step, not a wait.
    let mut platform = bounded(plan.start_timeout, "platform", async {
        Platform::build(PlatformInputs {
            settings: platform_settings,
            git,
            mirror_dir: settings.mirror_dir.clone(),
            blob_cache_bytes: settings.blob_cache_bytes,
            pool: pool.clone(),
            edges: edges.clone(),
            cas,
            closing: drain.closing(),
            // The registry is wired in T5. Until then the hub says plainly that it cannot take a report.
            ingest: Ingest::unavailable(),
            agent_listener: sockets.agent,
            replica_listener: sockets.replica,
            drain: plan.drain,
        })
        .await
        .map_err(StartError::Platform)
    })
    .await?;
    // Who the caller is. The event stream, the one route of the platform that needs the answer, is mounted with it.
    let (identity, mounts) = Identity::build(
        IdentityInputs {
            settings: &identity_settings,
            pool: pool.clone(),
            edges: &edges,
            audit: Arc::clone(&platform.audit),
            leases: Arc::clone(&platform.leases),
            bus: Arc::new(platform.bus.clone()),
            // Teams is the write paths' (T5); until it is wired the notifier says it cannot deliver.
            notifier: Arc::new(UnavailableNotifier),
            closing: drain.closing(),
            start_timeout: plan.start_timeout,
            schedule: AuditSchedule::default(),
        },
        &mut platform.tasks,
    )
    .await?;
    let events = platform.events_module(Arc::clone(&identity.auth) as _);
    let hub = compose(
        settings, plan, &pool, &web_cert, &edges, &platform, &identity, mounts, events, drain,
    )
    .await?;
    // Every module has registered its jobs; the runner starts now, on the leases this replica can win.
    let jobs = platform.start_jobs();
    serve_hub(
        plan,
        &hub,
        web_cert,
        &edges,
        sockets.web,
        sockets.ops,
        platform,
        jobs,
        identity,
    )
}

/// Serve the composed hub on the web and operations listeners (the platform is already serving its two).
#[allow(clippy::too_many_arguments)]
fn serve_hub(
    plan: &Plan,
    hub: &Hub,
    web_cert: Arc<WebCert>,
    edges: &Edges,
    web_listener: TcpListener,
    ops_listener: TcpListener,
    platform: Platform,
    jobs: Option<JobRunnerHandle>,
    identity: Identity,
) -> Result<Running, StartError> {
    let acceptor =
        TlsAcceptor::from(serve::tls13_resolver_config(web_cert.resolver()).map_err(|_| StartError::Tls)?);
    let web_addr = web_listener.local_addr().map_err(|_| StartError::Bind("web"))?;
    let ops_addr = ops_listener
        .local_addr()
        .map_err(|_| StartError::Bind("operations"))?;

    let drain = hub.drain.clone();
    let web_stop = {
        let drain = drain.clone();
        async move { drain.stopping().await }
    };
    let web = tokio::spawn(serve::serve(
        web_listener,
        Some(acceptor),
        hub.web.clone(),
        web_stop,
        ServeLimits {
            max_connections: plan.max_connections,
            drain_timeout: plan.drain,
            ..ServeLimits::default()
        },
    ));
    // The operations listener outlives the web one: `/readyz` has to say "shutting down" until the very end.
    let (ops_stop, mut ops_stopped) = watch::channel(false);
    let ops = tokio::spawn(serve::serve(
        ops_listener,
        None,
        hub.ops.clone(),
        async move {
            let _ = ops_stopped.wait_for(|stopped| *stopped).await;
        },
        ServeLimits {
            max_connections: 64,
            drain_timeout: Duration::from_secs(2),
            ..ServeLimits::default()
        },
    ));
    let mut tasks = platform.tasks;
    tasks.adopt(
        "reload-web-certificate",
        web_cert.spawn_reload(
            Arc::clone(&edges.secrets),
            Arc::clone(&edges.clock),
            drain.closing(),
        ),
    );
    let (agent_addr, replica_addr) = (platform.agent_addr, platform.replica_addr);

    info!(
        web = %web_addr,
        ops = %ops_addr,
        agent = %agent_addr,
        replica = %replica_addr,
        "listening"
    );
    Ok(Running {
        web_addr,
        ops_addr,
        agent_addr,
        replica_addr,
        drain,
        grace: plan.grace,
        web,
        ops,
        ops_stop,
        tasks: Some(tasks),
        jobs,
        identity: Some(identity),
        routes: hub.routes.clone(),
    })
}

impl Running {
    /// Where the web listener is bound (the port is the real one when the setting asked for 0).
    #[must_use]
    pub fn web_addr(&self) -> SocketAddr {
        self.web_addr
    }

    /// Where the operations listener is bound.
    #[must_use]
    pub fn ops_addr(&self) -> SocketAddr {
        self.ops_addr
    }

    /// Where the agent and sentinel listener is bound.
    #[must_use]
    pub fn agent_addr(&self) -> SocketAddr {
        self.agent_addr
    }

    /// Where the replica listener is bound.
    #[must_use]
    pub fn replica_addr(&self) -> SocketAddr {
        self.replica_addr
    }

    /// Every route of every module on the web listener, with its guard (S4).
    #[must_use]
    pub fn routes(&self) -> &[RouteSpec] {
        &self.routes
    }

    /// Resolves with the name of a listener that ended on its own. It never should, until [`Running::drain`] was called.
    async fn listener_died(&mut self) -> &'static str {
        tokio::select! {
            _ = &mut self.web => "web",
            _ = &mut self.ops => "operations",
        }
    }

    /// Not ready, wait the grace period, stop accepting, let connections finish, stop the jobs, wait for the tasks, then stop the
    /// operations listener. A second stop request is the caller's to handle by dropping this future.
    async fn drain(&mut self) {
        self.drain.shutdown(self.grace).await;
        // The jobs are told to stop at the same moment as the listeners, and release their leases while the connections drain, so
        // that another replica can take them at once and the two waits do not add up (the pod's grace period is the sum of the
        // longer one and a margin).
        let jobs = self.jobs.take();
        let stopping_jobs = async {
            if let Some(jobs) = jobs {
                jobs.shutdown().await;
            }
        };
        let (_, ()) = tokio::join!(&mut self.web, stopping_jobs);
        if let Some(tasks) = self.tasks.take() {
            // The gRPC listeners drain connections for as long as the web listener did, in the same time; what is left is waited
            // for briefly and then cut off.
            let overran = tasks.finish(TASKS_GRACE).await;
            if !overran.is_empty() {
                warn!(tasks = ?overran, "tasks that did not stop in time were cut off");
            }
        }
        // The sessions' sweeper and batcher and the revocation listener end with the module, after nothing can ask them anything.
        drop(self.identity.take());
        self.ops_stop.send_replace(true);
        let _ = (&mut self.ops).await;
    }
}

/// Start the hub and serve until `stop` asks, then drain. See the module's notes for what each ending means.
pub async fn run(settings: HubSettings, edges: Edges, mut stop: Stop) -> Outcome {
    let started = tokio::select! {
        biased;
        () = stop.requested() => {
            info!("stop requested while starting; nothing was serving");
            return Outcome::Stopped;
        }
        started = start(settings, edges) => started,
    };
    let mut running = match started {
        Ok(running) => running,
        Err(error) => return Outcome::Failed(error),
    };
    tokio::select! {
        () = stop.requested() => {}
        died = running.listener_died() => {
            error!(listener = died, "a listener stopped by itself");
            return Outcome::Failed(StartError::ListenerStopped(died));
        }
    }
    info!("stop requested; failing readiness and draining");
    tokio::select! {
        () = running.drain() => {
            info!("stopped");
            Outcome::Stopped
        }
        () = stop.requested() => {
            warn!("a second stop request; exiting without waiting for the drain");
            Outcome::Forced
        }
    }
}

/// Print one line for a person: the hub's streams are for machines once telemetry is up, and nothing is up yet.
#[allow(clippy::print_stderr)]
pub(crate) fn say(message: &str) {
    eprintln!("hub: {message}");
}

/// Install the stack's one crypto provider as the process default, so a library that asks for "the" provider cannot panic on
/// an ambiguous choice. The binary links both aws-lc-rs (everything of ours) and ring (the database driver's TLS).
pub fn install_crypto_provider() {
    // Another caller having installed one first is fine: it is the same provider, and the error says only that.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Everything `main` does, for `main` and for the dev example: arguments, settings, the runtime, the provider, the signal
/// streams (before anything is spawned), telemetry, the edges, the run, and the exit code.
///
/// `lookup` is the environment. `make_edges` builds the ports that reach outside the process: Google in the binary, software
/// in the example.
pub fn entry<A, S>(
    args: A,
    lookup: impl Fn(&str) -> Option<String>,
    make_edges: impl FnOnce(&HubSettings) -> Result<Edges, StartError>,
) -> ExitCode
where
    A: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    match cli::parse(args) {
        Ok(Command::Help) => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        // The Job that migrates the schema before the hub rolls (T2): it reads its own settings and starts nothing of the hub.
        Ok(Command::Migrate) => return migrate::entry(lookup),
        // The one-off ceremony that makes a CA certificate (T3): it needs KMS and nothing of the hub's settings.
        Ok(Command::CaInit(args)) => return ca_init::entry(&args),
        Ok(Command::Serve) => {}
        Err(error) => return usage_error(error),
    }
    let settings = match HubSettings::from_lookup(lookup) {
        Ok(settings) => settings,
        Err(error) => {
            say(&error.to_string());
            return ExitCode::from(2);
        }
    };
    if let Err(error) = settings.require_serving() {
        say(&error.to_string());
        return ExitCode::from(2);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("lk-hub")
        .build()
    else {
        say("the async runtime could not be created");
        return ExitCode::from(1);
    };
    let _inside = runtime.enter();
    install_crypto_provider();
    // Before telemetry, before the edges, before any task: a signal from here on is kept until the run looks at it.
    let Ok(stop) = Stop::os_signals() else {
        say("the signal handlers could not be installed");
        return ExitCode::from(1);
    };
    let telemetry = match telemetry::init(&settings) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            say(&error.to_string());
            return ExitCode::from(2);
        }
    };
    let edges = match make_edges(&settings) {
        Ok(edges) => edges,
        Err(error) => {
            error!(%error, "the hub could not start");
            drop(telemetry);
            return ExitCode::from(error.exit_code());
        }
    };
    let outcome = runtime.block_on(run(settings, edges, stop));
    if let Outcome::Failed(error) = &outcome {
        error!(%error, "the hub could not start");
    }
    drop(telemetry);
    // Dropping a runtime waits for every blocking call it has, and a Git fetch in flight is one that cannot be told to stop. The
    // drain has already waited as long as the pod allows, so what is left is cut off: a fetch that is cut off is a fetch the next
    // start does again.
    if outcome == Outcome::Forced {
        // Told twice: do not wait for the tasks that are still draining.
        runtime.shutdown_background();
    } else {
        runtime.shutdown_timeout(RUNTIME_GRACE);
    }
    ExitCode::from(outcome.exit_code())
}

#[allow(clippy::print_stdout)]
fn print_usage() {
    print!("{}", cli::USAGE);
}

fn usage_error(error: CliError) -> ExitCode {
    say(&format!("{error}\n{}", cli::USAGE.trim_end()));
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_setting_exits_2_and_everything_else_that_goes_wrong_exits_1() {
        assert_eq!(StartError::Config(ConfigError::Missing("LK_X")).exit_code(), 2);
        for error in [
            StartError::Google,
            StartError::Database(DbError::Connect),
            StartError::Schema(SchemaError::Query),
            StartError::WebCertificate(WebTlsError::Expired),
            StartError::Tls,
            StartError::Bind("web"),
            StartError::TimedOut("database"),
            StartError::ListenerStopped("web"),
            StartError::Platform(PlatformError::Job("git-poll")),
            StartError::Platform(PlatformError::Ca {
                kind: crate::wiring::CaKind::Replica,
                step: crate::wiring::CaStep::TimedOut,
            }),
        ] {
            assert_eq!(error.exit_code(), 1, "{error}");
        }
        assert_eq!(Outcome::Stopped.exit_code(), 0);
        assert_eq!(Outcome::Forced.exit_code(), 1);
        assert_eq!(Outcome::Failed(StartError::Tls).exit_code(), 1);
        assert_eq!(
            Outcome::Failed(StartError::Config(ConfigError::Invalid("LK_X"))).exit_code(),
            2
        );
    }

    #[test]
    fn a_start_error_names_the_component_and_never_a_value() {
        let shown = [
            StartError::Database(DbError::Connect).to_string(),
            StartError::WebCertificate(WebTlsError::Pem(crate::web_tls::PemError::KeyMismatch)).to_string(),
            StartError::WebApp(StaticError::NoPlaceholder).to_string(),
            StartError::TimedOut("web certificate").to_string(),
            StartError::Schema(SchemaError::Missing {
                version: 4,
                description: "write additions",
            })
            .to_string(),
        ];
        // T3: a CA that is not usable names the CA and the two settings that define it, and what is wrong with it, in fixed words.
        let ca = StartError::Platform(PlatformError::Ca {
            kind: crate::wiring::CaKind::Agent,
            step: crate::wiring::CaStep::Issuer(hub_platform::ca::CaError::KeyMismatch),
        })
        .to_string();
        assert_eq!(
            ca,
            "the agent CA is not usable (LK_AGENT_CA_KMS_REF, LK_AGENT_CA_CERT_REF): the CA certificate does not belong to the KMS key"
        );
        assert!(shown[0].contains("database"));
        assert!(shown[1].contains("LK_WEB_CERT_REF") && shown[1].contains("does not belong"));
        assert!(shown[2].contains("LK_WEB_DIST"));
        assert!(shown[3].contains("web certificate"));
        // T2: a schema that is behind names the migration and the command that fixes it, and the hub does not fix it itself.
        assert!(
            shown[4].contains("0004")
                && shown[4].contains("write additions")
                && shown[4].contains("hub migrate"),
            "{}",
            shown[4]
        );
    }

    /// A stop request that arrives while a start is still waiting ends the run cleanly and starts nothing.
    #[tokio::test]
    async fn a_stop_during_start_exits_cleanly_without_serving() {
        use async_trait::async_trait;
        use domain::Secret;
        use hub_platform::clock::SystemClock;
        use ports::{SecretError, SecretSource};

        /// A secret store that never answers, as Secret Manager does when the network is down.
        struct Hangs;

        #[async_trait]
        impl SecretSource for Hangs {
            async fn get(&self, _name: &str) -> Result<Secret<String>, SecretError> {
                std::future::pending().await
            }
        }

        /// A KMS that has no keys to show.
        struct NoKeys;

        #[async_trait]
        impl crate::edges::PublicKeys for NoKeys {
            async fn spki_der(&self, _key: &ports::KeyRef) -> Result<Vec<u8>, ports::KmsError> {
                Err(ports::KmsError::Unavailable)
            }
        }

        let map: std::collections::HashMap<&str, &str> = [
            ("LK_DATABASE_URL", "postgres://u:p@127.0.0.1:1/x"),
            ("LK_DB_CA_FILE", "/nonexistent.pem"),
            ("LK_WEB_CERT_REF", "web-cert"),
            ("LK_WEB_KEY_REF", "web-key"),
            (
                "LK_AGENT_CA_KMS_REF",
                "projects/p/locations/l/keyRings/r/cryptoKeys/agent/cryptoKeyVersions/1",
            ),
            ("LK_AGENT_CA_CERT_REF", "agent-ca"),
            (
                "LK_REPLICA_CA_KMS_REF",
                "projects/p/locations/l/keyRings/r/cryptoKeys/replica/cryptoKeyVersions/1",
            ),
            ("LK_REPLICA_CA_CERT_REF", "replica-ca"),
            ("LK_REPLICA_NAME", "hub-0"),
            ("LK_REPLICA_ADDRESS", "hub-0.hub.svc:8445"),
            ("LK_HUB_AUDIENCE", "https://hub.example"),
            ("LK_AGENT_ENDPOINT_HOSTS", "agents.example"),
            ("LK_GIT_BASE_URL", "https://github.com/acme/base.git"),
            ("LK_GIT_TENANT_URL", "https://github.com/acme/tenant.git"),
            ("LK_GIT_BASE_REPO", "acme/base"),
            ("LK_GIT_TENANT_REPO", "acme/tenant"),
            ("LK_ENTRA_TENANT_ID", "11111111-1111-4111-8111-111111111111"),
            ("LK_ENTRA_CLIENT_ID", "22222222-2222-4222-8222-222222222222"),
            ("LK_PUBLIC_ORIGIN", "https://hub.example"),
            ("LK_ENTRA_CLIENT_AUTH_REF", "entra-client"),
            ("LK_ENTRA_COOKIE_KEY_REF", "login-cookie"),
            ("LK_AUDIT_CHECKPOINT_BUCKET", "audit-checkpoints"),
            (
                "LK_AUDIT_CHECKPOINT_KMS_REF",
                "projects/p/locations/l/keyRings/r/cryptoKeys/audit/cryptoKeyVersions/1",
            ),
            ("LK_START_TIMEOUT_SECS", "60"),
        ]
        .into_iter()
        .collect();
        let settings = HubSettings::from_lookup(|name| map.get(name).map(|v| (*v).to_owned())).unwrap();
        let edges = Edges::new(
            Arc::new(Hangs),
            Arc::new(SystemClock),
            Arc::new(ports::fakes::FakeKmsSigner::new(&[])),
            Arc::new(NoKeys),
            Arc::new(ports::fakes::FakeTokenVerifier::new()),
            crate::testing::unreachable_identity(),
        );
        let (handle, stop) = Stop::channel();
        handle.stop();
        let outcome = tokio::time::timeout(Duration::from_secs(5), Box::pin(run(settings, edges, stop)))
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::Stopped);
    }

    #[tokio::test]
    async fn a_step_that_hangs_is_an_exit_naming_the_step() {
        let slow = std::future::pending::<Result<(), StartError>>();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            bounded(Duration::from_millis(20), "web certificate", slow),
        )
        .await
        .unwrap();
        assert_eq!(result, Err(StartError::TimedOut("web certificate")));
    }
}
