//! Prometheus metrics for `/metrics` (T7): how long scans take, how many files are tracked, how big deltas are, whether
//! the agent is connected, and how many operations ended in which way.
//!
//! Every label value comes from a closed enum below. A path, a request id, a swimlane or a Deployment name never becomes
//! a label (S10, and Prometheus would grow a series per value), so the series count is fixed at start-up and the
//! exposition has the same size however long the agent runs.

use std::sync::Arc;
use std::time::Duration;

use domain::OpError;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue, text};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

use crate::transport::session::ConnectionState;

/// The kind of walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ScanKind {
    /// Only files whose stat changed are read.
    Stat,
    /// Every file is read and hashed again.
    Full,
}

impl ScanKind {
    const ALL: [Self; 2] = [Self::Stat, Self::Full];
}

/// How a walk ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ScanResult {
    /// The tree was updated.
    Done,
    /// The walk failed; the tree is unchanged.
    Failed,
    /// The root listed as empty and the tree was kept (a dropped mount is not a mass deletion).
    Held,
}

impl ScanResult {
    const ALL: [Self; 3] = [Self::Done, Self::Failed, Self::Held];
}

/// What the hub asked of the agent, one value per command that has an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum Operation {
    Read,
    Write,
    Delete,
    ClusterReport,
    Restart,
    Notify,
    FetchServed,
}

impl Operation {
    const ALL: [Self; 7] = [
        Self::Read,
        Self::Write,
        Self::Delete,
        Self::ClusterReport,
        Self::Restart,
        Self::Notify,
        Self::FetchServed,
    ];
}

/// How an operation ended. The five error codes of the wire, and success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum Outcome {
    Ok,
    Conflict,
    NotFound,
    Denied,
    Unsupported,
    Io,
}

impl Outcome {
    const ALL: [Self; 6] = [
        Self::Ok,
        Self::Conflict,
        Self::NotFound,
        Self::Denied,
        Self::Unsupported,
        Self::Io,
    ];

    pub fn of(error: Option<OpError>) -> Self {
        match error {
            None => Self::Ok,
            Some(OpError::Conflict) => Self::Conflict,
            Some(OpError::NotFound) => Self::NotFound,
            Some(OpError::Denied) => Self::Denied,
            Some(OpError::Unsupported) => Self::Unsupported,
            Some(OpError::Io) => Self::Io,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
enum State {
    Disconnected,
    Connecting,
    Connected,
}

impl From<ConnectionState> for State {
    fn from(state: ConnectionState) -> Self {
        match state {
            ConnectionState::Disconnected => Self::Disconnected,
            ConnectionState::Connecting => Self::Connecting,
            ConnectionState::Connected => Self::Connected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ScanKindLabel {
    kind: ScanKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ScanLabels {
    kind: ScanKind,
    result: ScanResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct OperationLabels {
    operation: Operation,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct StateLabel {
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
enum Attempt {
    Established,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct AttemptLabel {
    result: Attempt,
}

/// The agent's metrics and the registry that renders them. Cheap to share; every method takes `&self`.
#[derive(Debug)]
pub struct Metrics {
    registry: Registry,
    scan_seconds: Family<ScanKindLabel, Histogram>,
    scans: Family<ScanLabels, Counter>,
    files_tracked: Gauge,
    delta_entries: Histogram,
    delta_bytes: Histogram,
    connection_state: Family<StateLabel, Gauge>,
    connections: Family<AttemptLabel, Counter>,
    operations: Family<OperationLabels, Counter>,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        let mut registry = Registry::default();
        // 5 ms to about 80 s: a stat walk of 2,000 files is milliseconds, a full rehash of a big tree is seconds.
        let scan_seconds = Family::<ScanKindLabel, Histogram>::new_with_constructor(|| {
            Histogram::new(exponential_buckets(0.005, 2.0, 15))
        });
        let scans = Family::<ScanLabels, Counter>::default();
        let files_tracked = Gauge::default();
        // 1 to 16,384 entries, and 1 KiB to 4 MiB (the largest message).
        let delta_entries = Histogram::new(exponential_buckets(1.0, 4.0, 8));
        let delta_bytes = Histogram::new(exponential_buckets(1024.0, 4.0, 7));
        let connection_state = Family::<StateLabel, Gauge>::default();
        let connections = Family::<AttemptLabel, Counter>::default();
        let operations = Family::<OperationLabels, Counter>::default();

        registry.register(
            "lanekeeper_agent_scan_duration_seconds",
            "How long a walk of the NFS root took",
            scan_seconds.clone(),
        );
        registry.register(
            "lanekeeper_agent_scans",
            "Walks of the NFS root, by kind and how they ended",
            scans.clone(),
        );
        registry.register(
            "lanekeeper_agent_files_tracked",
            "Files in the Merkle tree",
            files_tracked.clone(),
        );
        registry.register(
            "lanekeeper_agent_delta_entries",
            "Files in one scan delta message",
            delta_entries.clone(),
        );
        registry.register(
            "lanekeeper_agent_delta_bytes",
            "File content bytes in one scan delta message",
            delta_bytes.clone(),
        );
        registry.register(
            "lanekeeper_agent_connection_state",
            "1 for the state the hub connection is in, 0 for the others",
            connection_state.clone(),
        );
        registry.register(
            "lanekeeper_agent_connection_attempts",
            "Attempts to connect to the hub, by whether the hub's configuration arrived",
            connections.clone(),
        );
        registry.register(
            "lanekeeper_agent_operations",
            "Hub commands carried out, by operation and outcome",
            operations.clone(),
        );

        let metrics = Self {
            registry,
            scan_seconds,
            scans,
            files_tracked,
            delta_entries,
            delta_bytes,
            connection_state,
            connections,
            operations,
        };
        metrics.set_connection(ConnectionState::Disconnected);
        metrics.create_every_series();
        Arc::new(metrics)
    }

    /// Every label combination exists from the start, at zero. The exposition then has the same lines for as long as the
    /// agent runs, an alert can compare with zero, and a scrape never shows a series appearing out of nowhere.
    fn create_every_series(&self) {
        for kind in ScanKind::ALL {
            let _ = self.scan_seconds.get_or_create(&ScanKindLabel { kind });
            for result in ScanResult::ALL {
                let _ = self.scans.get_or_create(&ScanLabels { kind, result });
            }
        }
        for operation in Operation::ALL {
            for outcome in Outcome::ALL {
                let _ = self
                    .operations
                    .get_or_create(&OperationLabels { operation, outcome });
            }
        }
        for result in [Attempt::Established, Attempt::Failed] {
            let _ = self.connections.get_or_create(&AttemptLabel { result });
        }
    }

    /// Metrics nobody scrapes, for a component built without a registry (tests, benchmarks).
    pub fn detached() -> Arc<Self> {
        Self::new()
    }

    pub fn scan(&self, kind: ScanKind, result: ScanResult, took: Duration) {
        self.scans.get_or_create(&ScanLabels { kind, result }).inc();
        if result == ScanResult::Done {
            self.scan_seconds
                .get_or_create(&ScanKindLabel { kind })
                .observe(took.as_secs_f64());
        }
    }

    pub fn files_tracked(&self, files: u64) {
        self.files_tracked.set(i64::try_from(files).unwrap_or(i64::MAX));
    }

    /// One delta message: how many files it carries and how many content bytes.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a histogram sample; 2^53 bytes is not reachable"
    )]
    pub fn delta(&self, entries: usize, bytes: u64) {
        self.delta_entries.observe(entries as f64);
        self.delta_bytes.observe(bytes as f64);
    }

    pub fn set_connection(&self, state: ConnectionState) {
        let current = State::from(state);
        for each in [State::Disconnected, State::Connecting, State::Connected] {
            self.connection_state
                .get_or_create(&StateLabel { state: each })
                .set(i64::from(each == current));
        }
    }

    pub fn connection_attempt(&self, established: bool) {
        let result = if established {
            Attempt::Established
        } else {
            Attempt::Failed
        };
        self.connections.get_or_create(&AttemptLabel { result }).inc();
    }

    pub fn operation(&self, operation: Operation, outcome: Outcome) {
        self.operations
            .get_or_create(&OperationLabels { operation, outcome })
            .inc();
    }

    /// The Prometheus text exposition.
    pub fn render(&self) -> String {
        let mut out = String::new();
        // Writing to a String cannot fail.
        if text::encode(&mut out, &self.registry).is_err() {
            out.clear();
        }
        out
    }
}

#[cfg(test)]
// Counter and gauge values are small whole numbers, which f64 holds exactly.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn value_of(text: &str, series: &str) -> Option<f64> {
        text.lines()
            .find(|l| l.starts_with(series) && l[series.len()..].starts_with(' '))
            .and_then(|l| l[series.len()..].trim().parse().ok())
    }

    #[test]
    fn a_scan_is_counted_by_kind_and_result_and_timed() {
        let metrics = Metrics::new();
        metrics.scan(ScanKind::Stat, ScanResult::Done, Duration::from_millis(40));
        metrics.scan(ScanKind::Stat, ScanResult::Failed, Duration::from_millis(900));
        let text = metrics.render();
        assert_eq!(
            value_of(
                &text,
                "lanekeeper_agent_scans_total{kind=\"Stat\",result=\"Done\"}"
            ),
            Some(1.0)
        );
        assert_eq!(
            value_of(
                &text,
                "lanekeeper_agent_scans_total{kind=\"Stat\",result=\"Failed\"}"
            ),
            Some(1.0)
        );
        // Only a walk that finished is timed: a failure's duration says nothing about the walk.
        assert_eq!(
            value_of(
                &text,
                "lanekeeper_agent_scan_duration_seconds_count{kind=\"Stat\"}"
            ),
            Some(1.0)
        );
    }

    #[test]
    fn connection_state_has_exactly_one_state_set() {
        let metrics = Metrics::new();
        metrics.set_connection(ConnectionState::Connected);
        let text = metrics.render();
        let set: Vec<f64> = ["Disconnected", "Connecting", "Connected"]
            .iter()
            .map(|s| {
                value_of(
                    &text,
                    &format!("lanekeeper_agent_connection_state{{state=\"{s}\"}}"),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(set, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn every_error_code_has_an_outcome() {
        use OpError::{Conflict, Denied, Io, NotFound, Unsupported};
        let outcomes: Vec<Outcome> = [
            None,
            Some(Conflict),
            Some(NotFound),
            Some(Denied),
            Some(Unsupported),
            Some(Io),
        ]
        .into_iter()
        .map(Outcome::of)
        .collect();
        assert_eq!(
            outcomes,
            [
                Outcome::Ok,
                Outcome::Conflict,
                Outcome::NotFound,
                Outcome::Denied,
                Outcome::Unsupported,
                Outcome::Io
            ]
        );
    }

    #[test]
    fn the_exposition_does_not_grow_with_use() {
        let metrics = Metrics::new();
        for _ in 0..1_000 {
            metrics.operation(Operation::Write, Outcome::Conflict);
            metrics.delta(3, 4096);
        }
        let first = metrics.render().lines().count();
        for _ in 0..1_000 {
            metrics.operation(Operation::Write, Outcome::Conflict);
            metrics.delta(3, 4096);
        }
        assert_eq!(metrics.render().lines().count(), first);
    }
}
