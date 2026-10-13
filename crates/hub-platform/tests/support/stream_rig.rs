//! The hub's gRPC services (`Agent`, and `Sentinel` once a sink is given) behind the real listener, with a real CA and real mutual
//! TLS, and a client that speaks the real protocol. For the acknowledgement and sentinel tests and the P15 harness.
//!
//! The database is a pool that never connects unless the test hands it a real one, so a test that never joins needs none.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::ShortText;
use hub_platform::agent_metrics::AgentMetrics;
use hub_platform::agent_service::{AgentService, StreamLimits};
use hub_platform::ca::CaIssuer;
use hub_platform::join::{JoinLimits, JoinService, JoinSettings};
use hub_platform::sentinel_service::{SentinelLimits, SentinelMetrics, SentinelService};
use ports::fakes::FakeTokenVerifier;
use ports::{ReportSink, SentinelSink};
use proto::pb;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Status, Streaming};

use super::ca::TestCa;
use super::grpc::{ClientIdentity, TestServer, agent_client, agent_identity, client_tls, dial};
use super::pg::PgTestAuditLog;

pub const AUDIENCE: &str = "https://hub.lanekeeper.test";

/// A pool that never connects unless a test uses the database.
pub fn lazy_pool() -> PgPool {
    PgPoolOptions::new().connect_lazy_with(
        PgConnectOptions::new()
            .host("127.0.0.1")
            .port(1)
            .username("none")
            .password("none")
            .database("none"),
    )
}

pub struct StreamRig {
    pub ca: TestCa,
    pub server: TestServer,
    pub metrics: AgentMetrics,
    pub verifier: FakeTokenVerifier,
    pub audit: Arc<PgTestAuditLog>,
    pub closing: watch::Sender<bool>,
}

impl StreamRig {
    /// The `Agent` service over `sink`, with no database.
    pub async fn agents(sink: Arc<dyn ReportSink>, limits: StreamLimits) -> Self {
        Self::build(lazy_pool(), sink, None, limits).await
    }

    pub async fn build(
        pool: PgPool,
        sink: Arc<dyn ReportSink>,
        sentinel: Option<(Arc<dyn SentinelSink>, SentinelLimits)>,
        limits: StreamLimits,
    ) -> Self {
        let ca = TestCa::new().await;
        let signer: Arc<dyn ports::KmsSigner> = ca.soft.clone();
        let issuer = CaIssuer::new(signer, ca.key.clone(), ca.ca_der.clone())
            .await
            .unwrap()
            .with_clock(ca.clock.clone());
        let verifier = FakeTokenVerifier::new();
        let audit = Arc::new(PgTestAuditLog::default());
        let metrics = AgentMetrics::default();
        let join = Arc::new(
            JoinService::new(
                pool,
                Arc::new(verifier.clone()),
                audit.clone(),
                Arc::new(issuer),
                ca.clock.clone(),
                JoinSettings {
                    hub_audience: ShortText::parse(AUDIENCE).unwrap(),
                    limits: JoinLimits::default(),
                },
            )
            .with_metrics(metrics.clone()),
        );
        let (closing, closing_rx) = watch::channel(false);
        let agent = AgentService::new(
            Arc::clone(&join),
            sink,
            closing_rx.clone(),
            metrics.clone(),
            limits,
        );
        let mut routes = agent.routes();
        if let Some((sentinel_sink, sentinel_limits)) = sentinel {
            routes = SentinelService::new(
                join,
                sentinel_sink,
                closing_rx,
                SentinelMetrics::default(),
                sentinel_limits,
            )
            .add_to(routes);
        }
        let server = TestServer::start(&ca, routes).await;
        Self {
            ca,
            server,
            metrics,
            verifier,
            audit,
            closing,
        }
    }

    pub async fn agent_identity(&self, swimlane: &str) -> ClientIdentity {
        agent_identity(&self.ca, swimlane).await
    }

    /// Open a `Connect` stream as the agent of `swimlane`, having sent `Hello`, and read the configuration the hub answers with.
    pub async fn open_agent(&self, swimlane: &str) -> AgentStream {
        let identity = self.agent_identity(swimlane).await;
        self.open_agent_with(&identity, swimlane).await.unwrap()
    }

    pub async fn open_agent_with(
        &self,
        identity: &ClientIdentity,
        swimlane: &str,
    ) -> Result<AgentStream, Status> {
        let tls = client_tls(&self.ca, Some(identity));
        let h2 = dial(self.server.addr, &tls)
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        let mut client = agent_client(h2);
        let (tx, rx) = mpsc::channel(256);
        tx.send(hello(swimlane)).await.unwrap();
        let response = client.connect(Request::new(ReceiverStream::new(rx))).await?;
        let mut stream = AgentStream {
            tx,
            rx: response.into_inner(),
        };
        // The first thing the hub sends is the agent's configuration.
        match stream.next().await {
            Ok(Some(pb::hub_message::Kind::AgentConfig(_))) => Ok(stream),
            other => panic!("expected the agent configuration first, got {other:?}"),
        }
    }
}

pub struct AgentStream {
    pub tx: mpsc::Sender<pb::AgentMessage>,
    pub rx: Streaming<pb::HubMessage>,
}

impl AgentStream {
    /// The next message from the hub, within 5 seconds.
    pub async fn next(&mut self) -> Result<Option<pb::hub_message::Kind>, Status> {
        self.next_within(Duration::from_secs(5))
            .await
            .expect("the hub answered within 5 seconds")
    }

    /// `None` if nothing arrived within `wait`.
    pub async fn next_within(
        &mut self,
        wait: Duration,
    ) -> Option<Result<Option<pb::hub_message::Kind>, Status>> {
        match tokio::time::timeout(wait, self.rx.message()).await {
            Err(_) => None,
            Ok(Err(status)) => Some(Err(status)),
            Ok(Ok(message)) => Some(Ok(message.and_then(|m| m.kind))),
        }
    }

    /// The `seq` of the next acknowledgement, skipping other messages; `None` if none arrives within `wait`.
    pub async fn next_ack(&mut self, wait: Duration) -> Option<u64> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next_within(left).await? {
                Ok(Some(pb::hub_message::Kind::Ack(ack))) => return Some(ack.seq),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return None,
            }
        }
    }

    pub async fn send(&self, kind: pb::agent_message::Kind) {
        self.tx.send(pb::AgentMessage { kind: Some(kind) }).await.unwrap();
    }

    pub async fn send_delta(&self, delta: pb::ScanDelta) {
        self.send(pb::agent_message::Kind::ScanDelta(delta)).await;
    }
}

pub fn hello(swimlane: &str) -> pb::AgentMessage {
    pb::AgentMessage {
        kind: Some(pb::agent_message::Kind::Hello(pb::Hello {
            agent_version: "0.1.0".to_owned(),
            swimlane: swimlane.to_owned(),
            cluster: "cluster-a".to_owned(),
            project: "proj".to_owned(),
            nfs_server: "nfs.internal".to_owned(),
            export: "/export".to_owned(),
            mount_root: "/mnt/config".to_owned(),
        })),
    }
}

/// A delta with no entries, the root of its `seq`, and `more` set as given.
pub fn delta(seq: u64, more: bool) -> pb::ScanDelta {
    let mut root = [0_u8; 32];
    root[..8].copy_from_slice(&seq.to_be_bytes());
    pb::ScanDelta {
        seq,
        new_root: Bytes::from(root.to_vec()),
        more,
        part: 0,
        ..Default::default()
    }
}
