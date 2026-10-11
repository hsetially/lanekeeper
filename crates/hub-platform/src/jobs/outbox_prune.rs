//! The outbox prune (T10, D80): the leaseholder deletes outbox rows older than an hour, in bounded batches.
//!
//! Every event is a row (plain `publish` too, plan A11), so the table grows with the event rate and must be trimmed. The relay
//! reads rows within seconds, and the replay of `Last-Event-ID` reads at most five minutes back, so a row an hour old has no reader
//! left: a client whose last id was pruned is told to refetch (`Replay::Resync`), as one whose id was never there.
//!
//! One replica prunes at a time (the job runs under a lease, D62), each statement deletes at most `batch` rows so that it holds
//! its row locks briefly, and a run does at most `max_batches` statements, so a backlog is worked off over several runs instead of
//! one long one. Before each statement the job asks the lease context whether it still holds the lease (rule 5). The statement
//! orders by the primary key, so it walks the oldest rows first and stops at the batch size: the table keeps an hour of events,
//! so even a run that finds nothing to delete reads at most that much.

use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;
use tracing::{debug, warn};

use super::{Job, JobContext, JobError, JobSpec, SpecError};
use crate::events::metrics::EventMetrics;

/// The job's name, which is also its lease's name.
pub const PRUNE_JOB: &str = "outbox.prune";
/// How long a row is kept.
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(3600);
/// The shortest retention a caller may ask for: longer than the SSE replay window (five minutes) and the relay's wait for a late
/// commit (ten seconds) together, with room to spare. A shorter one would delete rows a reader can still ask for.
pub const MIN_RETENTION: Duration = Duration::from_secs(10 * 60);
/// Rows deleted per statement.
pub const DEFAULT_BATCH: i64 = 1_000;
/// The most rows one statement may be asked to delete.
pub const MAX_BATCH: i64 = 10_000;
/// Statements per run: 100 batches is 100,000 rows, an hour of events at about 30 a second.
pub const DEFAULT_MAX_BATCHES: u32 = 100;
/// The most statements one run may be asked to make.
pub const MAX_MAX_BATCHES: u32 = 1_000;
/// How often the leaseholder prunes.
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(60);
/// The lease survives two missed renewals, so a dead holder is replaced within this.
pub const PRUNE_LEASE_TTL: Duration = Duration::from_secs(180);
/// A run is cut off after this; the next starts afresh.
pub const PRUNE_MAX_RUN: Duration = Duration::from_secs(5 * 60);
/// One statement.
const STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PruneConfigError {
    #[error("rows must be kept at least as long as events can be replayed")]
    RetentionTooShort,
    #[error("a batch or a run must be between one and its maximum")]
    BadLimit,
}

/// What one run deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pruned {
    pub rows: u64,
    /// Statements made, including the last one that found less than a batch.
    pub batches: u32,
}

#[derive(Clone)]
pub struct OutboxPrune {
    pool: PgPool,
    metrics: EventMetrics,
    retention: Duration,
    batch: i64,
    max_batches: u32,
}

impl std::fmt::Debug for OutboxPrune {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxPrune")
            .field("retention", &self.retention)
            .field("batch", &self.batch)
            .field("max_batches", &self.max_batches)
            .finish_non_exhaustive()
    }
}

impl OutboxPrune {
    #[must_use]
    pub fn new(pool: PgPool, metrics: EventMetrics) -> Self {
        Self {
            pool,
            metrics,
            retention: DEFAULT_RETENTION,
            batch: DEFAULT_BATCH,
            max_batches: DEFAULT_MAX_BATCHES,
        }
    }

    /// Keep rows for `retention` instead of an hour, at least [`MIN_RETENTION`].
    pub fn with_retention(mut self, retention: Duration) -> Result<Self, PruneConfigError> {
        if retention < MIN_RETENTION {
            return Err(PruneConfigError::RetentionTooShort);
        }
        self.retention = retention;
        Ok(self)
    }

    /// Delete at most `batch` rows per statement, from 1 to [`MAX_BATCH`].
    pub fn with_batch(mut self, batch: i64) -> Result<Self, PruneConfigError> {
        if !(1..=MAX_BATCH).contains(&batch) {
            return Err(PruneConfigError::BadLimit);
        }
        self.batch = batch;
        Ok(self)
    }

    /// Make at most `max_batches` statements per run, from 1 to [`MAX_MAX_BATCHES`].
    pub fn with_max_batches(mut self, max_batches: u32) -> Result<Self, PruneConfigError> {
        if !(1..=MAX_MAX_BATCHES).contains(&max_batches) {
            return Err(PruneConfigError::BadLimit);
        }
        self.max_batches = max_batches;
        Ok(self)
    }

    /// The spec to register with the `JobRunner`.
    pub fn spec() -> Result<JobSpec, SpecError> {
        Self::spec_with(PRUNE_INTERVAL, PRUNE_LEASE_TTL)
    }

    /// The same with another interval and lease, for tests that run on a real clock.
    pub fn spec_with(interval: Duration, lease_ttl: Duration) -> Result<JobSpec, SpecError> {
        JobSpec::new(PRUNE_JOB, interval, lease_ttl)?.with_max_run(PRUNE_MAX_RUN)
    }

    /// One statement: delete up to a batch of the oldest rows past the retention. Returns how many.
    async fn delete_batch(&self) -> Result<u64, JobError> {
        let call = sqlx::query(
            "DELETE FROM outbox WHERE id IN \
             (SELECT id FROM outbox \
              WHERE created_at < now() - make_interval(secs => $1::double precision) \
              ORDER BY id LIMIT $2)",
        )
        .bind(self.retention.as_secs_f64())
        .bind(self.batch)
        .execute(&self.pool);
        if let Ok(Ok(done)) = tokio::time::timeout(STATEMENT_TIMEOUT, call).await {
            Ok(done.rows_affected())
        } else {
            // The cause is not logged: a database error can carry statement text.
            warn!(
                code = "outbox_prune_failed",
                "the outbox prune could not delete rows"
            );
            Err(JobError::Failed("outbox_prune"))
        }
    }

    /// A run: statements until one deletes less than a batch, or the run's limit, or the lease is lost.
    pub async fn prune(&self, ctx: &JobContext) -> Result<Pruned, JobError> {
        let batch = self.batch.unsigned_abs();
        let mut total = Pruned::default();
        for _ in 0..self.max_batches {
            ctx.ensure_held()?;
            let deleted = self.delete_batch().await?;
            total.rows += deleted;
            total.batches += 1;
            self.metrics.pruned(deleted);
            if deleted < batch {
                break;
            }
        }
        Ok(total)
    }
}

#[async_trait]
impl Job for OutboxPrune {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        let done = self.prune(ctx).await?;
        if done.rows > 0 {
            debug!(rows = done.rows, batches = done.batches, "outbox rows pruned");
        }
        Ok(())
    }
}
