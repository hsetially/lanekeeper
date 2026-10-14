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
        }
    }
}

impl RegistryMetrics {
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
        assert!(!text.contains("sit1"), "no swimlane id is ever a label");
    }
}
