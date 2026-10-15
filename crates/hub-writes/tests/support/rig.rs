//! The write hub wired to a real Postgres with test doubles for the ports it uses: a fake agent with an in-memory NFS, the
//! registry fake, `PgTestAudit`, `PgTestBus`, `PgBlobs` and `PgToolWrites` (all over the real tables), the users fake and a
//! manual clock.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{
    ContentHash, EditRequest, Expected, NfsPath, Role, SwimlaneId, Timestamp, User, UserStatus, WriteCtx,
    WriteOutcome,
};
use hub_writes::{ManualClock, PREVIEW_KEY_SECRET, WriteConfig, WriteHub, WriteHubDeps};
use ports::conformance::sample::{active, files, hash, nfs, swimlane, write_ctx};
use ports::fakes::{FakeLeases, FakeNotifier, FakeRegistry, FakeSecretSource, FakeUsers};
use ports::{AuditEntry, content_hash};
use sqlx::PgPool;

use super::fake_agent::FakeAgent;
use super::live_registry::LiveRegistry;
use super::pg::TestDb;
use super::pg_audit::{PgTestAudit, committed};
use super::pg_blobs::PgBlobs;
use super::pg_bus::PgTestBus;
use super::pg_tool_writes::PgToolWrites;

/// 2026-10-15 00:00:00 UTC.
pub const START: i64 = 1_791_936_000_000;

pub struct Rig {
    pub db: TestDb,
    pub pool: PgPool,
    pub hub: WriteHub,
    pub users: Arc<FakeUsers>,
    pub agent: FakeAgent,
    pub registry: Arc<FakeRegistry>,
    pub blobs: Arc<PgBlobs>,
    pub bus: PgTestBus,
    pub audit: Arc<PgTestAudit>,
    pub notifier: Arc<FakeNotifier>,
    pub leases: Arc<FakeLeases>,
    pub secrets: Arc<FakeSecretSource>,
    pub clock: Arc<ManualClock>,
}

/// The key the rig's preview tokens are signed with.
pub const PREVIEW_KEY: &str = "rig-preview-key-0123456789abcdef-0123456789";

impl Rig {
    pub async fn new(db: TestDb) -> Self {
        Self::with_config(db, WriteConfig::default()).await
    }

    pub async fn with_config(db: TestDb, config: WriteConfig) -> Self {
        let pool = db.app_pool(8).await;
        Self::build(db, pool, config, false)
    }

    /// A rig whose secret source holds `secrets` (the preview key is not there unless the test puts it in).
    pub async fn with_secrets(db: TestDb, secrets: FakeSecretSource) -> Self {
        let pool = db.app_pool(8).await;
        let mut rig = Self::build(db, pool, WriteConfig::default(), false);
        rig.secrets = Arc::new(secrets);
        rig.hub = rig.replica();
        rig
    }

    /// A rig whose registry is never behind NFS (see `LiveRegistry`): the common case in service, and what the benchmarks use.
    pub async fn with_live_registry(db: TestDb, pool_size: u32) -> Self {
        let pool = db.app_pool(pool_size).await;
        Self::build(db, pool, WriteConfig::default(), true)
    }

    fn build(db: TestDb, pool: PgPool, config: WriteConfig, live: bool) -> Self {
        let users = Arc::new(FakeUsers::new());
        let agent = FakeAgent::new();
        let registry = Arc::new(FakeRegistry::new());
        let blobs = Arc::new(PgBlobs::new(pool.clone()));
        let bus = PgTestBus::new(pool.clone());
        let audit = Arc::new(PgTestAudit::new());
        let notifier = Arc::new(FakeNotifier::new());
        let leases = Arc::new(FakeLeases::new());
        let secrets = Arc::new(FakeSecretSource::new().with(PREVIEW_KEY_SECRET, PREVIEW_KEY));
        let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(START)));
        let reader: Arc<dyn ports::RegistryRead> = if live {
            Arc::new(LiveRegistry(agent.clone()))
        } else {
            registry.clone()
        };
        let hub = WriteHub::new(WriteHubDeps {
            pool: pool.clone(),
            users: users.clone(),
            audit: audit.clone(),
            bus: Arc::new(bus.clone()),
            gateway: Arc::new(agent.clone()),
            blobs: blobs.clone(),
            registry: reader,
            tool_writes: Arc::new(PgToolWrites),
            notifier: notifier.clone(),
            secrets: secrets.clone(),
            leases: leases.clone(),
            clock: clock.clone(),
            config,
        })
        .unwrap();
        Self {
            db,
            pool,
            hub,
            users,
            agent,
            registry,
            blobs,
            bus,
            audit,
            notifier,
            leases,
            secrets,
            clock,
        }
    }

    /// A second hub over the same database, agent and doubles: another replica.
    pub fn replica(&self) -> WriteHub {
        self.hub_with_notifier(self.notifier.clone())
    }

    /// A hub over the same database, agent and doubles that tells its admins through `notifier`.
    pub fn hub_with_notifier(&self, notifier: Arc<dyn ports::Notifier>) -> WriteHub {
        WriteHub::new(WriteHubDeps {
            pool: self.pool.clone(),
            users: self.users.clone(),
            audit: self.audit.clone(),
            bus: Arc::new(self.bus.clone()),
            gateway: Arc::new(self.agent.clone()),
            blobs: self.blobs.clone(),
            registry: self.registry.clone(),
            tool_writes: Arc::new(PgToolWrites),
            notifier,
            secrets: self.secrets.clone(),
            leases: self.leases.clone(),
            clock: self.clock.clone(),
            config: WriteConfig::default(),
        })
        .unwrap()
    }

    /// Add a user to the directory and to the `users` table (proposals have a foreign key to it).
    pub async fn add_user(&self, u: &User) {
        assert!(self.users.insert(u.clone()));
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status, requires_approval) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
        )
        .bind(u.id.tid().as_str())
        .bind(u.id.oid().as_str())
        .bind(u.email.as_str())
        .bind(u.display_name.as_str())
        .bind(u.role.map(|r| r.as_str().to_owned()))
        .bind(u.status.as_str())
        .bind(u.requires_approval)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    /// A user of the given role, added to both stores.
    pub async fn user(&self, n: u32, role: Role) -> User {
        let u = active(n, role);
        self.add_user(&u).await;
        u
    }

    pub async fn approval_user(&self, n: u32, role: Role) -> User {
        let mut u = active(n, role);
        u.requires_approval = true;
        self.add_user(&u).await;
        u
    }

    pub async fn user_with_status(&self, n: u32, role: Option<Role>, status: UserStatus) -> User {
        let mut u = active(n, Role::Viewer);
        u.role = role;
        u.status = status;
        self.add_user(&u).await;
        u
    }

    /// Make the swimlane exist; `unlocked` is the baseline confirmation (D2).
    pub async fn swimlane(&self, id: &str, unlocked: bool) -> SwimlaneId {
        sqlx::query("INSERT INTO projects (id, display_name) VALUES ('p', 'P') ON CONFLICT DO NOTHING")
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO swimlanes (id, project_id, display_name, editing_unlocked) VALUES ($1, 'p', $1, $2)",
        )
        .bind(id)
        .bind(unlocked)
        .execute(&self.pool)
        .await
        .unwrap();
        swimlane(id)
    }

    /// The swimlane deploys this tenant (Q34: a swimlane has a set).
    pub async fn tenant(&self, s: &SwimlaneId, tenant: &str) {
        sqlx::query(
            "INSERT INTO swimlane_tenants (swimlane_id, tenant_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(s.as_str())
        .bind(tenant)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    /// The cluster report lists this Deployment; `app` is its `SPRING_APPLICATION_NAME` (D88).
    pub async fn service(&self, s: &SwimlaneId, namespace: &str, name: &str, app: Option<&str>) {
        let env = app.map_or_else(
            || serde_json::json!({}),
            |a| serde_json::json!({ "SPRING_APPLICATION_NAME": a }),
        );
        sqlx::query(
            "INSERT INTO services (swimlane_id, namespace, name, env_values) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(s.as_str())
        .bind(namespace)
        .bind(name)
        .bind(env)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    /// The swimlane's files on the agent's NFS and in the registry.
    pub fn seed(&self, s: &SwimlaneId, items: &[(&str, &str)]) {
        for (p, b) in files(items) {
            self.agent.put(s, &p, &b);
        }
        self.catch_up_registry(s);
    }

    /// The registry sees what is on NFS now (it lags between writes and scans).
    pub fn catch_up_registry(&self, s: &SwimlaneId) {
        assert!(self.registry.seed(s, &self.agent.files_of(s)));
    }

    pub fn nfs_of(&self, s: &SwimlaneId, path: &str) -> Option<Bytes> {
        self.agent.content(s, &nfs(path))
    }

    pub async fn chain(&self) -> Vec<AuditEntry> {
        committed(&self.pool).await
    }

    pub async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// 20,000 pull-request links, a quarter of them merged, each for its own swimlane (`sit0` to `sit39`, which must exist), path
    /// and hash; and 5,000 jobs, every 500th of them unfinished. The rows the P12 benchmark reads, and the tests that run its probes.
    pub async fn seed_pr_rows(&self, user: &domain::UserId) {
        sqlx::query(
            "INSERT INTO pr_links (swimlane_id, path, nfs_hash, repo, branch, pr_number, state, url, author_tid, author_oid) \
             SELECT 'sit' || (g % 40), 'a/' || g || '.yml', sha256(g::text::bytea), 'tenant', 'lanekeeper/x/' || g, g, \
                    CASE WHEN g % 4 = 0 THEN 'merged' ELSE 'open' END, 'https://github.com/acme/r/pull/' || g, $1, $2 \
             FROM generate_series(1, 20000) g",
        )
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .execute(&self.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pr_jobs (author_tid, author_oid, swimlane_id, paths, title, state, step_data, created_at, updated_at) \
             SELECT $1, $2, 'sit' || (g % 40), ARRAY['a/' || g || '.yml'], 't', \
                    CASE WHEN g % 500 = 0 THEN 'committing' ELSE 'completed' END, '{}', \
                    to_timestamp($3::bigint / 1000.0) - interval '1 hour', to_timestamp($3::bigint / 1000.0) - interval '1 hour' \
             FROM generate_series(1, 5000) g",
        )
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .bind(START)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    pub fn advance_secs(&self, secs: i64) {
        self.clock.advance_secs(secs);
    }
}

/// An edit request over text.
pub fn edit(s: &SwimlaneId, path: &str, expected: Expected, content: &str) -> EditRequest {
    EditRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        content: Bytes::copy_from_slice(content.as_bytes()),
        change_eol: None,
    }
}

pub fn at(content: &str) -> Expected {
    Expected::Hash {
        hash: content_hash(content.as_bytes()),
    }
}

pub fn h(content: &str) -> ContentHash {
    hash(content.as_bytes())
}

pub fn ctx(u: &User, key: &str) -> WriteCtx {
    write_ctx(u, key)
}

pub fn applied(o: &WriteOutcome) -> (Option<ContentHash>, domain::AuditId) {
    match o {
        WriteOutcome::Applied { hash, audit_id, .. } => (*hash, *audit_id),
        other => panic!("expected Applied, got {other:?}"),
    }
}

/// The short real-time pause the tests allow the concurrency waits.
pub const SHORT_WAIT: Duration = Duration::from_millis(250);

pub fn nfs_path(p: &str) -> NfsPath {
    nfs(p)
}
