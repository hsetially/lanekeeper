//! The registry's metrics (S10): rates and durations with labels from closed sets. A swimlane id, a path, a hash or a
//! file's content is never a label; the numbers say how much, the logs say which.

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

/// The message of an agent that was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum MessageKind {
    Hello,
    Heartbeat,
    Delta,
    Cluster,
}

/// How a message ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum Outcome {
    Ok,
    /// A part of a delta was staged and acknowledged; the delta is not applied yet.
    Staged,
    HelloRequired,
    IdentityMismatch,
    Invalid,
    TooLarge,
    Unavailable,
}

/// What became of one file version of a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum VersionResult {
    /// A new current version of a file: the state moved.
    Recorded,
    /// Recorded in the history only: an older version than the stored one.
    HistoryOnly,
    /// Equal to the current version, so nothing to do.
    Unchanged,
    /// Already recorded by an earlier delivery (a replay).
    Replayed,
    /// A removal that moved the state.
    Removed,
}

/// What became of one blob offered to the settings index (T2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum IndexResult {
    /// Parsed, and its rows and marker written.
    Indexed,
    /// Did not parse (or is over a limit): marked, with no rows, so it is parsed once and not again.
    Unparseable,
    /// Had its marker already: nothing to do.
    AlreadyIndexed,
    /// The blob store does not have it.
    Missing,
    /// Could not be indexed now (the database or the pool failed); no marker, so the sweeper tries again.
    Failed,
    /// Offered to a full queue and dropped; the sweeper finds it.
    Dropped,
    /// Already waiting in the queue or being indexed on this replica.
    Duplicate,
}

/// What the Git index did with a moved head (T2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum MoveResult {
    /// Some swimlane has paths to recompute.
    Affected,
    /// The branch is not deployed by any swimlane (a template, the docs branch, an unknown branch) or nothing changed.
    Unconcerned,
    /// More paths than `max_affected_paths`: the swimlanes are recomputed whole.
    Whole,
}

/// A lookup in the cache of normalised Git hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum CacheResult {
    Hit,
    Miss,
}

/// What the pipeline did with a swimlane in one batch (T3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum LaneResult {
    /// Only the affected paths were recomputed.
    Incremental,
    /// The whole swimlane was loaded and computed (first sight, a rules change, too many paths, a nightly run).
    Whole,
    /// The swimlane has no inventory yet, so there is nothing to compare.
    NoInventory,
    /// The batch for the swimlane failed; its memory is dropped and the next batch starts from the database.
    Failed,
}

/// An in-process signal to the pipeline (T3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SignalResult {
    Accepted,
    /// The queue was full: the reconcile finds the change.
    Dropped,
}

/// How a `record_sync` ended (T4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SyncResult {
    /// Baselines set, the deployment recorded.
    Recorded,
    /// The swimlane has no confirmed baseline yet: only the deployment was recorded.
    DeploymentOnly,
    /// The report named a swimlane, branch, tenant or commit the hub does not accept.
    Rejected,
    /// The agent is not connected, so no scan could be asked for.
    AgentNotConnected,
    /// The scan was asked for and not applied in time.
    ScanTimeout,
    /// The database, the mirror, the audit log or the outbox failed.
    Failed,
}

/// What a tenant-suffix review did (T4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RematchResult {
    /// A new review was raised.
    Raised,
    /// The open review was refreshed with the new suffixes.
    Refreshed,
    /// The same suffixes were seen before: nothing to do.
    Unchanged,
    /// An admin is working on the open review, so it was left as it is; the next batch looks again.
    Deferred,
    /// The ranking could not be made (the mirror, the database).
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct SyncLabels {
    result: SyncResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RematchLabels {
    result: RematchResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct LaneLabels {
    result: LaneResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct SignalLabels {
    result: SignalResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct IndexLabels {
    result: IndexResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct MoveLabels {
    result: MoveResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct CacheLabels {
    result: CacheResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct MessageLabels {
    kind: MessageKind,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct VersionLabels {
    result: VersionResult,
}

/// Cloning shares the counters.
#[derive(Debug, Clone)]
pub struct RegistryMetrics {
    messages: Family<MessageLabels, Counter>,
    versions: Family<VersionLabels, Counter>,
    out_of_band: Counter,
    streams: Gauge,
    skipped_entries: Counter,
    dropped_env: Counter,
    apply_seconds: Histogram,
    index_blobs: Family<IndexLabels, Counter>,
    index_rows: Counter,
    index_redacted: Counter,
    index_queue: Gauge,
    index_seconds: Histogram,
    head_moves: Family<MoveLabels, Counter>,
    git_hashes: Family<CacheLabels, Counter>,
    tree_rows: Counter,
    pipeline_lanes: Family<LaneLabels, Counter>,
    pipeline_paths: Counter,
    pipeline_rows: Counter,
    pipeline_findings: Counter,
    pipeline_drift_paths: Counter,
    pipeline_baselines: Counter,
    pipeline_marks_cleared: Counter,
    pipeline_divergences: Counter,
    pipeline_reconciles: Counter,
    pipeline_signals: Family<SignalLabels, Counter>,
    pipeline_lanes_in_memory: Gauge,
    pipeline_seconds: Histogram,
    syncs: Family<SyncLabels, Counter>,
    sync_overwritten: Counter,
    sync_baselines: Counter,
    sync_seconds: Histogram,
    rematches: Family<RematchLabels, Counter>,
}

impl Default for RegistryMetrics {
    fn default() -> Self {
        Self {
            messages: Family::default(),
            versions: Family::default(),
            out_of_band: Counter::default(),
            streams: Gauge::default(),
            skipped_entries: Counter::default(),
            dropped_env: Counter::default(),
            // 1 ms to about 8 s.
            apply_seconds: Histogram::new(exponential_buckets(0.001, 2.0, 14)),
            index_blobs: Family::default(),
            index_rows: Counter::default(),
            index_redacted: Counter::default(),
            index_queue: Gauge::default(),
            // 1 ms to about 8 s per batch of blobs.
            index_seconds: Histogram::new(exponential_buckets(0.001, 2.0, 14)),
            head_moves: Family::default(),
            git_hashes: Family::default(),
            tree_rows: Counter::default(),
            pipeline_lanes: Family::default(),
            pipeline_paths: Counter::default(),
            pipeline_rows: Counter::default(),
            pipeline_findings: Counter::default(),
            pipeline_drift_paths: Counter::default(),
            pipeline_baselines: Counter::default(),
            pipeline_marks_cleared: Counter::default(),
            pipeline_divergences: Counter::default(),
            pipeline_reconciles: Counter::default(),
            pipeline_signals: Family::default(),
            pipeline_lanes_in_memory: Gauge::default(),
            // 1 ms to about 8 s; a whole swimlane takes seconds, an incremental batch milliseconds.
            pipeline_seconds: Histogram::new(exponential_buckets(0.001, 2.0, 14)),
            syncs: Family::default(),
            sync_overwritten: Counter::default(),
            sync_baselines: Counter::default(),
            // 10 ms to about 80 s: most of a sync's time is the agent's scan.
            sync_seconds: Histogram::new(exponential_buckets(0.01, 2.0, 14)),
            rematches: Family::default(),
        }
    }
}

impl RegistryMetrics {
    #[allow(clippy::too_many_lines)] // one registration per metric
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "registry_ingest_messages",
            "Messages from agents handled, by kind and outcome",
            self.messages.clone(),
        );
        registry.register(
            "registry_ingest_versions",
            "File versions of scan deltas, by what became of them",
            self.versions.clone(),
        );
        registry.register(
            "registry_ingest_out_of_band",
            "Changes on NFS that no tool write and no sync explains",
            self.out_of_band.clone(),
        );
        registry.register(
            "registry_ingest_streams",
            "Swimlanes that said hello on this replica",
            self.streams.clone(),
        );
        registry.register(
            "registry_ingest_skipped_entries",
            "Files the agent skipped (too large to read)",
            self.skipped_entries.clone(),
        );
        registry.register(
            "registry_ingest_dropped_env_values",
            "Environment values dropped because the variable is not on the allowlist",
            self.dropped_env.clone(),
        );
        registry.register(
            "registry_ingest_apply_seconds",
            "Time to apply one logical scan delta",
            self.apply_seconds.clone(),
        );
        registry.register(
            "registry_index_blobs",
            "Blobs offered to the settings index, by what became of them",
            self.index_blobs.clone(),
        );
        registry.register(
            "registry_index_rows",
            "Settings rows written to the settings index",
            self.index_rows.clone(),
        );
        registry.register(
            "registry_index_redacted_values",
            "Setting values the secret scan flagged, stored redacted",
            self.index_redacted.clone(),
        );
        registry.register(
            "registry_index_queue",
            "Blob hashes waiting for the settings index on this replica",
            self.index_queue.clone(),
        );
        registry.register(
            "registry_index_batch_seconds",
            "Time to index one batch of blobs",
            self.index_seconds.clone(),
        );
        registry.register(
            "registry_git_head_moves",
            "Moved Git heads handled, by outcome",
            self.head_moves.clone(),
        );
        registry.register(
            "registry_git_hash_cache",
            "Lookups of normalised Git hashes, by hit or miss",
            self.git_hashes.clone(),
        );
        registry.register(
            "registry_tree_rows_written",
            "Rows of effective_tree_hashes and effective_files written or deleted",
            self.tree_rows.clone(),
        );
        registry.register(
            "registry_pipeline_lanes",
            "Swimlanes handled by the pipeline, by how much of each was recomputed",
            self.pipeline_lanes.clone(),
        );
        registry.register(
            "registry_pipeline_paths",
            "Affected paths the pipeline recomputed",
            self.pipeline_paths.clone(),
        );
        registry.register(
            "registry_pipeline_rows_written",
            "file_state rows the pipeline wrote or deleted",
            self.pipeline_rows.clone(),
        );
        registry.register(
            "registry_pipeline_findings_written",
            "Findings the pipeline opened, changed or resolved",
            self.pipeline_findings.clone(),
        );
        registry.register(
            "registry_pipeline_drift_paths",
            "Paths named in DriftChanged events",
            self.pipeline_drift_paths.clone(),
        );
        registry.register(
            "registry_pipeline_baselines_set",
            "Baselines set because NFS and Git converged",
            self.pipeline_baselines.clone(),
        );
        registry.register(
            "registry_pipeline_marks_cleared",
            "Intentional-divergence marks cleared because the file moved on",
            self.pipeline_marks_cleared.clone(),
        );
        registry.register(
            "registry_pipeline_divergences",
            "Paths where a full recompute or the reconcile found the stored state wrong and repaired it",
            self.pipeline_divergences.clone(),
        );
        registry.register(
            "registry_pipeline_reconciles",
            "Reconcile scans run",
            self.pipeline_reconciles.clone(),
        );
        registry.register(
            "registry_pipeline_signals",
            "In-process signals to the pipeline, by outcome",
            self.pipeline_signals.clone(),
        );
        registry.register(
            "registry_pipeline_lanes_in_memory",
            "Swimlane states the pipeline holds in memory",
            self.pipeline_lanes_in_memory.clone(),
        );
        registry.register(
            "registry_pipeline_batch_seconds",
            "Time to bring one batch of swimlanes up to date",
            self.pipeline_seconds.clone(),
        );
        registry.register(
            "registry_sync_reports",
            "Sync reports handled by record_sync, by how they ended",
            self.syncs.clone(),
        );
        registry.register(
            "registry_sync_overwritten_changes",
            "Changes made only on NFS that a sync replaced (C7)",
            self.sync_overwritten.clone(),
        );
        registry.register(
            "registry_sync_baselines_set",
            "Baselines set by a sync report because NFS equals Git at the reported commits",
            self.sync_baselines.clone(),
        );
        registry.register(
            "registry_sync_seconds",
            "Time to record one sync report, including the wait for the scan",
            self.sync_seconds.clone(),
        );
        registry.register(
            "registry_tenant_rematches",
            "Reviews raised or refreshed because the tenant suffixes on NFS changed, by outcome",
            self.rematches.clone(),
        );
    }

    pub(crate) fn message(&self, kind: MessageKind, outcome: Outcome) {
        self.messages
            .get_or_create(&MessageLabels { kind, outcome })
            .inc();
    }

    pub(crate) fn version(&self, result: VersionResult, n: u64) {
        if n > 0 {
            self.versions.get_or_create(&VersionLabels { result }).inc_by(n);
        }
    }

    pub(crate) fn out_of_band(&self, n: u64) {
        if n > 0 {
            self.out_of_band.inc_by(n);
        }
    }

    pub(crate) fn streams(&self, n: usize) {
        self.streams.set(i64::try_from(n).unwrap_or(i64::MAX));
    }

    pub(crate) fn skipped(&self, n: usize) {
        if n > 0 {
            self.skipped_entries.inc_by(n as u64);
        }
    }

    pub(crate) fn dropped_env(&self, n: usize) {
        if n > 0 {
            self.dropped_env.inc_by(n as u64);
        }
    }

    pub(crate) fn applied(&self, seconds: f64) {
        self.apply_seconds.observe(seconds);
    }

    pub(crate) fn index_blob(&self, result: IndexResult, n: u64) {
        if n > 0 {
            self.index_blobs.get_or_create(&IndexLabels { result }).inc_by(n);
        }
    }

    pub(crate) fn index_rows(&self, n: u64) {
        if n > 0 {
            self.index_rows.inc_by(n);
        }
    }

    pub(crate) fn index_redacted(&self, n: u64) {
        if n > 0 {
            self.index_redacted.inc_by(n);
        }
    }

    pub(crate) fn index_queue(&self, n: usize) {
        self.index_queue.set(i64::try_from(n).unwrap_or(i64::MAX));
    }

    pub(crate) fn index_batch_seconds(&self, seconds: f64) {
        self.index_seconds.observe(seconds);
    }

    pub(crate) fn head_move(&self, result: MoveResult) {
        self.head_moves.get_or_create(&MoveLabels { result }).inc();
    }

    pub(crate) fn git_hash(&self, result: CacheResult) {
        self.git_hashes.get_or_create(&CacheLabels { result }).inc();
    }

    pub(crate) fn tree_rows(&self, n: u64) {
        if n > 0 {
            self.tree_rows.inc_by(n);
        }
    }

    pub(crate) fn pipeline_lane(&self, result: LaneResult) {
        self.pipeline_lanes.get_or_create(&LaneLabels { result }).inc();
    }

    pub(crate) fn pipeline_paths(&self, n: u64) {
        if n > 0 {
            self.pipeline_paths.inc_by(n);
        }
    }

    pub(crate) fn pipeline_rows(&self, n: u64) {
        if n > 0 {
            self.pipeline_rows.inc_by(n);
        }
    }

    pub(crate) fn pipeline_findings(&self, n: u64) {
        if n > 0 {
            self.pipeline_findings.inc_by(n);
        }
    }

    pub(crate) fn pipeline_drift_paths(&self, n: u64) {
        if n > 0 {
            self.pipeline_drift_paths.inc_by(n);
        }
    }

    pub(crate) fn pipeline_baselines(&self, n: u64) {
        if n > 0 {
            self.pipeline_baselines.inc_by(n);
        }
    }

    pub(crate) fn pipeline_marks_cleared(&self, n: u64) {
        if n > 0 {
            self.pipeline_marks_cleared.inc_by(n);
        }
    }

    pub(crate) fn pipeline_divergences(&self, n: u64) {
        if n > 0 {
            self.pipeline_divergences.inc_by(n);
        }
    }

    pub(crate) fn pipeline_reconcile(&self) {
        self.pipeline_reconciles.inc();
    }

    pub(crate) fn pipeline_signal(&self, result: SignalResult) {
        self.pipeline_signals
            .get_or_create(&SignalLabels { result })
            .inc();
    }

    pub(crate) fn pipeline_lanes_in_memory(&self, n: usize) {
        self.pipeline_lanes_in_memory
            .set(i64::try_from(n).unwrap_or(i64::MAX));
    }

    pub(crate) fn pipeline_batch_seconds(&self, seconds: f64) {
        self.pipeline_seconds.observe(seconds);
    }

    pub(crate) fn sync(&self, result: SyncResult, seconds: f64) {
        self.syncs.get_or_create(&SyncLabels { result }).inc();
        self.sync_seconds.observe(seconds);
    }

    pub(crate) fn sync_overwritten(&self, n: u64) {
        if n > 0 {
            self.sync_overwritten.inc_by(n);
        }
    }

    pub(crate) fn sync_baselines(&self, n: u64) {
        if n > 0 {
            self.sync_baselines.inc_by(n);
        }
    }

    pub(crate) fn rematch(&self, result: RematchResult) {
        self.rematches.get_or_create(&RematchLabels { result }).inc();
    }

    /// Sync reports by how they ended, for tests.
    pub fn sync_count(&self, result: SyncResult) -> u64 {
        self.syncs.get_or_create(&SyncLabels { result }).get()
    }

    /// Changes a sync overwrote (C7), for tests.
    pub fn sync_overwritten_count(&self) -> u64 {
        self.sync_overwritten.get()
    }

    /// Baselines set by sync reports, for tests.
    pub fn sync_baselines_count(&self) -> u64 {
        self.sync_baselines.get()
    }

    /// Tenant-suffix reviews by outcome, for tests.
    pub fn rematch_count(&self, result: RematchResult) -> u64 {
        self.rematches.get_or_create(&RematchLabels { result }).get()
    }

    /// Swimlanes handled by the pipeline, by result, for tests.
    pub fn pipeline_lane_count(&self, result: LaneResult) -> u64 {
        self.pipeline_lanes.get_or_create(&LaneLabels { result }).get()
    }

    /// Paths recomputed by the pipeline, for tests.
    pub fn pipeline_paths_count(&self) -> u64 {
        self.pipeline_paths.get()
    }

    /// `file_state` rows written or deleted by the pipeline, for tests.
    pub fn pipeline_rows_count(&self) -> u64 {
        self.pipeline_rows.get()
    }

    /// Paths named in `DriftChanged` events, for tests.
    pub fn pipeline_drift_paths_count(&self) -> u64 {
        self.pipeline_drift_paths.get()
    }

    /// Baselines set by convergence, for tests.
    pub fn pipeline_baselines_count(&self) -> u64 {
        self.pipeline_baselines.get()
    }

    /// Marks cleared, for tests.
    pub fn pipeline_marks_cleared_count(&self) -> u64 {
        self.pipeline_marks_cleared.get()
    }

    /// Paths repaired by a full recompute or the reconcile, for tests.
    pub fn pipeline_divergences_count(&self) -> u64 {
        self.pipeline_divergences.get()
    }

    /// Reconcile scans, for tests.
    pub fn pipeline_reconciles_count(&self) -> u64 {
        self.pipeline_reconciles.get()
    }

    /// Signals by outcome, for tests.
    pub fn pipeline_signal_count(&self, result: SignalResult) -> u64 {
        self.pipeline_signals
            .get_or_create(&SignalLabels { result })
            .get()
    }

    /// Swimlane states in memory, for tests.
    pub fn pipeline_lanes_in_memory_count(&self) -> i64 {
        self.pipeline_lanes_in_memory.get()
    }

    /// The count of blobs by what became of them in the settings index, for tests.
    pub fn index_count(&self, result: IndexResult) -> u64 {
        self.index_blobs.get_or_create(&IndexLabels { result }).get()
    }

    /// The settings rows written, for tests.
    pub fn index_rows_count(&self) -> u64 {
        self.index_rows.get()
    }

    /// The values stored redacted, for tests.
    pub fn index_redacted_count(&self) -> u64 {
        self.index_redacted.get()
    }

    /// The moved heads handled, by outcome, for tests.
    pub fn head_move_count(&self, result: MoveResult) -> u64 {
        self.head_moves.get_or_create(&MoveLabels { result }).get()
    }

    /// Lookups of normalised Git hashes, by hit or miss, for tests.
    pub fn git_hash_count(&self, result: CacheResult) -> u64 {
        self.git_hashes.get_or_create(&CacheLabels { result }).get()
    }

    /// The rows of the effective tables written or deleted, for tests.
    pub fn tree_rows_count(&self) -> u64 {
        self.tree_rows.get()
    }

    /// The count of one message kind and outcome, for tests.
    pub fn message_count(&self, kind: MessageKind, outcome: Outcome) -> u64 {
        self.messages
            .get_or_create(&MessageLabels { kind, outcome })
            .get()
    }

    /// The count of one version result, for tests.
    pub fn version_count(&self, result: VersionResult) -> u64 {
        self.versions.get_or_create(&VersionLabels { result }).get()
    }

    /// The count of out-of-band changes, for tests.
    pub fn out_of_band_count(&self) -> u64 {
        self.out_of_band.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_count_and_register() {
        let m = RegistryMetrics::default();
        m.message(MessageKind::Delta, Outcome::Ok);
        m.message(MessageKind::Delta, Outcome::Ok);
        m.version(VersionResult::Recorded, 3);
        m.version(VersionResult::Replayed, 0);
        m.out_of_band(2);
        assert_eq!(m.message_count(MessageKind::Delta, Outcome::Ok), 2);
        assert_eq!(m.message_count(MessageKind::Delta, Outcome::Invalid), 0);
        assert_eq!(m.version_count(VersionResult::Recorded), 3);
        assert_eq!(m.version_count(VersionResult::Replayed), 0);
        assert_eq!(m.out_of_band_count(), 2);
        m.index_blob(IndexResult::Indexed, 2);
        m.index_blob(IndexResult::Dropped, 0);
        m.index_rows(5);
        m.head_move(MoveResult::Whole);
        m.git_hash(CacheResult::Hit);
        m.tree_rows(3);
        m.sync(SyncResult::Recorded, 0.5);
        m.sync_overwritten(2);
        m.sync_overwritten(0);
        m.sync_baselines(7);
        m.rematch(RematchResult::Raised);
        assert_eq!(m.sync_count(SyncResult::Recorded), 1);
        assert_eq!(m.sync_count(SyncResult::ScanTimeout), 0);
        assert_eq!(m.sync_overwritten_count(), 2);
        assert_eq!(m.sync_baselines_count(), 7);
        assert_eq!(m.rematch_count(RematchResult::Raised), 1);
        assert_eq!(m.index_count(IndexResult::Indexed), 2);
        assert_eq!(m.index_count(IndexResult::Dropped), 0);
        assert_eq!(m.index_rows_count(), 5);
        assert_eq!(m.head_move_count(MoveResult::Whole), 1);
        assert_eq!(m.git_hash_count(CacheResult::Hit), 1);
        assert_eq!(m.tree_rows_count(), 3);
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        assert!(text.contains("registry_ingest_messages_total"), "{text}");
        assert!(text.contains("registry_index_blobs_total"), "{text}");
        assert!(text.contains("registry_sync_reports_total"), "{text}");
        assert!(text.contains("registry_tenant_rematches_total"), "{text}");
        assert!(!text.contains("sit1"), "no swimlane id is ever a label");
    }
}
