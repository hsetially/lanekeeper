//! A fake hub (S5, S6): it checks the credential, then issues a real certificate for the key in the agent's real CSR,
//! from a real CA, and records everything it was asked.
//!
//! [`FakeHub`] is the hub's `Join` and `CertRenewal` handling without a transport; the join and renewal tests use it
//! directly. [`HubServer`] wraps it in the real thing: a tonic `Agent` service built from `proto::grpc::agent_server`,
//! behind a real TLS 1.3 acceptor, reached over the in-memory network of `fake_net`. It speaks first after `Hello` the
//! way the hub does (`AgentConfig`), records every message, answers certificate renewals, and lets a test send any
//! message, valid or not.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::clock::Clock;
use agent::identity::idtoken::IdTokenSource;
use agent::identity::joiner::{JoinClient, RenewalChannel};
use agent::identity::jointoken::JoinTokenSource;
use agent::identity::{IdTokenError, JoinError, JoinTokenError, RenewError};
use async_trait::async_trait;
use bytes::Bytes;
use domain::{Secret, ShortText};
use proto::convert::{FromAgent, IssuedCert, JoinCredential, JoinParams, JoinSubject, ToAgent};
use proto::pb;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use super::fake_net::{FakeNet, PeerInfo};
use super::test_ca::{IssueSpec, TestCa};
use super::tls::{HubVersions, server_config};

/// Which kind of credential a join carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    GoogleIdToken,
    JoinToken,
}

#[derive(Debug, Clone)]
pub struct JoinRecord {
    pub subject: String,
    pub kind: CredentialKind,
    pub credential: String,
    pub csr: Bytes,
    pub at: Instant,
}

#[derive(Debug, Clone)]
pub struct RenewRecord {
    pub csr: Bytes,
    pub at: Instant,
}

/// What the next certificate looks like, as a function of the swimlane and the wall clock.
type Issuer = dyn Fn(&str, i64) -> IssueSpec + Send + Sync;

struct State {
    expect_google: Option<String>,
    expect_join: Option<String>,
    /// Refuse this many joins before accepting.
    reject_joins: usize,
    /// Fail renewals with these errors, in order; once empty, renewals succeed (unless `renewals_down`).
    renewal_failures: VecDeque<RenewError>,
    renewals_down: bool,
    joins: Vec<JoinRecord>,
    renewals: Vec<RenewRecord>,
    issuer: Arc<Issuer>,
}

pub struct FakeHub {
    ca: TestCa,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

impl fmt::Debug for FakeHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FakeHub")
    }
}

impl FakeHub {
    /// A hub that accepts any credential and issues the S5 certificate for the swimlane it is asked about.
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            ca: TestCa::new(),
            clock,
            state: Mutex::new(State {
                expect_google: None,
                expect_join: None,
                reject_joins: 0,
                renewal_failures: VecDeque::new(),
                renewals_down: false,
                joins: Vec::new(),
                renewals: Vec::new(),
                issuer: Arc::new(IssueSpec::agent),
            }),
        })
    }

    /// The CA that signs the agents' certificates, and the hub's own TLS certificate in [`HubServer`].
    pub fn ca(&self) -> &TestCa {
        &self.ca
    }

    /// Accept only this Google ID token.
    pub fn expect_google_token(&self, token: &str) {
        self.state.lock().unwrap().expect_google = Some(token.to_owned());
    }

    /// Accept only this join token.
    pub fn expect_join_token(&self, token: &str) {
        self.state.lock().unwrap().expect_join = Some(token.to_owned());
    }

    pub fn reject_next_joins(&self, n: usize) {
        self.state.lock().unwrap().reject_joins = n;
    }

    pub fn fail_next_renewals(&self, failures: &[RenewError]) {
        self.state
            .lock()
            .unwrap()
            .renewal_failures
            .extend(failures.iter().copied());
    }

    /// Every renewal fails with `NotConnected` from now on, as when the stream is down.
    pub fn stream_down(&self) {
        self.state.lock().unwrap().renewals_down = true;
    }

    /// Issue certificates described by `issuer` instead of the S5 default.
    pub fn issue_with(&self, issuer: impl Fn(&str, i64) -> IssueSpec + Send + Sync + 'static) {
        self.state.lock().unwrap().issuer = Arc::new(issuer);
    }

    pub fn joins(&self) -> Vec<JoinRecord> {
        self.state.lock().unwrap().joins.clone()
    }

    pub fn renewals(&self) -> Vec<RenewRecord> {
        self.state.lock().unwrap().renewals.clone()
    }

    /// A certificate for any CSR, as the hub would issue to `swimlane` now.
    pub fn issue(&self, swimlane: &str, csr: &[u8]) -> IssuedCert {
        let now_ms = self.clock.now().unix_millis();
        let spec = (self.state.lock().unwrap().issuer.clone())(swimlane, now_ms);
        let chain = self.ca.issue_for_csr(csr, &spec);
        IssuedCert {
            cert_chain_der: chain,
            not_after: domain::Timestamp::from_unix_millis(spec.not_after * 1000),
        }
    }
}

#[async_trait]
impl JoinClient for FakeHub {
    async fn join(&self, params: JoinParams) -> Result<IssuedCert, JoinError> {
        let (kind, credential) = match &params.credential {
            JoinCredential::GoogleIdToken(t) => (CredentialKind::GoogleIdToken, t.expose().clone()),
            JoinCredential::JoinToken(t) => (CredentialKind::JoinToken, t.expose().clone()),
        };
        let JoinSubject::Agent(swimlane) = &params.subject else {
            return Err(JoinError::Rejected);
        };
        let accepted = {
            let mut state = self.state.lock().unwrap();
            state.joins.push(JoinRecord {
                subject: swimlane.as_str().to_owned(),
                kind,
                credential: credential.clone(),
                csr: params.csr_der.clone(),
                at: Instant::now(),
            });
            let expected = match kind {
                CredentialKind::GoogleIdToken => state.expect_google.as_ref(),
                CredentialKind::JoinToken => state.expect_join.as_ref(),
            };
            let matches = expected.is_none_or(|e| *e == credential);
            if state.reject_joins > 0 {
                state.reject_joins -= 1;
                false
            } else {
                matches
            }
        };
        if !accepted {
            return Err(JoinError::Rejected);
        }
        Ok(self.issue(swimlane.as_str(), &params.csr_der))
    }
}

#[async_trait]
impl RenewalChannel for FakeHub {
    async fn renew(&self, csr_der: Bytes) -> Result<IssuedCert, RenewError> {
        let swimlane = {
            let mut state = self.state.lock().unwrap();
            state.renewals.push(RenewRecord {
                csr: csr_der.clone(),
                at: Instant::now(),
            });
            if state.renewals_down {
                return Err(RenewError::NotConnected);
            }
            if let Some(failure) = state.renewal_failures.pop_front() {
                return Err(failure);
            }
            // The stream's mTLS identity names the swimlane; the fake uses the one it is told about.
            "sit1"
        };
        Ok(self.issue(swimlane, &csr_der))
    }
}

/// An ID token source that answers from a script and counts its calls.
#[derive(Debug)]
pub struct ScriptedIdTokens {
    answer: Mutex<Result<String, IdTokenError>>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedIdTokens {
    pub fn new(answer: Result<&str, IdTokenError>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer.map(str::to_owned)),
            calls: Mutex::new(Vec::new()),
        })
    }

    /// The audiences it was asked for.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl IdTokenSource for ScriptedIdTokens {
    async fn id_token(&self, audience: &ShortText) -> Result<Secret<String>, IdTokenError> {
        self.calls.lock().unwrap().push(audience.as_str().to_owned());
        self.answer.lock().unwrap().clone().map(Secret::new)
    }
}

/// A join token source that answers from a script and counts its calls.
#[derive(Debug)]
pub struct ScriptedJoinTokens {
    answer: Mutex<Result<String, JoinTokenError>>,
    calls: Mutex<usize>,
}

impl ScriptedJoinTokens {
    pub fn new(answer: Result<&str, JoinTokenError>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer.map(str::to_owned)),
            calls: Mutex::new(0),
        })
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

#[async_trait]
impl JoinTokenSource for ScriptedJoinTokens {
    async fn join_token(&self) -> Result<Secret<String>, JoinTokenError> {
        *self.calls.lock().unwrap() += 1;
        self.answer.lock().unwrap().clone().map(Secret::new)
    }
}

// ------------------------------------------------------------------ the hub as a gRPC server

/// The name the hub's certificate carries and the agent connects to.
pub const HUB_NAME: &str = "hub.lanekeeper.test";
pub const HUB_URL: &str = "https://hub.lanekeeper.test:8443";

/// What the hub saw of one `Join` call.
#[derive(Debug, Clone)]
pub struct JoinSeen {
    /// Whether the TLS handshake carried a client certificate. A join must not need one.
    pub has_client_cert: bool,
    pub grpc_encoding: Option<String>,
}

struct Conn {
    peer: PeerInfo,
    grpc_encoding: Option<String>,
    received: Vec<pb::AgentMessage>,
    to_agent: mpsc::Sender<Result<pb::HubMessage, Status>>,
    agent_closed: bool,
}

struct HubState {
    config: pb::AgentConfig,
    joins: Vec<JoinSeen>,
    /// One slot per `Connect`, in order. A forgotten connection leaves `None` behind so that indices stay valid.
    conns: Vec<Option<Conn>>,
}

struct Shared {
    hub: Arc<FakeHub>,
    state: Mutex<HubState>,
    changed: watch::Sender<u64>,
    require_client_cert: AtomicBool,
    answer_renewals: AtomicBool,
    refuse_connect: AtomicBool,
    send_config: AtomicBool,
    /// Acknowledge every scan delta the moment it arrives, as the real hub does after it has applied it (T9).
    auto_ack: AtomicBool,
}

impl Shared {
    fn touch(&self) {
        self.changed.send_modify(|version| *version += 1);
    }
}

/// The fake hub as a server on a [`FakeNet`].
pub struct HubServer {
    pub hub: Arc<FakeHub>,
    pub net: Arc<FakeNet>,
    shared: Arc<Shared>,
}

impl fmt::Debug for HubServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HubServer")
    }
}

impl HubServer {
    /// A hub that offers TLS 1.3, requires a client certificate on `Connect`, and answers renewals.
    pub fn start(clock: Arc<dyn Clock>) -> Arc<Self> {
        Self::start_with(clock, HubVersions::Tls13Only)
    }

    pub fn start_with(clock: Arc<dyn Clock>, versions: HubVersions) -> Arc<Self> {
        let hub = FakeHub::new(clock.clone());
        let tls = server_config(hub.ca(), &[HUB_NAME], hub.ca(), versions, clock);
        let shared = Arc::new(Shared {
            hub: hub.clone(),
            state: Mutex::new(HubState {
                config: pb::AgentConfig {
                    scan_interval_secs: 10,
                    heartbeat_interval_secs: 10,
                    max_file_bytes: 0,
                    deny_globs: Vec::new(),
                    env_allowlist: Vec::new(),
                    tenants: vec!["sit1".to_owned()],
                },
                joins: Vec::new(),
                conns: Vec::new(),
            }),
            changed: watch::channel(0).0,
            require_client_cert: AtomicBool::new(true),
            answer_renewals: AtomicBool::new(true),
            refuse_connect: AtomicBool::new(false),
            send_config: AtomicBool::new(true),
            auto_ack: AtomicBool::new(true),
        });
        let serving = Arc::clone(&shared);
        let net = FakeNet::new(tls, move |io| {
            let service = proto::grpc::agent_server(HubService {
                shared: Arc::clone(&serving),
            });
            async move {
                let incoming = tokio_stream::once(Ok::<_, io::Error>(io));
                let _ = tonic::transport::Server::builder()
                    .serve_with_incoming(service, incoming)
                    .await;
            }
        });
        Arc::new(Self { hub, net, shared })
    }

    /// The content of `LK_HUB_CA_FILE`.
    pub fn ca_pem(&self) -> String {
        self.hub.ca().pem()
    }

    /// The `AgentConfig` the hub sends after `Hello`, from the next connection on.
    pub fn set_config(&self, config: pb::AgentConfig) {
        self.shared.state.lock().unwrap().config = config;
    }

    /// Accept `Connect` without a client certificate, to show what the agent does when the hub does not insist.
    pub fn set_require_client_cert(&self, required: bool) {
        self.shared.require_client_cert.store(required, Ordering::SeqCst);
    }

    /// Answer every `Connect` with a refusal (`PermissionDenied`), as a hub that no longer trusts the certificate.
    pub fn set_refuse_connect(&self, refuse: bool) {
        self.shared.refuse_connect.store(refuse, Ordering::SeqCst);
    }

    /// Whether the hub sends its `AgentConfig` after `Hello`. A hub that does not is a hub that is stuck.
    /// Whether the hub acknowledges deltas (the default). Off, the agent's spool keeps everything it sends.
    pub fn set_auto_ack(&self, ack: bool) {
        self.shared.auto_ack.store(ack, Ordering::SeqCst);
    }

    pub fn set_send_config(&self, send: bool) {
        self.shared.send_config.store(send, Ordering::SeqCst);
    }

    /// Stop answering `CertRenewalRequest`, as a hub that is busy or broken.
    pub fn set_answer_renewals(&self, answer: bool) {
        self.shared.answer_renewals.store(answer, Ordering::SeqCst);
    }

    pub fn joins(&self) -> Vec<JoinSeen> {
        self.shared.state.lock().unwrap().joins.clone()
    }

    /// How many `Connect` streams have been opened so far.
    pub fn connection_count(&self) -> usize {
        self.shared.state.lock().unwrap().conns.len()
    }

    /// The `index`-th stream (from 0).
    pub fn connection(&self, index: usize) -> ConnHandle {
        ConnHandle {
            shared: Arc::clone(&self.shared),
            index,
        }
    }

    /// Drop what is recorded about the streams before the `n`-th (from 1), for tests that run for hundreds of
    /// connections and measure memory. The streams still count; their handles can no longer be used.
    pub fn forget_connections_before(&self, n: usize) {
        let mut state = self.shared.state.lock().unwrap();
        for slot in state.conns.iter_mut().take(n.saturating_sub(1)) {
            *slot = None;
        }
    }

    /// Wait until the hub has seen at least `n` streams, and return the `n`-th (from 1).
    pub async fn wait_for_connection(&self, n: usize) -> ConnHandle {
        wait(&self.shared, |s| s.conns.len() >= n).await;
        self.connection(n - 1)
    }

    /// Cut every connection, refuse new ones for `down_for` (virtual time), then accept again: a hub restart.
    pub async fn restart(&self, down_for: Duration) {
        self.net.set_reachable(false);
        self.net.kill_connections();
        tokio::time::sleep(down_for).await;
        self.net.set_reachable(true);
    }
}

/// The longest a test waits for the hub to see something. It is virtual time under `tokio::time::pause`, so a missing
/// event fails the test in a blink instead of hanging it.
pub const WAIT_LIMIT: Duration = Duration::from_secs(600);

/// Wait until `condition` holds for the hub's state. Safe against missed updates: the version is read first.
async fn wait(shared: &Shared, condition: impl Fn(&HubState) -> bool) {
    let mut changes = shared.changed.subscribe();
    tokio::time::timeout(WAIT_LIMIT, async {
        loop {
            if condition(&shared.state.lock().unwrap()) {
                return;
            }
            changes.changed().await.unwrap();
        }
    })
    .await
    .expect("timed out waiting for the fake hub to see what the test expects");
}

/// One `Connect` stream, from the hub's side.
#[derive(Clone)]
pub struct ConnHandle {
    shared: Arc<Shared>,
    index: usize,
}

impl fmt::Debug for ConnHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ConnHandle({})", self.index)
    }
}

impl ConnHandle {
    fn with<T>(&self, f: impl FnOnce(&Conn) -> T) -> T {
        let state = self.shared.state.lock().unwrap();
        f(state.conns[self.index]
            .as_ref()
            .expect("this connection was forgotten"))
    }

    /// The client certificate the agent presented in the TLS handshake (the leaf, DER).
    pub fn client_cert(&self) -> Option<Vec<u8>> {
        self.with(|c| c.peer.client_cert.clone())
    }

    /// The `grpc-encoding` the agent used for its messages on this stream.
    pub fn grpc_encoding(&self) -> Option<String> {
        self.with(|c| c.grpc_encoding.clone())
    }

    /// Everything the agent sent, decoded the way the hub decodes it.
    pub fn received(&self) -> Vec<FromAgent> {
        self.with(|c| {
            c.received
                .iter()
                .map(|m| FromAgent::from_proto(m.clone()).unwrap().unwrap())
                .collect()
        })
    }

    pub fn received_count(&self) -> usize {
        self.with(|c| c.received.len())
    }

    /// The `Hello` that opened the stream.
    pub fn hello(&self) -> domain::Hello {
        match self.received().into_iter().next() {
            Some(FromAgent::Hello(hello)) => hello,
            other => panic!("the first message must be Hello, got {other:?}"),
        }
    }

    /// Wait until the agent has sent a message that satisfies `pred`, and return it.
    pub async fn wait_for(&self, pred: impl Fn(&FromAgent) -> bool) -> FromAgent {
        let index = self.index;
        let found = std::cell::RefCell::new(None);
        wait(&self.shared, |state| {
            // `state` is the locked hub state: do not lock it again from in here.
            let hit = state.conns[index]
                .as_ref()
                .expect("this connection was forgotten")
                .received
                .iter()
                .map(|m| FromAgent::from_proto(m.clone()).unwrap().unwrap())
                .find(|m| pred(m));
            let done = hit.is_some();
            *found.borrow_mut() = hit;
            done
        })
        .await;
        found.into_inner().unwrap()
    }

    /// Wait until the agent has closed its side of the stream or the connection is gone.
    pub async fn wait_closed(&self) {
        let index = self.index;
        wait(&self.shared, |s| {
            s.conns[index].as_ref().is_none_or(|c| c.agent_closed)
        })
        .await;
    }

    /// Send a valid message to the agent.
    pub async fn send(&self, message: ToAgent) {
        self.send_raw(message.into_proto()).await;
    }

    /// Send any message, valid or not.
    pub async fn send_raw(&self, message: pb::HubMessage) {
        let tx = self.with(|c| c.to_agent.clone());
        tx.send(Ok(message))
            .await
            .expect("the agent end of the stream is gone");
    }

    /// End the stream from the hub's side in an orderly way.
    pub fn end_stream(&self) {
        // Replace the sender with a closed one: dropping the last sender ends the response stream.
        let (closed, _) = mpsc::channel(1);
        let mut state = self.shared.state.lock().unwrap();
        state.conns[self.index]
            .as_mut()
            .expect("this connection was forgotten")
            .to_agent = closed;
    }
}

#[derive(Clone)]
struct HubService {
    shared: Arc<Shared>,
}

fn encoding_of<T>(request: &Request<T>) -> Option<String> {
    request
        .metadata()
        .get("grpc-encoding")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

#[tonic::async_trait]
impl pb::agent_server::Agent for HubService {
    async fn join(&self, request: Request<pb::JoinRequest>) -> Result<Response<pb::JoinResponse>, Status> {
        let peer = request
            .extensions()
            .get::<PeerInfo>()
            .cloned()
            .unwrap_or_default();
        self.shared.state.lock().unwrap().joins.push(JoinSeen {
            has_client_cert: peer.client_cert.is_some(),
            grpc_encoding: encoding_of(&request),
        });
        self.shared.touch();
        let params = JoinParams::try_from(request.into_inner())
            .map_err(|_| Status::invalid_argument("not a valid join request"))?;
        match JoinClient::join(&*self.shared.hub, params).await {
            Ok(issued) => Ok(Response::new(pb::JoinResponse::from(issued))),
            Err(JoinError::Rejected) => Err(Status::unauthenticated("the credential was refused")),
            Err(_) => Err(Status::unavailable("try again")),
        }
    }

    type ConnectStream = ReceiverStream<Result<pb::HubMessage, Status>>;

    async fn connect(
        &self,
        request: Request<Streaming<pb::AgentMessage>>,
    ) -> Result<Response<Self::ConnectStream>, Status> {
        let peer = request
            .extensions()
            .get::<PeerInfo>()
            .cloned()
            .unwrap_or_default();
        if self.shared.refuse_connect.load(Ordering::SeqCst) {
            return Err(Status::permission_denied("not allowed"));
        }
        if self.shared.require_client_cert.load(Ordering::SeqCst) && peer.client_cert.is_none() {
            return Err(Status::unauthenticated("a client certificate is required"));
        }
        let grpc_encoding = encoding_of(&request);
        let mut from_agent = request.into_inner();

        // Like the real hub, read `Hello` before answering. An agent that waits for the response before it sends
        // `Hello` never gets here, and the test that connects hangs until its timeout.
        let hello = from_agent
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("the stream ended before Hello"))?;
        if !matches!(&hello.kind, Some(pb::agent_message::Kind::Hello(_))) {
            return Err(Status::invalid_argument("the first message must be Hello"));
        }

        let (to_agent, response) = mpsc::channel(16);
        if self.shared.send_config.load(Ordering::SeqCst) {
            let config = self.shared.state.lock().unwrap().config.clone();
            to_agent
                .send(Ok(ToAgent::Config(config.try_into().unwrap()).into_proto()))
                .await
                .map_err(|_| Status::internal("response channel closed"))?;
        }
        let index = {
            let mut state = self.shared.state.lock().unwrap();
            state.conns.push(Some(Conn {
                peer,
                grpc_encoding,
                received: vec![hello],
                to_agent,
                agent_closed: false,
            }));
            state.conns.len() - 1
        };
        self.shared.touch();

        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            while let Ok(Some(message)) = from_agent.message().await {
                let renewal_csr = match &message.kind {
                    Some(pb::agent_message::Kind::CertRenewalRequest(r)) => Some(r.csr_der.clone()),
                    _ => None,
                };
                let delta_seq = match &message.kind {
                    Some(pb::agent_message::Kind::ScanDelta(d)) => Some(d.seq),
                    _ => None,
                };
                {
                    let mut state = shared.state.lock().unwrap();
                    match state.conns[index].as_mut() {
                        Some(conn) => conn.received.push(message),
                        None => break,
                    }
                }
                shared.touch();
                if let Some(seq) = delta_seq.filter(|_| shared.auto_ack.load(Ordering::SeqCst)) {
                    let to_agent = shared.state.lock().unwrap().conns[index]
                        .as_ref()
                        .map(|c| c.to_agent.clone());
                    if let Some(to_agent) = to_agent {
                        let _ = to_agent.send(Ok(ToAgent::Ack(seq).into_proto())).await;
                    }
                }
                if let Some(csr) = renewal_csr {
                    if shared.answer_renewals.load(Ordering::SeqCst) {
                        if let Ok(issued) = RenewalChannel::renew(&*shared.hub, csr).await {
                            let reply = ToAgent::CertRenewal(issued).into_proto();
                            // Looked up now, not held: a stream the hub ended must stay ended.
                            let to_agent = shared.state.lock().unwrap().conns[index]
                                .as_ref()
                                .map(|c| c.to_agent.clone());
                            if let Some(to_agent) = to_agent {
                                let _ = to_agent.send(Ok(reply)).await;
                            }
                        }
                    }
                }
            }
            if let Some(conn) = shared.state.lock().unwrap().conns[index].as_mut() {
                conn.agent_closed = true;
            }
            shared.touch();
        });
        Ok(Response::new(ReceiverStream::new(response)))
    }
}
