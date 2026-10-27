//! The platform's cluster rig (plan 18, T3; shared with T7): a real Postgres with TLS and the migrations, two CAs over software KMS
//! keys, a Git remote, and replicas built by the hub's own composition code (`Platform::build`) with software only at the edges
//! (Secret Manager, KMS, the verifier of Google ID tokens).
//!
//! `platform_cluster.rs` proves the composition with it; `seed.rs` proves that an agent or a sentinel whose identity `hub seed`
//! registered can join through the same listener. A test crate that includes this file declares the modules it uses, with these
//! names: `support`, `git_remote`, `platform_support`, `secrets` and `tls_pg`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use domain::{NfsPath, RequestId, ShortText, SwimlaneId};
use hub::config::{CaSettings, GitCredentialSource, GitSettings, PlatformSettings};
use hub::edges::{Edges, PublicKeys};
use hub::wiring::{Ingest, Platform, PlatformInputs, prepare_cas};
use hub_platform::ca::profile::PeerName;
use hub_platform::gateway::forward::PeerAddress;
use hub_platform::git::mirror::RemoteUrl;
use hub_platform::git::payload::RepoFullName;
use hub_platform::jobs::JobRunnerHandle;
use ports::fakes::{FakeReportSink, FakeSentinelSink, FakeTokenVerifier};
use ports::{GoogleIdentity, KeyRef, KmsError, KmsSigner, Signature};
use proto::pb;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::git_remote::fake_github::GitRemote;
use crate::platform_support::ca::{AGENT_CA_KEY, REPLICA_CA_KEY, TestCa};
use crate::platform_support::clock::ManualClock;
use crate::platform_support::soft_kms::SoftKmsSigner;
use crate::secrets::MemorySecrets;
use crate::{support, tls_pg};

pub const AUDIENCE: &str = "https://hub.lanekeeper.test";
pub const SERVICE_ACCOUNT: &str = "agent-sit1@project.iam.gserviceaccount.com";
pub const WEBHOOK_SECRET: &str = "whsec_PlatformClusterSecret";
pub const BASE_REPO: &str = "acme/configuration-base-saas";
pub const TENANT_REPO: &str = "acme/csp-tenant-data";

/// Signs for both CAs: whichever of the two software keys holds the name.
pub struct Kms {
    agent: Arc<SoftKmsSigner>,
    replica: Arc<SoftKmsSigner>,
}

#[async_trait]
impl KmsSigner for Kms {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError> {
        if self.agent.has(&key) {
            self.agent.sign_digest(key, digest).await
        } else {
            self.replica.sign_digest(key, digest).await
        }
    }
}

#[async_trait]
impl PublicKeys for Kms {
    async fn spki_der(&self, key: &KeyRef) -> Result<Vec<u8>, KmsError> {
        self.agent
            .public_key_spki_der(key)
            .or_else(|| self.replica.public_key_spki_der(key))
            .ok_or(KmsError::KeyNotFound)
    }
}

/// One replica: the platform, its job runner, and the signal that closes it.
pub struct Replica {
    pub platform: Platform,
    pub jobs: Option<JobRunnerHandle>,
    pub closing: watch::Sender<bool>,
    pub pool: PgPool,
}

pub struct Cluster {
    pub pg: tls_pg::TlsPg,
    pub agent_ca: TestCa,
    pub replica_ca: TestCa,
    pub verifier: FakeTokenVerifier,
    pub secrets: Arc<MemorySecrets>,
    pub edges: Edges,
    pub git: GitSettings,
    pub remote: GitRemote,
    pub replicas: Vec<Replica>,
    /// The mirrors' volumes and the database's CA live as long as the cluster does.
    #[allow(dead_code)]
    pub volumes: Vec<tempfile::TempDir>,
    #[allow(dead_code)]
    pub pki: support::pki::TestCa,
}

pub fn pem_of(der: &[u8]) -> String {
    pem::encode_config(
        &pem::Pem::new("CERTIFICATE", der.to_vec()),
        pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF),
    )
}

impl Cluster {
    /// A database, two CAs over software keys, a Git remote, and no replica yet.
    pub async fn prepare() -> Option<Self> {
        let pki = support::pki::TestCa::new("lanekeeper-cluster-ca");
        let pg = tls_pg::start(&pki).await?;
        pg.migrate().await;
        let clock = ManualClock::at_t0();
        let (agent_key, replica_key) = (
            KeyRef::parse(AGENT_CA_KEY).unwrap(),
            KeyRef::parse(REPLICA_CA_KEY).unwrap(),
        );
        let (agent_soft, replica_soft) = (
            Arc::new(SoftKmsSigner::new(&[&agent_key])),
            Arc::new(SoftKmsSigner::new(&[&replica_key])),
        );
        let kms = Arc::new(Kms {
            agent: Arc::clone(&agent_soft),
            replica: Arc::clone(&replica_soft),
        });
        let signer: Arc<dyn KmsSigner> = kms.clone();
        let agent_ca = TestCa::over_signer(agent_soft, Arc::clone(&signer), agent_key, clock.clone()).await;
        let replica_ca = TestCa::over_signer(replica_soft, signer, replica_key, clock.clone()).await;

        let secrets = Arc::new(MemorySecrets::default());
        secrets.put("agent-ca-cert", &pem_of(&agent_ca.ca_der));
        secrets.put("replica-ca-cert", &pem_of(&replica_ca.ca_der));
        secrets.put("github-read-token", "planted-git-token");
        secrets.put("github-webhook-secret", WEBHOOK_SECRET);

        let verifier = FakeTokenVerifier::new();
        let edges = Edges::new(
            secrets.clone(),
            clock,
            kms.clone(),
            kms,
            Arc::new(verifier.clone()),
            hub::testing::unreachable_identity(),
        );

        let remote = GitRemote::new();
        remote.commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "base");
        remote.commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "tenant");
        let git = GitSettings {
            base_url: RemoteUrl::local(&remote.url()),
            tenant_url: RemoteUrl::local(&remote.url()),
            base_repo: RepoFullName::parse(BASE_REPO).unwrap(),
            tenant_repo: RepoFullName::parse(TENANT_REPO).unwrap(),
            webhook_secret: "github-webhook-secret".to_owned(),
            credential: GitCredentialSource::SecretManager("github-read-token".to_owned()),
            fetch_timeout: Duration::from_secs(60),
            queue_capacity: 16,
            tree_index_entries: 10_000,
        };
        Some(Self {
            pg,
            agent_ca,
            replica_ca,
            verifier,
            secrets,
            edges,
            git,
            remote,
            replicas: Vec::new(),
            volumes: Vec::new(),
            pki,
        })
    }

    pub async fn start(replicas: usize) -> Option<Self> {
        let mut cluster = Self::prepare().await?;
        for i in 0..replicas {
            cluster.add_replica(i).await;
        }
        Some(cluster)
    }

    /// The settings of replica `i` over listeners that are already bound, so that the address it advertises is its real one.
    pub fn settings(i: usize, agent: &TcpListener, replica: &TcpListener) -> PlatformSettings {
        PlatformSettings {
            agent_addr: agent.local_addr().unwrap(),
            replica_addr: replica.local_addr().unwrap(),
            agent_ca: CaSettings {
                kms_ref: KeyRef::parse(AGENT_CA_KEY).unwrap(),
                cert_ref: "agent-ca-cert".to_owned(),
            },
            replica_ca: CaSettings {
                kms_ref: KeyRef::parse(REPLICA_CA_KEY).unwrap(),
                cert_ref: "replica-ca-cert".to_owned(),
            },
            replica_name: PeerName::parse(&format!("hub-{i}")).unwrap(),
            replica_address: PeerAddress::parse(&format!(
                "localhost:{}",
                replica.local_addr().unwrap().port()
            ))
            .unwrap(),
            audience: ShortText::parse(AUDIENCE).unwrap(),
            // The name `platform_support::grpc::dial` checks the server's certificate for.
            agent_hosts: vec!["hub.lanekeeper.test".to_owned()],
        }
    }

    pub async fn add_replica(&mut self, i: usize) {
        let agent_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let replica_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let settings = Self::settings(i, &agent_listener, &replica_listener);
        let cas = prepare_cas(&settings, &self.edges, Duration::from_secs(30))
            .await
            .unwrap();
        let volume = tempfile::tempdir().unwrap();
        let pool = self.pg.app_pool().await;
        let (closing, closing_rx) = watch::channel(false);
        let mut platform = Platform::build(PlatformInputs {
            settings,
            git: self.git.clone(),
            mirror_dir: volume.path().to_path_buf(),
            blob_cache_bytes: 16 * 1024 * 1024,
            pool: pool.clone(),
            edges: self.edges.clone(),
            cas,
            closing: closing_rx,
            ingest: Ingest {
                agents: Arc::new(FakeReportSink::new()),
                sentinels: Arc::new(FakeSentinelSink::new()),
            },
            agent_listener,
            replica_listener,
            drain: Duration::from_secs(5),
        })
        .await
        .unwrap();
        let jobs = platform.start_jobs();
        self.replicas.push(Replica {
            platform,
            jobs,
            closing,
            pool,
        });
        self.volumes.push(volume);
    }

    pub fn agent_addr(&self, i: usize) -> std::net::SocketAddr {
        self.replicas[i].platform.agent_addr
    }

    pub async fn seed_swimlane(&self, id: &str, service_account: Option<&str>) {
        sqlx::query(
            "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1') ON CONFLICT DO NOTHING",
        )
        .execute(&self.replicas[0].pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO swimlanes (id, project_id, display_name, agent_service_account) VALUES ($1, 'p1', $1, $2)",
        )
        .bind(id)
        .bind(service_account)
        .execute(&self.replicas[0].pool)
        .await
        .unwrap();
    }

    pub fn accept_google(&self, token: &str, email: &str) {
        self.verifier.accept_google(
            token,
            AUDIENCE,
            GoogleIdentity {
                subject: ShortText::parse("1234567890").unwrap(),
                email: ShortText::parse(email).unwrap(),
                audience: ShortText::parse(AUDIENCE).unwrap(),
            },
        );
    }

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(&self.replicas[0].pool)
            .await
            .unwrap()
    }

    /// Every readiness check of a replica's platform, by name.
    pub async fn readiness(&self, i: usize) -> Vec<(&'static str, bool)> {
        let mut out = Vec::new();
        for (name, check) in &self.replicas[i].platform.checks {
            out.push((*name, check.ready().await));
        }
        out
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        for replica in &self.replicas {
            replica.closing.send_replace(true);
        }
    }
}

pub async fn eventually(what: &str, within: Duration, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + within;
    loop {
        if condition().await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {within:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn swimlane(id: &str) -> SwimlaneId {
    SwimlaneId::parse(id).unwrap()
}

pub fn read(id: &str) -> domain::HubCommand {
    domain::HubCommand::ReadFile {
        request_id: RequestId::parse(id).unwrap(),
        path: NfsPath::parse("app/a.yml").unwrap(),
    }
}

pub fn join_request(swimlane: &str, token: &str, csr: &[u8]) -> pb::JoinRequest {
    pb::JoinRequest {
        swimlane_id: swimlane.to_owned(),
        csr_der: Bytes::copy_from_slice(csr),
        credential: Some(pb::join_request::Credential::GoogleIdToken(token.to_owned())),
        kind: pb::PeerKind::Agent as i32,
    }
}

macro_rules! cluster {
    ($n:expr) => {
        match Cluster::start($n).await {
            Some(cluster) => cluster,
            None => return,
        }
    };
}
