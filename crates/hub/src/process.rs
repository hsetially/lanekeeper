//! The hub process (plan 18, T1): starts it, serves until told to stop, drains, and says how it ended.
//!
//! What this commit assembles is the shell of the hub: the settings, the database pool, the web certificate, the web listener
//! (TLS 1.3, the certificate reloaded on a timer), the operations listener (`/healthz`, `/readyz`, `/metrics`), the drain, and
//! readiness. The modules come in later tasks and plug into the same `Deps`: identity (T4), registry and writes (T5), MCP (T6).
//!
//! **It never starts on a fake.** Every start step is a real component or an error with a fixed message that names the
//! component, never a value. `Edges` is the only place software can be substituted, and the binary fills it with Google.
//!
//! **Exit codes.** 0: stopped on request. 1: could not start, or stopped for a reason of its own, or was told twice. 2: the
//! command line or the settings were refused (nothing was started).
//!
//! **Stopping.** The first SIGTERM or SIGINT fails readiness, waits the grace period (so the load balancer notices), stops the
//! web accept loop, gives connections in flight the drain period, then stops the operations listener and the background tasks.
//! A second signal at any point exits at once. A signal during start abandons the start and exits 0: nothing was serving.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

use crate::cli::{self, CliError, Command};
use crate::compose::{BuildError, Deps, Hub};
use crate::config::{ConfigError, DatabaseSettings, HubSettings, WebTlsSettings};
use crate::db::{self, DbError, PostgresReady};
use crate::edges::Edges;
use crate::ready::{CertsReady, Expiring};
use crate::serve::{self, ServeLimits};
use crate::session_auth::DenySessions;
use crate::shutdown::{Drain, Stop};
use crate::static_files::{StaticError, StaticFiles};
use crate::telemetry;
use crate::web_tls::{WebCert, WebTlsError};

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
    #[error("the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF): {0}")]
    WebCertificate(WebTlsError),
    #[error("the web listener's TLS configuration was refused")]
    Tls,
    #[error("the web app cannot be served from LK_WEB_DIST: {0}")]
    WebApp(StaticError),
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
    web_dist: PathBuf,
    max_connections: usize,
    grace: Duration,
    drain: Duration,
    start_timeout: Duration,
}

/// A running hub: both listeners are bound and serving.
pub struct Running {
    web_addr: SocketAddr,
    ops_addr: SocketAddr,
    drain: Drain,
    grace: Duration,
    web: JoinHandle<()>,
    ops: JoinHandle<()>,
    ops_stop: watch::Sender<bool>,
    reload: JoinHandle<()>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("web_addr", &self.web_addr)
            .field("ops_addr", &self.ops_addr)
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

/// What loading found: the pool and the certificate the listener will serve with.
struct Loaded {
    pool: sqlx::PgPool,
    web_cert: Arc<WebCert>,
}

/// Connect to the database and load the web certificate, at the same time, each under the start timeout.
///
/// Both steps run to their end even when one fails, so that a start that fails on two things says both (a network policy that
/// blocks Postgres and Google at once is one cause with two symptoms, and the first symptom alone sends a person the wrong
/// way). The error returned is the database's if both failed; the other is logged here.
async fn load(
    plan: &Plan,
    database: &DatabaseSettings,
    web_tls: &WebTlsSettings,
    edges: &Edges,
) -> Result<Loaded, StartError> {
    let begun = Instant::now();
    let (pool, web_cert) = tokio::join!(
        bounded(plan.start_timeout, "database", async {
            db::connect(database).await.map_err(StartError::Database)
        }),
        bounded(plan.start_timeout, "web certificate", async {
            WebCert::load(&*edges.secrets, web_tls, &*edges.clock)
                .await
                .map_err(StartError::WebCertificate)
        }),
    );
    let (pool, web_cert) = match (pool, web_cert) {
        (Ok(pool), Ok(web_cert)) => (pool, web_cert),
        (Err(database), Err(certificate)) => {
            error!(error = %certificate, "the hub could not start");
            return Err(database);
        }
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => return Err(error),
    };
    info!(
        elapsed_ms = u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX),
        "the database and the web certificate are ready"
    );
    Ok(Loaded { pool, web_cert })
}

/// Compose the hub from what was loaded. Until the identity module is wired (T4) nobody has a session.
async fn compose(
    settings: HubSettings,
    plan: &Plan,
    loaded: &Loaded,
    edges: &Edges,
) -> Result<Hub, StartError> {
    let dist = plan.web_dist.clone();
    let files = tokio::task::spawn_blocking(move || StaticFiles::open(&dist))
        .await
        .map_err(|_| StartError::WebApp(StaticError::NoRoot))?
        .map_err(StartError::WebApp)?;
    let held: Vec<Arc<dyn Expiring>> = vec![Arc::clone(&loaded.web_cert) as Arc<dyn Expiring>];
    let hub = Hub::build(Deps {
        settings,
        auth: Arc::new(DenySessions),
        modules: Vec::new(),
        static_files: Some(Arc::new(files)),
        checks: vec![
            ("postgres", Arc::new(PostgresReady(loaded.pool.clone()))),
            (
                "certificates",
                Arc::new(CertsReady::new(Arc::clone(&edges.clock), held)),
            ),
        ],
    })
    .map_err(StartError::Compose)?;
    hub.metrics.register(|registry| {
        loaded.web_cert.metrics().register(registry);
        edges.google_metrics.register(registry);
    });
    Ok(hub)
}

/// Start the hub: check the settings, load the database and the web certificate, compose the hub, and open the listeners.
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
    let plan = Plan {
        web_addr: settings.web_addr,
        ops_addr: settings.ops_addr,
        web_dist: settings.web_dist.clone(),
        max_connections: settings.max_connections,
        grace: settings.shutdown_grace,
        drain: settings.shutdown_drain,
        start_timeout: settings.start_timeout,
    };
    info!(version = env!("CARGO_PKG_VERSION"), "starting");
    let loaded = load(&plan, &database, &web_tls, &edges).await?;
    // The connection URL held the password; the pool has what it needs.
    drop(database);
    let hub = compose(settings, &plan, &loaded, &edges).await?;
    serve_hub(&plan, &hub, loaded.web_cert, &edges).await
}

/// Open both listeners and serve the composed hub on them.
async fn serve_hub(
    plan: &Plan,
    hub: &Hub,
    web_cert: Arc<WebCert>,
    edges: &Edges,
) -> Result<Running, StartError> {
    let acceptor =
        TlsAcceptor::from(serve::tls13_resolver_config(web_cert.resolver()).map_err(|_| StartError::Tls)?);
    let web_listener = TcpListener::bind(plan.web_addr)
        .await
        .map_err(|_| StartError::Bind("web"))?;
    let ops_listener = TcpListener::bind(plan.ops_addr)
        .await
        .map_err(|_| StartError::Bind("operations"))?;
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
    let reload = web_cert.spawn_reload(
        Arc::clone(&edges.secrets),
        Arc::clone(&edges.clock),
        drain.closing(),
    );

    info!(web = %web_addr, ops = %ops_addr, "listening");
    Ok(Running {
        web_addr,
        ops_addr,
        drain,
        grace: plan.grace,
        web,
        ops,
        ops_stop,
        reload,
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

    /// Resolves with the name of a listener that ended on its own. It never should, until [`Running::drain`] was called.
    async fn listener_died(&mut self) -> &'static str {
        tokio::select! {
            _ = &mut self.web => "web",
            _ = &mut self.ops => "operations",
        }
    }

    /// Not ready, wait the grace period, stop accepting, let connections finish, then stop the rest. A second stop request
    /// is the caller's to handle by dropping this future.
    async fn drain(&mut self) {
        self.drain.shutdown(self.grace).await;
        let _ = (&mut self.web).await;
        self.ops_stop.send_replace(true);
        let _ = (&mut self.ops).await;
        let _ = (&mut self.reload).await;
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
fn say(message: &str) {
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
    if outcome == Outcome::Forced {
        // Told twice: do not wait for the tasks that are still draining.
        runtime.shutdown_background();
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
            StartError::WebCertificate(WebTlsError::Expired),
            StartError::Tls,
            StartError::Bind("web"),
            StartError::TimedOut("database"),
            StartError::ListenerStopped("web"),
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
        ];
        assert!(shown[0].contains("database"));
        assert!(shown[1].contains("LK_WEB_CERT_REF") && shown[1].contains("does not belong"));
        assert!(shown[2].contains("LK_WEB_DIST"));
        assert!(shown[3].contains("web certificate"));
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

        let map: std::collections::HashMap<&str, &str> = [
            ("LK_DATABASE_URL", "postgres://u:p@127.0.0.1:1/x"),
            ("LK_DB_CA_FILE", "/nonexistent.pem"),
            ("LK_WEB_CERT_REF", "web-cert"),
            ("LK_WEB_KEY_REF", "web-key"),
            ("LK_START_TIMEOUT_SECS", "60"),
        ]
        .into_iter()
        .collect();
        let settings = HubSettings::from_lookup(|name| map.get(name).map(|v| (*v).to_owned())).unwrap();
        let edges = Edges::new(Arc::new(Hangs), Arc::new(SystemClock));
        let (handle, stop) = Stop::channel();
        handle.stop();
        let outcome = tokio::time::timeout(Duration::from_secs(5), run(settings, edges, stop))
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
