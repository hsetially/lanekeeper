//! The nightly full recompute (T3, D62, D63, P6): the engine's batch function over every swimlane, compared with what is stored, with
//! any divergence repaired and counted.
//!
//! Events keep the state right between nights; the nightly job is what proves it. It loads every swimlane that has an inventory from the
//! database and Git ([`Pipeline::refresh_all`]), in batches of `nightly_batch_lanes` swimlanes so that the content of their distinct files
//! (a batch shares the parse of a file that its swimlanes hold alike) is the only content in memory at a time, computes them with
//! [`engine::recompute_all`], and makes the database say what the computation says: a path whose row, finding or effective hash differs is
//! rewritten, with the same events a live change would send. A swimlane that was in memory is replaced by the new computation, and
//! the paths where the two disagreed are counted (`registry_pipeline_divergences`, [`NightlyReport::repaired_paths`]): the incremental
//! pipeline is right when that stays at zero.
//!
//! The job is leased (one replica runs it) and idempotent. The budget is P6: all 40 swimlanes of the design scale in 3 minutes, database
//! reads and writes included (`hub-registry/full_recompute_target_scale`).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use domain::SwimlaneId;
use hub_platform::jobs::{Job, JobContext, JobError};
use tracing::{info, warn};

use super::Pipeline;
use crate::error::RegistryError;

/// What a nightly run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NightlyReport {
    /// Swimlanes computed.
    pub lanes: usize,
    pub paths: usize,
    /// Paths where the stored state (memory, if the swimlane was in memory) was not what the computation says.
    pub repaired_paths: usize,
    /// Rows written or deleted to make the database say what the computation says.
    pub rows_changed: usize,
    /// Swimlanes that could not be computed.
    pub failed: Vec<SwimlaneId>,
    pub took: Duration,
}

impl Pipeline {
    /// Compute every swimlane that has an inventory from scratch, `nightly_batch_lanes` at a time. `held` is asked before each batch and
    /// ends the run with its error.
    ///
    /// # Errors
    ///
    /// `LeaseLost` from `held`, or `Failed` when the database or the mirror cannot be read at all.
    pub async fn refresh_all_with(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send,
    ) -> Result<NightlyReport, JobError> {
        let started = Instant::now();
        let inner = &self.inner;
        let mut report = NightlyReport::default();
        held()?;
        let all = inner
            .all_with_inventory()
            .await
            .map_err(|_| JobError::Failed("registry_nightly_lanes"))?;
        for batch in all.chunks(inner.config.nightly_batch_lanes) {
            held()?;
            let (rules, view) = match inner.prepare(batch).await {
                Ok(p) => p,
                Err(error) => {
                    warn!(
                        ?error,
                        "the nightly recompute could not read its rules or the mirror"
                    );
                    return Err(JobError::Failed("registry_nightly_prepare"));
                }
            };
            let lanes: Vec<_> = batch.iter().filter_map(|id| rules.get(id).cloned()).collect();
            let mut exec = inner.exec.lock().await;
            for (id, result) in inner.refresh_lanes(&mut exec, lanes, &view).await {
                match result {
                    Ok(lane) => {
                        report.lanes += 1;
                        report.paths += lane.paths;
                        report.repaired_paths += lane.repaired;
                        report.rows_changed += lane.rows_written + lane.rows_deleted;
                        inner.metrics.pipeline_lane(lane.result);
                    }
                    Err(error) => {
                        warn!(swimlane = %id, ?error, "the nightly recompute failed for a swimlane");
                        inner.metrics.pipeline_lane(crate::metrics::LaneResult::Failed);
                        report.failed.push(id);
                    }
                }
            }
            inner.metrics.pipeline_lanes_in_memory(exec.store.len());
        }
        report.took = started.elapsed();
        info!(
            lanes = report.lanes,
            paths = report.paths,
            repaired = report.repaired_paths,
            failed = report.failed.len(),
            seconds = report.took.as_secs_f64(),
            "nightly full recompute"
        );
        Ok(report)
    }

    /// The nightly recompute without a lease: for tests and the benchmark.
    ///
    /// # Errors
    ///
    /// As [`refresh_all_with`](Self::refresh_all_with).
    pub async fn refresh_all(&self) -> Result<NightlyReport, RegistryError> {
        self.refresh_all_with(|| Ok(()))
            .await
            .map_err(|_| RegistryError::Database)
    }
}

/// The leased nightly job.
#[derive(Debug, Clone)]
pub struct NightlyJob {
    pipeline: Pipeline,
}

impl NightlyJob {
    pub(crate) fn new(pipeline: Pipeline) -> Self {
        Self { pipeline }
    }
}

#[async_trait]
impl Job for NightlyJob {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.pipeline
            .refresh_all_with(|| ctx.ensure_held())
            .await
            .map(|_| ())
    }
}
