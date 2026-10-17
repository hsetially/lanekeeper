//! The leased job `registry-attribution-grace` (T7, A19, P14).
//!
//! A record of a local write usually reaches the hub **before** the agent reports the change (the sentinel flushes every second, the agent
//! walks every ten and waits for quiet): the observation is applied as `unknown`, and the record is already in `sentinel_records`. The
//! sink cannot see that (it runs when a batch arrives), and ingestion does not look (the apply loop gains no statement for this, P15), so
//! this job does, every `attribution_interval`:
//!
//! 1. **The swimlanes that have a sentinel** (`sentinels.swimlane_id`), at most `attribution_lanes`. Nothing else can be explained by one.
//! 2. For each, **a look, without the swimlane's lock**: the observations of the last `sentinel_lookback` that a stored record explains
//!    (and that are `unknown` or a file-owner hint: an NFS client's change is found by a record that arrives later, through the sink), and
//!    the observations that are an NFS client's (A19: applied `nfs_client_grace` ago, not older than `nfs_client_window`, every sentinel
//!    of the swimlane seen since, no record explains them). With neither, the swimlane's lock is never taken: the usual state.
//! 3. **The upgrade, under the lock** (the one every apply of the swimlane holds, taken before the audit chain's), after a second look:
//!    the records first, so that an observation a record explains is never called an NFS client's, then the NFS clients, in one
//!    transaction with their audit and outbox events.
//!
//! A sentinel's record reaches the observation within one interval of the observation being applied; a record that arrives after it is
//! attributed by the sink at once. P14 asks for 30 seconds and the interval is 5.
//!
//! A run is bounded (swimlanes, rows read), asks [`hub_platform::jobs::JobContext::ensure_held`] before each swimlane and stops when the lease
//! is lost, and never fails because one swimlane did: it counts the failure and the next run looks again. Ids and counts are all it logs.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::SwimlaneId;
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use ports::AuditActor;

use super::Inner;
use crate::config::RegistryConfig;
use crate::error::RegistryError;
use crate::ingest::apply::LOCK_SQL;
use crate::metrics::AttributionResult;

/// The job's name, which is also its lease's name.
pub const GRACE_JOB: &str = "registry-attribution-grace";

/// The audit actor of an upgrade the job makes: the registry's own component.
const COMPONENT: &str = "registry-attribution-grace";

/// The swimlanes that have a sentinel and their sentinels (`$1` the most swimlanes).
pub const LANES_SQL: &str = "SELECT swimlane_id, array_agg(id ORDER BY id) FROM sentinels WHERE swimlane_id IS NOT NULL \
     GROUP BY swimlane_id ORDER BY swimlane_id LIMIT $1";

/// What one run did, in counts. Nothing in it is a path, a user or a hash.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraceReport {
    /// Swimlanes looked at.
    pub lanes: usize,
    /// Observations that became `sentinel_login`, by a record stored before them.
    pub sentinel_login: usize,
    /// Observations that became `nfs_client`.
    pub nfs_client: usize,
    /// Swimlanes that could not be handled; the next run looks at them again.
    pub failed: usize,
    /// More swimlanes had a sentinel than `attribution_lanes`; the next run goes on.
    pub truncated: bool,
}

/// The job. Cloning shares everything.
#[derive(Clone)]
pub struct AttributionGrace {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AttributionGrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttributionGrace").finish_non_exhaustive()
    }
}

impl AttributionGrace {
    pub(super) fn new(inner: Arc<Inner>) -> Self {
        Self { inner }
    }

    /// How the job is scheduled: every `attribution_interval`, under a 90 second lease that the runner renews every 30, for at most ten
    /// minutes.
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes (the interval is range-checked by `RegistryConfig::validate`).
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(GRACE_JOB, config.attribution_interval, Duration::from_secs(90))?
            .with_max_run(Duration::from_secs(10 * 60))
    }

    /// The job (`registry-attribution-grace`), for the hub's job runner.
    pub fn job(&self) -> Arc<dyn Job> {
        Arc::new(Run { grace: self.clone() })
    }

    /// One run, as a test calls it: the lease is taken to be held.
    ///
    /// # Errors
    ///
    /// `Failed` when the list of swimlanes cannot be read.
    pub async fn run_once(&self) -> Result<GraceReport, JobError> {
        self.sweep(|| Ok(())).await
    }

    /// One run. `held` is asked before each swimlane and ends the run with its error.
    ///
    /// # Errors
    ///
    /// `LeaseLost` from `held`; `Failed` when the list of swimlanes cannot be read.
    pub async fn sweep(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send,
    ) -> Result<GraceReport, JobError> {
        let inner = &*self.inner;
        held()?;
        let now_ms = inner.clock.now().unix_millis();
        let limit = i64::try_from(inner.config.attribution_lanes).unwrap_or(i64::MAX);
        let lanes: Vec<(String, Vec<i64>)> = inner
            .db
            .run(
                "attribution_lanes",
                sqlx::query_as(LANES_SQL)
                    .bind(limit.saturating_add(1))
                    .fetch_all(inner.db.pool()),
            )
            .await
            .map_err(|_| JobError::Failed("registry_attribution_lanes"))?;
        let mut report = GraceReport {
            truncated: i64::try_from(lanes.len()).is_ok_and(|n| n > limit),
            ..GraceReport::default()
        };
        for (lane, sentinels) in lanes.iter().take(inner.config.attribution_lanes) {
            held()?;
            let Ok(lane) = SwimlaneId::parse(lane) else {
                continue;
            };
            if let Ok((login, nfs)) = inner.attribute_lane(&lane, sentinels, now_ms).await {
                report.lanes += 1;
                report.sentinel_login += login;
                report.nfs_client += nfs;
            } else {
                report.failed += 1;
                inner.metrics.attribution(AttributionResult::LaneFailed, 1);
            }
        }
        // Counts only (S10).
        tracing::debug!(
            lanes = report.lanes,
            sentinel_login = report.sentinel_login,
            nfs_client = report.nfs_client,
            failed = report.failed,
            "attribution grace"
        );
        Ok(report)
    }
}

impl Inner {
    /// What the job does for one swimlane: `(sentinel_login, nfs_client)` upgrades.
    async fn attribute_lane(
        &self,
        lane: &SwimlaneId,
        sentinels: &[i64],
        now_ms: i64,
    ) -> Result<(usize, usize), RegistryError> {
        // Look first, without the lock: with nothing to do (the usual state) the swimlane's lock is never taken.
        {
            let mut conn = self.db.run("attribution_lane", self.db.pool().acquire()).await?;
            let by_record = self
                .observation_candidates(&mut conn, lane, sentinels, now_ms)
                .await?;
            let nfs = self.nfs_candidates(&mut conn, lane, now_ms).await?;
            if by_record.is_empty() && nfs.is_empty() {
                return Ok((0, 0));
            }
        }

        let mut tx = self.db.begin().await?;
        self.db
            .run(
                "attribution_lock",
                sqlx::query(LOCK_SQL).bind(lane.as_str()).execute(&mut *tx),
            )
            .await?;
        let component = domain::ShortText::parse(COMPONENT).map_err(|_| RegistryError::Invalid)?;
        let actor = AuditActor::System { component };
        // Again, under the lock: what an apply or a batch did since the look is what is read.
        let by_record = self
            .observation_candidates(&mut tx, lane, sentinels, now_ms)
            .await?;
        let login = self
            .upgrade_from_records(
                &mut tx,
                lane,
                by_record,
                actor.clone(),
                AttributionResult::SentinelByJob,
            )
            .await?;
        // After the records, in the same transaction: the statement sees what the first one wrote, so an observation a record has just
        // explained is not an NFS client's.
        let nfs = self.nfs_candidates(&mut tx, lane, now_ms).await?;
        let nfs = self.upgrade_nfs_client(&mut tx, lane, nfs, actor).await?;
        self.db.commit(tx).await?;
        Ok((login, nfs))
    }
}

struct Run {
    grace: AttributionGrace,
}

#[async_trait]
impl Job for Run {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.grace.sweep(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec = AttributionGrace::spec(&RegistryConfig::default())
            .expect("the default config gives a valid spec");
        assert_eq!(spec.name(), GRACE_JOB);
        assert_eq!(GRACE_JOB, "registry-attribution-grace");
        assert!(ports::is_valid_lease_name(GRACE_JOB));
    }

    #[test]
    fn a_config_the_spec_refuses_is_an_error_and_not_a_panic() {
        let config = RegistryConfig {
            attribution_interval: Duration::ZERO,
            ..RegistryConfig::default()
        };
        assert!(AttributionGrace::spec(&config).is_err());
    }

    #[test]
    fn the_job_looks_only_at_swimlanes_that_have_a_sentinel_and_bounds_them() {
        assert!(LANES_SQL.contains("swimlane_id IS NOT NULL"));
        assert!(LANES_SQL.contains("LIMIT $1"));
    }
}
