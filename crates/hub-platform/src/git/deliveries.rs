//! `webhook_deliveries` on Postgres: the replay guard of the GitHub webhook, and its daily prune (D64, S18).
//!
//! A delivery id is inserted with `ON CONFLICT DO NOTHING`; the insert that adds a row is the first time, and one that adds none is
//! a replay. Ids older than the retention are deleted by a leased job, in bounded batches, so the table (and its index on
//! `received_at`) stays small however many deliveries GitHub makes.

use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;

use super::webhook::{DeliveryError, DeliveryLog};
use crate::jobs::{Job, JobContext, JobError, JobSpec, SpecError};

/// One database call.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a delivery id is remembered. GitHub redelivers within hours; a week is generous.
pub const RETENTION_DAYS: i32 = 7;
/// Rows deleted per statement.
pub const PRUNE_BATCH: i64 = 5_000;
/// Statements per run: 20 batches is 100,000 rows, far more than a day of deliveries.
const MAX_BATCHES: usize = 20;

pub const PRUNE_JOB: &str = "webhook-deliveries-prune";

/// The replay guard over `webhook_deliveries`.
#[derive(Clone)]
pub struct PgDeliveryLog {
    pool: PgPool,
}

impl std::fmt::Debug for PgDeliveryLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgDeliveryLog").finish_non_exhaustive()
    }
}

impl PgDeliveryLog {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Delete up to `batch` delivery ids older than `days`. Returns how many.
    pub async fn prune(&self, days: i32, batch: i64) -> Result<u64, DeliveryError> {
        let call = sqlx::query(
            "DELETE FROM webhook_deliveries WHERE delivery_id IN \
             (SELECT delivery_id FROM webhook_deliveries \
              WHERE received_at < now() - make_interval(days => $1) \
              ORDER BY received_at LIMIT $2)",
        )
        .bind(days)
        .bind(batch)
        .execute(&self.pool);
        match tokio::time::timeout(QUERY_TIMEOUT, call).await {
            Ok(Ok(done)) => Ok(done.rows_affected()),
            Ok(Err(_)) | Err(_) => Err(DeliveryError::Unavailable),
        }
    }
}

#[async_trait]
impl DeliveryLog for PgDeliveryLog {
    async fn first_time(&self, id: &str, event: &str) -> Result<bool, DeliveryError> {
        let call = sqlx::query(
            "INSERT INTO webhook_deliveries (delivery_id, event) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(event)
        .execute(&self.pool);
        match tokio::time::timeout(QUERY_TIMEOUT, call).await {
            Ok(Ok(done)) => Ok(done.rows_affected() == 1),
            Ok(Err(_)) | Err(_) => Err(DeliveryError::Unavailable),
        }
    }
}

/// The daily prune, under a lease so that one replica does it.
#[derive(Debug, Clone)]
pub struct DeliveryPrune {
    log: PgDeliveryLog,
}

impl DeliveryPrune {
    #[must_use]
    pub fn new(log: PgDeliveryLog) -> Self {
        Self { log }
    }

    pub fn spec() -> Result<JobSpec, SpecError> {
        JobSpec::new(
            PRUNE_JOB,
            Duration::from_secs(24 * 3600),
            Duration::from_secs(3600),
        )
    }
}

#[async_trait]
impl Job for DeliveryPrune {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        for _ in 0..MAX_BATCHES {
            ctx.ensure_held()?;
            let deleted = self
                .log
                .prune(RETENTION_DAYS, PRUNE_BATCH)
                .await
                .map_err(|_| JobError::Failed("webhook_deliveries_prune"))?;
            if deleted < PRUNE_BATCH.unsigned_abs() {
                break;
            }
        }
        Ok(())
    }
}
