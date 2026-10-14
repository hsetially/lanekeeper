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
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        assert!(text.contains("registry_ingest_messages_total"), "{text}");
        assert!(!text.contains("sit1"), "no swimlane id is ever a label");
    }
}
