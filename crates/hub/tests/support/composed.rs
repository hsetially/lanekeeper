//! The composed hub as a process, with the people and machines that talk to it (plan 18, T5): the tests of the registry and the write
//! paths need what the process tests do (`hub_dev` against a real Postgres, started the way the binary starts) and four things more.
//!
//! - **A browser.** Users and sessions are made the way sign-in makes them (rows, and the two cookies), in the test process, over the
//!   database the hub reads. The hub finds the session in the table on the first request, as it does for one made by another replica.
//!   [`Composed::web`] sends a request over TLS 1.3 with the cookies, the double-submit token and the `Origin` of a page.
//! - **An agent.** [`Composed::connect_agent`] joins through the real agent listener with a Google ID token (the example accepts the
//!   tokens `LK_DEV_GOOGLE_TOKENS` names), gets its certificate from the hub's agent CA, opens the `Connect` stream with it, and
//!   answers every command with success, as the platform's fake agent does.
//! - **GitHub.** The real `GithubClient` talks to a small HTTP server in the test process (`LocalGithub`): `GET /user` and the two
//!   repositories' permissions, for a fine-grained token that belongs to `octocat`.
//! - **More than one replica** on the one database, each with its own mirror volume and secrets directory (the example makes its CA
//!   keys at start, so replicas that shared a directory would overwrite each other's CA certificates).
//!
//! Nothing here is in `src/`: `tests/release_build.rs` keeps the software out of the binary.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use domain::{ContentHash, Guid, Role, Timestamp, UserId, UserStatus};
use hub::edges::IdentityClock;
use hub_identity::session::{SessionPolicy, Sessions};
use hub_platform::clock::SystemClock;
use hub_platform::tls::{ClientTls, roots_from_der};
use ports::content_hash;
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;

use crate::child::{Child, Listening};
use crate::fake_github::{self, LocalGithub, Reply as GithubReply, Seen};
use crate::net;
use crate::platform_support::fake_agent::answer;
use crate::platform_support::grpc::{ClientIdentity, agent_client, dial_named};
use crate::rig::Rig;

/// The swimlane the tests register, the service account its agent runs as, and the Google ID token the example accepts for it.
pub const SWIMLANE: &str = "sit1";
pub const SERVICE_ACCOUNT: &str = "agent-sit1@project.iam.gserviceaccount.com";
pub const GOOGLE_TOKEN: &str = "gid-sit1";
/// A fine-grained token of the shape the vault accepts, and the GitHub account the fake says it belongs to.
pub const GITHUB_TOKEN: &str = "github_pat_11T5COMPOSED0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHIJKLMN";
pub const GITHUB_LOGIN: &str = "octocat";
/// The secret that signs the webhook deliveries (the file `github-webhook-secret` of the hub's secrets).
pub const WEBHOOK_SECRET: &str = "whsec_T5ComposedWebhookSecret";
pub const BASE_REPO: &str = "acme/configuration-base-saas";
pub const TENANT_REPO: &str = "acme/csp-tenant-data";
/// The tenant id of the rig's Entra tenant, which the users of the tests belong to.
pub const TENANT: &str = "11111111-1111-4111-8111-111111111111";
/// The `Origin` of a page: the hub's `LK_PUBLIC_ORIGIN`.
pub const ORIGIN: &str = "https://localhost";

/// What the fake GitHub says: the token is `octocat`'s and can push to both repositories; every other token is a 401.
pub fn github_handler() -> fake_github::Handler {
    Arc::new(|seen: &Seen, _| {
        let bearer = seen
            .header("authorization")
            .unwrap_or_default()
            .trim_start_matches("Bearer ")
            .to_owned();
        if bearer != GITHUB_TOKEN {
            return GithubReply::json(401, r#"{"message":"Bad credentials"}"#);
        }
        if seen.path == "/user" {
            GithubReply::json(200, &format!(r#"{{"login":"{GITHUB_LOGIN}"}}"#)).with_header(
                "github-authentication-token-expiration",
                "2030-01-01 00:00:00 UTC",
            )
        } else if seen.path.starts_with("/repos/") {
            GithubReply::json(200, r#"{"permissions":{"push":true}}"#)
        } else {
            GithubReply::json(404, r#"{"message":"Not Found"}"#)
        }
    })
}

/// One running hub process.
pub struct Hub {
    pub child: Child,
    pub addrs: Listening,
    /// The hub's secrets directory: it writes its CA certificates there at start.
    pub secrets: PathBuf,
    _own_secrets: Option<tempfile::TempDir>,
    _own_mirrors: Option<tempfile::TempDir>,
}

impl Hub {
    pub fn web(&self) -> SocketAddr {
        self.addrs.web
    }

    pub fn ops(&self) -> SocketAddr {
        self.addrs.ops
    }
}

/// A signed-in browser's two cookies.
#[derive(Debug, Clone)]
pub struct Cookies {
    pub session: String,
    pub csrf: String,
}

/// The rig, a fake GitHub, the database as the app role, and the hubs started on them.
pub struct Composed {
    pub rig: Rig,
    pub github: LocalGithub,
    pub pool: PgPool,
    pub hubs: Vec<Hub>,
    sessions: Arc<Sessions>,
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

impl Composed {
    /// A migrated database, a Git remote, a fake GitHub, the swimlane `sit1` registered with its service account, and `replicas` hubs
    /// that say they are ready. `None` when Docker is not there and not required.
    pub async fn start(replicas: usize, extra: &[(&str, &str)]) -> Option<Self> {
        let rig = Rig::start().await?;
        std::fs::write(rig.secrets.path().join("github-webhook-secret"), WEBHOOK_SECRET).unwrap();
        let github = LocalGithub::start(github_handler()).await;
        let pool = rig.pg.app_pool().await;
        let sessions = Arc::new(Sessions::new(
            pool.clone(),
            Arc::new(IdentityClock(Arc::new(SystemClock))),
            &SessionPolicy::default(),
        ));
        let mut composed = Self {
            rig,
            github,
            pool,
            hubs: Vec::new(),
            sessions,
        };
        composed.register_swimlane(SWIMLANE, SERVICE_ACCOUNT).await;
        for n in 0..replicas {
            composed.add_hub(n, extra).await;
        }
        Some(composed)
    }

    /// What an administrator would have registered: the project, the swimlane with the service account its agent runs as, and the
    /// tenant it deploys. (`hub seed`, T7, does this from a file.)
    pub async fn register_swimlane(&self, id: &str, service_account: &str) {
        sqlx::query(
            "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1') ON CONFLICT DO NOTHING",
        )
        .execute(&self.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO swimlanes (id, project_id, display_name, agent_service_account) VALUES ($1, 'p1', $1, $2)",
        )
        .bind(id)
        .bind(service_account)
        .execute(&self.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO swimlane_tenants (swimlane_id, tenant_id) VALUES ($1, $1)")
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    /// Start replica `n` (`hub-{n}`) on the same database. The first uses the rig's directories; the others have their own.
    pub async fn add_hub(&mut self, n: usize, extra: &[(&str, &str)]) {
        let name = format!("hub-{n}");
        let github = self.github.base();
        let tokens = format!("{GOOGLE_TOKEN}={SERVICE_ACCOUNT}");
        let (secrets, own_secrets, mirrors, own_mirrors) = if n == 0 {
            (
                self.rig.secrets.path().to_path_buf(),
                None,
                self.rig.mirrors.path().to_path_buf(),
                None,
            )
        } else {
            let secrets = tempfile::tempdir().unwrap();
            copy_dir(self.rig.secrets.path(), secrets.path());
            let mirrors = tempfile::tempdir().unwrap();
            (
                secrets.path().to_path_buf(),
                Some(secrets),
                mirrors.path().to_path_buf(),
                Some(mirrors),
            )
        };
        let (secrets_dir, mirrors_dir) = (secrets.display().to_string(), mirrors.display().to_string());
        let mut env: Vec<(&str, &str)> = vec![
            ("LK_REPLICA_NAME", &name),
            ("LK_DEV_GITHUB_API", &github),
            ("LK_DEV_GOOGLE_TOKENS", &tokens),
            ("LK_DEV_SECRETS_DIR", &secrets_dir),
            ("LK_MIRROR_DIR", &mirrors_dir),
        ];
        env.extend_from_slice(extra);
        let child = self.rig.spawn(&env).await;
        let addrs = child.wait_listening_all(Duration::from_secs(60));
        eventually(Duration::from_secs(60), "the hub to be ready", || async {
            net::http_get(addrs.ops, "/readyz").await.status == 200
        })
        .await;
        self.hubs.push(Hub {
            child,
            addrs,
            secrets,
            _own_secrets: own_secrets,
            _own_mirrors: own_mirrors,
        });
    }

    // ------------------------------------------------------------------------------------------------------ the browser

    /// A user the way sign-in would have made one, and a session for them.
    pub async fn signed_in(&self, n: u32, role: Option<Role>, status: UserStatus) -> (UserId, Cookies) {
        let id = UserId::new(
            Guid::parse(TENANT).unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        );
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT DO NOTHING",
        )
        .bind(id.tid().as_str())
        .bind(id.oid().as_str())
        .bind(format!("user{n}@example.com"))
        .bind(format!("User {n}"))
        .bind(role.map(Role::as_str))
        .bind(status.as_str())
        .execute(&self.pool)
        .await
        .unwrap();
        let mut tx = self.pool.begin().await.unwrap();
        let made = self.sessions.store().create_in(&mut tx, &id).await.unwrap();
        tx.commit().await.unwrap();
        (
            id,
            Cookies {
                session: made.session.expose().clone(),
                csrf: made.csrf.expose().clone(),
            },
        )
    }

    /// A request to replica `hub` as a page makes it: the cookies, the token and the `Origin`. `body` is JSON.
    pub async fn web(
        &self,
        hub: usize,
        method: &str,
        path: &str,
        as_user: Option<&Cookies>,
        headers: &[(&str, &str)],
        body: Option<&serde_json::Value>,
    ) -> net::Reply {
        let cookie;
        let mut all: Vec<(&str, &str)> = Vec::new();
        if let Some(cookies) = as_user {
            cookie = format!(
                "__Host-lk_session={}; __Host-lk_csrf={}",
                cookies.session, cookies.csrf
            );
            all.push(("Cookie", &cookie));
            if method != "GET" {
                all.push(("X-CSRF-Token", &cookies.csrf));
                all.push(("Origin", ORIGIN));
            }
        }
        if body.is_some() {
            all.push(("Content-Type", "application/json"));
        }
        all.extend_from_slice(headers);
        let text = body.map(ToString::to_string);
        net::https_request(
            self.hubs[hub].web(),
            &self.rig.ca.cert_der(),
            method,
            path,
            &all,
            text.as_deref().map(str::as_bytes),
        )
        .await
    }

    pub async fn get(&self, as_user: &Cookies, path: &str) -> net::Reply {
        self.web(0, "GET", path, Some(as_user), &[], None).await
    }

    // ------------------------------------------------------------------------------------------------------- the agent

    /// An agent of `swimlane`: joins through replica `hub`'s agent listener with `token`, then opens the `Connect` stream and says
    /// hello. Panics with the status if the hub refuses.
    pub async fn connect_agent(&self, hub: usize, swimlane: &str, token: &str) -> AgentLink {
        AgentLink::connect(&self.hubs[hub], swimlane, token).await
    }

    // ------------------------------------------------------------------------------------------------------ the database

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.pool).await.unwrap()
    }

    /// The actions of the audit events after `seq`, oldest first.
    pub async fn audit_after(&self, seq: i64) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_events WHERE seq > $1 ORDER BY seq")
            .bind(seq)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    /// The newest audit sequence number, to count the events a step adds.
    pub async fn audit_mark(&self) -> i64 {
        self.count("SELECT COALESCE(max(seq), 0) FROM audit_events").await
    }
}

/// Poll `check` every 50 ms until it is true, for at most `within`. Returns how long it took.
pub async fn eventually<F, Fut>(within: Duration, what: &str, mut check: F) -> Duration
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let started = Instant::now();
    loop {
        if check().await {
            return started.elapsed();
        }
        assert!(
            started.elapsed() < within,
            "{what} did not happen within {within:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ----------------------------------------------------------------------------------------------------------- the agent

/// An open `Connect` stream of an agent that joined through the real listener.
pub struct AgentLink {
    /// The files this agent has on its NFS: what it reported, and what it has written since.
    files: Arc<Mutex<HashMap<String, ContentHash>>>,
    commands: Arc<Mutex<Vec<domain::HubCommand>>>,
    ended: Arc<AtomicBool>,
    sender: mpsc::Sender<pb::AgentMessage>,
    task: JoinHandle<()>,
    /// The certificate chain the hub issued for it, for the tests that look at what the agent was given.
    pub chain_der: Vec<Vec<u8>>,
}

/// A connection to the hub's agent listener, naming it as its certificate does (`LK_AGENT_ENDPOINT_HOSTS` is `localhost` in the rig).
async fn dial_agent(hub: &Hub, tls: &ClientTls) -> crate::platform_support::grpc::H2 {
    dial_named(hub.addrs.agent, tls.connector(), "localhost")
        .await
        .unwrap()
        .0
}

impl AgentLink {
    async fn connect(hub: &Hub, swimlane: &str, token: &str) -> Self {
        let pem_text = std::fs::read_to_string(hub.secrets.join("agent-ca-cert")).unwrap();
        let ca_der = pem::parse(pem_text).unwrap().into_contents();
        let roots = roots_from_der(&[ca_der]).unwrap();
        let clock: Arc<dyn hub_platform::clock::Clock> = Arc::new(SystemClock);

        // `Join` needs no client certificate: the agent has none yet.
        let anonymous = ClientTls::new(roots.clone(), None, Arc::clone(&clock)).unwrap();
        let mut joining = agent_client(dial_agent(hub, &anonymous).await);
        let key = rcgen::KeyPair::generate().unwrap();
        let csr = crate::platform_support::ca::csr_for(&key);
        let joined = joining
            .join(Request::new(pb::JoinRequest {
                swimlane_id: swimlane.to_owned(),
                csr_der: Bytes::copy_from_slice(&csr),
                credential: Some(pb::join_request::Credential::GoogleIdToken(token.to_owned())),
                kind: pb::PeerKind::Agent as i32,
            }))
            .await
            .unwrap_or_else(|status| panic!("the hub refused the join: {status}"))
            .into_inner();
        let identity = ClientIdentity {
            key,
            chain_der: joined.cert_chain_der.iter().map(|c| c.to_vec()).collect(),
        };

        // The stream, with the certificate the hub issued.
        let tls = ClientTls::new(roots, Some(identity.certified()), clock).unwrap();
        let mut client = agent_client(dial_agent(hub, &tls).await);
        let (tx, rx) = mpsc::channel::<pb::AgentMessage>(256);
        tx.send(pb::AgentMessage {
            kind: Some(pb::agent_message::Kind::Hello(pb::Hello {
                agent_version: "0.1.0".to_owned(),
                swimlane: swimlane.to_owned(),
                cluster: "cluster-a".to_owned(),
                project: "proj".to_owned(),
                nfs_server: "nfs.internal".to_owned(),
                export: "/export".to_owned(),
                mount_root: "/mnt/config".to_owned(),
            })),
        })
        .await
        .unwrap();
        let mut stream = client
            .connect(Request::new(ReceiverStream::new(rx)))
            .await
            .unwrap_or_else(|status| panic!("the hub refused the stream: {status}"))
            .into_inner();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let files: Arc<Mutex<HashMap<String, ContentHash>>> = Arc::default();
        let ended = Arc::new(AtomicBool::new(false));
        let task = {
            let (commands, files, ended, tx) = (
                Arc::clone(&commands),
                Arc::clone(&files),
                Arc::clone(&ended),
                tx.clone(),
            );
            tokio::spawn(async move {
                while let Ok(Some(message)) = stream.message().await {
                    let Ok(Some(ToAgent::Command(command))) = ToAgent::from_proto(message) else {
                        continue;
                    };
                    commands
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(command.clone());
                    let reply = match &command {
                        domain::HubCommand::WriteFile {
                            request_id,
                            path,
                            expected,
                            bytes,
                        } => Some(write_file(&files, request_id, path, expected, bytes)),
                        other => answer(other),
                    };
                    if let Some(reply) = reply {
                        let _ = tx.send(FromAgent::Reply(reply).into_proto()).await;
                    }
                }
                ended.store(true, Ordering::SeqCst);
            })
        };
        Self {
            files,
            commands,
            ended,
            sender: tx,
            task,
            chain_der: identity.chain_der,
        }
    }

    /// The commands the hub has sent on this stream, oldest first.
    pub fn received(&self) -> Vec<domain::HubCommand> {
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The hash of `path` on this agent's NFS now.
    pub fn file(&self, path: &str) -> Option<ContentHash> {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(path)
            .copied()
    }

    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// Send one scan delta, as the agent does after its walk.
    pub async fn send_delta(&self, delta: domain::ScanDelta) {
        {
            let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            for entry in &delta.entries {
                files.insert(entry.path.as_str().to_owned(), entry.hash);
            }
        }
        self.sender
            .send(FromAgent::Delta(delta).into_proto())
            .await
            .unwrap();
    }
}

impl Drop for AgentLink {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What a real agent does with `WriteFile`: compare what the hub expects with the file, refuse on a mismatch and say what is
/// there (`Conflict`), and otherwise write it and say what is there now. (The hub sends the hash it expects; it is the agent that
/// refuses, rule 11.)
fn write_file(
    files: &Mutex<HashMap<String, ContentHash>>,
    request_id: &domain::RequestId,
    path: &domain::NfsPath,
    expected: &domain::Expected,
    bytes: &Bytes,
) -> domain::AgentReply {
    let mut files = files.lock().unwrap_or_else(PoisonError::into_inner);
    let current = files.get(path.as_str()).copied();
    let result = if expected.matches(current.as_ref()) {
        let written = content_hash(bytes);
        files.insert(path.as_str().to_owned(), written);
        domain::OpResult {
            request_id: request_id.clone(),
            ok: true,
            error: None,
            current_hash: Some(written),
        }
    } else {
        domain::OpResult {
            request_id: request_id.clone(),
            ok: false,
            error: Some(domain::OpError::Conflict),
            current_hash: current,
        }
    };
    domain::AgentReply::Op(result)
}

/// A full scan holding `files`, observed `age_ms` ago, as the first scan of an agent is.
pub fn full_scan(seq: u64, files: &[(&str, &[u8])], age_ms: i64) -> domain::ScanDelta {
    let now = now_ms();
    domain::ScanDelta {
        seq,
        base_root: None,
        new_root: content_hash(format!("root-{seq}").as_bytes()),
        entries: files
            .iter()
            .map(|(path, content)| entry(path, content, now - age_ms))
            .collect(),
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more: false,
        part: 0,
        gap: None,
    }
}

/// One file as the agent reports it.
pub fn entry(path: &str, content: &[u8], observed_at_ms: i64) -> domain::ScanEntry {
    domain::ScanEntry {
        path: domain::NfsPath::parse(path).unwrap(),
        hash: content_hash(content),
        size: content.len() as u64,
        mtime: Timestamp::from_unix_millis(observed_at_ms - 1_000),
        observed_at: Timestamp::from_unix_millis(observed_at_ms),
        denied: false,
        bytes: Some(Bytes::copy_from_slice(content)),
    }
}

/// The wall clock's now, in Unix milliseconds.
fn now_ms() -> i64 {
    use hub_platform::clock::Clock;
    SystemClock.now().unix_millis()
}

/// The hash of a text, for the assertions that compare with what a read returns.
pub fn hash_of(content: &[u8]) -> ContentHash {
    content_hash(content)
}
