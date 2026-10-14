//! The read service over a real Postgres, wired the way the hub wires it: the pool of `lanekeeper_app`, the blob store over the
//! `blobs` table, the in-memory Git, a scripted agent gateway, the clock of the rig. Files get in through ingestion, as an agent
//! sends them, so what a read sees is what the registry stored.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    CompareRef, Comparison, ContentSource, EffectiveConfig, FileContent, FileEntry, Finding, FindingsQuery,
    Grid, GridQuery, LogicalFile, NfsPath, Page, Paged, Role, ServiceState, SettingHit, SettingsQuery,
    SwimlaneId, SwimlaneSummary, TextQuery, TextResults, TreeComparison, User, Version,
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
}
