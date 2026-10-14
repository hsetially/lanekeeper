//! A registry over a real Postgres, wired the way the hub wires it, for the ingestion tests: the pool of `lanekeeper_app`, a
//! blob store over the `blobs` table, an event bus over the `outbox` table, an audit log over the real chain, and a clock the
//! test moves by hand.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use bytes::Bytes;
use domain::{
    AgentConfig, ClusterReport, ContentHash, Heartbeat, HeartbeatAction, Hello, NfsPath, ScanDelta,
    ScanEntry, ShortText, SwimlaneId, Timestamp,
};
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::EventMetrics;
use hub_registry::{
    AgentIngest, IndexDeps, IngestDeps, ManualClock, Pipeline, PipelineDeps, PipelineHandle, RegistryConfig,
    RegistryMetrics, SettingsIndexer,
};
use ports::{AgentIdentity, EventBus, GitReader, ReportSink, SinkError, content_hash};
use sqlx::{PgPool, Row};
use tokio::sync::watch;

use super::pg::TestDb;
use super::pg_audit::PgTestAudit;
use super::pg_blobs::PgBlobs;
use super::pg_bus::PgTestBus;

pub struct Rig {
    pub db: TestDb,
    pub pool: PgPool,
    pub bus: Arc<PgTestBus>,
    pub audit: Arc<PgTestAudit>,
    pub blobs: Arc<PgBlobs>,
    pub clock: Arc<ManualClock>,
    pub metrics: RegistryMetrics,
    pub ingest: AgentIngest,
    pub swimlane: SwimlaneId,
    pub identity: AgentIdentity,
    /// The settings index that ingestion offers its blobs to, when the rig was built `with_index`; its worker runs as a task.
    pub indexer: Option<SettingsIndexer>,
    /// The real event bus over the outbox (the one the hub runs), when the rig was built `with_event_bus`: ingestion publishes
    /// through it and the pipeline subscribes to it. `bus` stays the reader of the outbox for the assertions.
    pub events: Option<PgEventBus>,
    _stop: Option<watch::Sender<bool>>,
}

/// `observed_at` of the first scan in a test: a moment before the clock's `T0_MS`.
pub const T: i64 = ManualClock::T0_MS - 600_000;

impl Rig {
    /// A rig for swimlane `sit1`, or `None` when there is no database and it is not required.
    pub async fn new() -> Option<Self> {
        Self::with("sit1", RegistryConfig::default()).await
    }

    pub async fn with(swimlane: &str, config: RegistryConfig) -> Option<Self> {
        Self::build(swimlane, config, false).await
    }

    /// A rig whose ingestion offers every structured blob to a settings index, with its worker running.
    pub async fn with_index(swimlane: &str, config: RegistryConfig) -> Option<Self> {
        Self::build(swimlane, config, true).await
    }

    /// The settings indexer of a rig built `with_index`.
    pub fn index(&self) -> &SettingsIndexer {
        self.indexer.as_ref().expect("the rig was built with_index")
    }

    /// A settings indexer over this rig's database, blob store and clock, with its own queue: a second replica. Its worker is not
    /// started; the caller runs it, or calls `index_now`.
    pub fn another_indexer(
        &self,
        config: RegistryConfig,
        metrics: RegistryMetrics,
    ) -> (SettingsIndexer, hub_registry::SettingsWorker) {
        SettingsIndexer::new(IndexDeps {
            pool: self.pool.clone(),
            blobs: Arc::clone(&self.blobs) as _,
            clock: Arc::clone(&self.clock) as _,
            config,
            metrics,
        })
        .unwrap()
    }

    /// A rig whose ingestion publishes through the real `PgEventBus` (NOTIFY, relay, broadcast), so that a pipeline subscribed to it
    /// hears what ingestion publishes, as in the hub.
    pub async fn with_event_bus(swimlane: &str, config: RegistryConfig) -> Option<Self> {
        Self::build_with(swimlane, config, false, true).await
    }

    async fn build(swimlane: &str, config: RegistryConfig, index: bool) -> Option<Self> {
        Self::build_with(swimlane, config, index, false).await
    }

    async fn build_with(swimlane: &str, config: RegistryConfig, index: bool, real_bus: bool) -> Option<Self> {
        let db = TestDb::ready().await?;
        let pool = db.app_pool().await;
        db.seed_swimlane(&pool, swimlane).await;
        let (events, stop) = if real_bus {
            let bus = PgEventBus::new(pool.clone(), BusSettings::default(), EventMetrics::default());
            let (tx, rx) = watch::channel(false);
            // The tasks end when the sender in the rig is dropped; their handles are not needed.
            let _tasks = bus.start(rx).await;
            (Some(bus), Some(tx))
        } else {
            (None, None)
        };
        let bus = Arc::new(PgTestBus::new(pool.clone()));
        let ingest_bus: Arc<dyn EventBus> = events.clone().map_or_else(
            || Arc::clone(&bus) as Arc<dyn EventBus>,
            |e| Arc::new(e) as Arc<dyn EventBus>,
        );
        let audit = Arc::new(PgTestAudit::default());
        let blobs = Arc::new(PgBlobs::new(pool.clone()));
        let clock = Arc::new(ManualClock::at_t0());
        let metrics = RegistryMetrics::default();
        let indexer = if index {
            let (indexer, worker) = SettingsIndexer::new(IndexDeps {
                pool: pool.clone(),
                blobs: Arc::clone(&blobs) as _,
                clock: Arc::clone(&clock) as _,
                config: config.clone(),
                metrics: metrics.clone(),
            })
            .unwrap();
            tokio::spawn(worker.run());
            Some(indexer)
        } else {
            None
        };
        let ingest = AgentIngest::new(IngestDeps {
            pool: pool.clone(),
            blobs: Arc::clone(&blobs) as _,
            bus: ingest_bus,
            audit: Arc::clone(&audit) as _,
            clock: Arc::clone(&clock) as _,
            config,
            metrics: metrics.clone(),
        })
        .unwrap();
        if let Some(indexer) = &indexer {
            assert!(ingest.attach_settings_index(indexer.clone()).is_ok());
        }
        let swimlane = SwimlaneId::parse(swimlane).unwrap();
        Some(Self {
            identity: AgentIdentity::new(swimlane.clone()),
            db,
            pool,
            bus,
            audit,
            blobs,
            clock,
            metrics,
            ingest,
            swimlane,
            indexer,
            events,
            _stop: stop,
        })
    }

    /// The pipeline over this rig's database, blobs, audit log, clock and metrics, reading `git`, on the real event bus when the
    /// rig has one and on the outbox reader (which delivers nothing) when it does not.
    pub fn pipeline(&self, git: Arc<dyn GitReader>, config: RegistryConfig) -> (Pipeline, PipelineHandle) {
        self.pipeline_with(git, config, Arc::clone(&self.audit) as _)
    }

    /// The same, with an audit log of its own (a test can make it fail without ingestion failing too).
    pub fn pipeline_with(
        &self,
        git: Arc<dyn GitReader>,
        config: RegistryConfig,
        audit: Arc<dyn ports::AuditLog>,
    ) -> (Pipeline, PipelineHandle) {
        let bus: Arc<dyn EventBus> = self.events.clone().map_or_else(
            || Arc::clone(&self.bus) as Arc<dyn EventBus>,
            |e| Arc::new(e) as Arc<dyn EventBus>,
        );
        Pipeline::new(PipelineDeps {
            pool: self.pool.clone(),
            blobs: Arc::clone(&self.blobs) as _,
            git,
            bus,
            audit,
            clock: Arc::clone(&self.clock) as _,
            config,
            metrics: self.metrics.clone(),
        })
        .unwrap()
    }

    /// Another swimlane in the same database, with its tenants, and (when `inventory`) a `swimlane_registry` row that says its first
    /// inventory has been applied, as a full scan would.
    pub async fn add_swimlane(&self, id: &str, tenants: &[&str], inventory: bool) {
        sqlx::query(
            "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1') ON CONFLICT DO NOTHING",
        )
        .execute(&self.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO swimlanes (id, project_id, display_name) VALUES ($1, 'p1', $1) ON CONFLICT DO NOTHING")
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
        for tenant in tenants {
            self.db.seed_tenant(&self.pool, id, tenant).await;
        }
        sqlx::query(
            "INSERT INTO swimlane_registry (swimlane_id, first_inventory_at) \
             VALUES ($1, CASE WHEN $2 THEN now() END) \
             ON CONFLICT (swimlane_id) DO UPDATE SET first_inventory_at = EXCLUDED.first_inventory_at",
        )
        .bind(id)
        .bind(inventory)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    pub async fn hello(&self) -> AgentConfig {
        self.ingest
            .hello(&self.identity, hello(self.swimlane.as_str()))
            .await
            .unwrap()
    }

    pub async fn heartbeat(&self, root: ContentHash) -> Result<HeartbeatAction, SinkError> {
        self.ingest
            .heartbeat(
                &self.identity,
                Heartbeat {
                    scan_seq: 1,
                    merkle_root: root,
                    file_count: 1,
                },
            )
            .await
    }

    pub async fn delta(&self, d: ScanDelta) -> Result<(), SinkError> {
        self.ingest.delta(&self.identity, d).await
    }

    pub async fn cluster(&self, c: ClusterReport) -> Result<(), SinkError> {
        self.ingest.cluster(&self.identity, c).await
    }

    /// Say hello, then deliver `files` as one full scan with root `root(1)`.
    pub async fn first_scan(&self, files: &[(&str, &str)]) {
        self.hello().await;
        let entries = files.iter().map(|(p, c)| entry(p, c.as_bytes(), T)).collect();
        self.delta(delta(1, None, root(1), entries)).await.unwrap();
    }

    // ------------------------------------------------------------------------------------------ what the database holds

    pub async fn state(&self, path: &str) -> Option<StateRow> {
        let row = sqlx::query(
            "SELECT nfs_hash::bytea, nfs_drift_hash::bytea, git_hash::bytea, baseline_hash::bytea, class, size, denied, logical_file, \
                    kind, role, state, eol_style, encoding, bom, (EXTRACT(EPOCH FROM observed_at) * 1000)::bigint AS at \
             FROM file_state WHERE swimlane_id = $1 AND path = $2",
        )
        .bind(self.swimlane.as_str())
        .bind(path)
        .fetch_optional(&self.pool)
        .await
        .unwrap()?;
        let h = |b: Option<Vec<u8>>| b.map(|b| ContentHash::from_bytes(b.try_into().unwrap()));
        Some(StateRow {
            hash: h(row.get("nfs_hash")),
            drift_hash: h(row.get("nfs_drift_hash")),
            git_hash: h(row.get("git_hash")),
            baseline_hash: h(row.get("baseline_hash")),
            class: row.get("class"),
            size: row.get("size"),
            denied: row.get("denied"),
            logical_file: row.get("logical_file"),
            kind: row.get("kind"),
            role: row.get("role"),
            state: row.get("state"),
            eol: row.get("eol_style"),
            encoding: row.get("encoding"),
            bom: row.get("bom"),
            observed_at_ms: row.get::<Option<i64>, _>("at"),
        })
    }

    pub async fn observations(&self, path: &str) -> Vec<ObservationRow> {
        let rows = sqlx::query(
            "SELECT hash::bytea, source, class, initial, attribution_source, attribution_confidence, attribution_actor, \
                    during_job, (EXTRACT(EPOCH FROM observed_at) * 1000)::bigint AS at \
             FROM file_observations WHERE swimlane_id = $1 AND path = $2 ORDER BY observed_at, id LIMIT 1000",
        )
        .bind(self.swimlane.as_str())
        .bind(path)
        .fetch_all(&self.pool)
        .await
        .unwrap();
        rows.into_iter()
            .map(|row| ObservationRow {
                hash: row
                    .get::<Option<Vec<u8>>, _>("hash")
                    .map(|b| ContentHash::from_bytes(b.try_into().unwrap())),
                source: row.get("source"),
                class: row.get("class"),
                initial: row.get("initial"),
                attribution_source: row.get("attribution_source"),
                attribution_confidence: row.get("attribution_confidence"),
                attribution_actor: row.get("attribution_actor"),
                during_job: row.get("during_job"),
                observed_at_ms: row.get("at"),
            })
            .collect()
    }

    pub async fn count(&self, sql: &str) -> i64 {
        self.db.count(&self.pool, sql).await
    }

    pub async fn root(&self) -> Option<ContentHash> {
        sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT hash::bytea FROM swimlane_merkle WHERE swimlane_id = $1 AND dir_path = ''",
        )
        .bind(self.swimlane.as_str())
        .fetch_optional(&self.pool)
        .await
        .unwrap()
        .map(|b| ContentHash::from_bytes(b.try_into().unwrap()))
    }

    pub async fn needs_full_scan(&self) -> bool {
        sqlx::query_scalar("SELECT needs_full_scan FROM swimlane_registry WHERE swimlane_id = $1")
            .bind(self.swimlane.as_str())
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRow {
    pub hash: Option<ContentHash>,
    pub drift_hash: Option<ContentHash>,
    pub git_hash: Option<ContentHash>,
    pub baseline_hash: Option<ContentHash>,
    pub class: String,
    pub size: Option<i64>,
    pub denied: bool,
    pub logical_file: String,
    pub kind: String,
    pub role: Option<String>,
    pub state: String,
    pub eol: Option<String>,
    pub encoding: Option<String>,
    pub bom: Option<bool>,
    pub observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRow {
    pub hash: Option<ContentHash>,
    pub source: String,
    pub class: Option<String>,
    pub initial: bool,
    pub attribution_source: String,
    pub attribution_confidence: String,
    pub attribution_actor: Option<serde_json::Value>,
    pub during_job: Option<String>,
    pub observed_at_ms: i64,
}

// ---------------------------------------------------------------------------------------------- message builders

pub fn text(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

pub fn path(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

pub fn hello(swimlane: &str) -> Hello {
    Hello {
        agent_version: text("0.1.0"),
        swimlane: SwimlaneId::parse(swimlane).unwrap(),
        cluster: text("gke-sit1"),
        project: text("proj"),
        nfs_server: text("nfs.internal"),
        export: text("/export"),
        mount_root: text("/mnt/config"),
    }
}

/// A deterministic root: `root(1)`, `root(2)`, ...
pub fn root(n: u8) -> ContentHash {
    content_hash(&[b'r', n])
}

pub fn entry(p: &str, content: &[u8], observed_at_ms: i64) -> ScanEntry {
    ScanEntry {
        path: path(p),
        hash: content_hash(content),
        size: content.len() as u64,
        mtime: Timestamp::from_unix_millis(observed_at_ms - 1_000),
        observed_at: Timestamp::from_unix_millis(observed_at_ms),
        denied: false,
        bytes: Some(Bytes::copy_from_slice(content)),
    }
}

pub fn delta(
    seq: u64,
    base: Option<ContentHash>,
    new_root: ContentHash,
    entries: Vec<ScanEntry>,
) -> ScanDelta {
    ScanDelta {
        seq,
        base_root: base,
        new_root,
        entries,
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more: false,
        part: 0,
        gap: None,
    }
}

pub fn part(
    seq: u64,
    part: u32,
    more: bool,
    base: Option<ContentHash>,
    new_root: ContentHash,
    entries: Vec<ScanEntry>,
) -> ScanDelta {
    ScanDelta {
        more,
        part,
        ..delta(seq, base, new_root, entries)
    }
}
