//! T11 (S5, S6, S9, S11, S22, D72, D74): the `Sentinel` service. The sentinel daemon on the NFS VM joins with the VM's
//! service-account ID token, which must be the one registered for that VM, and then reports spooled audit batches over mutual TLS
//! with a certificate whose name is `spiffe://lanekeeper/sentinel/<name>`. The hub answers with its settings after `SentinelHello`
//! and acknowledges a batch with the sequence number the sink returned, after the sink returned it.
//!
//! The hub side is the real `SentinelService` and `JoinService` behind the real listener over real mutual TLS, with a real CA and a
//! real Postgres for the registered service accounts; the sink is a scripted double.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{AuditRecordBatch, SentinelConfig, SentinelHello, ShortText};
use hub_platform::agent_service::StreamLimits;
use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::sentinel_service::SentinelLimits;
use ports::fakes::FakeReportSink;
use ports::{AckSeq, GoogleIdentity, SentinelIdentity, SentinelSink, SinkError};
use proto::pb;
use sqlx::{PgPool, Row};
use support::ca::fresh_csr;
use support::grpc::{ClientIdentity, agent_client, client_tls, dial, identity_for, sentinel_client};
use support::log_capture::LogCapture;
use support::pg::TestDb;
use support::stream_rig::{AUDIENCE, StreamRig, hello as agent_hello};
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Status, Streaming};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

const VM: &str = "nfs-vm-1";
const SERVICE_ACCOUNT: &str = "sentinel-nfs-vm-1@project.iam.gserviceaccount.com";
const WITHIN: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(400);

// ------------------------------------------------------------------------------------------------ the sink double

#[derive(Default)]
struct ScriptedSentinelSink {
    hellos: Mutex<Vec<(SentinelIdentity, SentinelHello)>>,
    started: Mutex<Vec<u64>>,
    committed: Mutex<Vec<(SentinelIdentity, u64, usize)>>,
    gates: Mutex<HashMap<u64, Arc<Notify>>>,
    failures: Mutex<HashMap<u64, SinkError>>,
    /// What the sink says it has applied up to, for a seq: a batch it had already applied is answered with the highest seq so far.
    answers: Mutex<HashMap<u64, u64>>,
    fail_hello: Mutex<Option<SinkError>>,
}

impl ScriptedSentinelSink {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn hold(&self, seq: u64) -> Arc<Notify> {
        let gate = Arc::new(Notify::new());
        self.gates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seq, Arc::clone(&gate));
        gate
    }

    fn fail(&self, seq: u64, error: SinkError) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seq, error);
    }

    fn answer(&self, seq: u64, acked: u64) {
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seq, acked);
    }

    fn hellos(&self) -> Vec<(SentinelIdentity, SentinelHello)> {
        self.hellos.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn committed(&self) -> Vec<(SentinelIdentity, u64, usize)> {
        self.committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn wait_started(&self, seq: u64) {
        for _ in 0..250 {
            if self.started.lock().unwrap().contains(&seq) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the sink was never asked for batch {seq}");
    }
}

#[async_trait]
impl SentinelSink for ScriptedSentinelSink {
    async fn hello(&self, id: &SentinelIdentity, h: SentinelHello) -> Result<SentinelConfig, SinkError> {
        if let Some(error) = *self.fail_hello.lock().unwrap_or_else(PoisonError::into_inner) {
            return Err(error);
        }
        self.hellos
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id.clone(), h));
        Ok(SentinelConfig {
            max_batch_records: 500,
            flush_interval_secs: 5,
        })
    }

    async fn records(&self, id: &SentinelIdentity, batch: AuditRecordBatch) -> Result<AckSeq, SinkError> {
        self.started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(batch.seq);
        let gate = self
            .gates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&batch.seq)
            .cloned();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        if let Some(error) = self
            .failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&batch.seq)
        {
            return Err(*error);
        }
        self.committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id.clone(), batch.seq, batch.records.len()));
        let acked = self
            .answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&batch.seq)
            .copied()
            .unwrap_or(batch.seq);
        Ok(AckSeq::new(acked))
    }
}

// --------------------------------------------------------------------------------------------------------- the rig

struct Rig {
    db: TestDb,
    pool: PgPool,
    sink: Arc<ScriptedSentinelSink>,
    hub: StreamRig,
}

impl Rig {
    async fn new() -> Option<Self> {
        Self::with_limits(SentinelLimits::default()).await
    }

    async fn with_limits(limits: SentinelLimits) -> Option<Self> {
        let db = TestDb::ready().await?;
        let pool = db.app_pool().await;
        let sink = ScriptedSentinelSink::new();
        let hub = StreamRig::build(
            pool.clone(),
            Arc::new(FakeReportSink::new()),
            Some((sink.clone(), limits)),
            StreamLimits::default(),
        )
        .await;
        sqlx::query("INSERT INTO sentinels (vm, export_root, service_account) VALUES ($1, '/export', $2)")
            .bind(VM)
            .bind(SERVICE_ACCOUNT)
            .execute(&pool)
            .await
            .unwrap();
        Some(Self { db, pool, sink, hub })
    }

    fn accept_google(&self, token: &str, email: &str) {
        self.hub.verifier.accept_google(
            token,
            AUDIENCE,
            GoogleIdentity {
                subject: ShortText::parse("1234567890").unwrap(),
                email: ShortText::parse(email).unwrap(),
                audience: ShortText::parse(AUDIENCE).unwrap(),
            },
        );
    }

    /// `Sentinel.Join` with no client certificate, as a sentinel that has none yet.
    async fn join(
        &self,
        vm: &str,
        kind: pb::PeerKind,
        credential: pb::join_request::Credential,
        csr: Vec<u8>,
    ) -> Result<pb::JoinResponse, Status> {
        let tls = client_tls(&self.hub.ca, None);
        let h2 = dial(self.hub.server.addr, &tls).await.unwrap();
        sentinel_client(h2)
            .join(Request::new(pb::JoinRequest {
                swimlane_id: vm.to_owned(),
                csr_der: Bytes::from(csr),
                credential: Some(credential),
                kind: kind as i32,
            }))
            .await
            .map(tonic::Response::into_inner)
    }

    async fn join_with_google(&self, vm: &str, token: &str) -> Result<pb::JoinResponse, Status> {
        let (_key, csr) = fresh_csr();
        self.join(
            vm,
            pb::PeerKind::Sentinel,
            pb::join_request::Credential::GoogleIdToken(token.to_owned()),
            csr,
        )
        .await
    }

    async fn sentinel_identity(&self, name: &str) -> ClientIdentity {
        identity_for(
            &self.hub.ca,
            &CertProfile::sentinel(&PeerName::parse(name).unwrap()),
        )
        .await
    }

    async fn audit_rows(&self) -> Vec<(String, String)> {
        sqlx::query("SELECT action, actor_kind FROM audit_events ORDER BY seq")
            .fetch_all(&self.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect()
    }

    /// Open `Report` as `identity` (none: no client certificate), having sent `first`.
    async fn report(
        &self,
        identity: Option<&ClientIdentity>,
        first: pb::SentinelMessage,
    ) -> Result<Link, Status> {
        let tls = client_tls(&self.hub.ca, identity);
        let h2 = dial(self.hub.server.addr, &tls)
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        let mut client = sentinel_client(h2);
        let (tx, rx) = mpsc::channel(64);
        tx.send(first).await.unwrap();
        let response = client.report(Request::new(ReceiverStream::new(rx))).await?;
        Ok(Link {
            tx,
            rx: response.into_inner(),
        })
    }

    /// A sentinel with a valid certificate for `VM`, past `SentinelHello` and its settings.
    async fn open(&self) -> Link {
        let identity = self.sentinel_identity(VM).await;
        let mut link = self.report(Some(&identity), hello(VM)).await.unwrap();
        assert!(matches!(
            link.next().await,
            Some(Ok(pb::sentinel_ack::Kind::Settings(_)))
        ));
        link
    }
}

struct Link {
    tx: mpsc::Sender<pb::SentinelMessage>,
    rx: Streaming<pb::SentinelAck>,
}

impl Link {
    async fn next_within(&mut self, wait: Duration) -> Option<Result<pb::sentinel_ack::Kind, Status>> {
        match tokio::time::timeout(wait, self.rx.message()).await {
            Err(_) | Ok(Ok(None)) => None,
            Ok(Err(status)) => Some(Err(status)),
            Ok(Ok(Some(message))) => message.kind.map(Ok),
        }
    }

    async fn next(&mut self) -> Option<Result<pb::sentinel_ack::Kind, Status>> {
        self.next_within(WITHIN).await
    }

    /// The next acknowledged `seq`, skipping settings; `None` if nothing arrives within `wait`.
    async fn next_ack(&mut self, wait: Duration) -> Option<u64> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next_within(left).await? {
                Ok(pb::sentinel_ack::Kind::AckedSeq(seq)) => return Some(seq),
                Ok(pb::sentinel_ack::Kind::Settings(_)) => {}
                Err(_) => return None,
            }
        }
    }

    async fn send(&self, kind: pb::sentinel_message::Kind) {
        self.tx
            .send(pb::SentinelMessage { kind: Some(kind) })
            .await
            .unwrap();
    }

    async fn send_batch(&self, seq: u64, records: usize) {
        self.send(pb::sentinel_message::Kind::Batch(batch(seq, records)))
            .await;
    }

    /// The status the stream ends with, skipping any messages before it.
    async fn ends_with(&mut self) -> Option<Status> {
        loop {
            let next = tokio::time::timeout(WITHIN, self.rx.message())
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

fn hello(vm: &str) -> pb::SentinelMessage {
    pb::SentinelMessage {
        kind: Some(pb::sentinel_message::Kind::Hello(pb::SentinelHello {
            vm: vm.to_owned(),
            export_root: "/export".to_owned(),
            version: "0.1.0".to_owned(),
        })),
    }
}

fn record(path: &str) -> pb::AuditRecord {
    pb::AuditRecord {
        time_ms: 1_800_000_000_000,
        path: path.to_owned(),
        operation: pb::AuditOperation::Write as i32,
        success: true,
        login_user: "alice".to_owned(),
        effective_user: "app".to_owned(),
        exe: "/usr/bin/vim".to_owned(),
        comm: "vim".to_owned(),
    }
}

fn batch(seq: u64, records: usize) -> pb::AuditRecordBatch {
    pb::AuditRecordBatch {
        seq,
        records: (0..records)
            .map(|i| record(&format!("svc/file-{i}.yml")))
            .collect(),
    }
}

fn san_uris(der: &[u8]) -> Vec<String> {
    let (_, cert) = X509Certificate::from_der(der).unwrap();
    cert.subject_alternative_name()
        .unwrap()
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    GeneralName::URI(u) => Some((*u).to_owned()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------------------ Join

#[tokio::test]
async fn sentinel_join_issues_a_24h_certificate_with_the_sentinel_san_and_audits_it() {
    let Some(rig) = Rig::new().await else { return };
    rig.accept_google("gid-vm1", SERVICE_ACCOUNT);

    let response = rig.join_with_google(VM, "gid-vm1").await.unwrap();

    assert_eq!(response.cert_chain_der.len(), 2, "the leaf and the CA");
    assert_eq!(
        san_uris(&response.cert_chain_der[0]),
        [format!("spiffe://lanekeeper/sentinel/{VM}")],
        "the name comes from the registered VM, not from anything the request carried"
    );
    let (_, leaf) = X509Certificate::from_der(&response.cert_chain_der[0]).unwrap();
    let life = leaf.validity().not_after.timestamp() - leaf.validity().not_before.timestamp();
    assert_eq!(
        life,
        24 * 3600 + 300,
        "24 hours, and the five minutes of back-dating"
    );
    assert_eq!(
        response.not_after_ms,
        rig.hub.ca.clock_now_ms() + 24 * 3600 * 1000
    );
    // Client authentication only: this certificate is never a server's.
    let eku = leaf.extended_key_usage().unwrap().unwrap().value;
    assert!(eku.client_auth && !eku.server_auth, "{eku:?}");
    // One audit event, by the sentinel, from the committed transaction.
    assert_eq!(
        rig.audit_rows().await,
        [("agent_joined".to_owned(), "sentinel".to_owned())]
    );
}

#[tokio::test]
async fn sentinel_join_with_wrong_service_account_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    // A valid Google ID token, but of another service account than the one registered for this VM.
    rig.accept_google("gid-other", "someone-else@project.iam.gserviceaccount.com");
    // The service account of the right VM, used to ask for a certificate of another VM that is registered to someone else.
    sqlx::query("INSERT INTO sentinels (vm, export_root, service_account) VALUES ('nfs-vm-2', '/export', 'sentinel-nfs-vm-2@project.iam.gserviceaccount.com')")
        .execute(&rig.pool)
        .await
        .unwrap();
    rig.accept_google("gid-vm1", SERVICE_ACCOUNT);

    for (vm, token) in [(VM, "gid-other"), ("nfs-vm-2", "gid-vm1")] {
        let status = rig.join_with_google(vm, token).await.unwrap_err();
        assert_eq!(status.code(), Code::Unauthenticated, "{vm}");
        assert_eq!(status.message(), "", "no reason is given");
    }
    assert!(
        rig.audit_rows().await.is_empty(),
        "nothing was issued, so nothing is audited"
    );
}

#[tokio::test]
async fn sentinel_join_failures_are_indistinguishable() {
    let Some(rig) = Rig::new().await else { return };
    sqlx::query("INSERT INTO sentinels (vm, export_root) VALUES ('no-account', '/export')")
        .execute(&rig.pool)
        .await
        .unwrap();
    rig.accept_google("gid-vm1", SERVICE_ACCOUNT);
    rig.accept_google("gid-other", "someone-else@project.iam.gserviceaccount.com");

    let mut seen = Vec::new();
    // Wrong account for a known VM, an unknown VM, a known VM with no registered account, an unknown token, a bad name.
    for (vm, token) in [
        (VM, "gid-other"),
        ("unknown-vm", "gid-vm1"),
        ("no-account", "gid-vm1"),
        (VM, "gid-never-issued"),
        ("../etc/passwd", "gid-vm1"),
    ] {
        let status = rig.join_with_google(vm, token).await.unwrap_err();
        seen.push((status.code(), status.message().to_owned()));
    }
    assert!(
        seen.iter().all(|s| s == &(Code::Unauthenticated, String::new())),
        "every refusal looks the same: {seen:?}"
    );
}

#[tokio::test]
async fn sentinel_join_with_a_join_token_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    // Join tokens are an agent's fallback, bound to a swimlane. A sentinel has only its VM's identity.
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    rig.db
        .seed_join_token(&rig.pool, "sit1", "jt-for-sit1", 60, 3600)
        .await;
    let (_key, csr) = fresh_csr();
    let status = rig
        .join(
            VM,
            pb::PeerKind::Sentinel,
            pb::join_request::Credential::JoinToken("jt-for-sit1".to_owned()),
            csr,
        )
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::Unauthenticated);
    assert_eq!(
        rig.db
            .count(
                &rig.pool,
                "SELECT count(*) FROM join_tokens WHERE used_at IS NOT NULL"
            )
            .await,
        0
    );
}

#[tokio::test]
async fn sentinel_join_with_an_agent_kind_is_refused_on_the_sentinel_service() {
    let Some(rig) = Rig::new().await else { return };
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.accept_google("gid-vm1", SERVICE_ACCOUNT);
    // The same service account is registered for a swimlane and for the VM here; the service still decides by kind.
    let (_key, csr) = fresh_csr();
    let status = rig
        .join(
            "sit1",
            pb::PeerKind::Agent,
            pb::join_request::Credential::GoogleIdToken("gid-vm1".to_owned()),
            csr,
        )
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::Unauthenticated);
    assert!(rig.audit_rows().await.is_empty());
}

#[tokio::test]
async fn a_failing_audit_write_means_no_sentinel_certificate() {
    let Some(rig) = Rig::new().await else { return };
    rig.accept_google("gid-vm1", SERVICE_ACCOUNT);
    rig.hub.audit.set_failing(true);
    let status = rig.join_with_google(VM, "gid-vm1").await.unwrap_err();
    assert_eq!(
        status.code(),
        Code::Unavailable,
        "the certificate is dropped unsent"
    );
    assert!(rig.audit_rows().await.is_empty());
    rig.hub.audit.set_failing(false);
    assert!(
        rig.join_with_google(VM, "gid-vm1").await.is_ok(),
        "and a later try works"
    );
    assert_eq!(rig.audit_rows().await.len(), 1);
}

#[tokio::test]
async fn sentinel_join_logs_never_contain_the_credential() {
    let Some(rig) = Rig::new().await else { return };
    let (capture, _guard) = LogCapture::install();
    rig.accept_google(
        "gid-SECRET-token-value",
        "someone-else@project.iam.gserviceaccount.com",
    );
    let _ = rig.join_with_google(VM, "gid-SECRET-token-value").await;
    let _ = rig.join_with_google("unknown-vm", "gid-SECRET-token-value").await;
    let log = capture.text();
    assert!(!log.is_empty(), "the refusal is logged with a code");
    assert!(
        log.contains("join_email_mismatch") || log.contains("join_sentinel"),
        "{log}"
    );
    for secret in ["gid-SECRET-token-value", SERVICE_ACCOUNT, "someone-else@"] {
        assert!(!log.contains(secret), "{secret} is in the log: {log}");
    }
}

// ------------------------------------------------------------------------------------------- kinds of certificate

#[tokio::test]
async fn sentinel_cert_is_not_accepted_on_agent_service_and_reverse() {
    let Some(rig) = Rig::new().await else { return };
    let sentinel = rig.sentinel_identity(VM).await;
    let agent = rig.hub.agent_identity("sit1").await;
    let replica = identity_for(
        &rig.hub.ca,
        &CertProfile::replica(&PeerName::parse("hub-0").unwrap(), &["localhost".to_owned()]).unwrap(),
    )
    .await;

    // A sentinel's certificate on `Agent.Connect`.
    let tls = client_tls(&rig.hub.ca, Some(&sentinel));
    let h2 = dial(rig.hub.server.addr, &tls).await.unwrap();
    let (tx, rx) = mpsc::channel::<pb::AgentMessage>(4);
    tx.send(agent_hello("sit1")).await.unwrap();
    let status = agent_client(h2)
        .connect(Request::new(ReceiverStream::new(rx)))
        .await
        .err()
        .unwrap();
    assert_eq!(
        status.code(),
        Code::PermissionDenied,
        "a sentinel is not an agent"
    );

    // An agent's certificate, and a replica's, on `Sentinel.Report`.
    for (who, identity) in [("agent", &agent), ("replica", &replica)] {
        let status = rig.report(Some(identity), hello(VM)).await.err().unwrap();
        assert_eq!(status.code(), Code::PermissionDenied, "{who}");
    }
    assert!(
        rig.sink.hellos().is_empty(),
        "the sink heard nothing from any of them"
    );

    // The control: each certificate on its own service works.
    assert!(rig.report(Some(&sentinel), hello(VM)).await.is_ok());
}

#[tokio::test]
async fn sentinel_report_without_client_cert_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    let status = rig.report(None, hello(VM)).await.err().unwrap();
    assert_eq!(status.code(), Code::Unauthenticated);
    assert!(rig.sink.hellos().is_empty());
}

// ----------------------------------------------------------------------------------------------------- the stream

#[tokio::test]
async fn sentinel_report_hello_vm_must_equal_san_name() {
    let Some(rig) = Rig::new().await else { return };
    let identity = rig.sentinel_identity(VM).await;
    for other in ["nfs-vm-2", "nfs-vm-", "NFS-VM-1", "nfs-vm-1-extra", ""] {
        let status = rig.report(Some(&identity), hello(other)).await.err().unwrap();
        // An empty or invalid name does not even decode; a valid other name is a certificate speaking for another VM.
        assert!(
            matches!(status.code(), Code::PermissionDenied | Code::FailedPrecondition),
            "{other:?}: {status:?}"
        );
        assert_eq!(status.message(), "");
    }
    assert!(rig.sink.hellos().is_empty(), "the sink never heard of another VM");
    assert!(
        rig.report(Some(&identity), hello(VM)).await.is_ok(),
        "the control: its own name connects"
    );
}

#[tokio::test]
async fn sentinel_report_requires_hello_first() {
    let Some(rig) = Rig::new().await else { return };
    let identity = rig.sentinel_identity(VM).await;
    for first in [
        pb::SentinelMessage {
            kind: Some(pb::sentinel_message::Kind::Batch(batch(1, 1))),
        },
        pb::SentinelMessage {
            kind: Some(pb::sentinel_message::Kind::Heartbeat(pb::SentinelHeartbeat {
                sent_at_ms: 1,
            })),
        },
        pb::SentinelMessage { kind: None },
    ] {
        let status = rig.report(Some(&identity), first).await.err().unwrap();
        assert_eq!(status.code(), Code::FailedPrecondition);
    }
    assert!(rig.sink.hellos().is_empty() && rig.sink.committed().is_empty());
}

#[tokio::test]
async fn sentinel_report_hello_must_arrive_within_the_hello_timeout() {
    let limits = SentinelLimits {
        hello_timeout: Duration::from_millis(150),
        ..SentinelLimits::default()
    };
    let Some(rig) = Rig::with_limits(limits).await else {
        return;
    };
    let identity = rig.sentinel_identity(VM).await;
    let tls = client_tls(&rig.hub.ca, Some(&identity));
    let h2 = dial(rig.hub.server.addr, &tls).await.unwrap();
    // The sentinel opens the stream and says nothing.
    let (_tx, rx) = mpsc::channel::<pb::SentinelMessage>(1);
    let status = sentinel_client(h2)
        .report(Request::new(ReceiverStream::new(rx)))
        .await
        .err()
        .unwrap();
    assert_eq!(status.code(), Code::DeadlineExceeded);
    assert!(rig.sink.hellos().is_empty());
}

#[tokio::test]
async fn sentinel_report_sends_settings_after_hello_and_gives_the_sink_the_certificate_identity() {
    let Some(rig) = Rig::new().await else { return };
    let identity = rig.sentinel_identity(VM).await;
    let mut link = rig.report(Some(&identity), hello(VM)).await.unwrap();
    match link.next().await {
        Some(Ok(pb::sentinel_ack::Kind::Settings(settings))) => {
            assert_eq!(
                (settings.max_batch_records, settings.flush_interval_secs),
                (500, 5)
            );
        }
        other => panic!("expected the settings first, got {other:?}"),
    }
    let hellos = rig.sink.hellos();
    assert_eq!(hellos.len(), 1);
    assert_eq!(
        hellos[0].0.name().as_str(),
        VM,
        "the identity is the certificate's"
    );
    assert_eq!(hellos[0].1.vm.as_str(), VM);
    assert_eq!(hellos[0].1.export_root.as_str(), "/export");
}

#[tokio::test]
async fn sentinel_batch_is_acked_with_the_seq_the_sink_returns_only_after_it_returns() {
    let Some(rig) = Rig::new().await else { return };
    let gate = rig.sink.hold(1);
    let mut link = rig.open().await;

    link.send_batch(1, 3).await;
    rig.sink.wait_started(1).await;
    assert_eq!(
        link.next_ack(QUIET).await,
        None,
        "no ack while the sink is still applying the batch"
    );
    assert!(rig.sink.committed().is_empty());

    gate.notify_one();
    assert_eq!(link.next_ack(WITHIN).await, Some(1));
    let committed = rig.sink.committed();
    assert_eq!(committed.len(), 1);
    assert_eq!(
        (committed[0].0.name().as_str(), committed[0].1, committed[0].2),
        (VM, 1, 3)
    );

    // A batch the sink had already applied is answered with the highest seq so far, and that is what is sent, not the batch's own.
    rig.sink.answer(2, 5);
    link.send_batch(2, 1).await;
    assert_eq!(link.next_ack(WITHIN).await, Some(5));
    assert_eq!(link.next_ack(QUIET).await, None, "one ack per batch");
}

#[tokio::test]
async fn sentinel_acks_follow_batch_order() {
    let Some(rig) = Rig::new().await else { return };
    let mut link = rig.open().await;
    for seq in 1..=10_u64 {
        link.send_batch(seq, 1).await;
    }
    let mut acked = Vec::new();
    for _ in 1..=10 {
        acked.push(link.next_ack(WITHIN).await.expect("an ack per batch"));
    }
    assert_eq!(acked, (1..=10).collect::<Vec<_>>());
}

#[tokio::test]
async fn sentinel_failed_batch_sends_no_ack() {
    let Some(rig) = Rig::new().await else { return };
    rig.sink.fail(2, SinkError::Unavailable);
    rig.sink.fail(3, SinkError::TooLarge);
    rig.sink.fail(4, SinkError::Invalid);
    let mut link = rig.open().await;
    for seq in 1..=5_u64 {
        link.send_batch(seq, 1).await;
    }
    let mut acked = Vec::new();
    while let Some(seq) = link.next_ack(QUIET).await {
        acked.push(seq);
    }
    assert_eq!(
        acked,
        [1, 5],
        "the sentinel keeps what was refused in its spool and resends it"
    );
}

#[tokio::test]
async fn sentinel_heartbeat_is_accepted_and_is_not_acknowledged() {
    let Some(rig) = Rig::new().await else { return };
    let mut link = rig.open().await;
    link.send(pb::sentinel_message::Kind::Heartbeat(pb::SentinelHeartbeat {
        sent_at_ms: 5,
    }))
    .await;
    assert_eq!(link.next_ack(QUIET).await, None);
    link.send_batch(1, 1).await;
    assert_eq!(link.next_ack(WITHIN).await, Some(1), "the stream is alive");
}

#[tokio::test]
async fn sentinel_report_invalid_message_ends_the_stream_and_unknown_kinds_are_skipped() {
    let Some(rig) = Rig::new().await else { return };

    // A message with a path that escapes the root fails validation before a domain value exists: the stream ends.
    let mut link = rig.open().await;
    let mut bad = batch(1, 1);
    bad.records[0].path = "../../etc/passwd".to_owned();
    link.send(pb::sentinel_message::Kind::Batch(bad)).await;
    let status = link.ends_with().await.expect("the stream ends with a status");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(rig.sink.committed().is_empty(), "the sink never saw it");

    // An empty oneof is a kind this build does not know: skipped, and the stream goes on.
    let mut link = rig.open().await;
    link.tx.send(pb::SentinelMessage { kind: None }).await.unwrap();
    link.send_batch(2, 1).await;
    assert_eq!(link.next_ack(WITHIN).await, Some(2));
}

#[tokio::test]
async fn sentinel_stream_ends_when_the_hub_is_closing() {
    let Some(rig) = Rig::new().await else { return };
    let mut link = rig.open().await;
    rig.hub.closing.send_replace(true);
    let status = link.ends_with().await.expect("the stream ends with a status");
    assert_eq!(
        status.code(),
        Code::Unavailable,
        "the sentinel reconnects elsewhere"
    );
}

#[tokio::test]
async fn sentinel_report_hello_refused_by_the_sink_ends_the_call_without_a_reason() {
    let Some(rig) = Rig::new().await else { return };
    *rig.sink.fail_hello.lock().unwrap() = Some(SinkError::Unavailable);
    let identity = rig.sentinel_identity(VM).await;
    let status = rig.report(Some(&identity), hello(VM)).await.err().unwrap();
    assert_eq!(status.code(), Code::Unavailable);
    assert_eq!(status.message(), "");
}

#[tokio::test]
async fn sentinel_join_attempts_are_limited() {
    let Some(rig) = Rig::new().await else { return };
    rig.accept_google("gid-other", "someone-else@project.iam.gserviceaccount.com");
    let mut codes = Vec::new();
    for _ in 0..40 {
        codes.push(rig.join_with_google(VM, "gid-other").await.unwrap_err().code());
    }
    assert!(
        codes.contains(&Code::ResourceExhausted),
        "a flood of refused joins for one VM is cut off: {codes:?}"
    );
    assert_eq!(codes[0], Code::Unauthenticated);
}
