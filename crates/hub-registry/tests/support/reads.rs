//! The read service over a real Postgres, wired the way the hub wires it: the pool of `lanekeeper_app`, the blob store over the
//! `blobs` table, the in-memory Git, a scripted agent gateway, the clock of the rig. Files get in through ingestion, as an agent
//! sends them, so what a read sees is what the registry stored.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    BaselineState, BaselineStatus, CompareQuery, CompareRef, Comparison, ContentSource, DriftQuery,
    EffectiveConfig, EffectiveQuery, FileContent, FileEntry, Finding, FindingsQuery, Grid, GridQuery,
    LogicalFile, NfsPath, Page, Paged, Role, ServeRequest, ServedFile, ServiceState, SettingHit,
    SettingsQuery, ShortText, SwimlaneId, SwimlaneSummary, SyncingInfo, TenantId, TenantSet, TextQuery,
    TextResults, TreeComparison, User, Version,
};
use hub_registry::config::RegistryConfig;
use hub_registry::{ReadDeps, RegistryService};
use ports::fakes::FakeAgentGateway;
use ports::{ReadError, RegistryRead};

use super::world::World;

pub use ports::conformance::sample::active;

pub struct Reads {
    pub world: World,
    pub service: RegistryService,
    pub gateway: FakeAgentGateway,
    pub config: RegistryConfig,
}

impl Reads {
    /// A swimlane `sit1` that deploys `tenants`, and the read service over its database.
    pub async fn new(tenants: &[&str]) -> Option<Self> {
        Self::with(tenants, RegistryConfig::default()).await
    }

    pub async fn with(tenants: &[&str], config: RegistryConfig) -> Option<Self> {
        let world = World::with_config(tenants, config.clone()).await?;
        let gateway = FakeAgentGateway::default();
        let service = RegistryService::new(ReadDeps {
            pool: world.rig.pool.clone(),
            blobs: Arc::clone(&world.rig.blobs) as _,
            git: Arc::new(world.git.clone()),
            gateway: Arc::new(gateway.clone()),
            clock: Arc::clone(&world.rig.clock) as _,
            config: config.clone(),
            metrics: world.rig.metrics.clone(),
        })
        .unwrap();
        Some(Self {
            world,
            service,
            gateway,
            config,
        })
    }

    /// Another read service over the same database, blobs and Git with another configuration: two strategies, one set of data.
    pub fn service_with(&self, config: RegistryConfig) -> RegistryService {
        RegistryService::new(ReadDeps {
            pool: self.world.rig.pool.clone(),
            blobs: Arc::clone(&self.world.rig.blobs) as _,
            git: Arc::new(self.world.git.clone()),
            gateway: Arc::new(self.gateway.clone()),
            clock: Arc::clone(&self.world.rig.clock) as _,
            config,
            metrics: hub_registry::RegistryMetrics::default(),
        })
        .unwrap()
    }

    pub fn viewer() -> User {
        active(1, Role::Viewer)
    }

    /// Run a statement that arranges what a read will see (a derived column, a row of a table another component writes). Reads do not
    /// care how a state got there, so the tests of reads set it directly.
    pub async fn sql(&self, statement: &str) {
        sqlx::raw_sql(statement)
            .execute(&self.world.rig.pool)
            .await
            .unwrap();
    }

    /// Bring the settings index up to date for every structured blob the swimlanes have, as the sweeper does.
    pub async fn index_all(&self) {
        let (indexer, _worker) = self
            .world
            .rig
            .another_indexer(self.config.clone(), hub_registry::RegistryMetrics::default());
        hub_registry::SettingsSweeper::new(indexer)
            .sweep(|| Ok(()))
            .await
            .unwrap();
    }

    pub fn lane(&self) -> SwimlaneId {
        self.world.lane()
    }
}

pub fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

pub fn logical(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

pub fn lane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

pub fn page(limit: u32) -> Page {
    Page::new(limit, None).unwrap()
}

/// The names of a page of entries, with `/` after a folder.
pub fn names(p: &Paged<FileEntry>) -> Vec<String> {
    p.items
        .iter()
        .map(|e| format!("{}{}", e.path, if e.is_dir { "/" } else { "" }))
        .collect()
}

/// `Reads` as the scenario of the port's conformance suite: `seed` makes a swimlane exist with exactly these files.
pub struct Scenario(pub Reads);

#[async_trait]
impl ports::conformance::RegistryScenario for Scenario {
    async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) {
        use ports::ReportSink;
        let reads = &self.0;
        reads.world.rig.add_swimlane(s.as_str(), &["sit1"], false).await;
        let identity = ports::AgentIdentity::new(s.clone());
        let ingest = &reads.world.rig.ingest;
        ingest
            .hello(&identity, super::rig::hello(s.as_str()))
            .await
            .unwrap();
        let entries = files
            .iter()
            .map(|(p, c)| super::rig::entry(p.as_str(), c, super::rig::T))
            .collect();
        ingest
            .delta(
                &identity,
                super::rig::delta(1, None, super::rig::root(1), entries),
            )
            .await
            .unwrap();
    }

    /// Replace the swimlane's tenant set, and give the files it holds the classification the new set makes of their names, as the pipeline
    /// does when the rules change (the logical file and the kind are derived columns, `ingest::rules`).
    async fn set_tenants(&self, s: &SwimlaneId, tenants: &[TenantId]) -> bool {
        let pool = &self.0.world.rig.pool;
        let set = TenantSet::new(tenants.iter().cloned()).unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("DELETE FROM swimlane_tenants WHERE swimlane_id = $1")
            .bind(s.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        for t in tenants {
            sqlx::query("INSERT INTO swimlane_tenants (swimlane_id, tenant_id) VALUES ($1, $2)")
                .bind(s.as_str())
                .bind(t.as_str())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM file_state WHERE swimlane_id = $1")
            .bind(s.as_str())
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        for path in paths {
            let facts = engine::classify_path(
                &domain::PathMappingRule::default(),
                &NfsPath::parse(&path).unwrap(),
                &set,
                |_| true,
            )
            .unwrap();
            sqlx::query(
                "UPDATE file_state SET logical_file = $3, kind = $4 WHERE swimlane_id = $1 AND path = $2",
            )
            .bind(s.as_str())
            .bind(&path)
            .bind(facts.logical.as_str())
            .bind(facts.kind.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        true
    }

    /// Replace the swimlane's services with these, each in its pickup state (a service that is `live` has the state row a pipeline run
    /// would leave it, and `pending_restarts` does not list it).
    async fn set_pending_restarts(&self, s: &SwimlaneId, services: Vec<ServiceState>) -> bool {
        let pool = &self.0.world.rig.pool;
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("DELETE FROM services WHERE swimlane_id = $1")
            .bind(s.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        for svc in services {
            let env = serde_json::json!({
                "CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED": svc.notifications_enabled.to_string(),
                "CONFIG_CLIENT_CACHE_TTL": svc.cache_ttl_secs.to_string(),
            });
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO services (swimlane_id, namespace, name, env_values) VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(s.as_str())
            .bind(svc.service.namespace())
            .bind(svc.service.name())
            .bind(env)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            let files: Vec<&str> = svc.pending_files.iter().map(NfsPath::as_str).collect();
            sqlx::query(
                "INSERT INTO service_state (service_id, pickup_state, pending_since, live_by, pending_files) \
                 VALUES ($1, $2, (timestamptz 'epoch' + ($3::bigint::text || ' milliseconds')::interval), (timestamptz 'epoch' + ($4::bigint::text || ' milliseconds')::interval), $5)",
            )
            .bind(id)
            .bind(svc.pickup.as_str())
            .bind(svc.pending_since.map(domain::Timestamp::unix_millis))
            .bind(svc.live_by.map(domain::Timestamp::unix_millis))
            .bind(serde_json::json!(files))
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        true
    }

    /// The tier, and the name of a project the swimlane belongs to (a swimlane always has one, so a project of `None` cannot be set).
    async fn set_tier_and_project(
        &self,
        s: &SwimlaneId,
        tier: Option<ShortText>,
        project: Option<ShortText>,
    ) -> bool {
        let Some(project) = project else { return false };
        let pool = &self.0.world.rig.pool;
        let id: String = format!(
            "p-{}",
            project
                .as_str()
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
        );
        sqlx::query(
            "INSERT INTO projects (id, display_name) VALUES ($1, $2) \
             ON CONFLICT (id) DO UPDATE SET display_name = EXCLUDED.display_name",
        )
        .bind(&id)
        .bind(project.as_str())
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("UPDATE swimlanes SET tier = $2, project_id = $3 WHERE id = $1")
            .bind(s.as_str())
            .bind(tier.as_ref().map(ShortText::as_str))
            .bind(&id)
            .execute(pool)
            .await
            .unwrap();
        true
    }

    /// The baseline the swimlane has: a confirmation is the columns the adoption's confirmation writes (and who, as a user row); an
    /// adoption in progress is an open adoption with nothing confirmed; unconfirmed is neither. Whatever adoption was open is closed first.
    async fn set_baseline(&self, s: &SwimlaneId, baseline: BaselineStatus) -> bool {
        let pool = &self.0.world.rig.pool;
        self.set_open_adoption(s, false).await;
        let (unlocked, at, by) = match baseline.state {
            BaselineState::Confirmed => (true, baseline.at.map(domain::Timestamp::unix_millis), baseline.by),
            BaselineState::Unconfirmed | BaselineState::InProgress => (false, None, None),
        };
        let (tid, oid) = match &by {
            Some(user) => {
                let (tid, oid) = (user.id.tid().as_str(), user.id.oid().as_str());
                sqlx::query(
                    "INSERT INTO users (tid, oid, email, display_name, role, status) \
                     VALUES ($1, $2, $3, $4, 'admin', 'active') \
                     ON CONFLICT (tid, oid) DO UPDATE SET display_name = EXCLUDED.display_name",
                )
                .bind(tid)
                .bind(oid)
                .bind(format!("{oid}@example.com"))
                .bind(user.display_name.as_str())
                .execute(pool)
                .await
                .unwrap();
                (Some(tid.to_owned()), Some(oid.to_owned()))
            }
            None => (None, None),
        };
        sqlx::query(
            "UPDATE swimlanes SET editing_unlocked = $2, \
               baseline_confirmed_at = (timestamptz 'epoch' + ($3::bigint::text || ' milliseconds')::interval), \
               baseline_confirmed_by_tid = $4, baseline_confirmed_by_oid = $5 WHERE id = $1",
        )
        .bind(s.as_str())
        .bind(unlocked)
        .bind(at)
        .bind(tid)
        .bind(oid)
        .execute(pool)
        .await
        .unwrap();
        if baseline.state == BaselineState::InProgress {
            self.set_open_adoption(s, true).await;
        }
        true
    }

    /// Open the sync window of a Job (the window of another Job that is open is closed), or close the open ones.
    async fn set_syncing(&self, s: &SwimlaneId, syncing: Option<SyncingInfo>) -> bool {
        let pool = &self.0.world.rig.pool;
        sqlx::query("UPDATE sync_windows SET closed_at = now() WHERE swimlane_id = $1 AND closed_at IS NULL")
            .bind(s.as_str())
            .execute(pool)
            .await
            .unwrap();
        if let Some(w) = syncing {
            sqlx::query(
                "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at) \
                 VALUES ($1, $2, $3, (timestamptz 'epoch' + ($4::bigint::text || ' milliseconds')::interval)) \
                 ON CONFLICT (swimlane_id, job_uid) DO UPDATE SET closed_at = NULL, opened_at = EXCLUDED.opened_at",
            )
            .bind(s.as_str())
            .bind(w.job.name())
            .bind(w.job.uid())
            .bind(w.since.unix_millis())
            .execute(pool)
            .await
            .unwrap();
        }
        true
    }

    /// Open an adoption on the swimlane (an `initial` one, in progress), or cancel the open ones. The baseline columns are left alone.
    async fn set_open_adoption(&self, s: &SwimlaneId, open: bool) -> bool {
        let pool = &self.0.world.rig.pool;
        if open {
            sqlx::query(
                "INSERT INTO adoptions (swimlane_id, reason, status) VALUES ($1, 'initial', 'in_progress') \
                 ON CONFLICT (swimlane_id, reason) WHERE status IN ('in_progress', 'review') DO NOTHING",
            )
            .bind(s.as_str())
            .execute(pool)
            .await
            .unwrap();
        } else {
            sqlx::query(
                "UPDATE adoptions SET status = 'cancelled' WHERE swimlane_id = $1 AND status IN ('in_progress', 'review')",
            )
            .bind(s.as_str())
            .execute(pool)
            .await
            .unwrap();
        }
        true
    }
}

#[async_trait]
impl RegistryRead for Scenario {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        self.0.service.swimlanes(u).await
    }
    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.0.service.tree(u, s, prefix, page).await
    }
    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<FileContent, ReadError> {
        self.0.service.content(u, s, p, src).await
    }
    async fn history(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        self.0.service.history(u, s, p, page).await
    }
    async fn compare(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        self.0.service.compare(u, l, r, p).await
    }
    async fn compare_tree(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.0.service.compare_tree(u, l, r, page).await
    }
    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
        self.0.service.grid(u, q).await
    }
    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        self.0.service.search_settings(u, q).await
    }
    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError> {
        self.0.service.search_text(u, q).await
    }
    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        self.0.service.findings(u, q).await
    }
    async fn effective(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        self.0.service.effective(u, s, f).await
    }
    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        self.0.service.pending_restarts(u, s).await
    }
    // The methods `contract/07-mcp` added. A wrapper must forward every one: a method it leaves out answers `Unavailable` (the port's
    // default) and hides what the service under it would say, so the suite would test the wrapper and not the service.
    async fn effective_for(
        &self,
        u: &User,
        s: &SwimlaneId,
        q: &EffectiveQuery,
    ) -> Result<EffectiveConfig, ReadError> {
        self.0.service.effective_for(u, s, q).await
    }
    async fn compare_in(&self, u: &User, q: &CompareQuery) -> Result<Comparison, ReadError> {
        self.0.service.compare_in(u, q).await
    }
    async fn compare_tree_under(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.0.service.compare_tree_under(u, l, r, prefix, page).await
    }
    async fn drift(
        &self,
        u: &User,
        s: &SwimlaneId,
        q: &DriftQuery,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.0.service.drift(u, s, q, page).await
    }
    async fn auto_notify(&self, u: &User, s: &SwimlaneId) -> Result<bool, ReadError> {
        self.0.service.auto_notify(u, s).await
    }
    async fn served(&self, u: &User, s: &SwimlaneId, r: &ServeRequest) -> Result<ServedFile, ReadError> {
        self.0.service.served(u, s, r).await
    }
}
