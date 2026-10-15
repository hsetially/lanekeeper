//! The structure checks C9, C10 and C11 (T10, D82 to D88, A45): the findings that the engine's lane state does not carry, computed from the
//! registry's own facts with the engine's check functions and written by fingerprint like the findings of C1 to C8.
//!
//! | Check | Reads | Engine function |
//! |---|---|---|
//! | C9 `ambiguous_file_name` | the files on NFS (their logical names), the channels `channels.yml` lists | [`engine::c9_ambiguous_file_names`] |
//! | C10 `unresolved_placeholder` | the resource files' text, the property views of their applications and the tenants, the names of the environment of the Deployments that read each file | [`engine::c10_unresolved_placeholders`] ([`c10`]) |
//! | C11 `config_server_restart_required` | the folders and when each was first seen, `channels.yml` (now, its newest version's time, the version the config-server started with), the config-server's start | [`engine::c11_config_server_restart_required`] |
//!
//! The registry decides what goes in (which files, which views, which times); the facts come only from the engine (rule 7). No result holds a
//! value, and no log line a name or a text: counts and ids (S10).
//!
//! # Where it runs, and what it costs the incremental recompute
//!
//! **Never inside the pipeline's batch.** The leased job `registry-structure-sweeper` ([`Structure::spec`], every `structure_sweep_interval`,
//! 10 seconds) looks at the swimlanes' *gates*: the version the pipeline stamps on a commit that changed something, the config-server's start
//! and the fingerprint of the rules the state was computed under, one statement for all swimlanes. A swimlane whose gate is what the sweeper
//! saw when it last computed it is not touched again (`Unchanged`: no statement but the gate). So `Pipeline::process` runs the statements it
//! ran before this module existed (a test pins the count for a one-file change), the benchmarks of P1, P2, P5 and P6 do not move, and a
//! swimlane that is busy is computed at most once per sweep, however many batches it had.
//!
//! **When a swimlane is computed, only what changed is evaluated.**
//!
//! - C9 and C11 are pure functions of rows that one computation reads anyway (the swimlane's files, its folders), and C9 is remembered by a
//!   digest of the names it was computed from (a content-only change leaves it unchanged).
//! - C10 is cached **by content** ([`c10`]): a result is kept under the file's path and blob and the digests of the property sources and the
//!   environment names it depends on. A one-file change to a resource reads **one blob** and evaluates **one file**; a change to a property
//!   source evaluates the files of the applications it can reach, because any placeholder may now be filled or empty, and no others.
//! - Nothing is written when nothing differs: the findings are compared with the stored ones first, with the pool, and a transaction is
//!   opened only for a difference.
//!
//! What no gate announces (a Deployment's environment names, a folder mapping an admin changed, a replica that took over) is found by the
//! **full pass**, every `structure_full_interval` (10 minutes): every swimlane is computed whatever its gate says, which costs file reads only
//! for what the caches do not hold.
//!
//! # Findings
//!
//! Rows are keyed by kind and a **fingerprint with an identity of its own** ([`super::findings_sync::fingerprint_keyed`]): the file name of a
//! C9 (its first copy changes when an earlier one appears), the path of a C10, the reason of a C11. A finding that stays keeps its row and
//! its `detected_at`; one that is gone is resolved. Severity comes from the rules in force ([`crate::severity`]); the engine's defaults rank
//! none of C9 to C11 (they are low unless an admin's rule says more). A computation that changes findings, in one transaction under the
//! swimlane's structure lock: writes them, bumps the swimlane's version (so that an `ETag` over the swimlane moves), recounts its open findings,
//! and publishes `FindingsChanged` through the outbox (D80). Findings are derived facts, so there is **no audit event** (D93).
//!
//! # Limits
//!
//! - **No tenant, no C10**: a swimlane that nobody has adopted has no view to look a key up in.
//! - **No known config-server start, no C11**: the two kinds of C11 that compare with it, and the ones that need the version at the start,
//!   switch off, as the pickup computation does (D105).
//! - **A resource's readers** are what [`crate::pickup::consumers`] knows: the application of its first folder and the Deployments an admin
//!   linked. A file with none is read by any service, and its environment is that of all of them: a placeholder is reported only when no
//!   Deployment of the swimlane has the variable.
//! - **A view over the engine's limits** is skipped with a log line: an empty view would report every key.
//! - **`channels.yml` is the root file with the list `channels:`**; an entry that is not a channel name is left out.

mod c10;
pub mod inputs;
mod sweeper;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use domain::{ChannelName, ContentHash, MergeMode, NfsPath, RepoPath, SwimlaneId, TenantSet, Timestamp};
use engine::{
    ChannelsYml, CheckDetail, CheckFile, CheckResult, ConfigServerFacts, DenyMatcher, FileFacts, FolderSeen,
    Limits, Syntax, c9_ambiguous_file_names, c11_config_server_restart_required,
};
use moka::sync::Cache;
use ports::{BlobStore, EventBus};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use tracing::{debug, warn};

use self::inputs::{FileRow, Gate};
pub use self::sweeper::{JOB_NAME, SweepReport};
use super::findings_store::{Store, reconcile};
use super::findings_sync::{
    FindingKey, FindingRow, fingerprint_keyed, row_from_result, structure_kind_names,
};
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::denied::is_denied;
use crate::error::RegistryError;
use crate::metrics::{RegistryMetrics, StructureResult};
use crate::pickup::consumers::{Directory, Mapping, Reader, is_read};
use crate::severity::SeverityRuleSource;
use crate::tenants;

/// The confirmed folder mappings: the swimlane's own and the global ones, as the pickup computation reads them. `$1` the swimlane, `$2` the
/// limit.
pub use crate::pickup::MAPPINGS_SQL;

/// The most folder mappings read for one swimlane.
const MAX_MAPPINGS: i64 = 10_000;
/// The most channels `channels.yml` is read for.
const MAX_CHANNELS: usize = 1_024;
/// The root file that lists the channels.
const CHANNELS_FILE: &str = "channels.yml";

/// A hash that cannot be confused by moving a byte from one field to the next: every field is length-prefixed.
pub(super) struct Digest32(Sha256);

impl Digest32 {
    pub(super) fn new(tag: &str) -> Self {
        let mut d = Self(Sha256::new());
        d.field(tag.as_bytes());
        d
    }

    pub(super) fn field(&mut self, value: &[u8]) {
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value);
    }

    pub(super) fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// How much of the gate decides whether a swimlane is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A swimlane whose gate is what it was when it was last computed is left alone.
    Gated,
    /// Every swimlane is computed whatever its gate says: the full pass.
    Full,
}

/// What the structure checks need from the rest of the hub.
pub struct StructureDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub blobs: Arc<dyn BlobStore>,
    pub bus: Arc<dyn EventBus>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    /// The rules findings are rated with (the same source the pipeline uses).
    pub severity: SeverityRuleSource,
}

impl std::fmt::Debug for StructureDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructureDeps").finish_non_exhaustive()
    }
}

/// What one swimlane's computation did, in counts (S10: nothing of a file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneOutcome {
    pub swimlane: SwimlaneId,
    pub result: StructureResult,
    /// Files read from the database for the computation.
    pub files: usize,
    /// C9 was computed (and not taken from the memory of the last computation, whose names are the same).
    pub c9_computed: bool,
    /// C10: resource files looked at, evaluated now, and answered from the cache; and the blobs the computation read in all (resources,
    /// property sources and `channels.yml`).
    pub c10_files: usize,
    pub c10_evaluated: usize,
    pub c10_cached: usize,
    pub blobs_read: usize,
    /// Findings written: new, changed and resolved.
    pub inserted: usize,
    pub updated: usize,
    pub resolved: usize,
}

impl LaneOutcome {
    fn new(swimlane: SwimlaneId, result: StructureResult) -> Self {
        Self {
            swimlane,
            result,
            files: 0,
            c9_computed: false,
            c10_files: 0,
            c10_evaluated: 0,
            c10_cached: 0,
            blobs_read: 0,
            inserted: 0,
            updated: 0,
            resolved: 0,
        }
    }

    /// Findings written in all.
    pub fn written(&self) -> usize {
        self.inserted + self.updated + self.resolved
    }
}

/// What a computation of several swimlanes did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub lanes: Vec<LaneOutcome>,
    /// Swimlanes that could not be computed (the database, the blob store, the outbox). Nothing of them was written.
    pub failed: Vec<SwimlaneId>,
}

/// The part of a gate that says whether anything moved: the version, the config-server's start, the rules' fingerprint.
type GateKey = (i64, Option<i64>, Option<Vec<u8>>);

/// What the replica remembers of a swimlane: the gate it was last computed under, and the names C9 was last computed from. Only a saving:
/// the findings in the database are the truth, and a replica that has forgotten computes the swimlane again.
#[derive(Default)]
struct Memory {
    gates: BTreeMap<SwimlaneId, GateKey>,
    c9: BTreeMap<SwimlaneId, ([u8; 32], Arc<Vec<CheckResult>>)>,
    /// The swimlanes the full pass in progress has computed: a pass that a run could not finish (it computed as many swimlanes as it may, or
    /// lost its lease, or one failed) goes on with the others at the next run and does not start again from the first.
    pass: std::collections::BTreeSet<SwimlaneId>,
}

pub(crate) struct Inner {
    db: Db,
    blobs: Arc<dyn BlobStore>,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
    config: RegistryConfig,
    metrics: RegistryMetrics,
    severity: SeverityRuleSource,
    deny: DenyMatcher,
    limits: Limits,
    memory: Mutex<Memory>,
    cache: c10::C10Cache,
    /// The channels `channels.yml` lists, by its blob.
    channels: Cache<ContentHash, Arc<Vec<ChannelName>>>,
    /// When the sweeper last computed every swimlane (Unix milliseconds); the lowest value before the first pass.
    last_full: std::sync::atomic::AtomicI64,
}

/// The structure checks. Cloning shares them.
#[derive(Clone)]
pub struct Structure {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Structure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Structure").finish_non_exhaustive()
    }
}

/// The names of the channels in the bytes of `channels.yml`: the entries of its `channels:` list that are channel names.
pub(crate) fn channels_of(bytes: &[u8], limits: &Limits) -> Vec<ChannelName> {
    let Ok(index) = engine::index_settings(bytes, Syntax::Yaml, limits) else {
        return Vec::new();
    };
    let mut out: Vec<ChannelName> = index
        .rows
        .iter()
        .filter_map(|row| {
            let rest = row.path.as_str().strip_prefix("channels[")?.strip_suffix(']')?;
            rest.parse::<u32>().ok()?;
            ChannelName::parse(row.value.text.trim()).ok()
        })
        .collect();
    out.sort();
    out.dedup();
    out.truncate(MAX_CHANNELS);
    out
}

impl Structure {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range or a deny glob does not compile.
    pub fn new(deps: StructureDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let deny = DenyMatcher::with_globs(deps.config.extra_deny_globs.iter().map(String::as_str))
            .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?;
        let limits = Limits {
            input_bytes: usize::try_from(deps.config.max_file_bytes).unwrap_or(usize::MAX),
            ..Limits::default()
        };
        let cache = c10::C10Cache::new(&deps.config);
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                blobs: deps.blobs,
                bus: deps.bus,
                clock: deps.clock,
                metrics: deps.metrics,
                severity: deps.severity,
                deny,
                limits,
                memory: Mutex::new(Memory::default()),
                cache,
                channels: Cache::builder().max_capacity(4_096).build(),
                last_full: std::sync::atomic::AtomicI64::new(i64::MIN),
                config: deps.config,
            }),
        })
    }

    /// Compute the structure checks of these swimlanes, one after the other. In [`Mode::Gated`] a swimlane that nothing moved in is left alone;
    /// a swimlane that fails is in the report and the others go on (the sweeper looks at it again).
    pub async fn recompute(&self, swimlanes: &[SwimlaneId], mode: Mode) -> Report {
        let mut report = Report::default();
        if swimlanes.is_empty() {
            return report;
        }
        let Ok(gates) = inputs::gates(&self.inner.db, Some(swimlanes), swimlanes.len()).await else {
            self.inner.metrics.structure_lane(StructureResult::Failed);
            report.failed.extend(swimlanes.iter().cloned());
            return report;
        };
        for swimlane in swimlanes {
            match gates.iter().find(|g| g.swimlane == *swimlane) {
                // No inventory, or not a swimlane: nothing to check.
                None => {
                    self.inner.metrics.structure_lane(StructureResult::NotReady);
                    report
                        .lanes
                        .push(LaneOutcome::new(swimlane.clone(), StructureResult::NotReady));
                }
                Some(gate) => match self.inner.lane(gate, mode).await {
                    Ok(outcome) => report.lanes.push(outcome),
                    Err(_) => report.failed.push(swimlane.clone()),
                },
            }
        }
        report
    }

    /// Forget what this replica remembers and what the caches hold: the next computation reads everything. For a test that compares a cold
    /// computation with a warm one.
    pub fn forget(&self) {
        if let Ok(mut m) = self.inner.memory.lock() {
            *m = Memory::default();
        }
        self.inner.cache.clear();
        self.inner.channels.invalidate_all();
    }
}

impl Inner {
    /// Whether the full pass in progress has computed the swimlane.
    fn passed(&self, swimlane: &SwimlaneId) -> bool {
        self.memory.lock().is_ok_and(|m| m.pass.contains(swimlane))
    }

    fn mark_passed(&self, swimlane: &SwimlaneId) {
        if let Ok(mut m) = self.memory.lock() {
            m.pass.insert(swimlane.clone());
        }
    }

    /// The full pass is over: the next one starts from the first swimlane.
    fn end_pass(&self) {
        if let Ok(mut m) = self.memory.lock() {
            m.pass.clear();
        }
    }

    fn remembers(&self, gate: &Gate) -> bool {
        self.memory.lock().is_ok_and(|m| {
            m.gates
                .get(&gate.swimlane)
                .is_some_and(|k| (k.0, k.1, k.2.as_deref()) == gate.key())
        })
    }

    fn remember(&self, swimlane: &SwimlaneId, version: i64, gate: &Gate) {
        if let Ok(mut m) = self.memory.lock() {
            m.gates
                .insert(swimlane.clone(), (version, gate.started, gate.rules.clone()));
        }
    }

    /// One swimlane: look at its gate, compute what it says, write what differs.
    pub(crate) async fn lane(&self, gate: &Gate, mode: Mode) -> Result<LaneOutcome, RegistryError> {
        let swimlane = &gate.swimlane;
        if !gate.ready() {
            self.metrics.structure_lane(StructureResult::NotReady);
            return Ok(LaneOutcome::new(swimlane.clone(), StructureResult::NotReady));
        }
        if mode == Mode::Gated && self.remembers(gate) {
            self.metrics.structure_lane(StructureResult::Unchanged);
            return Ok(LaneOutcome::new(swimlane.clone(), StructureResult::Unchanged));
        }
        match self.compute_and_write(gate).await {
            Ok(outcome) => {
                self.metrics.structure_lane(StructureResult::Computed);
                self.metrics.structure_work(
                    outcome.written(),
                    outcome.c10_evaluated,
                    outcome.c10_cached,
                    outcome.blobs_read,
                );
                // Ids and counts only (S10): no path, no name, no text.
                debug!(
                    swimlane = %swimlane,
                    files = outcome.files,
                    c10_evaluated = outcome.c10_evaluated,
                    c10_cached = outcome.c10_cached,
                    blobs_read = outcome.blobs_read,
                    written = outcome.written(),
                    "structure checks computed"
                );
                Ok(outcome)
            }
            Err(error) => {
                self.metrics.structure_lane(StructureResult::Failed);
                warn!(swimlane = %swimlane, ?error, "the structure checks of a swimlane could not be computed");
                Err(error)
            }
        }
    }

    async fn compute_and_write(&self, gate: &Gate) -> Result<LaneOutcome, RegistryError> {
        let swimlane = &gate.swimlane;
        let mut out = LaneOutcome::new(swimlane.clone(), StructureResult::Computed);
        let rows = self.compute(gate, &mut out).await?;
        let version = self.write(swimlane, &rows, &mut out).await?;
        // The version the next look compares with: ours, when ours is the only change since the gate was read; otherwise the gate's, so
        // that whatever the pipeline committed in between is seen at the next look.
        let remembered = match version {
            Some(v) if v == gate.version.saturating_add(1) => v,
            _ => gate.version,
        };
        self.remember(swimlane, remembered, gate);
        Ok(out)
    }

    /// The tenants a swimlane stores, read with the pool.
    async fn tenants(&self, swimlane: &SwimlaneId) -> Result<TenantSet, RegistryError> {
        let ids: Vec<String> = self
            .db
            .run(
                "structure_tenants",
                sqlx::query_scalar(tenants::STORED_SQL)
                    .bind(swimlane.as_str())
                    .bind(i64::try_from(TenantSet::MAX_TENANTS + 1).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        TenantSet::new(ids.iter().filter_map(|t| domain::TenantId::parse(t).ok()))
            .map_err(|_| RegistryError::Corrupt)
    }

    /// The confirmed folder mappings of the swimlane and the global ones.
    async fn mappings(&self, swimlane: &SwimlaneId) -> Result<Vec<Mapping>, RegistryError> {
        let rows = self
            .db
            .run(
                "structure_mappings",
                sqlx::query(MAPPINGS_SQL)
                    .bind(swimlane.as_str())
                    .bind(MAX_MAPPINGS)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| Mapping {
                of_swimlane: r.get("own"),
                folder: r.get("folder_path"),
                namespace: r.get("namespace"),
                deployment: r.get("deployment"),
            })
            .collect())
    }

    /// The channels a blob lists, cached by the blob (read once, here).
    async fn channels_in(
        &self,
        blob: &ContentHash,
        out: &mut LaneOutcome,
    ) -> Result<Arc<Vec<ChannelName>>, RegistryError> {
        if let Some(hit) = self.channels.get(blob) {
            return Ok(hit);
        }
        let got = tokio::time::timeout(self.config.blob_timeout, self.blobs.get(blob))
            .await
            .map_err(|_| RegistryError::Timeout)??;
        // A blob the store does not have lists no channel: `channels.yml` is then as good as absent, and nothing is cached.
        let Some(bytes) = got else {
            return Ok(Arc::new(Vec::new()));
        };
        out.blobs_read += 1;
        let list = Arc::new(channels_of(&bytes, &self.limits));
        self.channels.insert(*blob, Arc::clone(&list));
        Ok(list)
    }

    /// C11's `channels.yml` facts: the time of its newest version (the epoch for a version of the first inventory, A11), and the channels it
    /// listed when the config-server started.
    async fn channels_history(
        &self,
        swimlane: &SwimlaneId,
        started: i64,
        out: &mut LaneOutcome,
    ) -> Result<(Timestamp, Vec<ChannelName>), RegistryError> {
        let path = CHANNELS_FILE;
        let newest = self
            .db
            .run(
                "structure_channels_newest",
                sqlx::query(&inputs::newest_observation_sql())
                    .bind(swimlane.as_str())
                    .bind(path)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        let changed_at = newest.map_or(0, |r| {
            let initial: bool = r.get("initial");
            if initial { 0 } else { r.get::<i64, _>("at") }
        });
        let at_start = self
            .db
            .run(
                "structure_channels_at_start",
                sqlx::query(&inputs::observation_at_sql())
                    .bind(swimlane.as_str())
                    .bind(path)
                    .bind(started)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        let blob: Option<Vec<u8>> = if let Some(r) = at_start {
            // The version current at the start; a deletion lists nothing.
            r.get("hash")
        } else {
            // Nothing was observed before the start. The first inventory is taken to be what the config-server read (nothing observed
            // changed it since); a file that first appeared later was not there.
            let first = self
                .db
                .run(
                    "structure_channels_first",
                    sqlx::query(inputs::FIRST_OBSERVATION_SQL)
                        .bind(swimlane.as_str())
                        .bind(path)
                        .fetch_optional(self.db.pool()),
                )
                .await?;
            first.and_then(|r| {
                if r.get::<bool, _>("initial") {
                    r.get("hash")
                } else {
                    None
                }
            })
        };
        let list = match blob {
            Some(bytes) => {
                let hash = crate::read::rows::hash(&bytes)?;
                (*self.channels_in(&hash, out).await?).clone()
            }
            None => Vec::new(),
        };
        Ok((Timestamp::from_unix_millis(changed_at), list))
    }

    /// C9: the names in more than one folder, remembered by the names it was computed from.
    fn c9(
        &self,
        swimlane: &SwimlaneId,
        files: &[FileRow],
        tenants: &TenantSet,
        channels: &[ChannelName],
        out: &mut LaneOutcome,
    ) -> Arc<Vec<CheckResult>> {
        let read: Vec<&FileRow> = files
            .iter()
            .filter(|f| is_read(&f.path, &f.logical, tenants))
            .collect();
        let mut digest = Digest32::new("c9");
        for f in &read {
            digest.field(f.path.as_str().as_bytes());
            digest.field(f.logical.as_str().as_bytes());
        }
        digest.field(b"channels");
        for c in channels {
            digest.field(c.as_str().as_bytes());
        }
        let digest = digest.finish();
        if let Ok(m) = self.memory.lock() {
            if let Some((seen, results)) = m.c9.get(swimlane) {
                if *seen == digest {
                    return Arc::clone(results);
                }
            }
        }
        out.c9_computed = true;
        // The facts a C9 reads are the path and the logical name; the rest is what the stored row says.
        let facts: Vec<(&FileRow, FileFacts)> = read
            .into_iter()
            .filter_map(|f| {
                Some((
                    f,
                    FileFacts {
                        kind: f.kind,
                        logical: f.logical.clone(),
                        tenant: tenants::tenant_suffix(&f.path, &f.logical),
                        repo_path: RepoPath::parse(f.path.as_str()).ok()?,
                    },
                ))
            })
            .collect();
        let views: Vec<CheckFile<'_>> = facts
            .iter()
            .map(|(f, facts)| CheckFile {
                path: &f.path,
                facts,
                class: f.class,
                hash: f.blob,
                merge_mode: MergeMode::WholeFile,
                structured: None,
                root_settings: None,
                drift: None,
                overwrite: None,
                base_history: None,
            })
            .collect();
        let results = Arc::new(c9_ambiguous_file_names(&views, channels, &self.limits));
        if let Ok(mut m) = self.memory.lock() {
            m.c9.insert(swimlane.clone(), (digest, Arc::clone(&results)));
        }
        results
    }

    /// C11, when the config-server's start is known.
    async fn c11(
        &self,
        swimlane: &SwimlaneId,
        gate: &Gate,
        files: &[FileRow],
        channels_now: &[ChannelName],
        out: &mut LaneOutcome,
    ) -> Result<Vec<CheckResult>, RegistryError> {
        let Some(started) = gate.started else {
            return Ok(Vec::new());
        };
        let folders = inputs::folders(&self.db, swimlane).await?;
        let seen: Vec<FolderSeen<'_>> = folders
            .iter()
            .map(|(dir, at)| FolderSeen {
                dir,
                // A folder of the first inventory is taken as seen at the epoch (A11): it was there when the config-server started.
                first_seen: Timestamp::from_unix_millis(at.unwrap_or(0)),
            })
            .collect();
        let yml = files.iter().find(|f| f.path.as_str() == CHANNELS_FILE);
        let (changed_at, at_start) = self.channels_history(swimlane, started, out).await?;
        let facts = ConfigServerFacts {
            started: Timestamp::from_unix_millis(started),
            channels_now,
            channels_at_start: Some(&at_start),
            channels_yml: yml.map(|f| ChannelsYml {
                path: &f.path,
                changed_at,
            }),
            folders: &seen,
        };
        Ok(c11_config_server_restart_required(&facts, &self.limits))
    }

    /// Everything the checks say about a swimlane now, as rows.
    async fn compute(
        &self,
        gate: &Gate,
        out: &mut LaneOutcome,
    ) -> Result<BTreeMap<FindingKey, FindingRow>, RegistryError> {
        let swimlane = &gate.swimlane;
        let files = inputs::files(&self.db, swimlane).await?;
        out.files = files.len();
        let tenants = self.tenants(swimlane).await?;

        // The channels the swimlane lists now (`channels.yml` at the root).
        let channels_now: Arc<Vec<ChannelName>> =
            match files.iter().find(|f| f.path.as_str() == CHANNELS_FILE) {
                Some(f) if !is_denied(&self.deny, &f.path, f.denied, f.class) => {
                    self.channels_in(&f.blob, out).await?
                }
                _ => Arc::new(Vec::new()),
            };

        let mut results: Vec<CheckResult> = self
            .c9(swimlane, &files, &tenants, &channels_now, out)
            .as_ref()
            .clone();

        // C10 needs the Deployments and who reads each file; with no tenant there is nothing to check.
        if !tenants.is_empty() {
            let services = inputs::services(&self.db, swimlane).await?;
            let mappings = self.mappings(swimlane).await?;
            let mut apps: Vec<domain::AppName> = services.iter().filter_map(|s| s.app.clone()).collect();
            apps.sort();
            apps.dedup();
            let readers: Vec<Reader<'_>> = services
                .iter()
                .map(|s| Reader {
                    namespace: &s.namespace,
                    name: &s.name,
                    app: s.app.as_ref(),
                })
                .collect();
            let directory = Directory::new(&readers, &mappings);
            let ctx = c10::Ctx {
                db: &self.db,
                blobs: &*self.blobs,
                cache: &self.cache,
                limits: &self.limits,
                deny: &self.deny,
                blob_timeout: self.config.blob_timeout,
                batch_bytes: self.config.structure_batch_bytes,
            };
            let input = c10::Input {
                swimlane,
                files: &files,
                tenants: &tenants,
                apps: &apps,
                services: &services,
                directory: &directory,
                channels: &channels_now,
            };
            let c10 = c10::run(&ctx, &input).await?;
            out.c10_files = c10.files;
            out.c10_evaluated = c10.evaluated;
            out.c10_cached = c10.cached;
            out.blobs_read += c10.blobs_read;
            results.extend(c10.results);
        }

        results.extend(self.c11(swimlane, gate, &files, &channels_now, out).await?);
        if results.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rules = self.severity.current().await?;
        let by_path: BTreeMap<&NfsPath, &FileRow> = files.iter().map(|f| (&f.path, f)).collect();
        let mut rows = BTreeMap::new();
        for result in &results {
            let file = result
                .path
                .as_ref()
                .and_then(|p| by_path.get(p))
                .map(|f| (f.state, f.class));
            let fingerprint = fingerprint_keyed(result.kind, &identity_of(result));
            let path = result.path.as_ref().map(|p| p.as_str().to_owned());
            rows.insert(
                (result.kind, fingerprint.clone()),
                row_from_result(&rules.rules, result, file, path, fingerprint),
            );
        }
        Ok(rows)
    }

    /// Make the stored findings of the structure kinds say what `rows` say ([`findings_store::reconcile`]). Nothing is written, and no
    /// transaction opened, when they already do. Returns the version the swimlane's row has afterwards, when this wrote.
    async fn write(
        &self,
        swimlane: &SwimlaneId,
        rows: &BTreeMap<FindingKey, FindingRow>,
        out: &mut LaneOutcome,
    ) -> Result<Option<i64>, RegistryError> {
        let store = Store {
            db: &self.db,
            bus: &*self.bus,
            clock: &*self.clock,
            rows: self.config.rows_per_statement,
        };
        let kinds = structure_kind_names();
        let done = reconcile(&store, swimlane, &kinds, |_| rows.clone()).await?;
        out.inserted = done.inserted;
        out.updated = done.updated;
        out.resolved = done.resolved;
        Ok(done.version)
    }
}

/// What names a result among the others of its kind: the file name of a C9, the path of a C10, the reason of a C11.
fn identity_of(result: &CheckResult) -> String {
    match &result.detail {
        CheckDetail::AmbiguousFileName { name, .. } => name.clone(),
        CheckDetail::ConfigServerRestart { reason, .. } => format!("{reason:?}"),
        _ => result
            .path
            .as_ref()
            .map_or_else(String::new, |p| p.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use domain::FindingKind;

    use super::*;

    #[test]
    fn the_channels_are_the_entries_of_the_list_that_are_names() {
        let limits = Limits::default();
        let names = |text: &str| -> Vec<String> {
            channels_of(text.as_bytes(), &limits)
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        assert_eq!(
            names("channels:\n  - web\n  - atm-iso\n  - web\n"),
            ["atm-iso", "web"]
        );
        assert!(names("other:\n  - web\n").is_empty());
        assert!(names("channels: [unclosed").is_empty());
        assert_eq!(
            names("channels:\n  - \"not a name!\"\n  - ok\n"),
            ["ok"],
            "an entry that is not a channel name is left out"
        );
    }

    proptest::proptest! {
        /// `channels.yml` is a file of the tenant's repository: whatever its bytes are, reading it neither panics nor returns more than
        /// the bound, and what it returns is sorted, without repeats, and all names.
        #[test]
        fn whatever_the_bytes_of_channels_yml_are_the_channels_are_bounded_sorted_and_names(
            bytes in proptest::collection::vec(proptest::arbitrary::any::<u8>(), 0..4096),
        ) {
            let names = channels_of(&bytes, &Limits::default());
            proptest::prop_assert!(names.len() <= MAX_CHANNELS);
            proptest::prop_assert!(names.windows(2).all(|w| w[0] < w[1]));
        }

        #[test]
        fn a_list_of_many_names_is_cut_at_the_bound(count in 0usize..(MAX_CHANNELS * 3)) {
            let text = (0..count).fold(String::from("channels:\n"), |mut text, i| {
                text.push_str("  - c");
                text.push_str(&i.to_string());
                text.push('\n');
                text
            });
            let names = channels_of(text.as_bytes(), &Limits::default());
            proptest::prop_assert_eq!(names.len(), count.min(MAX_CHANNELS));
        }
    }

    #[test]
    fn every_structure_kind_has_an_identity_of_its_own_that_does_not_move_with_the_others() {
        let a = fingerprint_keyed(FindingKind::AmbiguousFileName, "x.yaml");
        assert_eq!(a, fingerprint_keyed(FindingKind::AmbiguousFileName, "x.yaml"));
        assert_ne!(a, fingerprint_keyed(FindingKind::AmbiguousFileName, "y.yaml"));
        assert_ne!(a, fingerprint_keyed(FindingKind::UnresolvedPlaceholder, "x.yaml"));
        assert_eq!(structure_kind_names().len(), 3);
    }

    #[test]
    fn the_statements_name_every_bind_they_use() {
        let gates = inputs::gates_sql();
        assert!(gates.contains("$2") && !gates.contains("$3") && !gates.contains("OFFSET"));
        for sql in [
            inputs::FILES_SQL,
            inputs::SERVICES_SQL,
            inputs::FIRST_OBSERVATION_SQL,
        ] {
            assert!(
                !sql.contains("OFFSET") && sql.contains("LIMIT") || sql.contains("$2"),
                "{sql}"
            );
        }
    }
}
