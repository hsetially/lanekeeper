//! Everything `hub-platform` provides, wired in the order the dependencies run (plan 18, T3).
//!
//! `Platform::build` makes, from the settings and the edges: the audit log, the leases (under this replica's name), the event bus
//! and its listener, the blob store, the Git mirrors with the fetch worker, the syncer, the reader and the webhook, the agent
//! gateway over the replicas, the join service, the `Agent`, `Sentinel` and `Replica` services, and the two mTLS listeners that
//! serve them. It does not start the job runner: the registry and the write paths register their jobs before it starts
//! (`Platform::jobs`, then `start_jobs`).
//!
//! Every task it spawns is adopted by [`Tasks`](crate::wiring::Tasks), stops on the `closing` signal, and is a reason for
//! `/readyz` to fail if it ends early. No fake is built here: the edges are the only seam, and what ingests the reports is a
//! parameter ([`Ingest`]) that T5 fills with the registry.
//!
//! The event stream is not mounted by `build`. It needs the module that says who the caller is (T4), and that module needs this
//! one's bus, audit log and leases, so the stream is added afterwards with [`Platform::events_module`].

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use domain::{
    AgentConfig, AuditRecordBatch, ClusterReport, Heartbeat, HeartbeatAction, Hello, RepoKind, ScanDelta,
    SentinelConfig, SentinelHello,
};
use hub_identity::audit::PgAuditLog;
use hub_platform::SessionAuth;
use hub_platform::agent_metrics::AgentMetrics;
use hub_platform::agent_service::{AgentService, StreamLimits};
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PgBlobStore};
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::EventMetrics;
use hub_platform::events::sse::{SseSettings, SseState};
use hub_platform::events::visibility::RoleVisibility;
use hub_platform::gateway::forward::{ReplicaClient, ReplicaClientLimits};
use hub_platform::gateway::metrics::GatewayMetrics;
use hub_platform::gateway::registry::ConnectionRegistry;
use hub_platform::gateway::replica_service::ReplicaService;
use hub_platform::gateway::{GatewaySettings, HubGateway};
use hub_platform::git::credentials::{GitCredentials, MountedFileCredentials, SecretManagerCredentials};
use hub_platform::git::deliveries::PgDeliveryLog;
use hub_platform::git::fetch_queue::FetchQueue;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorSettings};
use hub_platform::git::reader::{GixReader, ReaderSettings};
use hub_platform::git::sync::{
    Announcer, FetchWorker, MirrorSync, MirrorSyncer, RetrySettings, StartupRetry, spawn_initialize,
};
use hub_platform::git::webhook::{WebhookSettings, WebhookState};
use hub_platform::jobs::{JobMetrics, JobRunner, JobRunnerHandle};
use hub_platform::join::{JoinLimits, JoinService, JoinSettings};
use hub_platform::leases::PgLeases;
use hub_platform::listener::{GrpcLimits, ListenerStats, serve_grpc};
use hub_platform::sentinel_service::{SentinelLimits, SentinelMetrics, SentinelService};
use hub_platform::server_identity::spawn_rotation;
use hub_platform::tls::{ClientTls, RotatingServerCert, ServerTls, roots_from_der};
use ports::{
    AckSeq, AgentIdentity, AuditLog, EventBus, Leases, ReportSink, SentinelIdentity, SentinelSink, SinkError,
};
use prometheus_client::registry::Registry;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::PlatformError;
use super::grpc::Cas;
use super::jobs::{self, JobParts};
use super::tasks::Tasks;
use crate::compose::Module;
use crate::config::{GitCredentialSource, GitSettings, PlatformSettings};
use crate::edges::Edges;
use crate::events::EventsModule;
use crate::git::{GitModule, MirrorsReady};
use crate::ops::ReadyCheck;
use crate::ready::{EventsReady, Expiring, JobsReady, RotatingExpiry};

/// How long a replica's claim on an agent connection may go unrenewed before the others call it dead (gateway docs: 30 s).
const STALE_AFTER: Duration = Duration::from_secs(30);

/// What takes the reports that agents and sentinels send. T5 passes the registry's; until then the hub passes
/// [`Ingest::unavailable`], which refuses every message with a typed error, so an agent that joins is told the hub cannot take its
/// report yet and nothing is stored or acknowledged that was not.
#[derive(Clone)]
pub struct Ingest {
    pub agents: Arc<dyn ReportSink>,
    pub sentinels: Arc<dyn SentinelSink>,
}

impl std::fmt::Debug for Ingest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ingest").finish_non_exhaustive()
    }
}

impl Ingest {
    /// Refuses everything: the sinks of a hub whose registry is not wired yet (plan 18 G25).
    #[must_use]
    pub fn unavailable() -> Self {
        let sink = Arc::new(UnavailableIngest);
        Self {
            agents: sink.clone(),
            sentinels: sink,
        }
    }
}

/// Answers every message with [`SinkError::Unavailable`].
#[derive(Debug, Clone, Copy)]
struct UnavailableIngest;

#[async_trait]
impl ReportSink for UnavailableIngest {
    async fn hello(&self, _id: &AgentIdentity, _h: Hello) -> Result<AgentConfig, SinkError> {
        Err(SinkError::Unavailable)
    }

    async fn heartbeat(&self, _id: &AgentIdentity, _hb: Heartbeat) -> Result<HeartbeatAction, SinkError> {
        Err(SinkError::Unavailable)
    }

    async fn delta(&self, _id: &AgentIdentity, _d: ScanDelta) -> Result<(), SinkError> {
        Err(SinkError::Unavailable)
    }

    async fn cluster(&self, _id: &AgentIdentity, _c: ClusterReport) -> Result<(), SinkError> {
        Err(SinkError::Unavailable)
    }
}

#[async_trait]
impl SentinelSink for UnavailableIngest {
    async fn hello(&self, _id: &SentinelIdentity, _h: SentinelHello) -> Result<SentinelConfig, SinkError> {
        Err(SinkError::Unavailable)
    }

    async fn records(&self, _id: &SentinelIdentity, _batch: AuditRecordBatch) -> Result<AckSeq, SinkError> {
        Err(SinkError::Unavailable)
    }
}

/// What `Platform::build` is made from.
pub struct PlatformInputs {
    pub settings: PlatformSettings,
    pub git: GitSettings,
    /// Where the Git mirrors live: the volume that survives a restart.
    pub mirror_dir: PathBuf,
    /// What the blob cache may hold.
    pub blob_cache_bytes: u64,
    pub pool: PgPool,
    pub edges: Edges,
    /// Both authorities, set up by [`prepare_cas`](crate::wiring::prepare_cas).
    pub cas: Cas,
    /// Turns true when the hub stops: every task and stream of the platform ends then.
    pub closing: watch::Receiver<bool>,
    pub ingest: Ingest,
    /// The agent listener's socket, already bound.
    pub agent_listener: TcpListener,
    /// The replica listener's socket, already bound. Its address must be the one `settings.replica_address` advertises.
    pub replica_listener: TcpListener,
    /// How long connections in flight get after a listener stops accepting.
    pub drain: Duration,
}

impl std::fmt::Debug for PlatformInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformInputs")
            .field("replica", &self.settings.replica_name)
            .finish_non_exhaustive()
    }
}

/// The platform, built and serving its two gRPC listeners.
pub struct Platform {
    /// The routes this adds to the web router: the Git webhook. The event stream follows with [`Platform::events_module`].
    pub modules: Vec<Arc<dyn Module>>,
    /// Readiness: the mirrors, the bus, the jobs and the tasks.
    pub checks: Vec<(&'static str, Arc<dyn ReadyCheck>)>,
    /// The certificates the platform serves with, for the readiness check over expiry.
    pub certificates: Vec<Arc<dyn Expiring>>,
    /// The jobs registered so far. The registry and the write paths add theirs; [`Platform::start_jobs`] starts them.
    pub jobs: Option<JobRunner>,
    pub tasks: Tasks,
    pub gateway: Arc<HubGateway>,
    pub bus: PgEventBus,
    pub audit: Arc<dyn AuditLog>,
    pub leases: Arc<dyn Leases>,
    pub blobs: Arc<PgBlobStore>,
    pub mirror: Arc<Mirror>,
    pub reader: Arc<GixReader>,
    pub queue: Arc<FetchQueue>,
    pub agent_addr: SocketAddr,
    pub replica_addr: SocketAddr,
    pub agent_stats: Arc<ListenerStats>,
    pub replica_stats: Arc<ListenerStats>,
    pub agent_cert: Arc<RotatingServerCert>,
    pub replica_cert: Arc<RotatingServerCert>,
    pub job_metrics: JobMetrics,
    jobs_running: Arc<AtomicBool>,
    metrics: Metrics,
    closing: watch::Receiver<bool>,
}

impl std::fmt::Debug for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Platform")
            .field("agent_addr", &self.agent_addr)
            .field("replica_addr", &self.replica_addr)
            .finish_non_exhaustive()
    }
}

/// The platform's counters, kept so they can be registered once the hub's registry exists.
#[derive(Clone)]
struct Metrics {
    events: EventMetrics,
    blobs: BlobMetrics,
    git: GitMetrics,
    agents: AgentMetrics,
    sentinels: SentinelMetrics,
    gateway: GatewayMetrics,
    jobs: JobMetrics,
}

/// Resolves when `closing` is true (or its sender is gone).
async fn closed(mut closing: watch::Receiver<bool>) {
    let _ = closing.wait_for(|stopping| *stopping).await;
}

fn credentials(git: &GitSettings, edges: &Edges) -> Arc<dyn GitCredentials> {
    match &git.credential {
        GitCredentialSource::SecretManager(name) => {
            Arc::new(SecretManagerCredentials::new(Arc::clone(&edges.secrets), name))
        }
        GitCredentialSource::MountedFile(path) => Arc::new(MountedFileCredentials::new(path.clone())),
    }
}

impl Platform {
    /// Build the platform and start its listeners.
    #[allow(clippy::too_many_lines)]
    pub async fn build(inputs: PlatformInputs) -> Result<Self, PlatformError> {
        let PlatformInputs {
            settings,
            git,
            mirror_dir,
            blob_cache_bytes,
            pool,
            edges,
            cas,
            closing,
            ingest,
            agent_listener,
            replica_listener,
            drain,
        } = inputs;
        let metrics = Metrics {
            events: EventMetrics::default(),
            blobs: BlobMetrics::default(),
            git: GitMetrics::default(),
            agents: AgentMetrics::default(),
            sentinels: SentinelMetrics::default(),
            gateway: GatewayMetrics::default(),
            jobs: JobMetrics::default(),
        };
        let mut tasks = Tasks::new(closing.clone());
        let mut checks: Vec<(&'static str, Arc<dyn ReadyCheck>)> = Vec::new();

        // The ports that sit on Postgres.
        let audit: Arc<dyn AuditLog> = Arc::new(PgAuditLog::new());
        let leases: Arc<dyn Leases> = Arc::new(PgLeases::new(pool.clone(), &settings.replica_name));
        let bus = PgEventBus::new(pool.clone(), BusSettings::default(), metrics.events.clone());
        let bus_tasks = bus.start(closing.clone()).await;
        tasks.adopt("event-listener", bus_tasks.listener);
        tasks.adopt("event-relay", bus_tasks.relay);
        let bus_port: Arc<dyn EventBus> = Arc::new(bus.clone());
        let listening = bus.clone();
        checks.push((
            "events",
            Arc::new(EventsReady::new(move || listening.listener_connected())),
        ));
        let blobs = Arc::new(PgBlobStore::new(
            pool.clone(),
            BlobSettings {
                cache_bytes: blob_cache_bytes,
                ..BlobSettings::default()
            },
            metrics.blobs.clone(),
        ));

        // Git: the mirrors, what keeps them current, and the webhook that says when.
        let mut mirror_settings =
            MirrorSettings::new(mirror_dir, git.base_url.clone(), git.tenant_url.clone());
        mirror_settings.fetch_timeout = git.fetch_timeout;
        let mirror = Mirror::open(mirror_settings, credentials(&git, &edges), metrics.git.clone())
            .map_err(PlatformError::Mirror)?;
        let was_empty = !mirror.ready();
        let announcer =
            Announcer::start(Arc::clone(&mirror), Arc::clone(&bus_port), metrics.git.clone()).await;
        tasks.adopt_finite(
            "mirror-initialize",
            fill_mirrors(&mirror, &announcer, was_empty, closing.clone()),
        );
        checks.push(("mirrors", Arc::new(MirrorsReady::new(Arc::clone(&mirror)))));
        let queue = FetchQueue::new(git.queue_capacity);
        let syncer = Arc::new(MirrorSyncer::new(Arc::clone(&mirror), Arc::clone(&announcer)));
        tasks.adopt(
            "git-fetch",
            FetchWorker::new(Arc::clone(&queue), syncer.clone(), RetrySettings::default())
                .spawn(closing.clone()),
        );
        tasks.adopt(
            "git-follow",
            MirrorSync::new(Arc::clone(&mirror), announcer, Arc::clone(&bus_port)).spawn(closing.clone()),
        );
        let reader = Arc::new(
            GixReader::new(
                Arc::clone(&mirror),
                &ReaderSettings {
                    tree_index_entries: git.tree_index_entries,
                    ..ReaderSettings::default()
                },
                metrics.git.clone(),
            )
            .map_err(PlatformError::Reader)?,
        );
        let deliveries = PgDeliveryLog::new(pool.clone());
        let webhook = WebhookState::new(
            WebhookSettings::new(
                &git.webhook_secret,
                vec![
                    (RepoKind::Base, git.base_repo.clone()),
                    (RepoKind::Tenant, git.tenant_repo.clone()),
                ],
            ),
            Arc::clone(&edges.secrets),
            Arc::new(deliveries.clone()),
            Arc::clone(&queue),
            metrics.git.clone(),
        );

        // The agent gateway over the replicas, and the join service every agent and sentinel starts with.
        let replica_roots = roots_from_der(&[cas.replica.root_der.clone()]).map_err(PlatformError::Tls)?;
        let agent_roots = roots_from_der(&[cas.agent.root_der.clone()]).map_err(PlatformError::Tls)?;
        let registry = ConnectionRegistry::new(
            pool.clone(),
            settings.replica_name.clone(),
            settings.replica_address.clone(),
            STALE_AFTER,
        );
        let client = ReplicaClient::new(
            ClientTls::new_rotating(
                replica_roots.clone(),
                Arc::clone(&cas.replica.cert),
                Arc::clone(&edges.clock),
            )
            .map_err(PlatformError::Tls)?,
            ReplicaClientLimits::default(),
        );
        let gateway = HubGateway::new(
            registry,
            client,
            metrics.gateway.clone(),
            GatewaySettings::default(),
        );
        tasks.adopt("gateway-liveness", gateway.spawn_liveness(closing.clone()));
        let join = Arc::new(
            JoinService::new(
                pool.clone(),
                Arc::clone(&edges.tokens),
                Arc::clone(&audit),
                Arc::clone(&cas.agent.issuer),
                Arc::clone(&edges.clock),
                JoinSettings {
                    hub_audience: settings.audience.clone(),
                    limits: JoinLimits::default(),
                },
            )
            .with_metrics(metrics.agents.clone()),
        );

        // The two mTLS listeners. Each trusts the clients of its own CA and no other.
        let limits = GrpcLimits {
            drain_timeout: drain,
            ..GrpcLimits::default()
        };
        let agent_routes = SentinelService::new(
            Arc::clone(&join),
            ingest.sentinels,
            closing.clone(),
            metrics.sentinels.clone(),
            SentinelLimits::default(),
        )
        .add_to(
            AgentService::new(
                join,
                ingest.agents,
                closing.clone(),
                metrics.agents.clone(),
                StreamLimits::default(),
            )
            .with_gateway(Arc::clone(&gateway))
            .routes(),
        );
        let agent_tls = ServerTls::new(agent_roots, Arc::clone(&cas.agent.cert), Arc::clone(&edges.clock))
            .map_err(PlatformError::Tls)?;
        let agent_stats = Arc::new(ListenerStats::default());
        let agent_addr = agent_listener
            .local_addr()
            .map_err(|_| PlatformError::Listener("agent"))?;
        tasks.adopt(
            "grpc-agent",
            tokio::spawn(serve_grpc(
                agent_listener,
                agent_tls.acceptor(),
                agent_routes,
                closed(closing.clone()),
                limits,
                Arc::clone(&agent_stats),
            )),
        );
        let replica_tls = ServerTls::new(
            replica_roots,
            Arc::clone(&cas.replica.cert),
            Arc::clone(&edges.clock),
        )
        .map_err(PlatformError::Tls)?;
        let replica_stats = Arc::new(ListenerStats::default());
        let replica_addr = replica_listener
            .local_addr()
            .map_err(|_| PlatformError::Listener("replica"))?;
        tasks.adopt(
            "grpc-replica",
            tokio::spawn(serve_grpc(
                replica_listener,
                replica_tls.acceptor(),
                ReplicaService::new(gateway.local(), metrics.gateway.clone()).routes(),
                closed(closing.clone()),
                limits,
                Arc::clone(&replica_stats),
            )),
        );

        // The hub's own certificates are renewed at half their life, by tasks that stop with the hub.
        for (name, ca) in [
            ("renew-agent-certificate", &cas.agent),
            ("renew-replica-certificate", &cas.replica),
        ] {
            tasks.adopt(
                name,
                spawn_rotation(
                    Arc::clone(&ca.issuer),
                    ca.profile.clone(),
                    Arc::clone(&ca.cert),
                    &ca.issued,
                    closing.clone(),
                ),
            );
        }

        // The four jobs of the platform. The runner starts later, when everything that registers a job has.
        let mut runner = JobRunner::new(Arc::clone(&leases), metrics.jobs.clone());
        jobs::register(
            &mut runner,
            JobParts {
                pool: pool.clone(),
                events: metrics.events.clone(),
                git: metrics.git.clone(),
                syncer,
                reader: Arc::clone(&reader),
                deliveries,
            },
        )?;
        let jobs_running = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&jobs_running);
        checks.push((
            "jobs",
            Arc::new(JobsReady::new(move || running.load(Ordering::SeqCst))),
        ));

        let modules: Vec<Arc<dyn Module>> = vec![Arc::new(GitModule::new(Arc::new(webhook)))];
        checks.push(("tasks", Arc::new(tasks.alive())));
        let certificates: Vec<Arc<dyn Expiring>> = vec![
            Arc::new(RotatingExpiry::new(Arc::clone(&cas.agent.cert))),
            Arc::new(RotatingExpiry::new(Arc::clone(&cas.replica.cert))),
        ];
        Ok(Self {
            modules,
            checks,
            certificates,
            jobs: Some(runner),
            tasks,
            gateway,
            bus,
            audit,
            leases,
            blobs,
            mirror,
            reader,
            queue,
            agent_addr,
            replica_addr,
            agent_stats,
            replica_stats,
            agent_cert: Arc::clone(&cas.agent.cert),
            replica_cert: Arc::clone(&cas.replica.cert),
            job_metrics: metrics.jobs.clone(),
            jobs_running,
            metrics,
            closing,
        })
    }

    /// `GET /api/v1/events`, answered to the callers `auth` vouches for (T4). Its streams end with the hub, and each ends with the
    /// session that opened it when `auth` tells sessions apart (G2).
    #[must_use]
    pub fn events_module(&self, auth: Arc<dyn SessionAuth>) -> Arc<dyn Module> {
        let sse = SseState::new(
            self.bus.source(),
            auth,
            Arc::new(RoleVisibility),
            SseSettings::default(),
            self.closing.clone(),
            self.metrics.events.clone(),
        );
        Arc::new(EventsModule::new(sse))
    }

    /// Start the registered jobs. Call once, after the registry and the write paths have added theirs; a second call returns
    /// `None`. Stop them with [`JobRunnerHandle::shutdown`], which releases the leases they hold.
    pub fn start_jobs(&mut self) -> Option<JobRunnerHandle> {
        let runner = self.jobs.take()?;
        let handle = runner.start();
        self.jobs_running.store(true, Ordering::SeqCst);
        Some(handle)
    }

    /// Register the platform's counters on the hub's registry.
    pub fn register_metrics(&self, registry: &mut Registry) {
        self.metrics.events.register(registry);
        self.metrics.blobs.register(registry);
        self.metrics.git.register(registry);
        self.metrics.agents.register(registry);
        self.metrics.sentinels.register(registry);
        self.metrics.gateway.register(registry);
        self.metrics.jobs.register(registry);
    }
}

/// Fill any mirror that is empty, and with it the baseline of what has already been announced.
///
/// A replica that starts on an empty volume would otherwise announce every branch of both repositories as moved, to every
/// replica and the registry, as the first fetch completes. The heads that are on disk when the fill is done are what the other
/// replicas have already seen, so they are recorded as known (`observe`) and only later moves are announced. A volume that
/// already held the mirrors needs none of this: `Announcer::start` took its baseline from them.
fn fill_mirrors(
    mirror: &Arc<Mirror>,
    announcer: &Arc<Announcer>,
    was_empty: bool,
    closing: watch::Receiver<bool>,
) -> JoinHandle<()> {
    let filling = spawn_initialize(Arc::clone(mirror), StartupRetry::default(), closing);
    let (mirror, announcer) = (Arc::clone(mirror), Arc::clone(announcer));
    tokio::spawn(async move {
        let _ = filling.await;
        if !was_empty || !mirror.ready() {
            return;
        }
        for repo in [RepoKind::Base, RepoKind::Tenant] {
            let reading = Arc::clone(&mirror);
            let Ok(Ok(snapshot)) = tokio::task::spawn_blocking(move || reading.refs(repo)).await else {
                continue;
            };
            for (branch, head) in &snapshot.branches {
                announcer.observe(repo, branch, head).await;
            }
        }
    })
}
