//! The leased job `registry-sentinel-partitions` (T7, S9, S21): the one caller of `maintain_sentinel_partitions`.
//!
//! `sentinel_records` is partitioned by month, and only the migrator may create or drop a partition (DDL). `lanekeeper_app` holds `EXECUTE`
//! on one `SECURITY DEFINER` function that creates last month, this month and the next two, moves default-partition rows into a month that
//! is new, and drops the months that ended before the retention (and the default partition's rows older than it). The migration says
//! the hub calls it from a leased job, and that it must run at least monthly (`db/migrations/README.md`); this is that job. It had no
//! caller until now: without it the months ahead run out, every record then lands in the default partition, and nothing is ever pruned.
//!
//! The retention is the admin's `retention_settings.sentinel_keep_days`, with the function's own floor of **90 days** in front of it
//! (the function refuses less, so a smaller setting is raised to 90 here rather than failing the job every day). The function is
//! idempotent and takes an advisory lock, so two replicas that both run it do no harm; the lease keeps it to one.
//!
//! The call has a timeout of its own, longer than a query's (it may move rows out of the default partition into a new month); a run that
//! fails is reported as `Failed` and the next one tries again.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};

use super::Inner;
use crate::config::RegistryConfig;
use crate::db::run_within;

/// The job's name, which is also its lease's name.
pub const PARTITIONS_JOB: &str = "registry-sentinel-partitions";

/// The retention the admin chose.
pub const KEEP_SQL: &str = "SELECT sentinel_keep_days FROM retention_settings WHERE singleton LIMIT 1";

/// The maintenance, with the retention in days.
pub const MAINTAIN_SQL: &str = "SELECT created, dropped FROM maintain_sentinel_partitions($1) LIMIT 1";

/// The function's floor (`min_keep_days` in `0001_init.sql`): it refuses a smaller retention.
const FLOOR_DAYS: i32 = 90;

/// One call of the function may move the rows of a month out of the default partition.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// What one run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PartitionReport {
    /// Partitions created.
    pub created: i32,
    /// Partitions dropped, past their retention.
    pub dropped: i32,
    /// The retention the function was called with.
    pub keep_days: i32,
}

/// The retention to call the function with: the admin's, and never below the floor the function holds.
pub(crate) fn keep_days(setting: Option<i32>) -> i32 {
    setting.map_or(FLOOR_DAYS, |d| d.clamp(FLOOR_DAYS, 3650))
}

/// The job. Cloning shares everything.
#[derive(Clone)]
pub struct SentinelPartitions {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SentinelPartitions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SentinelPartitions").finish_non_exhaustive()
    }
}

impl SentinelPartitions {
    pub(super) fn new(inner: Arc<Inner>) -> Self {
        Self { inner }
    }

    /// How the job is scheduled: every `sentinel_partition_interval`, under a 90 second lease, for at most five minutes.
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes.
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(
            PARTITIONS_JOB,
            config.sentinel_partition_interval,
            Duration::from_secs(90),
        )?
        .with_max_run(Duration::from_secs(5 * 60))
    }

    pub fn job(&self) -> Arc<dyn Job> {
        Arc::new(Run { job: self.clone() })
    }

    /// One run, as a test calls it: the lease is taken to be held.
    ///
    /// # Errors
    ///
    /// `Failed` when the retention cannot be read or the function does not finish.
    pub async fn run_once(&self) -> Result<PartitionReport, JobError> {
        self.sweep(|| Ok(())).await
    }

    /// One run. `held` is asked before the call and ends the run with its error.
    ///
    /// # Errors
    ///
    /// `LeaseLost` from `held`; `Failed` when the retention cannot be read or the function does not finish.
    pub async fn sweep(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send,
    ) -> Result<PartitionReport, JobError> {
        let inner = &*self.inner;
        held()?;
        let setting: Option<i32> = inner
            .db
            .run(
                "sentinel_keep_days",
                sqlx::query_scalar(KEEP_SQL).fetch_optional(inner.db.pool()),
            )
            .await
            .map_err(|_| JobError::Failed("registry_sentinel_keep_days"))?;
        let keep = keep_days(setting);
        held()?;
        let (created, dropped): (i32, i32) = run_within(
            CALL_TIMEOUT,
            "sentinel_partitions",
            sqlx::query_as(MAINTAIN_SQL).bind(keep).fetch_one(inner.db.pool()),
        )
        .await
        .map_err(|_| JobError::Failed("registry_sentinel_partitions"))?;
        tracing::debug!(created, dropped, keep_days = keep, "sentinel partitions");
        Ok(PartitionReport {
            created,
            dropped,
            keep_days: keep,
        })
    }
}

struct Run {
    job: SentinelPartitions,
}

#[async_trait]
impl Job for Run {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.job.sweep(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retention_is_the_admins_and_never_below_the_floor_of_the_function() {
        assert_eq!(keep_days(None), 90);
        assert_eq!(
            keep_days(Some(1)),
            90,
            "the function refuses less than 90 days, so the job asks for 90"
        );
        assert_eq!(keep_days(Some(89)), 90);
        assert_eq!(keep_days(Some(90)), 90);
        assert_eq!(keep_days(Some(365)), 365);
        assert_eq!(keep_days(Some(3650)), 3650);
        assert_eq!(keep_days(Some(100_000)), 3650);
    }

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec = SentinelPartitions::spec(&RegistryConfig::default()).expect("a valid spec");
        assert_eq!(spec.name(), PARTITIONS_JOB);
        assert!(ports::is_valid_lease_name(PARTITIONS_JOB));
    }
}
