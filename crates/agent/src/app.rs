//! The agent put together (T7): the loops, how they are started, and how they are stopped.
//!
//! [`App::run`] owns the life of the process once the identity exists:
//!
//! - the **scanner** keeps the Merkle tree current and spools the deltas it builds ([`crate::scan`], [`crate::spool`]);
//! - the **spool** housekeeping deletes what the hub acknowledged and keeps the sequence reservation ahead;
//! - the **session** keeps a stream to the hub, reconnecting with backoff ([`crate::transport::session`]);
//! - on every connection the **link handler** answers the hub's commands, sends the heartbeat, and forwards cluster
//!   reports ([`AppHandler`]);
//! - the **joiner** renews the certificate over that stream ([`crate::identity::joiner`]), when there is one;
//! - a small **watchdog** turns the scanner's counters into liveness for `/healthz`.
//!
//! # Stopping
//!
//! When the `shutdown` future given to [`App::run`] completes (SIGTERM, in the binary), the agent stops being ready at
//! once, so that probes and traffic move away. The session then hands what is queued to the connection and closes the
//! stream in an orderly way, which takes at most a few seconds, and everything else is stopped. If any loop ends on
//! its own (which only a panic can make it do), `run` returns an error and the process exits non-zero, so that the
//! kubelet restarts it.
//!
//! # The Kubernetes watchers
//!
//! They are started on the first connection, when the hub's configuration (and with it the environment allowlist) has
//! arrived, and not before: starting earlier would list every Deployment twice. They keep running across reconnects.
//! On every connection the agent first throws away reports that were queued while it was away (the full report that
//! follows says everything they said), sends the full report, and then forwards the deltas as they come.

use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use domain::{ClusterReport, ServiceRef, ShortText};
use proto::convert::FromAgent;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{MissedTickBehavior, interval, timeout};
use tracing::{error, info, warn};

use crate::clock::Clock;
use crate::config::{Settings, Tunables};
use crate::dispatch::{CommandHandler, Dispatcher};
use crate::fileops::FileOps;
use crate::identity::KubeError;
use crate::identity::joiner::{IdentityHandle, Joiner};
use crate::kube::helm::HelmFilter;
use crate::kube::jobs::JobMatcher;
use crate::kube::{Cluster, ClusterOps, ReportError, RestartError, WatchConfig};
use crate::ops::{Health, Metrics, Progress};
use crate::root::NfsRoot;
use crate::scan::{ScanHandler, Scanner};
use crate::spool::Spool;
use crate::transport::session::{Link, LinkHandler, Session, SessionConfig};
use crate::transport::{HubTransport, Outbox};
use crate::tree::{FsSource, Pool, TreeSource, WalkConfig};
use crate::windows::WindowLedger;
use crate::windows::announce::{Told, WindowAnnouncer};

/// A walk or a scan error must be seen within this long, or `/healthz` fails. The hub can slow the walk to 300 s, so
/// this is more than twice that.
pub const SCAN_STALL: Duration = Duration::from_secs(11 * 60);
/// How often the watchdog looks at the scanner's counters.
const WATCHDOG_PERIOD: Duration = Duration::from_secs(5);
/// What the session may take, once asked to stop, beyond its own flush and close waits.
const SHUTDOWN_MARGIN: Duration = Duration::from_secs(3);

/// Why [`App::run`] returned an error.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A loop that must run for as long as the agent does has ended.
    #[error("the {0} loop ended unexpectedly")]
    LoopEnded(&'static str),
    #[error("the worker pool could not be started")]
    Pool,
    #[error("the settings do not make a valid Hello")]
    Hello,
    #[error("{0} holds a glob that does not compile")]
    Glob(&'static str),
}

/// Everything [`App`] is made of. The binary fills it in from the environment ([`crate::process`]); a test fills it with
/// fakes.
pub struct Parts {
    pub settings: Settings,
    pub root: NfsRoot,
    pub transport: Arc<dyn HubTransport>,
    pub identity: Arc<IdentityHandle>,
    pub clock: Arc<dyn Clock>,
    pub health: Arc<Health>,
    pub metrics: Arc<Metrics>,
    /// Renews the certificate over the stream. Without one the certificate is never renewed.
    pub joiner: Option<Arc<Joiner>>,
    /// The Kubernetes API. Without one the cluster commands are answered `UNSUPPORTED` and nothing is reported.
    pub kube: Option<::kube::Client>,
    /// Where the tree comes from. Default: the NFS root, walked on the worker pool.
    pub source: Option<Arc<dyn TreeSource>>,
    /// The durable spool every delta the scanner builds goes through (T9).
    pub spool: Spool,
    pub session: SessionConfig,
}

impl fmt::Debug for Parts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parts")
            .field("swimlane", &self.settings.swimlane.as_str())
            .field("kube", &self.kube.is_some())
            .field("joiner", &self.joiner.is_some())
            .finish_non_exhaustive()
    }
}

impl Parts {
    #[allow(
        clippy::too_many_arguments,
        reason = "one constructor names every part the agent needs; the optional ones have builder methods"
    )]
    pub fn new(
        settings: Settings,
        root: NfsRoot,
        transport: Arc<dyn HubTransport>,
        identity: Arc<IdentityHandle>,
        clock: Arc<dyn Clock>,
        health: Arc<Health>,
        metrics: Arc<Metrics>,
        spool: Spool,
    ) -> Self {
        Self {
            settings,
            root,
            transport,
            identity,
            clock,
            health,
            metrics,
            joiner: None,
            kube: None,
            source: None,
            spool,
            session: SessionConfig::default(),
        }
    }

    #[must_use]
    pub fn with_joiner(mut self, joiner: Arc<Joiner>) -> Self {
        self.joiner = Some(joiner);
        self
    }

    #[must_use]
    pub fn with_kube(mut self, client: ::kube::Client) -> Self {
        self.kube = Some(client);
        self
    }

    #[must_use]
    pub fn with_source(mut self, source: Arc<dyn TreeSource>) -> Self {
        self.source = Some(source);
        self
    }

    #[must_use]
    pub fn with_session_config(mut self, config: SessionConfig) -> Self {
        self.session = config;
        self
    }
}

/// The agent, ready to run.
#[derive(Debug)]
pub struct App {
    parts: Parts,
}

impl App {
    pub fn new(parts: Parts) -> Self {
        Self { parts }
    }

    /// Run until `shutdown` completes or a loop ends. Must be called inside a tokio runtime.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<(), AppError> {
        let Parts {
            settings,
            root,
            transport,
            identity,
            clock,
            health,
            metrics,
            joiner,
            kube,
            source,
            spool,
            session: session_config,
        } = self.parts;

        let source = match source {
            Some(source) => source,
            None => production_source(&settings, &root)?,
        };
        // Everything the scanner builds goes to the spool and is sent from there; the spool also owns the sequence
        // counter, so a restart never repeats a number.
        let scanner = Scanner::with_seq(
            source,
            Arc::clone(&clock),
            spool.sink(),
            Arc::clone(&metrics),
            spool.seq(),
        );

        let (ledger, announcer) = sync_windows(&clock, &scanner, &spool);
        let cluster = match kube {
            Some(client) => Some(Arc::new(ClusterRunner::new(
                client,
                &settings,
                Arc::clone(&clock),
                ledger,
                announcer,
            )?)),
            None => None,
        };
        let ops = FileOps::new(root, Arc::new(scanner.clone()));
        let mut dispatcher = Dispatcher::new(ops).with_metrics(Arc::clone(&metrics));
        if let Some(cluster) = &cluster {
            dispatcher = dispatcher.with_cluster(Arc::clone(cluster) as Arc<dyn ClusterOps>);
        }
        let mut scan_handler = ScanHandler::new(scanner.clone(), spool.clone())
            .with_commands(Arc::new(dispatcher) as Arc<dyn CommandHandler>);
        if let Some(cluster) = &cluster {
            // Applied before the next message is read: a command that follows a configuration sees its allowlist.
            let cluster = Arc::clone(cluster);
            scan_handler = scan_handler.with_config_hook(Arc::new(move |tunables: &Tunables| {
                cluster.apply_allowlist(&tunables.env_allowlist);
            }));
        }
        let handler: Arc<dyn LinkHandler> = Arc::new(AppHandler {
            scan: scan_handler,
            cluster,
        });

        let hello = crate::transport::session::hello(&settings).map_err(|_| AppError::Hello)?;
        let flush_and_close = session_config.flush_timeout;
        let session = Arc::new(
            Session::new(hello, transport, Arc::clone(&identity), session_config)
                .with_metrics(Arc::clone(&metrics)),
        );
        health.follow_connection(session.state());

        // The session is its own task, because shutdown has to wait for it; the other loops are stopped.
        let session_guard = health.task("session");
        let session_task = {
            let (session, handler) = (Arc::clone(&session), Arc::clone(&handler));
            tokio::spawn(async move {
                let _alive = session_guard;
                session.serve(handler).await;
            })
        };
        let mut loops = spawn_loops(&health, &scanner, &spool, &identity, &session, joiner);
        info!(swimlane = settings.swimlane.as_str(), "the agent is running");

        let mut session_task = session_task;
        let outcome = tokio::select! {
            () = shutdown => {
                info!("shutdown requested");
                Ok(())
            }
            ended = loops.join_next() => {
                // Only a panic ends these loops.
                error!("a loop of the agent ended; stopping");
                Err(AppError::LoopEnded(match ended { Some(Ok(name)) => name, _ => "agent" }))
            }
            _ = &mut session_task => {
                error!("the session loop ended; stopping");
                Err(AppError::LoopEnded("session"))
            }
        };

        health.begin_shutdown();
        session.shutdown();
        let grace = flush_and_close + Duration::from_secs(2) + SHUTDOWN_MARGIN;
        if timeout(grace, &mut session_task).await.is_err() {
            warn!("the session did not stop in time; stopping it");
            session_task.abort();
        }
        loops.abort_all();
        while loops.join_next().await.is_some() {}
        info!("the agent has stopped");
        outcome
    }
}

/// The sync Jobs' windows: the watchers feed the ledger, the scanner tags what it finds with them, and the announcer tells
/// the hub, closing a window only after the last delta tagged with it (T10, D75).
fn sync_windows(
    clock: &Arc<dyn Clock>,
    scanner: &Scanner,
    spool: &Spool,
) -> (Arc<WindowLedger>, Arc<WindowAnnouncer>) {
    let ledger = Arc::new(WindowLedger::new(Arc::clone(clock)));
    scanner.attach_windows(Arc::clone(&ledger));
    let announcer = Arc::new(WindowAnnouncer::new(
        Arc::clone(&ledger),
        scanner.clone(),
        spool.clone(),
    ));
    (ledger, announcer)
}

/// The loops that run for as long as the agent does, each with the claim to be alive that `/healthz` reads.
fn spawn_loops(
    health: &Health,
    scanner: &Scanner,
    spool: &Spool,
    identity: &Arc<IdentityHandle>,
    session: &Arc<Session>,
    joiner: Option<Arc<Joiner>>,
) -> JoinSet<&'static str> {
    let mut loops: JoinSet<&'static str> = JoinSet::new();
    {
        let scanner = scanner.clone();
        let guard = health.task("scanner");
        loops.spawn(async move {
            let _alive = guard;
            let never: Infallible = scanner.run().await;
            match never {}
        });
    }
    loops.spawn(watchdog(
        scanner.clone(),
        health.progress("scanner progress", SCAN_STALL),
    ));
    {
        let spool = spool.clone();
        let guard = health.task("spool housekeeping");
        loops.spawn(async move {
            let _alive = guard;
            let never: Infallible = spool.run_maintenance().await;
            match never {}
        });
    }
    if let Some(joiner) = joiner {
        let (identity, session) = (Arc::clone(identity), Arc::clone(session));
        let guard = health.task("certificate renewal");
        loops.spawn(async move {
            let _alive = guard;
            let never: Infallible = joiner.maintain(&identity, &*session).await;
            match never {}
        });
    }
    loops
}

/// The production tree source: the NFS root, walked and hashed on a pool of the configured size.
fn production_source(settings: &Settings, root: &NfsRoot) -> Result<Arc<dyn TreeSource>, AppError> {
    let ignore: Vec<&str> = settings.ignore_globs.iter().map(ShortText::as_str).collect();
    // Deny globs arrive with the hub's configuration (T11); until then nothing is denied.
    let walk = WalkConfig::new(&ignore, &[]).map_err(|_| AppError::Glob("LK_IGNORE_GLOBS"))?;
    let pool = Pool::new(settings.pool_threads).map_err(|_| AppError::Pool)?;
    Ok(Arc::new(FsSource::new(root.clone(), pool, walk)))
}

/// Turns the scanner's counters into liveness: a scan that finished, failed or was held counts as progress.
async fn watchdog(scanner: Scanner, progress: Progress) -> &'static str {
    let mut ticker = interval(WATCHDOG_PERIOD);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut seen = 0;
    loop {
        ticker.tick().await;
        let stats = scanner.stats();
        let now = stats.scans() + stats.scan_errors() + stats.empty_root_holds();
        if now != seen {
            seen = now;
            progress.tick();
        }
    }
}

// ------------------------------------------------------------------------------------------ the link handler

/// What the agent does on one connection: the scanner's part, and forwarding cluster reports.
struct AppHandler {
    scan: ScanHandler,
    cluster: Option<Arc<ClusterRunner>>,
}

#[async_trait]
impl LinkHandler for AppHandler {
    async fn handle(&self, link: Link) {
        let Some(cluster) = &self.cluster else {
            return self.scan.handle(link).await;
        };
        let outbox = link.outbox.clone();
        let tunables = link.tunables.clone();
        // What this connection has been told about the sync windows: every connection starts afresh (A21).
        let told = Mutex::new(Told::default());
        // The watchers are started before the first command is read, so that a restart or a report asked for in the
        // first moment is answered by a cluster that exists.
        cluster.start_once(&tunables.env_allowlist);
        tokio::select! {
            () = self.scan.handle(link) => {}
            () = cluster.forward(&outbox, &tunables, &told) => {}
            () = cluster.announcer.run(&outbox, &told) => {}
        }
    }
}

/// The Kubernetes side, started on the first connection.
struct ClusterRunner {
    client: ::kube::Client,
    config: WatchConfig,
    clock: Arc<dyn Clock>,
    announcer: Arc<WindowAnnouncer>,
    started: OnceLock<Running>,
}

struct Running {
    cluster: Arc<Cluster>,
    /// Taken by the connection that forwards the reports, and put back when it ends.
    reports: Mutex<Option<mpsc::Receiver<ClusterReport>>>,
}

impl fmt::Debug for ClusterRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClusterRunner")
            .field("started", &self.started.get().is_some())
            .finish_non_exhaustive()
    }
}

/// Hands the report receiver back when the connection that took it ends.
struct Lease<'a> {
    slot: &'a Mutex<Option<mpsc::Receiver<ClusterReport>>>,
    reports: Option<mpsc::Receiver<ClusterReport>>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(reports) = self.reports.take() {
            *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(reports);
        }
    }
}

impl ClusterRunner {
    fn new(
        client: ::kube::Client,
        settings: &Settings,
        clock: Arc<dyn Clock>,
        ledger: Arc<WindowLedger>,
        announcer: Arc<WindowAnnouncer>,
    ) -> Result<Self, AppError> {
        let config = WatchConfig::new(
            settings.namespaces.clone(),
            HelmFilter::new(&settings.helm_hint_chart_globs)
                .map_err(|_| AppError::Glob("LK_HELM_HINT_CHART_GLOBS"))?,
            JobMatcher::new(&settings.sync_job_name_globs, settings.sync_job_label.as_ref())
                .map_err(|_| AppError::Glob("LK_SYNC_JOB_NAME_GLOBS"))?,
        )
        .with_windows(ledger);
        Ok(Self {
            client,
            config,
            clock,
            announcer,
            started: OnceLock::new(),
        })
    }

    /// Start the watchers with the hub's allowlist, unless they are running already.
    fn start_once(&self, allowlist: &[ShortText]) -> &Running {
        self.started.get_or_init(|| {
            let config = self.config.clone().with_env_allowlist(allowlist);
            let (cluster, reports) = Cluster::start_with(&self.client, config, Arc::clone(&self.clock));
            Running {
                cluster: Arc::new(cluster),
                reports: Mutex::new(Some(reports)),
            }
        })
    }

    /// The hub's allowlist, when it differs from the one the watchers use they list their Deployments again.
    fn apply_allowlist(&self, names: &[ShortText]) {
        if let Some(running) = self.started.get() {
            running.cluster.set_env_allowlist(names);
        }
    }

    /// Send the full report, then every delta, until the connection ends.
    async fn forward(&self, outbox: &Outbox, tunables: &Tunables, told: &Mutex<Told>) {
        let running = self.start_once(&tunables.env_allowlist);
        // A hub that connects again with another allowlist makes the watchers list again.
        running.cluster.set_env_allowlist(&tunables.env_allowlist);
        let Some(reports) = running
            .reports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            warn!("the cluster reports are still in use by the previous connection; not forwarding");
            return std::future::pending().await;
        };
        let mut lease = Lease {
            slot: &running.reports,
            reports: Some(reports),
        };
        let Some(reports) = lease.reports.as_mut() else {
            return;
        };
        // Reports queued while the hub was away are older than the full report that follows, and say less.
        while reports.try_recv().is_ok() {}
        match running.cluster.full_report().await {
            Ok(mut report) => {
                // The windows this connection may be told of now; the closes follow once the spool has drained (A21).
                report.sync_windows = self
                    .announcer
                    .due(&mut told.lock().unwrap_or_else(PoisonError::into_inner));
                if send(outbox, report).await.is_err() {
                    return;
                }
            }
            Err(ReportError::NotSynced) => {
                warn!(
                    "the cluster has not been listed yet; no full report on connect (the hub can ask for one)"
                );
            }
        }
        while let Some(report) = reports.recv().await {
            if send(outbox, report).await.is_err() {
                return;
            }
        }
        std::future::pending::<()>().await;
    }
}

/// Queue a cluster report. `Err` means the connection is gone; a report that can never fit is logged and skipped.
async fn send(outbox: &Outbox, report: ClusterReport) -> Result<(), ()> {
    match outbox.send(FromAgent::Cluster(report)).await {
        Ok(()) => Ok(()),
        Err(crate::transport::OutboxError::TooLarge) => {
            warn!("a cluster report is too large to send; skipped");
            Ok(())
        }
        Err(_) => Err(()),
    }
}

#[async_trait]
impl ClusterOps for ClusterRunner {
    async fn full_report(&self) -> Result<ClusterReport, ReportError> {
        match self.started.get() {
            Some(running) => {
                let mut report = running.cluster.full_report().await?;
                report.sync_windows = self.announcer.snapshot();
                Ok(report)
            }
            None => Err(ReportError::NotSynced),
        }
    }

    async fn restart(&self, service: &ServiceRef) -> Result<(), RestartError> {
        match self.started.get() {
            Some(running) => running.cluster.restart(service).await,
            None => Err(RestartError::Api(KubeError::Transport {
                op: "restart a Deployment",
            })),
        }
    }
}
