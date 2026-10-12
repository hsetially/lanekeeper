//! Counters and gauges this crate keeps, as plain atomics (S9).
//!
//! The hub's metrics exporter (03b) reads them through [`CounterSource`], so this crate needs no metrics library and the
//! exporter needs no knowledge of this crate's internals. Names follow the Prometheus style and carry no labels, so there is
//! nothing to blow up the cardinality.
//!
//! # Alerting from Prometheus, independently of the in-process notifier
//!
//! The nightly verification tells the admins through the notifier ([`crate::audit::NightlyVerifyJob`]), and the notifier can be
//! down, mis-configured or silent. These series let operations alert on the same events from Prometheus, and survive a restart of
//! the pod (a counter that resets is read by `increase`, which handles resets):
//!
//! | Alert | Expression |
//! |---|---|
//! | tampering or a broken log | `increase(audit_verify_failures_total[1d]) > 0` |
//! | the tables changed shape (a blinded verifier) | `increase(audit_verify_schema_changes_total[1d]) > 0` |
//! | the verifier could not finish, twice running | `audit_verify_unfinished_runs_in_a_row >= 2` |
//! | the verifier did not finish, in a pod that restarts daily | `increase(audit_verify_errors_total[3d]) >= 2` |
//! | no verification has completed for a day and a half | `increase(audit_verify_runs_total[36h]) == 0 or absent(audit_verify_runs_total)` |
//!
//! The third is a gauge that this process holds in memory: it is lost when the pod restarts, which is why the fourth and fifth
//! exist (they read history that Prometheus keeps).

use std::sync::atomic::{AtomicU64, Ordering};

/// One monotonically increasing counter at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub name: &'static str,
    pub help: &'static str,
    pub value: u64,
}

/// A value that can go up and down, at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gauge {
    pub name: &'static str,
    pub help: &'static str,
    pub value: u64,
}

/// Anything that can list its counters (and gauges).
pub trait CounterSource: Send + Sync {
    fn counters(&self) -> Vec<Counter>;

    /// The values that go down as well as up. None by default.
    fn gauges(&self) -> Vec<Gauge> {
        Vec::new()
    }
}

/// The counters of the identity and audit component. Share one instance (`Arc`) between the jobs.
#[derive(Debug, Default)]
pub struct IdentityMetrics {
    verify_runs: AtomicU64,
    verify_failures: AtomicU64,
    verify_errors: AtomicU64,
    verify_schema_changes: AtomicU64,
    verify_unfinished_in_a_row: AtomicU64,
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

    /// Nightly verifications that found the tables changed in shape (a column dropped, renamed or retyped), so that they could not
    /// read them. Any rise is an incident: the verifier is blinded.
    pub fn audit_verify_schema_changes_total(&self) -> u64 {
        self.verify_schema_changes.load(Ordering::Relaxed)
    }

    /// Nightly verifications in a row, up to now, that ended without a verdict (the tables could not be read to the end). Zero
    /// after a run that ran to the end. A gauge held in memory by the replica that last ran the job, so a restart resets it.
    pub fn audit_verify_unfinished_runs_in_a_row(&self) -> u64 {
        self.verify_unfinished_in_a_row.load(Ordering::Relaxed)
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

    /// A run that could not finish still found these: they are counted, though the run is not (it is counted as an error).
    pub(crate) fn verify_found(&self, failures: u64) {
        self.verify_failures.fetch_add(failures, Ordering::Relaxed);
    }

    pub(crate) fn verify_errored(&self) {
        self.verify_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn verify_schema_changed(&self) {
        self.verify_schema_changes.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn set_verify_unfinished_in_a_row(&self, runs: u64) {
        self.verify_unfinished_in_a_row.store(runs, Ordering::Relaxed);
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
                name: "audit_verify_schema_changes_total",
                help: "Audit chain verifications that found the audit tables changed in shape and could not read them; any increase is an incident",
                value: self.audit_verify_schema_changes_total(),
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

    fn gauges(&self) -> Vec<Gauge> {
        vec![Gauge {
            name: "audit_verify_unfinished_runs_in_a_row",
            help: "Nightly audit chain verifications in a row that ended without a verdict; held in memory, reset by a restart",
            value: self.audit_verify_unfinished_runs_in_a_row(),
        }]
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
        m.verify_schema_changed();
        m.set_verify_unfinished_in_a_row(4);
        m.checkpoint_written();
        m.lines_shipped(5);
        assert_eq!(m.audit_verify_runs_total(), 1);
        assert_eq!(m.audit_verify_failures_total(), 3);
        assert_eq!(m.audit_verify_errors_total(), 1);
        assert_eq!(m.audit_verify_schema_changes_total(), 1);
        assert_eq!(m.audit_verify_unfinished_runs_in_a_row(), 4);
        m.set_verify_unfinished_in_a_row(0);
        assert_eq!(m.audit_verify_unfinished_runs_in_a_row(), 0, "a gauge goes down");
        assert_eq!(m.audit_checkpoints_written_total(), 1);
        assert_eq!(m.audit_lines_shipped_total(), 5);
        let names: Vec<_> = m.counters().iter().map(|c| c.name).collect();
        assert!(names.contains(&"audit_verify_failures_total"));
        assert!(names.contains(&"audit_verify_schema_changes_total"));
        // The gauge is listed apart from the counters: a counter that went down would read as a reset.
        assert!(!names.contains(&"audit_verify_unfinished_runs_in_a_row"));
        let gauges: Vec<_> = m.gauges().iter().map(|g| g.name).collect();
        assert_eq!(gauges, ["audit_verify_unfinished_runs_in_a_row"]);
    }
}
