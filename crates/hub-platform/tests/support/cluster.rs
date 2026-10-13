//! N hub replicas in one process over one real Postgres, for the multi-replica tests (D62).
//!
//! Each replica is built from the same pieces the hub is built from: its own connection pool, join service, `AgentService`
//! behind the agent listener (agent CA), `HubGateway`, and `ReplicaService` behind the replica listener (replica CA). Fake
//! agents speak the real protocol over real mutual TLS with certificates from the CA under test, so routing, forwarding and
//! certificates are exercised together.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use domain::ShortText;
use hub_platform::agent_metrics::AgentMetrics;
use hub_platform::agent_service::{AgentService, StreamLimits};
use hub_platform::ca::CaIssuer;
use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::gateway::forward::{PeerAddress, ReplicaClient, ReplicaClientLimits};
use hub_platform::gateway::metrics::GatewayMetrics;
use hub_platform::gateway::registry::ConnectionRegistry;
use hub_platform::gateway::replica_service::ReplicaService;
use hub_platform::gateway::{GatewaySettings, HubGateway};
use hub_platform::join::{JoinLimits, JoinService, JoinSettings};
use hub_platform::listener::{GrpcLimits, ListenerStats, serve_grpc};
use hub_platform::server_identity::issue_server_identity;
use hub_platform::tls::{ClientTls, RotatingServerCert, ServerTls, roots_from_der};
use ports::fakes::{FakeReportSink, FakeTokenVerifier};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use super::ca::{REPLICA_CA_KEY, TestCa};
use super::fake_agent::FakeAgent;
use super::grpc::TestServer;
use super::pg::{PgTestAuditLog, TestDb};

pub const AUDIENCE: &str = "https://hub.lanekeeper.test";
/// The DNS name on every replica's certificate. All replicas listen on 127.0.0.1, on different ports.
pub const REPLICA_DNS: &str = "localhost";

#[derive(Debug, Clone, Copy)]
pub struct ClusterOptions {
    pub hubs: usize,
    pub settings: GatewaySettings,
    /// How long a registration may go unrenewed before the registry calls it dead.
    pub stale_after: Duration,
}

impl ClusterOptions {
    pub fn new(hubs: usize) -> Self {
        Self {
            hubs,
            settings: GatewaySettings {
                // Short, so that a test notices a superseded stream within a moment instead of ten seconds.
                refresh_interval: Duration::from_millis(200),
                ..GatewaySettings::default()
            },
            stale_after: Duration::from_secs(30),
        }
    }
}

pub struct TestHub {
    pub name: String,
    pub gateway: Arc<HubGateway>,
    pub gateway_metrics: GatewayMetrics,
    pub agent_metrics: AgentMetrics,
    pub sink: FakeReportSink,
    pub agent_addr: SocketAddr,
    pub replica_addr: SocketAddr,
    pub replica_stats: Arc<ListenerStats>,
    pub replica_cert: Arc<RotatingServerCert>,
    pub pool: PgPool,
    agent_server: Option<TestServer>,
    replica_stop: Option<oneshot::Sender<()>>,
    replica_task: Option<JoinHandle<()>>,
    liveness_stop: watch::Sender<bool>,
    liveness: Option<JoinHandle<()>>,
    closing: watch::Sender<bool>,
}

impl TestHub {
    /// Stop serving, as a replica that crashed does: the listeners go away and nothing is released in the registry.
    pub fn kill(&mut self) {
        self.agent_server.take();
        if let Some(task) = self.replica_task.take() {
            task.abort();
        }
        if let Some(task) = self.liveness.take() {
            task.abort();
        }
    }

    /// Tell every stream on this replica to end (a graceful shutdown).
    pub fn close_streams(&self) {
        self.closing.send_replace(true);
    }
}

impl Drop for TestHub {
    fn drop(&mut self) {
        self.kill();
        let _ = self.replica_stop.take();
        let _ = self.liveness_stop.send(true);
    }
}

pub struct TestCluster {
    pub db: TestDb,
    pub pool: PgPool,
    pub agent_ca: TestCa,
    pub replica_ca: TestCa,
    pub hubs: Vec<TestHub>,
}

impl TestCluster {
    /// `None` when there is no Docker and it is not required (the helper has said so).
    pub async fn start(hubs: usize) -> Option<Self> {
        Self::start_with(ClusterOptions::new(hubs)).await
    }

    pub async fn start_with(options: ClusterOptions) -> Option<Self> {
        let db = TestDb::ready().await?;
        let pool = db.app_pool().await;
        let agent_ca = TestCa::new().await;
        let replica_ca = TestCa::with_key(REPLICA_CA_KEY).await;
        let mut hubs = Vec::new();
        for i in 0..options.hubs {
            hubs.push(TestHub::start(i, &db, &agent_ca, &replica_ca, options).await);
        }
        Some(Self {
            db,
            pool,
            agent_ca,
            replica_ca,
            hubs,
        })
    }

    pub async fn seed(&self, swimlanes: &[&str]) {
        for s in swimlanes {
            self.db.seed_swimlane(&self.pool, s, None).await;
        }
    }

    pub async fn agent(&self, swimlane: &str) -> FakeAgent {
        FakeAgent::new(&self.agent_ca, swimlane).await
    }
}

impl TestHub {
    async fn start(
        i: usize,
        db: &TestDb,
        agent_ca: &TestCa,
        replica_ca: &TestCa,
        options: ClusterOptions,
    ) -> Self {
        let name = format!("hub-{i}");
        let peer = PeerName::parse(&name).unwrap();
        let pool = db.app_pool().await;

        // The replica listener first: the registry needs the address this replica will write into `agent_connections`.
        let replica_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let replica_addr = replica_listener.local_addr().unwrap();
        let address = PeerAddress::parse(&format!("{REPLICA_DNS}:{}", replica_addr.port())).unwrap();
        let profile = CertProfile::replica(&peer, &[REPLICA_DNS.to_owned()]).unwrap();
        let (certified, _issued) = issue_server_identity(&replica_ca.issuer, &profile).await.unwrap();
        let replica_cert = RotatingServerCert::new(certified);
        let replica_roots = roots_from_der(&[replica_ca.ca_der.clone()]).unwrap();
        let server_tls = ServerTls::new(
            replica_roots.clone(),
            Arc::clone(&replica_cert),
            replica_ca.clock.clone(),
        )
        .unwrap();
        let client_tls =
            ClientTls::new_rotating(replica_roots, Arc::clone(&replica_cert), replica_ca.clock.clone())
                .unwrap();

        let registry = ConnectionRegistry::new(pool.clone(), peer.clone(), address, options.stale_after);
        let client = ReplicaClient::new(client_tls, ReplicaClientLimits::default());
        let gateway_metrics = GatewayMetrics::default();
        let gateway = HubGateway::new(registry, client, gateway_metrics.clone(), options.settings);

        let replica_stats = Arc::new(ListenerStats::default());
        let (replica_stop, stopped) = oneshot::channel::<()>();
        let replica_task = tokio::spawn(serve_grpc(
            replica_listener,
            server_tls.acceptor(),
            ReplicaService::new(gateway.local(), gateway_metrics.clone()).routes(),
            async move {
                let _ = stopped.await;
            },
            GrpcLimits::default(),
            Arc::clone(&replica_stats),
        ));

        let (liveness_stop, liveness_rx) = watch::channel(false);
        let liveness = gateway.spawn_liveness(liveness_rx);

        // The agent side: the same services the hub serves to agents, with the gateway attached.
        let signer: Arc<dyn ports::KmsSigner> = agent_ca.soft.clone();
        let issuer = CaIssuer::new(signer, agent_ca.key.clone(), agent_ca.ca_der.clone())
            .await
            .unwrap()
            .with_clock(agent_ca.clock.clone());
        let agent_metrics = AgentMetrics::default();
        let join = Arc::new(
            JoinService::new(
                pool.clone(),
                Arc::new(FakeTokenVerifier::new()),
                Arc::new(PgTestAuditLog::default()),
                Arc::new(issuer),
                agent_ca.clock.clone(),
                JoinSettings {
                    hub_audience: ShortText::parse(AUDIENCE).unwrap(),
                    limits: JoinLimits::default(),
                },
            )
            .with_metrics(agent_metrics.clone()),
        );
        let sink = FakeReportSink::new();
        let (closing, closing_rx) = watch::channel(false);
        let service = AgentService::new(
            join,
            Arc::new(sink.clone()),
            closing_rx,
            agent_metrics.clone(),
            StreamLimits::default(),
        )
        .with_gateway(Arc::clone(&gateway));
        let agent_server = TestServer::start(agent_ca, service.routes()).await;

        Self {
            name,
            gateway,
            gateway_metrics,
            agent_metrics,
            sink,
            agent_addr: agent_server.addr,
            replica_addr,
            replica_stats,
            replica_cert,
            pool,
            agent_server: Some(agent_server),
            replica_stop: Some(replica_stop),
            replica_task: Some(replica_task),
            liveness_stop,
            liveness: Some(liveness),
            closing,
        }
    }
}
