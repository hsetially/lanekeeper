//! The pickup sweeper (T10, A37): the leased job `registry-pickup-sweeper`.
//!
//! A change that is `live_within_ttl` becomes `live` when the client cache expires, which no event announces, and a notification written
//! by another component (prompt 06 sets `swimlanes.last_notified_at`) makes the services that listen live without telling anybody. The
//! sweeper is what makes the stored state follow:
//!
//! - **every run** (every `pickup_sweep_interval`, 30 seconds by default) it computes the swimlanes that have a service that is not live;
//! - **every `pickup_full_interval`** (ten minutes) it also computes every swimlane that has an inventory: what no event announced (an
//!   admin's folder mapping, a computation that failed, a replica that took over) is repaired by this one.
//!
//! A computation of a swimlane writes only the services whose state differs and publishes `PickupChanged` for exactly those, so a run that
//! finds nothing changed writes and says nothing, and a second run in a row is silent. The time it compares with is the injected clock, and
//! the job never sleeps: waiting is the job runner's schedule.
//!
//! A run is bounded: at most `pickup_sweep_lanes` swimlanes, and it asks [`JobContext::ensure_held`] before each swimlane it computes and
//! stops when the lease is lost. The full look is marked done only when it computed every swimlane it meant to, so a run that was cut short
//! or that failed on one swimlane makes the next run try the full look again.

use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use domain::SwimlaneId;
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use tracing::debug;

use super::{Inner, Pickups};
use crate::config::RegistryConfig;
use crate::error::RegistryError;

/// The job's name, which is also its lease's name.
pub const JOB_NAME: &str = "registry-pickup-sweeper";

/// The swimlanes that have a service that is not live. `$1` the limit. A scan of `service_state` (about as many rows as the Deployments of all
/// swimlanes), because no index serves `pickup_state` and the table is a contract: the cost is every 30 seconds, not every request.
pub const PENDING_LANES_SQL: &str = "SELECT DISTINCT s.swimlane_id FROM service_state st JOIN services s ON s.id = st.service_id \
     WHERE st.pickup_state <> 'live' ORDER BY s.swimlane_id LIMIT $1";

/// The swimlanes that have an inventory, for the full look. `$1` the limit.
pub const INVENTORIED_LANES_SQL: &str = "SELECT swimlane_id FROM swimlane_registry WHERE first_inventory_at IS NOT NULL \
     ORDER BY swimlane_id LIMIT $1";

/// What one run did, in counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Swimlanes computed.
    pub swimlanes: usize,
    /// Services whose state changed (and that `PickupChanged` named).
    pub changed: usize,
    /// Swimlanes that could not be computed; the next run looks at them again.
    pub failed: usize,
    /// This run looked at every swimlane with an inventory.
    pub full: bool,
    /// The run stopped at `pickup_sweep_lanes` and the next one goes on.
    pub truncated: bool,
}

impl Pickups {
    /// How the job is scheduled: every `pickup_sweep_interval`, under a 90 second lease that the runner renews every 30, for at most ten
    /// minutes.
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes (the interval is range-checked by `RegistryConfig::validate`).
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(JOB_NAME, config.pickup_sweep_interval, Duration::from_secs(90))?
            .with_max_run(Duration::from_secs(10 * 60))
    }

    /// The sweeper (`registry-pickup-sweeper`), for the hub's job runner.
    pub fn job(&self) -> std::sync::Arc<dyn Job> {
        std::sync::Arc::new(Sweeper {
            pickups: self.clone(),
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
        let full_every = i64::try_from(inner.config.pickup_full_interval.as_millis()).unwrap_or(i64::MAX);
        let full = inner
            .last_full
            .load(Ordering::SeqCst)
            .checked_add(full_every)
            .is_none_or(|due| due <= now);
        let limit = i64::try_from(inner.config.pickup_sweep_lanes).unwrap_or(i64::MAX);
        let lanes = inner
            .lanes_to_sweep(full, limit)
            .await
            .map_err(|_| JobError::Failed("registry_pickup_sweep"))?;
        let mut report = SweepReport {
            full,
            truncated: i64::try_from(lanes.len()).is_ok_and(|n| n > limit),
            ..SweepReport::default()
        };
        for lane in lanes.iter().take(inner.config.pickup_sweep_lanes) {
            held()?;
            if let Ok(changed) = inner.recompute_one(lane).await {
                report.swimlanes += 1;
                report.changed += changed;
            } else {
                inner.metrics.pickup_lane(crate::metrics::PickupResult::Failed);
                report.failed += 1;
            }
        }
        // The full look is done when it was made of every swimlane and none failed: otherwise the next run makes it again.
        if full && report.failed == 0 && !report.truncated {
            inner.last_full.store(now, Ordering::SeqCst);
        }
        debug!(
            swimlanes = report.swimlanes,
            changed = report.changed,
            failed = report.failed,
            full = report.full,
            "pickup sweep"
        );
        Ok(report)
    }
}

impl Inner {
    /// The swimlanes a run computes: those with a service that is not live, and on a full look every swimlane that has an inventory. In
    /// swimlane order, at most `limit` and one more, so that a run over the bound is seen as such.
    async fn lanes_to_sweep(&self, full: bool, limit: i64) -> Result<Vec<SwimlaneId>, RegistryError> {
        let pending: Vec<String> = self
            .db
            .run(
                "pickup_sweep_pending",
                sqlx::query_scalar(PENDING_LANES_SQL)
                    .bind(limit.saturating_add(1))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut all: std::collections::BTreeSet<String> = pending.into_iter().collect();
        if full {
            let inventoried: Vec<String> = self
                .db
                .run(
                    "pickup_sweep_inventoried",
                    sqlx::query_scalar(INVENTORIED_LANES_SQL)
                        .bind(limit.saturating_add(1))
                        .fetch_all(self.db.pool()),
                )
                .await?;
            all.extend(inventoried);
        }
        Ok(all.iter().filter_map(|id| SwimlaneId::parse(id).ok()).collect())
    }
}

struct Sweeper {
    pickups: Pickups,
}

#[async_trait]
impl Job for Sweeper {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.pickups.sweep(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec = Pickups::spec(&RegistryConfig::default()).expect("the default config gives a valid spec");
        assert_eq!(spec.name(), JOB_NAME);
        assert_eq!(JOB_NAME, "registry-pickup-sweeper");
        assert!(ports::is_valid_lease_name(JOB_NAME));
    }

    #[test]
    fn a_config_the_spec_refuses_is_an_error_and_not_a_panic() {
        let config = RegistryConfig {
            pickup_sweep_interval: Duration::ZERO,
            ..RegistryConfig::default()
        };
        assert!(Pickups::spec(&config).is_err());
    }
}
