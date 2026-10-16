//! One run of the retention job (T9): the staging sweep, the prune of `sync_windows`, and the collection of blobs.
//!
//! Every step is bounded (a batch, a byte limit, a number of statements), checks the lease before each statement that writes, and records
//! its failure as a fixed word without stopping the others. Nothing here formats a hash, a path or a name into a log line or a metric: counts
//! and bytes only (S10). See [`super::plan`] for what may be deleted and why the statements are shaped as they are.

use hub_platform::jobs::JobError;
use tracing::{info, warn};

use super::plan::{self, DELETE_SQL, KeepRules, LOCK_SQL, PLAN_SQL, Plan, SETTINGS_SQL};
use super::{Inner, Retention, RetentionReport};
use crate::db::run_within;
use crate::metrics::{RetentionBlobs, RetentionRows, RetentionStep, RetentionTable};

/// Processed windows whose every time is older than `$1` (Unix ms), the oldest rows first, at most `$2`. A window that is not processed
/// is never deleted here: the watch job owns it until it has processed or expired it.
const WINDOWS_DELETE_SQL: &str = "DELETE FROM sync_windows WHERE id IN (\
     SELECT id FROM sync_windows WHERE processed_at IS NOT NULL \
       AND processed_at < to_timestamp($1::double precision / 1000.0) \
       AND opened_at < to_timestamp($1::double precision / 1000.0) \
       AND (closed_at IS NULL OR closed_at < to_timestamp($1::double precision / 1000.0)) \
     ORDER BY id LIMIT $2)";

/// How many rows [`WINDOWS_DELETE_SQL`] would take, counting at most `$2` (a dry run).
const WINDOWS_COUNT_SQL: &str = "SELECT count(*)::bigint FROM (\
     SELECT 1 FROM sync_windows WHERE processed_at IS NOT NULL \
       AND processed_at < to_timestamp($1::double precision / 1000.0) \
       AND opened_at < to_timestamp($1::double precision / 1000.0) \
       AND (closed_at IS NULL OR closed_at < to_timestamp($1::double precision / 1000.0)) \
     LIMIT $2) w";

/// What [`Retention::collect`] did, in counts and bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Collected {
    /// Blobs deleted, and their bytes.
    pub deleted_blobs: usize,
    pub deleted_bytes: u64,
    /// Blobs of the plan that were left: touched, locked or referenced between the plan and the delete.
    pub skipped_blobs: usize,
    /// Blobs of a transaction that a reference or a lock stopped.
    pub contended_blobs: usize,
}

/// Why a step stopped.
enum Stop {
    /// The lease is not held any more (or whatever `held` said): the run ends, and it is not a failure.
    Job(JobError),
    /// The step failed. The word says which; the run goes on with the others.
    Failed(RetentionStep),
}

/// A chunk of blobs that could not be deleted.
enum Chunk {
    /// A foreign key refused the delete (a reference committed after the check): the chunk is tried once more, with a newer snapshot,
    /// and what it still cannot delete is left for the next run.
    Referenced,
    /// A lock stopped it (`lock_timeout`): left for the next run. Not tried again at once: whoever holds the lock is still there.
    Contended,
    /// The database failed.
    Failed,
}

/// How a database error is told apart (the SQLSTATE; never the text, which can name the host or the SQL).
enum Class {
    /// `23503`: a foreign key refused the delete, because a row that references the blob committed after the reference check. The
    /// constraint did its job; the chunk is tried once more, with a newer snapshot.
    ForeignKey,
    /// `55P03`: `lock_timeout`.
    LockTimeout,
    Other,
}

fn classify(error: &sqlx::Error) -> Class {
    match error {
        sqlx::Error::Database(db) => match db.code().as_deref() {
            Some("23503") => Class::ForeignKey,
            Some("55P03") => Class::LockTimeout,
            _ => Class::Other,
        },
        _ => Class::Other,
    }
}

/// A fixed word for a failure that is neither of those (S21: the driver's own text can name the host or the SQL).
fn why(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::PoolTimedOut => "pool_timed_out",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "io",
        sqlx::Error::Database(_) => "database",
        _ => "other",
    }
}

fn ms(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

fn secs(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}

impl Inner {
    fn now_ms(&self) -> i64 {
        self.clock.now().unix_millis()
    }

    /// The keep rules: the admin's row of `retention_settings`, and Q33's defaults when there is none.
    async fn rules(&self) -> Result<KeepRules, crate::error::RegistryError> {
        let row: Option<(i64, i64)> = self
            .db
            .run(
                "retention_settings",
                sqlx::query_as(SETTINGS_SQL).fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.map_or_else(KeepRules::default, |(versions, days)| KeepRules {
            // The table's own CHECK keeps both at 1 or more; a value that is not is read as the least that keeps something.
            versions: versions.max(1),
            days: days.max(1),
        }))
    }
}

impl Retention {
    /// One run, as a test calls it: the lease is taken to be held.
    ///
    /// # Errors
    ///
    /// `Failed` when a step failed (the others still ran, and the counts of what was done are in the log and the metrics).
    pub async fn run_once(&self) -> Result<RetentionReport, JobError> {
        self.sweep(|| Ok(())).await
    }

    /// One run. `held` is asked before every statement that writes, and ends the run with its error.
    ///
    /// # Errors
    ///
    /// The error of `held` (`LeaseLost`); `Failed` when a step failed (the others still ran).
    pub async fn sweep<F>(&self, held: F) -> Result<RetentionReport, JobError>
    where
        F: Fn() -> Result<(), JobError> + Send + Sync,
    {
        let inner = &*self.inner;
        held()?;
        let dry_run = inner.config.retention_dry_run;
        let mut report = RetentionReport {
            dry_run,
            ..RetentionReport::default()
        };
        let mut failed = false;
        let mut step = |result: Result<(), Stop>| -> Result<(), JobError> {
            match result {
                Ok(()) => Ok(()),
                Err(Stop::Job(e)) => Err(e),
                Err(Stop::Failed(which)) => {
                    failed = true;
                    inner.metrics.retention_failure(which);
                    warn!(
                        step = step_name(which),
                        "a step of the retention run failed; the others still ran"
                    );
                    Ok(())
                }
            }
        };

        // Staging first: the rows it removes are the last references a stale part held.
        let staging = self.staging(&held, dry_run).await;
        step(staging.map(|n| report.staging = n))?;
        let windows = self.windows(&held, dry_run).await;
        step(windows.map(|n| report.windows = n))?;
        let blobs = self.blobs(&held, dry_run).await;
        step(blobs.map(|(planned, collected)| {
            report.keep_versions = planned.rules().versions;
            report.keep_days = planned.rules().days;
            report.planned_blobs = planned.len();
            report.planned_bytes = planned.bytes();
            report.truncated = planned.truncated();
            report.deleted_blobs = collected.deleted_blobs;
            report.deleted_bytes = collected.deleted_bytes;
            report.skipped_blobs = collected.skipped_blobs;
            report.contended_blobs = collected.contended_blobs;
        }))?;

        inner.metrics.retention_run(dry_run, report.truncated);
        // Counts and bytes only (S10).
        info!(
            dry_run,
            keep_versions = report.keep_versions,
            keep_days = report.keep_days,
            planned_blobs = report.planned_blobs,
            planned_bytes = report.planned_bytes,
            truncated = report.truncated,
            deleted_blobs = report.deleted_blobs,
            deleted_bytes = report.deleted_bytes,
            skipped_blobs = report.skipped_blobs,
            contended_blobs = report.contended_blobs,
            windows = report.windows,
            staging = report.staging,
            "retention run"
        );
        if failed {
            return Err(JobError::Failed("registry_retention"));
        }
        Ok(report)
    }

    // ---------------------------------------------------------------------------------------------- blobs

    /// The plan and, unless this is a dry run, its collection.
    async fn blobs<F>(&self, held: &F, dry_run: bool) -> Result<(Plan, Collected), Stop>
    where
        F: Fn() -> Result<(), JobError> + Send + Sync,
    {
        let plan = self.plan().await.map_err(|_| Stop::Failed(RetentionStep::Plan))?;
        if dry_run {
            self.inner
                .metrics
                .retention_blobs(RetentionBlobs::WouldDelete, plan.len() as u64, plan.bytes());
            return Ok((plan, Collected::default()));
        }
        let collected = self.collect(&plan, held).await.map_err(|e| match e {
            JobError::Failed(_) => Stop::Failed(RetentionStep::Collect),
            other @ JobError::LeaseLost => Stop::Job(other),
        })?;
        Ok((plan, collected))
    }

    /// What a run would collect now: the blobs that pass every test, within the batch and the byte limit. Read only, and without a lock:
    /// the plan is a hint, and [`Retention::collect`] tests everything again before it deletes.
    ///
    /// # Errors
    ///
    /// `Failed` when the settings or the plan cannot be read in time.
    pub async fn plan(&self) -> Result<Plan, JobError> {
        let inner = &*self.inner;
        let cfg = &inner.config;
        let rules = inner
            .rules()
            .await
            .map_err(|_| JobError::Failed("registry_retention_settings"))?;
        let now_ms = inner.now_ms();
        let limit = i64::try_from(cfg.gc_batch.saturating_add(1)).unwrap_or(i64::MAX);
        let timeout = cfg.gc_scan_timeout;
        let rows: Vec<(Vec<u8>, i64)> = run_within(timeout, "retention_plan", async {
            // A read-only transaction, so that the server also cancels the scan at the timeout and the plan cannot write.
            let mut tx = inner.db.pool().begin().await?;
            sqlx::query("SET TRANSACTION READ ONLY").execute(&mut *tx).await?;
            sqlx::query("SELECT set_config('statement_timeout', $1, true)")
                .bind(ms(timeout).to_string())
                .execute(&mut *tx)
                .await?;
            sqlx::query_as(&PLAN_SQL)
                .bind(secs(cfg.gc_grace))
                .bind(rules.versions)
                .bind(rules.cutoff_ms(now_ms))
                .bind(limit)
                .fetch_all(&mut *tx)
                .await
        })
        .await
        .map_err(|_| JobError::Failed("registry_retention_plan"))?;
        Ok(plan::cut(rows, cfg.gc_batch, cfg.gc_max_bytes, rules))
    }

    /// Delete the blobs of `plan`, in transactions of `gc_chunk` blobs. Each transaction locks its blobs (`FOR UPDATE SKIP LOCKED`, the age
    /// tested in that statement) and deletes them as a second statement (every test again, on a newer snapshot) before it commits, so a
    /// blob that was stored again, locked by a writer or referenced since the plan is left where it is. `held` is asked before each
    /// transaction. A chunk that a foreign key refuses (a reference committed after the check) is tried once more; one that a lock or the
    /// database stops is left for the next run.
    ///
    /// # Errors
    ///
    /// The error of `held`; `Failed` when the database fails (what was deleted before stays deleted, and is counted).
    pub async fn collect<F>(&self, plan: &Plan, held: &F) -> Result<Collected, JobError>
    where
        F: Fn() -> Result<(), JobError> + Send + Sync,
    {
        let inner = &*self.inner;
        let cfg = &inner.config;
        let rules = plan.rules();
        let grace = secs(cfg.gc_grace);
        let cutoff = rules.cutoff_ms(inner.now_ms());
        let mut done = Collected::default();
        for chunk in plan.candidates().chunks(cfg.gc_chunk) {
            held()?;
            let hashes: Vec<Vec<u8>> = chunk.iter().map(|c| c.hash().as_bytes().to_vec()).collect();
            let planned_bytes: u64 = chunk.iter().map(plan::Candidate::size).sum();
            let mut outcome = self.delete_chunk(&hashes, rules, grace, cutoff).await;
            if matches!(outcome, Err(Chunk::Referenced)) {
                // A foreign key stopped it: the row that references the blob is committed now, so the check of the second try sees it.
                held()?;
                outcome = self.delete_chunk(&hashes, rules, grace, cutoff).await;
            }
            match outcome {
                Ok((deleted, bytes)) => {
                    let skipped = chunk.len().saturating_sub(deleted);
                    done.deleted_blobs += deleted;
                    done.deleted_bytes = done.deleted_bytes.saturating_add(bytes);
                    done.skipped_blobs += skipped;
                    inner
                        .metrics
                        .retention_blobs(RetentionBlobs::Deleted, deleted as u64, bytes);
                    inner.metrics.retention_blobs(
                        RetentionBlobs::Skipped,
                        skipped as u64,
                        planned_bytes.saturating_sub(bytes),
                    );
                }
                Err(Chunk::Contended | Chunk::Referenced) => {
                    done.contended_blobs += chunk.len();
                    inner.metrics.retention_blobs(
                        RetentionBlobs::Contended,
                        chunk.len() as u64,
                        planned_bytes,
                    );
                }
                Err(Chunk::Failed) => {
                    warn!(
                        deleted_blobs = done.deleted_blobs,
                        deleted_bytes = done.deleted_bytes,
                        "a delete transaction of the retention run failed; what was deleted before it stays deleted"
                    );
                    return Err(JobError::Failed("registry_retention_collect"));
                }
            }
        }
        Ok(done)
    }

    /// One transaction: lock, then delete what is still collectable. `(deleted, bytes)`.
    async fn delete_chunk(
        &self,
        hashes: &[Vec<u8>],
        rules: KeepRules,
        grace: i64,
        cutoff_ms: i64,
    ) -> Result<(usize, u64), Chunk> {
        let inner = &*self.inner;
        let cfg = &inner.config;
        let timeout = cfg.query_timeout;
        let fail = |error: &sqlx::Error, op: &'static str| -> Chunk {
            match classify(error) {
                Class::ForeignKey => Chunk::Referenced,
                Class::LockTimeout => {
                    warn!(
                        op,
                        "a retention transaction waited for a lock for too long and was dropped"
                    );
                    Chunk::Contended
                }
                Class::Other => {
                    warn!(op, why = why(error), "a retention statement failed");
                    Chunk::Failed
                }
            }
        };
        let timed_out = |op: &'static str| {
            warn!(op, why = "timed_out", "a retention statement failed");
            Chunk::Failed
        };
        // Dropping the transaction on any early return rolls it back.
        let mut tx = match tokio::time::timeout(timeout, inner.db.pool().begin()).await {
            Ok(Ok(tx)) => tx,
            Ok(Err(e)) => return Err(fail(&e, "retention_begin")),
            Err(_) => return Err(timed_out("retention_begin")),
        };
        // For this transaction only (`is_local`): the session's own settings are never changed, so the connection goes back to the pool as it
        // came.
        let settings = sqlx::query(
            "SELECT set_config('statement_timeout', $1, true), set_config('lock_timeout', $2, true)",
        )
        .bind(ms(timeout).to_string())
        .bind(ms(cfg.gc_lock_timeout).to_string())
        .execute(&mut *tx);
        match tokio::time::timeout(timeout, settings).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(fail(&e, "retention_settings")),
            Err(_) => return Err(timed_out("retention_settings")),
        }
        let lock = sqlx::query_scalar::<_, Vec<u8>>(LOCK_SQL)
            .bind(grace)
            .bind(hashes)
            .fetch_all(&mut *tx);
        let locked = match tokio::time::timeout(timeout, lock).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => return Err(fail(&e, "retention_lock")),
            Err(_) => return Err(timed_out("retention_lock")),
        };
        if locked.is_empty() {
            return Ok((0, 0));
        }
        // A statement of its own, after the lock: READ COMMITTED gives it a snapshot that sees every reference committed so far.
        let delete = sqlx::query_scalar::<_, i64>(&DELETE_SQL)
            .bind(grace)
            .bind(rules.versions)
            .bind(cutoff_ms)
            .bind(&locked)
            .fetch_all(&mut *tx);
        let sizes = match tokio::time::timeout(timeout, delete).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => return Err(fail(&e, "retention_delete")),
            Err(_) => return Err(timed_out("retention_delete")),
        };
        match tokio::time::timeout(timeout, tx.commit()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(fail(&e, "retention_commit")),
            Err(_) => return Err(timed_out("retention_commit")),
        }
        let bytes = sizes
            .iter()
            .map(|s| u64::try_from(*s).unwrap_or(0))
            .fold(0_u64, u64::saturating_add);
        Ok((sizes.len(), bytes))
    }

    // ---------------------------------------------------------------------------------------------- sync windows

    /// Delete the processed windows older than `sync_window_retention`, in batches; in a dry run, count them.
    async fn windows<F>(&self, held: &F, dry_run: bool) -> Result<usize, Stop>
    where
        F: Fn() -> Result<(), JobError> + Send + Sync,
    {
        let inner = &*self.inner;
        let cfg = &inner.config;
        let cutoff = inner.now_ms().saturating_sub(ms(cfg.sync_window_retention));
        let batch = i64::try_from(cfg.retention_prune_batch).unwrap_or(i64::MAX);
        if dry_run {
            let n: i64 =
                inner
                    .db
                    .run(
                        "retention_windows_count",
                        sqlx::query_scalar(WINDOWS_COUNT_SQL)
                            .bind(cutoff)
                            .bind(batch.saturating_mul(
                                i64::try_from(cfg.retention_prune_batches).unwrap_or(i64::MAX),
                            ))
                            .fetch_one(inner.db.pool()),
                    )
                    .await
                    .map_err(|_| Stop::Failed(RetentionStep::SyncWindows))?;
            let n = usize::try_from(n).unwrap_or(0);
            inner
                .metrics
                .retention_rows(RetentionTable::SyncWindows, RetentionRows::WouldDelete, n as u64);
            return Ok(n);
        }
        let mut total = 0_usize;
        for _ in 0..cfg.retention_prune_batches {
            held().map_err(Stop::Job)?;
            let done = inner
                .db
                .run(
                    "retention_windows",
                    sqlx::query(WINDOWS_DELETE_SQL)
                        .bind(cutoff)
                        .bind(batch)
                        .execute(inner.db.pool()),
                )
                .await
                .map_err(|_| Stop::Failed(RetentionStep::SyncWindows))?;
            let n = usize::try_from(done.rows_affected()).unwrap_or(usize::MAX);
            total = total.saturating_add(n);
            inner
                .metrics
                .retention_rows(RetentionTable::SyncWindows, RetentionRows::Deleted, n as u64);
            if n < cfg.retention_prune_batch {
                break;
            }
        }
        Ok(total)
    }

    // ---------------------------------------------------------------------------------------------- delta staging

    /// Sweep the stale staged parts through ingestion's own sweep, one bounded statement at a time; in a dry run, count them.
    async fn staging<F>(&self, held: &F, dry_run: bool) -> Result<usize, Stop>
    where
        F: Fn() -> Result<(), JobError> + Send + Sync,
    {
        let inner = &*self.inner;
        let cfg = &inner.config;
        let batch = cfg.retention_prune_batch;
        if dry_run {
            let n = inner
                .ingest
                .stale_staging_count(batch.saturating_mul(cfg.retention_prune_batches))
                .await
                .map_err(|_| Stop::Failed(RetentionStep::DeltaStaging))?;
            let n = usize::try_from(n).unwrap_or(usize::MAX);
            inner
                .metrics
                .retention_rows(RetentionTable::DeltaStaging, RetentionRows::WouldDelete, n as u64);
            return Ok(n);
        }
        let mut total = 0_usize;
        for _ in 0..cfg.retention_prune_batches {
            held().map_err(Stop::Job)?;
            let deleted = inner
                .ingest
                .sweep_staging_batch(batch)
                .await
                .map_err(|_| Stop::Failed(RetentionStep::DeltaStaging))?;
            let n = usize::try_from(deleted).unwrap_or(usize::MAX);
            total = total.saturating_add(n);
            inner
                .metrics
                .retention_rows(RetentionTable::DeltaStaging, RetentionRows::Deleted, deleted);
            if n < batch {
                break;
            }
        }
        Ok(total)
    }
}

fn step_name(step: RetentionStep) -> &'static str {
    match step {
        RetentionStep::Settings => "settings",
        RetentionStep::Plan => "plan",
        RetentionStep::Collect => "collect",
        RetentionStep::SyncWindows => "sync_windows",
        RetentionStep::DeltaStaging => "delta_staging",
    }
}
