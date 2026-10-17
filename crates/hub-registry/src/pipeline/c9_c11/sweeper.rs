//! The structure sweeper (T10): the leased job `registry-structure-sweeper`.
//!
//! - **every run** (every `structure_sweep_interval`, 10 seconds by default) it reads the gates of the swimlanes that have an inventory, in one
//!   statement, and computes the ones whose gate differs from what the replica saw when it last computed them: a swimlane the pipeline has
//!   committed a change to, or whose config-server started, or whose rules changed. A swimlane that nothing moved in costs nothing more;
//! - **every `structure_full_interval`** (ten minutes) it computes **every** swimlane that has an inventory, whatever its gate says: what no
//!   gate announces (a Deployment's environment names, a folder mapping an admin changed, a replica that took over) is found by this one. A
//!   full pass that one run cannot finish (the bound below, a lost lease, a failed swimlane) goes on at the next run with the swimlanes it has
//!   not computed, and is over when every one has been.
//!
//! The job never runs in the pipeline's loop, so the checks cannot slow a batch, and one run computes a swimlane once however many batches it
//! had. The time it compares with is the injected clock, and it never sleeps: waiting is the job runner's schedule.
//!
//! A run is bounded: it computes at most `structure_sweep_lanes` swimlanes (the ones it does not reach are still due at the next run: an
//! unchanged swimlane is not counted, so the bound cannot starve the swimlanes behind it), and it asks [`JobContext::ensure_held`] before each swimlane and stops when the
//! lease is lost. The full pass is marked done only when it computed every swimlane it meant to; the swimlanes it has done are remembered, so
//! a run that was cut short or that failed on one swimlane is followed by one that does the others.

use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use tracing::debug;

use super::{Mode, Structure, inputs};
use crate::config::RegistryConfig;
use crate::metrics::StructureResult;

/// The job's name, which is also its lease's name.
pub const JOB_NAME: &str = "registry-structure-sweeper";

/// The most gates one run reads: the swimlanes the registry can hold (the engine's batch takes at most 1,000 at once; a hub has tens).
const MAX_GATES: usize = 100_000;

/// What one run did, in counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Swimlanes computed.
    pub computed: usize,
    /// Swimlanes whose gate was what the replica had seen: nothing was read or written for them.
    pub unchanged: usize,
    /// Swimlanes the pipeline has not computed yet.
    pub not_ready: usize,
    /// Swimlanes that could not be computed; the next run looks at them again.
    pub failed: usize,
    /// Findings written (new, changed or resolved).
    pub written: usize,
    /// This run was the full pass.
    pub full: bool,
    /// More swimlanes were due than `structure_sweep_lanes`: the run computed that many and the next one goes on.
    pub truncated: bool,
}

impl Structure {
    /// How the job is scheduled: every `structure_sweep_interval`, under a 90 second lease that the runner renews every 30, for at most ten
    /// minutes.
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes (the interval is range-checked by `RegistryConfig::validate`).
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(JOB_NAME, config.structure_sweep_interval, Duration::from_secs(90))?
            .with_max_run(Duration::from_secs(10 * 60))
    }

    /// The sweeper (`registry-structure-sweeper`), for the hub's job runner.
    pub fn job(&self) -> std::sync::Arc<dyn Job> {
        std::sync::Arc::new(Sweeper {
            structure: self.clone(),
        })
    }

    /// One run, as a test calls it: the lease is taken to be held.
    ///
    /// # Errors
    ///
    /// `Failed` when the swimlanes cannot be listed.
    pub async fn run_once(&self) -> Result<SweepReport, JobError> {
        self.sweep(|| Ok(())).await
    }

    /// One run. `held` is asked before each swimlane and ends the run with its error.
    ///
    /// # Errors
    ///
    /// `LeaseLost` from `held`; `Failed` when the swimlanes cannot be listed.
    pub async fn sweep(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send,
    ) -> Result<SweepReport, JobError> {
        let inner = &*self.inner;
        held()?;
        let now = inner.clock.now().unix_millis();
        let full_every = i64::try_from(inner.config.structure_full_interval.as_millis()).unwrap_or(i64::MAX);
        let full = inner
            .last_full
            .load(Ordering::SeqCst)
            .checked_add(full_every)
            .is_none_or(|due| due <= now);
        let limit = inner.config.structure_sweep_lanes;
        // Every gate is read (one statement over a handful of rows per swimlane): the bound is on the swimlanes **computed**, because one that
        // is unchanged costs nothing, and a bound on the swimlanes looked at would starve the ones past it for ever.
        let gates = inputs::gates(&inner.db, None, MAX_GATES)
            .await
            .map_err(|_| JobError::Failed("registry_structure_sweep"))?;
        let mut report = SweepReport {
            full,
            ..SweepReport::default()
        };
        let mut computed = 0usize;
        for gate in gates.iter().take(MAX_GATES) {
            held()?;
            // What is due: in a full pass, every swimlane that the pass has not computed yet (a pass that was cut short goes on where it
            // stopped); otherwise the swimlanes whose gate is not what the replica has seen. The rest cost the look at the gate and nothing
            // more.
            let due = gate.ready()
                && if full {
                    !inner.passed(&gate.swimlane)
                } else {
                    !inner.remembers(gate)
                };
            if due && computed >= limit {
                report.truncated = true;
                continue;
            }
            let mode = if due && full { Mode::Full } else { Mode::Gated };
            let outcome = inner.lane(gate, mode).await;
            if let Ok(done) = &outcome {
                report.written += done.written();
            }
            match outcome.map(|o| o.result) {
                Ok(StructureResult::Computed) => {
                    report.computed += 1;
                    computed += 1;
                    if full {
                        inner.mark_passed(&gate.swimlane);
                    }
                }
                Ok(StructureResult::Unchanged) => report.unchanged += 1,
                Ok(StructureResult::NotReady) => report.not_ready += 1,
                Ok(StructureResult::Failed) | Err(_) => {
                    report.failed += 1;
                    computed += 1;
                }
            }
        }
        // The full pass is done when it was made of every swimlane and none failed: otherwise the next run makes it again.
        if full && report.failed == 0 && !report.truncated {
            inner.last_full.store(now, Ordering::SeqCst);
            inner.end_pass();
        }
        debug!(
            computed = report.computed,
            unchanged = report.unchanged,
            not_ready = report.not_ready,
            failed = report.failed,
            written = report.written,
            full = report.full,
            "structure sweep"
        );
        Ok(report)
    }
}

struct Sweeper {
    structure: Structure,
}

#[async_trait]
impl Job for Sweeper {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.structure.sweep(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec =
            Structure::spec(&RegistryConfig::default()).expect("the default config gives a valid spec");
        assert_eq!(spec.name(), JOB_NAME);
        assert_eq!(JOB_NAME, "registry-structure-sweeper");
        assert!(ports::is_valid_lease_name(JOB_NAME));
    }

    #[test]
    fn a_config_the_spec_refuses_is_an_error_and_not_a_panic() {
        let config = RegistryConfig {
            structure_sweep_interval: Duration::ZERO,
            ..RegistryConfig::default()
        };
        assert!(Structure::spec(&config).is_err());
    }
}
