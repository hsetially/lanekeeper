//! The pieces most transport tests need, wired together: a fake hub on a fake network, the agent's gRPC transport
//! pointed at it, and a way to get a real certificate through a real `Join`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use agent::clock::Clock;
use agent::config::{BaseUrl, Tunables};
use agent::identity::joiner::{IdentityHandle, JoinClient};
use agent::identity::{ClientIdentity, KeyMaterial};
use agent::transport::session::{Link, LinkHandler};
use agent::transport::tls::HubRoots;
use agent::transport::wire;
use agent::transport::{Connection, GrpcTransport, HubTransport, TransportError};
use async_trait::async_trait;
use domain::{Heartbeat, Secret, ShortText, SwimlaneId};
use proto::convert::{FromAgent, JoinCredential, JoinParams, JoinSubject, ToAgent};

use super::clock::TestClock;
use super::fake_hub::{HUB_URL, HubServer};
use super::test_ca::T0_MS;
use super::tls::HubVersions;

pub const WI_TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiJodHRwczovL2h1YiJ9.d2ktc2lnbmF0dXJl";

pub fn swimlane() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

pub fn hello() -> domain::Hello {
    let text = |s: &str| ShortText::parse(s).unwrap();
    domain::Hello {
        agent_version: text("0.0.0-test"),
        swimlane: swimlane(),
        cluster: text("gke-sit1"),
        project: text("bank-sit"),
        nfs_server: text("10.1.2.3"),
        export: text("/export/csp"),
        mount_root: text("/mnt/csp"),
    }
}

/// The `Hello` as the wire message the transport sends first.
pub fn hello_message() -> proto::pb::AgentMessage {
    wire::encode(FromAgent::Hello(hello()))
}

pub struct Rig {
    pub clock: Arc<TestClock>,
    pub server: Arc<HubServer>,
    pub transport: Arc<GrpcTransport>,
}

impl Rig {
    pub fn new() -> Self {
        Self::with(HubVersions::Tls13Only)
    }

    /// A rig whose wall clock is the real one when it is made and then follows real time, for tests and benchmarks in real
    /// time on a real directory: the quiet period compares a file's time from the kernel with the agent's clock, and a
    /// clock that starts at a made-up date would make every file look years old.
    pub fn real_time() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(T0_MS, |d| i64::try_from(d.as_millis()).unwrap_or(T0_MS));
        Self::starting_at(HubVersions::Tls13Only, now)
    }

    pub fn with(versions: HubVersions) -> Self {
        Self::starting_at(versions, T0_MS)
    }

    fn starting_at(versions: HubVersions, wall_ms: i64) -> Self {
        let clock = Arc::new(TestClock::starting_at(wall_ms));
        let server = HubServer::start_with(clock.clone(), versions);
        let roots = HubRoots::from_pem(server.ca_pem().as_bytes()).unwrap();
        let transport = Arc::new(
            GrpcTransport::new(
                &BaseUrl::parse(HUB_URL, "https").unwrap(),
                roots,
                server.net.dialer(),
            )
            .unwrap(),
        );
        Self {
            clock,
            server,
            transport,
        }
    }

    /// Join through the real `Join` call and check the certificate the way the joiner does.
    pub async fn join(&self) -> ClientIdentity {
        let key = KeyMaterial::generate().unwrap();
        let params = JoinParams {
            subject: JoinSubject::Agent(swimlane()),
            csr_der: key.csr_der().unwrap(),
            credential: JoinCredential::GoogleIdToken(Secret::new(WI_TOKEN.to_owned())),
        };
        let issued = JoinClient::join(&*self.transport, params).await.unwrap();
        ClientIdentity::verify(key, issued.cert_chain_der, &swimlane(), self.clock.now()).unwrap()
    }

    /// An identity handle holding a freshly joined certificate.
    pub async fn identity_handle(&self) -> Arc<IdentityHandle> {
        Arc::new(IdentityHandle::new(self.join().await))
    }

    pub async fn connect(&self, identity: &ClientIdentity) -> Result<Connection, TransportError> {
        self.transport.connect(identity, hello_message()).await
    }
}

impl Default for Rig {
    fn default() -> Self {
        Self::new()
    }
}

/// What a [`Recorder`] saw on one link.
#[derive(Debug, Clone)]
pub struct LinkLog {
    pub tunables: Tunables,
    pub received: Vec<ToAgent>,
}

/// A link handler that records what the hub sent, and sends one heartbeat per link so the hub sees application
/// traffic after the handshake.
#[derive(Debug, Default)]
pub struct Recorder {
    links: Mutex<Vec<LinkLog>>,
    outboxes: Mutex<Vec<agent::transport::Outbox>>,
    /// Keep only the latest outbox, and count the times a link started while the previous one was still open.
    lean: bool,
    stale_outboxes: std::sync::atomic::AtomicUsize,
    /// How many links have started, whether or not their logs are kept.
    started: std::sync::atomic::AtomicUsize,
}

impl Recorder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// For long runs that measure memory: holds on to the latest outbox only, so that the recorder itself does not grow
    /// by a queue per connection.
    pub fn lean() -> Arc<Self> {
        Arc::new(Self {
            lean: true,
            ..Self::default()
        })
    }

    /// How many links have started so far.
    pub fn links_started(&self) -> usize {
        self.started.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many times a new link began while the previous link's outbox was not closed yet. Only counted when lean.
    pub fn stale_outboxes(&self) -> usize {
        self.stale_outboxes.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn links(&self) -> Vec<LinkLog> {
        self.links.lock().unwrap().clone()
    }

    /// The outbox of each link so far, to check what happens to it when the link ends.
    pub fn outboxes(&self) -> Vec<agent::transport::Outbox> {
        self.outboxes.lock().unwrap().clone()
    }
}

#[async_trait]
impl LinkHandler for Recorder {
    async fn handle(&self, mut link: Link) {
        let n = {
            let mut links = self.links.lock().unwrap();
            if self.lean {
                links.clear();
            }
            links.push(LinkLog {
                tunables: link.tunables.clone(),
                received: Vec::new(),
            });
            self.started.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
        };
        {
            let mut outboxes = self.outboxes.lock().unwrap();
            if self.lean {
                if outboxes.first().is_some_and(|previous| !previous.is_closed()) {
                    self.stale_outboxes
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                outboxes.clear();
            }
            outboxes.push(link.outbox.clone());
        }
        let heartbeat = FromAgent::Heartbeat(Heartbeat {
            scan_seq: u64::try_from(n).unwrap(),
            merkle_root: domain::ContentHash::from_bytes([u8::try_from(n % 250).unwrap(); 32]),
            file_count: 0,
        });
        let _ = link.outbox.send(heartbeat).await;
        while let Some(message) = link.inbound.recv().await {
            let mut links = self.links.lock().unwrap();
            let last = links.len() - 1;
            links[last].received.push(message);
        }
    }
}
