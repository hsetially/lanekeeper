//! T3: `Connect` and `Join` over real TCP and real mutual TLS (S5, S6, S10, S11, S22).
//!
//! The hub side is the real `AgentService` behind the real listener, the real `JoinService` and CA; the ingestion sink is a
//! recording double (05 owns the real one); the database is a real Postgres where a test touches it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{AgentConfig, ClusterReport, Heartbeat, HeartbeatAction, Hello, ScanDelta, ShortText, TenantId};
use hub_platform::agent_metrics::AgentMetrics;
use hub_platform::agent_service::{AgentService, StreamLimits};
use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::ca::{CaIssuer, parse_csr};
use hub_platform::join::{JoinLimits, JoinService, JoinSettings};
use ports::fakes::FakeTokenVerifier;
use ports::{AgentIdentity, GoogleIdentity, ReportSink, SinkError};
use proto::pb;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use support::ca::{TestCa, csr_for, fresh_csr};
use support::grpc::{ClientIdentity, TestServer, agent_client, agent_identity, client_tls, dial};
use support::log_capture::LogCapture;
use support::pg::{PgTestAuditLog, TestDb};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Status, Streaming};
use x509_parser::prelude::{FromDer, X509Certificate};

const AUDIENCE: &str = "https://hub.lanekeeper.test";
const SERVICE_ACCOUNT: &str = "agent-sit1@project.iam.gserviceaccount.com";

// ---------------------------------------------------------------------------------------------- the sink double

#[derive(Default)]
struct Seen {
    hellos: Vec<(AgentIdentity, Hello)>,
    heartbeats: Vec<(AgentIdentity, Heartbeat)>,
    deltas: Vec<(AgentIdentity, ScanDelta)>,
    clusters: Vec<(AgentIdentity, ClusterReport)>,
}

struct RecordingSink {
    seen: Mutex<Seen>,
    action: Mutex<HeartbeatAction>,
    fail_hello: Mutex<Option<SinkError>>,
}

impl RecordingSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Seen::default()),
            action: Mutex::new(HeartbeatAction::None),
            fail_hello: Mutex::new(None),
        })
    }

    fn seen<T>(&self, read: impl FnOnce(&Seen) -> T) -> T {
        read(&self.seen.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn config() -> AgentConfig {
    AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 5,
        max_file_bytes: 1_000_000,
        deny_globs: vec![ShortText::parse("*.key").unwrap()],
        env_allowlist: vec![ShortText::parse("SPRING_PROFILES_ACTIVE").unwrap()],
        tenants: vec![TenantId::parse("sit1").unwrap()],
    }
}

#[async_trait]
impl ReportSink for RecordingSink {
    async fn hello(&self, id: &AgentIdentity, h: Hello) -> Result<AgentConfig, SinkError> {
        if let Some(error) = *self.fail_hello.lock().unwrap_or_else(PoisonError::into_inner) {
            return Err(error);
        }
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .hellos
            .push((id.clone(), h));
        Ok(config())
    }

    async fn heartbeat(&self, id: &AgentIdentity, hb: Heartbeat) -> Result<HeartbeatAction, SinkError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .heartbeats
            .push((id.clone(), hb));
        Ok(self.action.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }

    async fn delta(&self, id: &AgentIdentity, d: ScanDelta) -> Result<(), SinkError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .deltas
            .push((id.clone(), d));
        Ok(())
    }

    async fn cluster(&self, id: &AgentIdentity, c: ClusterReport) -> Result<(), SinkError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clusters
            .push((id.clone(), c));
        Ok(())
    }
}

// --------------------------------------------------------------------------------------------------- the rig

struct Rig {
    ca: TestCa,
    sink: Arc<RecordingSink>,
    server: TestServer,
    metrics: AgentMetrics,
    closing: watch::Sender<bool>,
    verifier: FakeTokenVerifier,
}

/// A pool that never connects unless a test uses the database.
fn lazy_pool() -> PgPool {
    PgPoolOptions::new().connect_lazy_with(
        PgConnectOptions::new()
            .host("127.0.0.1")
            .port(1)
            .username("none")
            .password("none")
            .database("none"),
    )
}

impl Rig {
    async fn start() -> Self {
        Self::build(lazy_pool(), StreamLimits::default()).await
    }

    async fn with_limits(limits: StreamLimits) -> Self {
        Self::build(lazy_pool(), limits).await
    }

    async fn with_db(pool: PgPool) -> Self {
        Self::build(pool, StreamLimits::default()).await
    }

    async fn build(pool: PgPool, limits: StreamLimits) -> Self {
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
                pool.clone(),
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
        let sink = RecordingSink::new();
        let (closing, closing_rx) = watch::channel(false);
        let service = AgentService::new(join, sink.clone(), closing_rx, metrics.clone(), limits);
        let server = TestServer::start(&ca, service.routes()).await;
        Self {
            ca,
            sink,
            server,
            metrics,
            closing,
            verifier,
        }
    }

    async fn agent(&self, swimlane: &str) -> ClientIdentity {
        agent_identity(&self.ca, swimlane).await
    }

    /// Open a `Connect` stream as `identity` (no certificate if `None`), having sent `first` as the first message.
    async fn open(&self, identity: Option<&ClientIdentity>, first: pb::AgentMessage) -> Result<Link, Status> {
        let tls = client_tls(&self.ca, identity);
        let h2 = dial(self.server.addr, &tls)
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        let mut client = agent_client(h2);
        let (tx, rx) = mpsc::channel(16);
        tx.send(first).await.unwrap();
        let response = client.connect(Request::new(ReceiverStream::new(rx))).await?;
        Ok(Link {
            tx,
            rx: response.into_inner(),
        })
    }

    async fn open_as(&self, swimlane: &str) -> Link {
        let identity = self.agent(swimlane).await;
        self.open(Some(&identity), hello(swimlane)).await.unwrap()
    }
}

struct Link {
    tx: mpsc::Sender<pb::AgentMessage>,
    rx: Streaming<pb::HubMessage>,
}

impl Link {
    async fn next(&mut self) -> Result<Option<pb::hub_message::Kind>, Status> {
        let message = tokio::time::timeout(Duration::from_secs(5), self.rx.message())
            .await
            .expect("the hub answered within 5 seconds")?;
        Ok(message.and_then(|m| m.kind))
    }

    async fn send(&self, kind: pb::agent_message::Kind) {
        self.tx.send(pb::AgentMessage { kind: Some(kind) }).await.unwrap();
    }

    /// The status the stream ends with, skipping any messages before it.
    async fn ends_with(&mut self) -> Option<Status> {
        loop {
            let next = tokio::time::timeout(Duration::from_secs(5), self.rx.message())
                .await
                .expect("the stream ended within 5 seconds");
            match next {
                Err(status) => return Some(status),
                Ok(None) => return None,
                Ok(Some(_)) => {}
            }
        }
    }
}

fn hello(swimlane: &str) -> pb::AgentMessage {
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

fn heartbeat(seq: u64) -> pb::agent_message::Kind {
    pb::agent_message::Kind::Heartbeat(pb::Heartbeat {
        scan_seq: seq,
        merkle_root: Bytes::from(vec![7u8; 32]),
        file_count: 3,
    })
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

// ----------------------------------------------------------------------------------------------- Connect

#[tokio::test]
async fn connect_sends_config_after_hello_and_gives_the_sink_the_certificate_identity() {
    let rig = Rig::start().await;
    let mut link = rig.open_as("sit1").await;
    match link.next().await.unwrap() {
        Some(pb::hub_message::Kind::AgentConfig(c)) => {
            assert_eq!(c.scan_interval_secs, 10);
            assert_eq!(c.tenants, ["sit1"]);
        }
        other => panic!("expected the agent configuration first, got {other:?}"),
    }
    rig.sink.seen(|s| {
        assert_eq!(s.hellos.len(), 1);
        assert_eq!(s.hellos[0].0.swimlane().as_str(), "sit1");
        assert_eq!(s.hellos[0].1.swimlane.as_str(), "sit1");
        assert_eq!(s.hellos[0].1.cluster.as_str(), "cluster-a");
    });
    assert_eq!(rig.metrics.streams_active(), 1);
}

#[tokio::test]
async fn connect_hello_swimlane_must_equal_certificate_san() {
    let rig = Rig::start().await;
    let identity = rig.agent("sit1").await;
    let status = rig.open(Some(&identity), hello("sit2")).await.err().unwrap();
    assert_eq!(status.code(), Code::PermissionDenied);
    assert_eq!(status.message(), "", "no reason is given");
    rig.sink
        .seen(|s| assert!(s.hellos.is_empty(), "the sink never heard of sit2"));
}

/// The third of the three certificate failures of S6 (the other two, forged and expired, fail at the handshake in `tls.rs`): a
/// perfectly valid certificate used to speak for another swimlane.
#[tokio::test]
async fn forged_expired_and_wrong_swimlane_certs_are_rejected_wrong_swimlane() {
    let rig = Rig::start().await;
    let identity = rig.agent("sit1").await;
    // The control: the same certificate for its own swimlane connects.
    assert!(rig.open(Some(&identity), hello("sit1")).await.is_ok());
    for other in ["sit2", "sit10", "sit", "sit1-extra"] {
        let status = rig.open(Some(&identity), hello(other)).await.err().unwrap();
        assert_eq!(status.code(), Code::PermissionDenied, "{other}");
    }
}

#[tokio::test]
async fn connect_requires_hello_first() {
    let rig = Rig::start().await;
    let identity = rig.agent("sit1").await;
    for first in [
        pb::AgentMessage {
            kind: Some(heartbeat(1)),
        },
        pb::AgentMessage { kind: None },
    ] {
        let status = rig.open(Some(&identity), first).await.err().unwrap();
        assert_eq!(status.code(), Code::FailedPrecondition);
    }
    rig.sink
        .seen(|s| assert!(s.hellos.is_empty() && s.heartbeats.is_empty()));
}

#[tokio::test]
async fn connect_without_client_cert_is_refused_by_the_service() {
    let rig = Rig::start().await;
    let status = rig.open(None, hello("sit1")).await.err().unwrap();
    assert_eq!(status.code(), Code::Unauthenticated);
    rig.sink.seen(|s| assert!(s.hellos.is_empty()));
}

#[tokio::test]
async fn sentinel_cert_is_not_accepted_on_agent_service() {
    let rig = Rig::start().await;
    // A perfectly valid certificate from the same CA, for the sentinel of an NFS VM.
    let key = rcgen::KeyPair::generate().unwrap();
    let csr = parse_csr(&csr_for(&key)).unwrap();
    let issued = rig
        .ca
        .issuer
        .issue(
            &CertProfile::sentinel(&PeerName::parse("nfs-vm-1").unwrap()),
            &csr,
        )
        .await
        .unwrap();
    let identity = ClientIdentity {
        key,
        chain_der: issued.chain_der.iter().map(|c| c.to_vec()).collect(),
    };
    let status = rig.open(Some(&identity), hello("sit1")).await.err().unwrap();
    assert_eq!(status.code(), Code::PermissionDenied);
    rig.sink.seen(|s| assert!(s.hellos.is_empty()));
}

#[tokio::test]
async fn connect_hello_must_arrive_within_the_hello_timeout() {
    let rig = Rig::with_limits(StreamLimits {
        hello_timeout: Duration::from_millis(150),
        ..StreamLimits::default()
    })
    .await;
    let identity = rig.agent("sit1").await;
    let tls = client_tls(&rig.ca, Some(&identity));
    let h2 = dial(rig.server.addr, &tls).await.unwrap();
    let mut client = agent_client(h2);
    let (_tx, rx) = mpsc::channel::<pb::AgentMessage>(1);
    let status = client
        .connect(Request::new(ReceiverStream::new(rx)))
        .await
        .err()
        .unwrap();
    assert_eq!(status.code(), Code::DeadlineExceeded);
}

#[tokio::test]
async fn connect_heartbeat_action_from_the_sink_becomes_a_command() {
    let rig = Rig::start().await;
    let mut link = rig.open_as("sit1").await;
    assert!(matches!(
        link.next().await.unwrap(),
        Some(pb::hub_message::Kind::AgentConfig(_))
    ));

    *rig.sink.action.lock().unwrap() = HeartbeatAction::RequestFullScan;
    link.send(heartbeat(1)).await;
    assert!(matches!(
        link.next().await.unwrap(),
        Some(pb::hub_message::Kind::RequestFullScan(_))
    ));

    *rig.sink.action.lock().unwrap() = HeartbeatAction::RequestDelta {
        since_root: domain::ContentHash::from_bytes([9; 32]),
    };
    link.send(heartbeat(2)).await;
    match link.next().await.unwrap() {
        Some(pb::hub_message::Kind::RequestDelta(r)) => {
            assert_eq!(r.since_root.as_ref(), [9u8; 32].as_slice());
        }
        other => panic!("expected RequestDelta, got {other:?}"),
    }

    *rig.sink.action.lock().unwrap() = HeartbeatAction::None;
    link.send(heartbeat(3)).await;
    wait_until("the third heartbeat", || {
        rig.sink.seen(|s| s.heartbeats.len() == 3)
    })
    .await;
    rig.sink.seen(|s| {
        assert!(
            s.heartbeats
                .iter()
                .all(|(id, _)| id.swimlane().as_str() == "sit1")
        );
        assert_eq!(s.heartbeats[2].1.scan_seq, 3);
    });
}

#[tokio::test]
async fn connect_hands_deltas_and_cluster_reports_to_the_sink() {
    let rig = Rig::start().await;
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();
    link.send(pb::agent_message::Kind::ScanDelta(pb::ScanDelta {
        seq: 5,
        new_root: Bytes::from(vec![1u8; 32]),
        entries: vec![pb::ScanEntry {
            path: "svc/app.yml".to_owned(),
            hash: Bytes::from(vec![2u8; 32]),
            size: 3,
            mtime_ms: 1,
            observed_at_ms: 2,
            denied: false,
            content: Some(Bytes::from_static(b"abc")),
        }],
        ..pb::ScanDelta::default()
    }))
    .await;
    link.send(pb::agent_message::Kind::ClusterReport(pb::ClusterReport {
        full: true,
        ..pb::ClusterReport::default()
    }))
    .await;
    wait_until("the delta and the report", || {
        rig.sink.seen(|s| s.deltas.len() == 1 && s.clusters.len() == 1)
    })
    .await;
    rig.sink.seen(|s| {
        assert_eq!(s.deltas[0].1.seq, 5);
        assert_eq!(s.deltas[0].1.entries[0].path.as_str(), "svc/app.yml");
        assert!(s.clusters[0].1.full);
    });
}

#[tokio::test]
async fn connect_skips_message_kinds_it_does_not_know_and_keeps_the_stream() {
    let rig = Rig::start().await;
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();
    // An empty oneof is what a newer peer's unknown variant looks like to this build.
    link.tx.send(pb::AgentMessage { kind: None }).await.unwrap();
    link.send(heartbeat(1)).await;
    wait_until("the heartbeat after the unknown message", || {
        rig.sink.seen(|s| s.heartbeats.len() == 1)
    })
    .await;
}

#[tokio::test]
async fn connect_ends_the_stream_on_a_message_that_fails_validation() {
    let rig = Rig::start().await;
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();
    // A traversal path never becomes a domain value (S11), so the sink never sees the delta.
    link.send(pb::agent_message::Kind::ScanDelta(pb::ScanDelta {
        seq: 1,
        new_root: Bytes::from(vec![1u8; 32]),
        entries: vec![pb::ScanEntry {
            path: "../../etc/passwd".to_owned(),
            hash: Bytes::from(vec![2u8; 32]),
            size: 0,
            mtime_ms: 0,
            observed_at_ms: 0,
            denied: false,
            content: None,
        }],
        ..pb::ScanDelta::default()
    }))
    .await;
    let status = link.ends_with().await.expect("the stream ends with a status");
    assert_eq!(status.code(), Code::InvalidArgument);
    rig.sink.seen(|s| assert!(s.deltas.is_empty()));
    wait_until("the stream to be counted closed", || {
        rig.metrics.streams_active() == 0
    })
    .await;
}

#[tokio::test]
async fn connect_ends_the_stream_on_a_second_hello() {
    let rig = Rig::start().await;
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();
    link.tx.send(hello("sit1")).await.unwrap();
    let status = link.ends_with().await.expect("status");
    assert_eq!(status.code(), Code::FailedPrecondition);
}

#[tokio::test]
async fn connect_reply_without_a_request_is_dropped_counted_and_the_stream_stays_open() {
    let rig = Rig::start().await;
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();
    link.send(pb::agent_message::Kind::OpResult(pb::OpResult {
        request_id: "req-that-nobody-sent".to_owned(),
        ok: true,
        ..pb::OpResult::default()
    }))
    .await;
    wait_until("the reply to be counted", || rig.metrics.unmatched_replies() == 1).await;
    link.send(heartbeat(1)).await;
    wait_until("a heartbeat after it", || {
        rig.sink.seen(|s| s.heartbeats.len() == 1)
    })
    .await;
}

#[tokio::test]
async fn connect_closes_an_idle_stream() {
    let rig = Rig::with_limits(StreamLimits {
        idle_timeout: Duration::from_millis(200),
        ..StreamLimits::default()
    })
    .await;
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();
    let status = link.ends_with().await.expect("status");
    assert_eq!(status.code(), Code::DeadlineExceeded);
}

#[tokio::test]
async fn connect_closes_every_stream_when_the_hub_is_closing() {
    let rig = Rig::start().await;
    let mut a = rig.open_as("sit1").await;
    let mut b = rig.open_as("sit2").await;
    a.next().await.unwrap();
    b.next().await.unwrap();
    assert_eq!(rig.metrics.streams_active(), 2);
    rig.closing.send_replace(true);
    for link in [&mut a, &mut b] {
        let status = link.ends_with().await.expect("status");
        assert_eq!(status.code(), Code::Unavailable, "the agent is told to reconnect");
    }
    wait_until("the streams to be counted closed", || {
        rig.metrics.streams_active() == 0
    })
    .await;
}

#[tokio::test]
async fn connect_sink_unavailable_at_hello_is_unavailable_not_a_refusal() {
    let rig = Rig::start().await;
    *rig.sink.fail_hello.lock().unwrap() = Some(SinkError::Unavailable);
    let identity = rig.agent("sit1").await;
    let status = rig.open(Some(&identity), hello("sit1")).await.err().unwrap();
    assert_eq!(status.code(), Code::Unavailable);
}

// ------------------------------------------------------------------------------------- size limits (S11)

#[tokio::test]
async fn proto_decode_is_size_limited() {
    let rig = Rig::start().await;
    let identity = rig.agent("sit1").await;
    let tls = client_tls(&rig.ca, Some(&identity));
    let h2 = dial(rig.server.addr, &tls).await.unwrap();
    // A client with no encode limit and no compression, to send what a hostile one would.
    let mut client = pb::agent_client::AgentClient::new(h2);
    let (tx, rx) = mpsc::channel(4);
    tx.send(hello("sit1")).await.unwrap();
    let mut stream = client
        .connect(Request::new(ReceiverStream::new(rx)))
        .await
        .unwrap()
        .into_inner();
    stream.message().await.unwrap();
    // One byte over the 4 MiB message limit.
    let too_big = pb::AgentMessage {
        kind: Some(pb::agent_message::Kind::ScanDelta(pb::ScanDelta {
            seq: 1,
            new_root: Bytes::from(vec![1u8; 32]),
            entries: vec![pb::ScanEntry {
                path: "a.yml".to_owned(),
                hash: Bytes::from(vec![2u8; 32]),
                content: Some(Bytes::from(vec![0u8; 4 * 1024 * 1024 + 1])),
                ..pb::ScanEntry::default()
            }],
            ..pb::ScanDelta::default()
        })),
    };
    tx.send(too_big).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(10), stream.message())
        .await
        .unwrap();
    assert!(
        ended.is_err(),
        "the oversize message ends the stream with an error: {ended:?}"
    );
    rig.sink.seen(|s| assert!(s.deltas.is_empty()));
}

#[tokio::test]
async fn proto_decode_refuses_a_declared_size_before_the_payload_arrives() {
    use futures::StreamExt;
    use http_body_util::StreamBody;
    use hyper::body::Frame;
    use tower::Service;

    let rig = Rig::start().await;
    let identity = rig.agent("sit1").await;
    let tls = client_tls(&rig.ca, Some(&identity));
    let mut h2 = dial(rig.server.addr, &tls).await.unwrap();
    // A gRPC message prefix that promises 64 MiB, and then nothing: the hub must refuse on the prefix alone, not wait for or
    // allocate the payload.
    let mut prefix = vec![0u8];
    prefix.extend_from_slice(&(64u32 * 1024 * 1024).to_be_bytes());
    let frames =
        futures::stream::once(
            async move { Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from(prefix))) },
        )
        .chain(futures::stream::pending());
    let request = http::Request::builder()
        .method("POST")
        .uri("/lanekeeper.agent.v1.Agent/Connect")
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(tonic::body::Body::new(StreamBody::new(frames)))
        .unwrap();
    futures::future::poll_fn(|cx| h2.poll_ready(cx)).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), h2.call(request))
        .await
        .expect("the hub answered without waiting for the 64 MiB")
        .unwrap();
    let status = Status::from_header_map(response.headers());
    let code = status.map(|s| s.code());
    assert!(
        matches!(code, Some(c) if c != Code::Ok),
        "answered with an error status, got {code:?}"
    );
    rig.sink.seen(|s| assert!(s.hellos.is_empty()));
}

// --------------------------------------------------------------------------------- Join over gRPC, with Postgres

fn google_join(swimlane: &str, token: &str, csr: &[u8]) -> pb::JoinRequest {
    pb::JoinRequest {
        swimlane_id: swimlane.to_owned(),
        csr_der: Bytes::copy_from_slice(csr),
        credential: Some(pb::join_request::Credential::GoogleIdToken(token.to_owned())),
        kind: pb::PeerKind::Agent as i32,
    }
}

fn token_join(swimlane: &str, token: &str, csr: &[u8]) -> pb::JoinRequest {
    pb::JoinRequest {
        credential: Some(pb::join_request::Credential::JoinToken(token.to_owned())),
        ..google_join(swimlane, "", csr)
    }
}

fn accept_google(rig: &Rig, token: &str, email: &str) {
    rig.verifier.accept_google(
        token,
        AUDIENCE,
        GoogleIdentity {
            subject: ShortText::parse("1").unwrap(),
            email: ShortText::parse(email).unwrap(),
            audience: ShortText::parse(AUDIENCE).unwrap(),
        },
    );
}

#[tokio::test]
async fn join_over_grpc_needs_no_client_certificate_and_the_certificate_then_connects() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", Some(SERVICE_ACCOUNT)).await;
    let rig = Rig::with_db(pool.clone()).await;
    accept_google(&rig, "gid-ok", SERVICE_ACCOUNT);

    // No certificate yet: the handshake carries none and `Join` works.
    let tls = client_tls(&rig.ca, None);
    let mut client = agent_client(dial(rig.server.addr, &tls).await.unwrap());
    let (key, csr) = fresh_csr();
    let response = client
        .join(Request::new(google_join("sit1", "gid-ok", &csr)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.cert_chain_der.len(), 2);
    assert_eq!(response.not_after_ms, rig.ca.clock_now_ms() + 24 * 3600 * 1000);

    // The certificate it got is the one `Connect` accepts, and its identity is the swimlane.
    let identity = ClientIdentity {
        key,
        chain_der: response.cert_chain_der.iter().map(|c| c.to_vec()).collect(),
    };
    let mut link = rig.open(Some(&identity), hello("sit1")).await.unwrap();
    assert!(matches!(
        link.next().await.unwrap(),
        Some(pb::hub_message::Kind::AgentConfig(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_events WHERE action = 'agent_joined'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn join_failures_are_indistinguishable_over_grpc() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", Some(SERVICE_ACCOUNT)).await;
    db.seed_swimlane(&pool, "sit2", None).await;
    let rig = Rig::with_db(pool.clone()).await;
    accept_google(&rig, "gid-other", "other@project.iam.gserviceaccount.com");
    db.seed_join_token(&pool, "sit2", "jt-expired", 60, -60).await;

    let tls = client_tls(&rig.ca, None);
    let mut client = agent_client(dial(rig.server.addr, &tls).await.unwrap());
    let (_key, csr) = fresh_csr();
    let mut statuses = Vec::new();
    for request in [
        google_join("sit1", "gid-other", &csr),
        google_join("sit1", "gid-unknown", &csr),
        google_join("nowhere", "gid-other", &csr),
        token_join("sit2", "jt-expired", &csr),
        token_join("sit2", "jt-unknown", &csr),
        google_join("sit1", "gid-other", b"not a csr"),
        // A request that does not even parse (a sentinel kind on the agent service, no swimlane id).
        pb::JoinRequest {
            kind: pb::PeerKind::Unspecified as i32,
            ..google_join("sit1", "gid-other", &csr)
        },
    ] {
        let status = client.join(Request::new(request)).await.unwrap_err();
        statuses.push((
            status.code(),
            status.message().to_owned(),
            status.details().len(),
            status.metadata().len(),
        ));
    }
    let first = statuses[0].clone();
    assert_eq!(first.0, Code::Unauthenticated);
    assert!(
        statuses.iter().all(|s| *s == first),
        "every failure looks the same: {statuses:?}"
    );
    assert_eq!(first.1, "", "and says nothing");
}

#[tokio::test]
async fn join_with_a_join_token_over_grpc_is_single_use() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", None).await;
    db.seed_join_token(&pool, "sit1", "jt-grpc", 60, 3600).await;
    let rig = Rig::with_db(pool).await;
    let tls = client_tls(&rig.ca, None);
    let mut client = agent_client(dial(rig.server.addr, &tls).await.unwrap());
    let (_key, csr) = fresh_csr();
    assert!(
        client
            .join(Request::new(token_join("sit1", "jt-grpc", &csr)))
            .await
            .is_ok()
    );
    let again = client
        .join(Request::new(token_join("sit1", "jt-grpc", &csr)))
        .await
        .unwrap_err();
    assert_eq!(again.code(), Code::Unauthenticated);
}

#[tokio::test]
async fn renewal_at_half_lifetime_over_the_stream_issues_a_new_certificate_and_throttles_a_repeat() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", Some(SERVICE_ACCOUNT)).await;
    let rig = Rig::with_db(pool.clone()).await;
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();

    rig.ca.clock.advance_secs(12 * 3600);
    let (new_key, new_csr) = fresh_csr();
    link.send(pb::agent_message::Kind::CertRenewalRequest(
        pb::CertRenewalRequest {
            csr_der: Bytes::from(new_csr.clone()),
        },
    ))
    .await;
    let renewed = match link.next().await.unwrap() {
        Some(pb::hub_message::Kind::CertRenewalResponse(r)) => r,
        other => panic!("expected a renewed certificate, got {other:?}"),
    };
    let (_, leaf) = X509Certificate::from_der(&renewed.cert_chain_der[0]).unwrap();
    let san: Vec<String> = leaf
        .subject_alternative_name()
        .unwrap()
        .unwrap()
        .value
        .general_names
        .iter()
        .map(|n| format!("{n:?}"))
        .collect();
    assert!(
        san.iter()
            .any(|s| s.contains("spiffe://lanekeeper/swimlane/sit1")),
        "{san:?}"
    );
    assert_eq!(
        leaf.public_key().subject_public_key.data.as_ref(),
        rcgen::PublicKeyData::der_bytes(&new_key)
    );
    assert_eq!(renewed.not_after_ms, rig.ca.clock_now_ms() + 24 * 3600 * 1000);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE action = 'agent_certificate_renewed'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );

    // A second request straight after is ignored: no second signature, no second audit event.
    link.send(pb::agent_message::Kind::CertRenewalRequest(
        pb::CertRenewalRequest {
            csr_der: Bytes::from(new_csr),
        },
    ))
    .await;
    link.send(heartbeat(1)).await;
    wait_until("the heartbeat behind the repeat", || {
        rig.sink.seen(|s| s.heartbeats.len() == 1)
    })
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE action = 'agent_certificate_renewed'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

// --------------------------------------------------------------------------------------------------- S10

#[tokio::test]
async fn log_capture_has_no_secret_content_or_env_value_connect_and_join() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", Some(SERVICE_ACCOUNT)).await;
    db.seed_join_token(&pool, "sit1", "jt-PLANTED-TOKEN", 60, 3600)
        .await;
    let rig = Rig::with_db(pool).await;
    accept_google(&rig, "gid-PLANTED-IDTOKEN", SERVICE_ACCOUNT);
    let (log, guard) = LogCapture::install();

    // Joins, failing and succeeding, with planted credentials.
    let tls = client_tls(&rig.ca, None);
    let mut client = agent_client(dial(rig.server.addr, &tls).await.unwrap());
    let (_key, csr) = fresh_csr();
    let _ = client
        .join(Request::new(google_join("sit1", "gid-PLANTED-WRONG", &csr)))
        .await;
    let _ = client
        .join(Request::new(token_join("sit1", "jt-PLANTED-WRONG", &csr)))
        .await;
    let _ = client
        .join(Request::new(token_join("sit1", "jt-PLANTED-TOKEN", &csr)))
        .await;
    let _ = client
        .join(Request::new(google_join("sit1", "gid-PLANTED-IDTOKEN", &csr)))
        .await;

    // A stream that carries file bytes and an environment value, and a sink that refuses them.
    let mut link = rig.open_as("sit1").await;
    link.next().await.unwrap();
    link.send(pb::agent_message::Kind::ScanDelta(pb::ScanDelta {
        seq: 1,
        new_root: Bytes::from(vec![1u8; 32]),
        entries: vec![pb::ScanEntry {
            path: "svc/secrets.yml".to_owned(),
            hash: Bytes::from(vec![2u8; 32]),
            size: 22,
            mtime_ms: 1,
            observed_at_ms: 1,
            denied: false,
            content: Some(Bytes::from_static(b"password: PLANTED-FILE-BYTES")),
        }],
        ..pb::ScanDelta::default()
    }))
    .await;
    link.send(pb::agent_message::Kind::ClusterReport(pb::ClusterReport {
        full: true,
        deployments: vec![pb::DeploymentInfo {
            service: Some(pb::ServiceRef {
                namespace: "ns".to_owned(),
                name: "svc".to_owned(),
            }),
            env_values: vec![pb::EnvValue {
                name: "SPRING_PROFILES_ACTIVE".to_owned(),
                value: "PLANTED-ENV-VALUE".to_owned(),
            }],
            ..pb::DeploymentInfo::default()
        }],
        ..pb::ClusterReport::default()
    }))
    .await;
    link.send(pb::agent_message::Kind::ScanDelta(pb::ScanDelta {
        seq: 2,
        entries: vec![pb::ScanEntry {
            path: "../PLANTED-TRAVERSAL".to_owned(),
            ..pb::ScanEntry::default()
        }],
        ..pb::ScanDelta::default()
    }))
    .await;
    link.ends_with().await;
    drop(guard);
    let text = log.text();
    for planted in [
        "PLANTED-TOKEN",
        "PLANTED-IDTOKEN",
        "PLANTED-WRONG",
        "PLANTED-FILE-BYTES",
        "PLANTED-ENV-VALUE",
        "PLANTED-TRAVERSAL",
    ] {
        assert!(!text.contains(planted), "{planted} reached a log line:\n{text}");
    }
    assert!(
        !log.is_empty(),
        "the hub said something, or this test proves nothing"
    );
}
