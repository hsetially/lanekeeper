//! Counters this crate keeps, as plain atomics (S9).
//!
//! The hub's metrics exporter (03b) reads them through [`CounterSource`], so this crate needs no metrics library and the
//! exporter needs no knowledge of this crate's internals. Names follow the Prometheus style and carry no labels, so there is
//! nothing to blow up the cardinality.

use std::sync::atomic::{AtomicU64, Ordering};

/// One monotonically increasing counter at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub name: &'static str,
    pub help: &'static str,
    pub value: u64,
}

/// Anything that can list its counters.
pub trait CounterSource: Send + Sync {
    fn counters(&self) -> Vec<Counter>;
}

/// The counters of the identity and audit component. Share one instance (`Arc`) between the jobs.
#[derive(Debug, Default)]
pub struct IdentityMetrics {
    verify_runs: AtomicU64,
    verify_failures: AtomicU64,
    verify_errors: AtomicU64,
    checkpoints_written: AtomicU64,
    lines_shipped: AtomicU64,
}

impl IdentityMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Nightly verifications that ran to the end.
    pub fn audit_verify_runs_total(&self) -> u64 {
        self.verify_runs.load(Ordering::Relaxed)
    }

    /// Mismatches the nightly verification found (a tampered row, a broken link, a bad checkpoint). Any rise is an incident.
    pub fn audit_verify_failures_total(&self) -> u64 {
        self.verify_failures.load(Ordering::Relaxed)
    }

    /// Nightly verifications that could not run to the end (the database failed).
    pub fn audit_verify_errors_total(&self) -> u64 {
        self.verify_errors.load(Ordering::Relaxed)
    }

    pub fn audit_checkpoints_written_total(&self) -> u64 {
        self.checkpoints_written.load(Ordering::Relaxed)
    }

    pub fn audit_lines_shipped_total(&self) -> u64 {
        self.lines_shipped.load(Ordering::Relaxed)
    }

    pub(crate) fn verify_ran(&self, failures: u64) {
        self.verify_runs.fetch_add(1, Ordering::Relaxed);
        self.verify_failures.fetch_add(failures, Ordering::Relaxed);
    }

    pub(crate) fn verify_errored(&self) {
        self.verify_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn checkpoint_written(&self) {
        self.checkpoints_written.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn lines_shipped(&self, n: u64) {
        self.lines_shipped.fetch_add(n, Ordering::Relaxed);
    }
}

impl CounterSource for IdentityMetrics {
    fn counters(&self) -> Vec<Counter> {
        vec![
            Counter {
                name: "audit_verify_runs_total",
                help: "Nightly audit chain verifications that ran to the end",
                value: self.audit_verify_runs_total(),
            },
            Counter {
                name: "audit_verify_failures_total",
                help: "Mismatches found by audit chain verification; any increase is an incident",
                value: self.audit_verify_failures_total(),
            },
            Counter {
                name: "audit_verify_errors_total",
                help: "Audit chain verifications that could not run to the end",
                value: self.audit_verify_errors_total(),
            },
            Counter {
                name: "audit_checkpoints_written_total",
                help: "Signed audit checkpoints written to the bucket and the table",
                value: self.audit_checkpoints_written_total(),
            },
            Counter {
                name: "audit_lines_shipped_total",
                help: "Audit events copied to Cloud Logging",
                value: self.audit_lines_shipped_total(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_start_at_zero_and_follow_the_jobs() {
        let m = IdentityMetrics::new();
        assert!(m.counters().iter().all(|c| c.value == 0));
        m.verify_ran(3);
        m.verify_errored();
        m.checkpoint_written();
        m.lines_shipped(5);
        assert_eq!(m.audit_verify_runs_total(), 1);
        assert_eq!(m.audit_verify_failures_total(), 3);
        assert_eq!(m.audit_verify_errors_total(), 1);
        assert_eq!(m.audit_checkpoints_written_total(), 1);
        assert_eq!(m.audit_lines_shipped_total(), 5);
        let names: Vec<_> = m.counters().iter().map(|c| c.name).collect();
        assert!(names.contains(&"audit_verify_failures_total"));
    }
}
