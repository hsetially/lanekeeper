//! What wakes the pipeline (T3, D66, D80): the events of the bus, and the in-process signals of the decisions that have no event.
//!
//! Two events carry facts the pipeline reacts to: `FileObserved` (a version of a file was seen, from ingestion) and `GitHeadMoved` (from
//! the mirror). A baseline or an intentional-divergence mark changes the drift of a path too, but it is written by a request on one
//! replica and nothing is published for it, so the code that writes it (T4 and T5) tells the pipeline through a [`PipelineHandle`]:
//! "these paths of this swimlane changed", or "recompute this swimlane" (the tenant set changed, an adoption was confirmed).
//!
//! The handle is a bounded queue in this process. On a replica that does not hold the pipeline's lease nobody reads it, and a signal
//! that finds the queue full is dropped and counted: neither loses anything, because the **reconcile** compares the pipeline's state with
//! the database (baselines and marks included) and with Git every `reconcile_interval`, and recomputes what differs. A signal is the
//! fast path, and the reconcile is what makes "the incremental state equals a full recompute" true whatever the signals did.

use domain::{NfsPath, SwimlaneId};
use ports::{EventFilter, EventKind};
use tokio::sync::mpsc;

use crate::metrics::{RegistryMetrics, SignalResult};

/// The most paths one signal names; a longer list is a whole-swimlane signal.
pub const MAX_SIGNAL_PATHS: usize = 5_000;

/// A decision that changes the drift of some paths without an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Signal {
    Paths {
        swimlane: SwimlaneId,
        paths: Vec<NfsPath>,
    },
    Lane {
        swimlane: SwimlaneId,
    },
}

/// The way for the rest of the registry to tell the pipeline that a baseline, a mark or a tenant set changed. Cloning shares the queue.
#[derive(Clone)]
pub struct PipelineHandle {
    tx: mpsc::Sender<Signal>,
    metrics: RegistryMetrics,
}

impl std::fmt::Debug for PipelineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineHandle").finish_non_exhaustive()
    }
}

impl PipelineHandle {
    pub(crate) fn new(tx: mpsc::Sender<Signal>, metrics: RegistryMetrics) -> Self {
        Self { tx, metrics }
    }

    fn send(&self, signal: Signal) -> bool {
        // Never waits: a request must not be held up by the pipeline (rule 5, rule 4).
        let accepted = self.tx.try_send(signal).is_ok();
        self.metrics.pipeline_signal(if accepted {
            SignalResult::Accepted
        } else {
            SignalResult::Dropped
        });
        accepted
    }

    /// These paths of the swimlane changed (a baseline was set, a mark was made or cleared). Returns whether the signal was queued;
    /// when it was not, the reconcile finds the change.
    pub fn paths_changed(&self, swimlane: SwimlaneId, paths: Vec<NfsPath>) -> bool {
        if paths.is_empty() {
            return true;
        }
        if paths.len() > MAX_SIGNAL_PATHS {
            return self.lane_changed(swimlane);
        }
        self.send(Signal::Paths { swimlane, paths })
    }

    /// The whole swimlane has to be recomputed (its tenants changed, its baseline was confirmed).
    pub fn lane_changed(&self, swimlane: SwimlaneId) -> bool {
        self.send(Signal::Lane { swimlane })
    }
}

/// The events the pipeline subscribes to.
pub(crate) fn event_filter() -> EventFilter {
    EventFilter {
        swimlanes: None,
        kinds: [EventKind::FileObserved, EventKind::GitHeadMoved]
            .into_iter()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> SwimlaneId {
        SwimlaneId::parse("sit1").unwrap()
    }

    fn path(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    #[test]
    fn a_signal_is_queued_and_a_full_queue_drops_and_counts() {
        let (tx, mut rx) = mpsc::channel(1);
        let metrics = RegistryMetrics::default();
        let handle = PipelineHandle::new(tx, metrics.clone());
        assert!(handle.paths_changed(lane(), vec![path("a.yml")]));
        assert!(!handle.lane_changed(lane()), "the queue holds one");
        assert_eq!(metrics.pipeline_signal_count(SignalResult::Accepted), 1);
        assert_eq!(metrics.pipeline_signal_count(SignalResult::Dropped), 1);
        assert_eq!(
            rx.try_recv().unwrap(),
            Signal::Paths {
                swimlane: lane(),
                paths: vec![path("a.yml")]
            }
        );
    }

    #[test]
    fn no_paths_is_nothing_and_too_many_paths_is_the_whole_swimlane() {
        let (tx, mut rx) = mpsc::channel(4);
        let handle = PipelineHandle::new(tx, RegistryMetrics::default());
        assert!(handle.paths_changed(lane(), Vec::new()));
        assert!(rx.try_recv().is_err(), "an empty list queues nothing");
        let many: Vec<NfsPath> = (0..=MAX_SIGNAL_PATHS)
            .map(|i| path(&format!("f{i}.yml")))
            .collect();
        assert!(handle.paths_changed(lane(), many));
        assert_eq!(rx.try_recv().unwrap(), Signal::Lane { swimlane: lane() });
    }

    #[test]
    fn the_subscription_is_the_two_events_that_carry_facts() {
        let filter = event_filter();
        assert_eq!(filter.kinds.len(), 2);
        assert!(filter.kinds.contains(&EventKind::FileObserved));
        assert!(filter.kinds.contains(&EventKind::GitHeadMoved));
        assert!(filter.swimlanes.is_none());
    }
}
