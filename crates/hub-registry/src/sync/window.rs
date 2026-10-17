//! Sync windows (T8, D75): what the hub does when a sync Job's window closes, and when one stays open too long.
//!
//! The agent reports a Job as running and as finished in its cluster reports ([`domain::ClusterReport::sync_windows`]). **Opening and
//! closing the rows** is ingestion's, in the transaction of the report that carries the event ([`crate::ingest`], T1): one row in
//! `sync_windows` for each (swimlane, Job uid), however many times the report is repeated, with `SyncWindowOpened` and
//! `SyncWindowClosed` in the outbox once each. The agent's times are clamped to the hub's clock there. What follows is the rest of
//! the life of a window, done by the leased job [`WATCH_JOB`] (`registry-sync-watch`), which runs every `sync_watch_interval`:
//!
//! | Step | What |
//! |---|---|
//! | 0 | **Old windows.** A closed window that nobody processed for `sync_window_expire_after` (24 h) is marked done and nothing is said: the hub was down, or the row is older than this job. |
//! | 0b | **The hold budget.** When a swimlane's windows have held its alerts for `sync_hold_budget` (4 h) within `sync_hold_budget_window` (24 h) the hold has ended (the gate says so from the rows, [`super::hold`]); the job says it once for each episode: a [`NotificationKind::SyncWindowStale`] notice (Medium), a log line and a metric. "Once" is `warned_at` of the swimlane's newest window: the episode has been said when some window of the swimlane has a `warned_at` at or after the instant the episode began, and the claim checks and writes it in one statement. |
//! | 1 | **Stale windows.** A window open for more than `sync_window_stale_after` (2 h) is warned about **once**: a log line, a metric and a [`NotificationKind::SyncWindowStale`] notification (A35). It stops holding the swimlane's alerts ([`super::hold`]). |
//! | 2 | **Closed windows.** For each swimlane with closed windows that nobody has processed, and no window still open (a second Job is still running: wait for it), all of its closed windows are processed together, as one. |
//!
//! # Processing a swimlane's closed windows
//!
//! The window carries no commit (D21 is undecided, A33), so the commits the sync deployed are the **current heads of the mirrored
//! branches**: the base repository's default branch and the branch of each tenant the swimlane deploys. Then the pieces of
//! `record_sync` ([`super`]) run, with **one scan request for the whole swimlane**, and then each tenant:
//!
//! 1. The files that are ahead on NFS are written down (before the scan, because the pipeline recomputes them as soon as it is applied).
//! 2. One full scan is requested and waited for.
//! 3. For each tenant, in order: **C7 first** (the files the sync replaced), then the baselines at that tenant's head, then the deployment
//!    record. The base files are claimed by the first tenant and not looked at again.
//! 4. The swimlane's state is allowed to settle (`sync_settle`): the pipeline recomputes what the scan changed after the scan was applied.
//! 5. **One alert evaluation** for the windows ([`super::alerts`]): one notification for each severity, whatever the number of files.
//! 6. The windows are marked processed, which ends their hold.
//!
//! A swimlane whose baseline nobody has confirmed gets its deployments recorded and nothing else (D22): there is no baseline to
//! compare a sync with, and no drift to alert on. Every step is idempotent, so a processing that failed is done again by the next run: the
//! agent is not connected, the scan did not come in time, the database was down. A window that has failed for `sync_close_give_up` is given
//! up: its alerts are evaluated without the sync's processing (a drift that was held must not be lost because the agent is gone), and it
//! is marked processed.
//!
//! # What is not here
//!
//! - **Retention of the rows.** `sync_windows` keeps a row for each Job for ever; its size is a row for each sync of each swimlane, and
//!   no task owns pruning it. The watch job's queries are bounded by `LIMIT`.
//! - **C7 from a version the pipeline had already recomputed** before the window was processed is found from the findings the pipeline
//!   wrote since the window opened ([`super::alerts`]); a window processed after a crash finds the findings and not the claim.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{Actor, CommitId, JobRef, NfsPath, Severity, ShortText, SwimlaneId, TenantId};
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use ports::{Notification, NotificationKind};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::hold::{Policy, exhaustion_start, held_spans, load_times, spent};
use super::overwrite::Ahead;
use super::{Inner, Report, SyncError, SyncService, SyncSource};
use crate::branches::BranchClassifier;
use crate::db::{ms, ts};
use crate::error::RegistryError;
use crate::metrics::WindowResult;
use crate::pipeline::blocking;
use crate::pipeline::git_side::live_heads;

/// The job's name, which is also its lease's name.
pub const WATCH_JOB: &str = "registry-sync-watch";

/// The source recorded with the deployments that a closed window makes (`SyncSource`).
const WINDOW_SOURCE: &str = "window";

/// The most closed windows of one swimlane read for one processing; more are processed by the next run.
const MAX_WINDOWS: i64 = 1_000;

/// What one run of the watch job did, in counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatchReport {
    /// Closed windows that nobody processed for `sync_window_expire_after` and that were marked done without anything being said.
    pub expired: usize,
    /// Open windows that were warned about as stale.
    pub warned: usize,
    /// Swimlanes whose closed windows were looked at.
    pub swimlanes: usize,
    /// Windows processed in full (the sync recorded, the alerts evaluated), or only their deployments (an unadopted swimlane).
    pub processed: usize,
    /// Windows given up: their alerts were evaluated without the sync's processing.
    pub given_up: usize,
    /// Swimlanes left for the next run: a Job is still open there, or the processing could not be done now.
    pub deferred: usize,
}

/// A closed window nobody has processed.
#[derive(Debug, Clone)]
struct ClosedWindow {
    id: i64,
    job: JobRef,
    opened_ms: i64,
    closed_ms: i64,
}

/// What came of one swimlane's windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Done {
    Processed,
    DeploymentOnly,
    GivenUp,
    /// The swimlane is gone: there is nothing to process.
    Dropped,
}

/// What the sync's own processing found.
#[derive(Debug, Default)]
struct Synced {
    /// The files whose NFS-only change the sync replaced (C7).
    claimed: BTreeSet<NfsPath>,
}

impl SyncService {
    /// How the watch job is scheduled: every `sync_watch_interval`, under a 90 second lease that the runner renews every 30.
    ///
    /// # Errors
    ///
    /// A spec the runner refuses (the interval is range-checked by `RegistryConfig::validate`).
    pub fn watch_spec(config: &crate::config::RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(WATCH_JOB, config.sync_watch_interval, Duration::from_secs(90))?
            .with_max_run(Duration::from_secs(30 * 60))
    }

    /// The watch job (`registry-sync-watch`), for the hub's job runner.
    pub fn watch_job(&self) -> Arc<dyn Job> {
        Arc::new(WatchJob {
            service: self.clone(),
        })
    }

    /// One run of the watch job: the stale windows are warned about, and the closed windows are processed. `held` is asked before each
    /// unit of work that writes and ends the run with its error.
    ///
    /// # Errors
    ///
    /// `LeaseLost` from `held`; `Failed` when the database cannot be read for the list of windows.
    pub async fn watch(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send,
    ) -> Result<WatchReport, JobError> {
        let inner = &self.inner;
        let mut report = WatchReport::default();
        held()?;
        report.expired = inner
            .expire_old()
            .await
            .map_err(|_| JobError::Failed("registry_sync_watch"))?;
        held()?;
        inner
            .warn_exhausted()
            .await
            .map_err(|_| JobError::Failed("registry_sync_watch"))?;
        held()?;
        report.warned = inner
            .warn_stale()
            .await
            .map_err(|_| JobError::Failed("registry_sync_watch"))?;
        held()?;
        let swimlanes = inner
            .swimlanes_with_closed_windows()
            .await
            .map_err(|_| JobError::Failed("registry_sync_watch"))?;
        for swimlane in swimlanes {
            held()?;
            report.swimlanes += 1;
            let windows = match inner.closed_windows(&swimlane).await {
                Ok(w) if !w.is_empty() => w,
                Ok(_) => continue,
                Err(_) => {
                    report.deferred += 1;
                    continue;
                }
            };
            match inner.still_running(&swimlane).await {
                Ok(false) => {}
                Ok(true) => {
                    debug!(swimlane = %swimlane, windows = windows.len(), "a sync Job is still running: the closed windows wait");
                    report.deferred += 1;
                    continue;
                }
                Err(_) => {
                    report.deferred += 1;
                    continue;
                }
            }
            match inner.process_windows(&swimlane, &windows).await {
                Ok(done) => {
                    let (result, count) = match done {
                        Done::Processed => (WindowResult::Processed, &mut report.processed),
                        Done::DeploymentOnly => (WindowResult::DeploymentOnly, &mut report.processed),
                        Done::GivenUp => (WindowResult::GivenUp, &mut report.given_up),
                        Done::Dropped => (WindowResult::Dropped, &mut report.processed),
                    };
                    for _ in &windows {
                        inner.metrics.window(result);
                    }
                    *count += windows.len();
                }
                Err(why) => {
                    // Nothing was lost: the windows are still unprocessed, and every step is idempotent.
                    info!(swimlane = %swimlane, windows = windows.len(), ?why, "the closed windows are processed on the next run");
                    inner.metrics.window(WindowResult::Deferred);
                    report.deferred += 1;
                }
            }
        }
        Ok(report)
    }
}

struct WatchJob {
    service: SyncService,
}

#[async_trait]
impl Job for WatchJob {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.service.watch(|| ctx.ensure_held()).await.map(|_| ())
    }
}

fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

impl Inner {
    // ------------------------------------------------------------------------------------------------ old windows

    /// Mark done the closed windows that nobody processed for `sync_window_expire_after`: the hub was down for that long, or the rows
    /// are older than this job, and a sync that old is neither worth a scan nor an alert. It also bounds every retry (a window that
    /// cannot be processed for any reason is retried until `sync_close_give_up`, given up after it, and expired at the latest here).
    async fn expire_old(&self) -> Result<usize, RegistryError> {
        let now = self.clock.now().unix_millis();
        let cutoff = now.saturating_sub(millis(self.config.sync_window_expire_after));
        let sql = format!(
            "UPDATE sync_windows SET processed_at = {now} WHERE id IN (\
               SELECT id FROM sync_windows WHERE closed_at IS NOT NULL AND processed_at IS NULL AND closed_at < {cutoff} \
               ORDER BY id LIMIT $3)",
            now = ts("$1"),
            cutoff = ts("$2"),
        );
        let done = self
            .db
            .run(
                "window_expire",
                sqlx::query(&sql)
                    .bind(now)
                    .bind(cutoff)
                    .bind(i64::try_from(self.config.sync_watch_batch.saturating_mul(64)).unwrap_or(i64::MAX))
                    .execute(self.db.pool()),
            )
            .await?;
        let n = usize::try_from(done.rows_affected()).unwrap_or(usize::MAX);
        if n > 0 {
            info!(
                windows = n,
                "closed sync windows that nobody processed for too long were marked done"
            );
            for _ in 0..n {
                self.metrics.window(WindowResult::Expired);
            }
        }
        Ok(n)
    }

    // ------------------------------------------------------------------------------------------------ the hold budget

    /// Say, once for each episode of exhaustion, that a swimlane's budget for holding alerts is spent (see [`super::hold`]): the
    /// enforcement is the gate's, derived from the windows' rows on every question; this is only the signal to people.
    ///
    /// "Once" is durable: the episode began at [`exhaustion_start`], and it has been said when some window of the swimlane has a
    /// `warned_at` at or after that instant. The claim writes `warned_at` on the swimlane's newest window in the same statement that
    /// checks it, so two runs (or two replicas) never say it twice, and a restart cannot make it say it again.
    async fn warn_exhausted(&self) -> Result<(), RegistryError> {
        let now = self.clock.now().unix_millis();
        let p = Policy::of(&self.config);
        // A swimlane can be spent only if some window of it held in the rolling window that ends now.
        let from = now.saturating_sub(p.within);
        let sql = format!(
            "SELECT swimlane_id FROM sync_windows WHERE opened_at > {open} OR closed_at > {closed} \
             GROUP BY swimlane_id ORDER BY swimlane_id LIMIT $3",
            open = ts("$1"),
            closed = ts("$2"),
        );
        let names: Vec<String> = self
            .db
            .run(
                "hold_budget_swimlanes",
                sqlx::query_scalar(&sql)
                    .bind(from.saturating_sub(p.stale_after))
                    .bind(from.saturating_sub(p.give_up))
                    .bind(i64::try_from(self.config.sync_watch_batch.saturating_mul(16)).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        for name in names {
            let Ok(swimlane) = SwimlaneId::parse(&name) else {
                continue;
            };
            let times = match load_times(&self.db, &swimlane, now, &p).await {
                Ok(times) => times,
                // Too many windows to count: the gate counts the budget as spent; there is no instant to say it began.
                Err(RegistryError::Capacity) => {
                    debug!(swimlane = %swimlane, "a swimlane has more sync windows than a hold budget can count");
                    continue;
                }
                Err(e) => return Err(e),
            };
            let Some(began) = exhaustion_start(&times, now, &p) else {
                continue;
            };
            if !self.claim_exhaustion(&swimlane, began, now).await? {
                continue;
            }
            let used_minutes = spent(&times, now, &p) / 60_000;
            let (budget_minutes, within_minutes) = (p.budget / 60_000, p.within / 60_000);
            warn!(
                swimlane = %swimlane,
                held_minutes = used_minutes,
                budget_minutes,
                within_minutes,
                "the hold budget of a swimlane is spent: its drift alerts are no longer held until the budget refills"
            );
            self.metrics.hold_budget_exhaustion();
            if let Some(n) = exhausted_notification(&swimlane, used_minutes, budget_minutes, within_minutes) {
                self.alerts.notify(n).await;
            }
        }
        Ok(())
    }

    /// Claim the notice of the episode that began at `began`: `true` for the one run that writes it.
    async fn claim_exhaustion(
        &self,
        swimlane: &SwimlaneId,
        began: i64,
        now: i64,
    ) -> Result<bool, RegistryError> {
        let sql = format!(
            "UPDATE sync_windows SET warned_at = {now} \
             WHERE id = (SELECT id FROM sync_windows WHERE swimlane_id = $1 ORDER BY opened_at DESC, id DESC LIMIT 1) \
               AND (warned_at IS NULL OR warned_at < {began}) \
               AND NOT EXISTS (SELECT 1 FROM sync_windows WHERE swimlane_id = $1 AND warned_at >= {began} LIMIT 1)",
            now = ts("$2"),
            began = ts("$3"),
        );
        let done = self
            .db
            .run(
                "hold_budget_claim",
                sqlx::query(&sql)
                    .bind(swimlane.as_str())
                    .bind(now)
                    .bind(began)
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(done.rows_affected() == 1)
    }

    // ------------------------------------------------------------------------------------------------ stale windows

    /// Warn once about every open window past `sync_window_stale_after`. The window is claimed (`warned_at`) before anything is said, so
    /// two runs never say it twice, and the log line and the metric exist whatever the notifier does.
    async fn warn_stale(&self) -> Result<usize, RegistryError> {
        let now = self.clock.now().unix_millis();
        let cutoff = now.saturating_sub(millis(self.config.sync_window_stale_after));
        // A window is warned about when nobody has said anything about it since it went stale: `warned_at` is also what says that the
        // budget ran out ([`Inner::warn_exhausted`]), which can be earlier, and must not take the stale warning away.
        let sql = format!(
            "UPDATE sync_windows SET warned_at = {now} WHERE id IN (\
               SELECT id FROM sync_windows WHERE closed_at IS NULL \
                 AND (warned_at IS NULL OR warned_at < opened_at + ($4::bigint::text || ' milliseconds')::interval) \
                 AND opened_at <= {cutoff} \
               ORDER BY id LIMIT $3) RETURNING swimlane_id, job_name, {opened}",
            now = ts("$1"),
            cutoff = ts("$2"),
            opened = ms("opened_at"),
        );
        let rows: Vec<(String, String, i64)> = self
            .db
            .run(
                "window_stale",
                sqlx::query_as(&sql)
                    .bind(now)
                    .bind(cutoff)
                    .bind(i64::try_from(self.config.sync_watch_batch.saturating_mul(16)).unwrap_or(i64::MAX))
                    .bind(millis(self.config.sync_window_stale_after))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let minutes = millis(self.config.sync_window_stale_after) / 60_000;
        for (swimlane, job, opened_ms) in &rows {
            warn!(
                swimlane = %swimlane,
                job = %job,
                open_minutes = (now - opened_ms) / 60_000,
                "a sync window has been open for too long: the Job may be stuck, and the swimlane's drift alerts are no longer held"
            );
            self.metrics.window_stale_warning();
            let Ok(id) = SwimlaneId::parse(swimlane) else {
                continue;
            };
            if let Some(n) = stale_notification(&id, minutes) {
                self.alerts.notify(n).await;
            }
        }
        Ok(rows.len())
    }

    // ------------------------------------------------------------------------------------------------ closed windows

    async fn swimlanes_with_closed_windows(&self) -> Result<Vec<SwimlaneId>, RegistryError> {
        let names: Vec<String> = self
            .db
            .run(
                "windows_swimlanes",
                sqlx::query_scalar(
                    "SELECT swimlane_id FROM sync_windows WHERE closed_at IS NOT NULL AND processed_at IS NULL \
                     GROUP BY swimlane_id ORDER BY min(closed_at), swimlane_id LIMIT $1",
                )
                .bind(i64::try_from(self.config.sync_watch_batch).unwrap_or(i64::MAX))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(names.iter().filter_map(|n| SwimlaneId::parse(n).ok()).collect())
    }

    async fn closed_windows(&self, swimlane: &SwimlaneId) -> Result<Vec<ClosedWindow>, RegistryError> {
        let sql = format!(
            "SELECT id, job_name, job_uid, {opened}, {closed} FROM sync_windows \
             WHERE swimlane_id = $1 AND closed_at IS NOT NULL AND processed_at IS NULL ORDER BY closed_at, id LIMIT $2",
            opened = ms("opened_at"),
            closed = ms("closed_at"),
        );
        let rows: Vec<(i64, String, String, i64, i64)> = self
            .db
            .run(
                "windows_closed",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(MAX_WINDOWS)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, name, uid, opened_ms, closed_ms)| {
                Some(ClosedWindow {
                    id,
                    job: JobRef::new(&name, &uid).ok()?,
                    opened_ms,
                    closed_ms,
                })
            })
            .collect())
    }

    /// A Job is running in the swimlane now: a window that is open and not stale. Its sync is not finished, so a closed window waits.
    async fn still_running(&self, swimlane: &SwimlaneId) -> Result<bool, RegistryError> {
        let cutoff = self
            .clock
            .now()
            .unix_millis()
            .saturating_sub(millis(self.config.sync_window_stale_after));
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM sync_windows WHERE swimlane_id = $1 AND closed_at IS NULL AND opened_at > {cutoff})",
            cutoff = ts("$2")
        );
        self.db
            .run(
                "windows_open",
                sqlx::query_scalar(&sql)
                    .bind(swimlane.as_str())
                    .bind(cutoff)
                    .fetch_one(self.db.pool()),
            )
            .await
    }

    /// Process the closed windows of one swimlane as one.
    async fn process_windows(
        &self,
        swimlane: &SwimlaneId,
        windows: &[ClosedWindow],
    ) -> Result<Done, SyncError> {
        let now = self.clock.now().unix_millis();
        // What the windows held: the evaluation looks at what was written then, because the rest was told when it happened.
        let spans = held_spans(
            &windows
                .iter()
                .map(|w| (w.opened_ms, w.closed_ms))
                .collect::<Vec<_>>(),
            now,
            millis(self.config.sync_window_stale_after),
            millis(self.config.max_clock_skew),
        );
        let oldest_close = windows.iter().map(|w| w.closed_ms).min().unwrap_or(now);
        let given_up = now.saturating_sub(oldest_close) >= millis(self.config.sync_close_give_up);

        let lane = match self.lane(swimlane).await {
            Ok(l) => l,
            Err(SyncError::UnknownSwimlane) => {
                // The swimlane is gone (its windows go with it); if any are left there is nothing to record.
                self.mark_processed(swimlane, windows).await?;
                return Ok(Done::Dropped);
            }
            Err(e) => return Err(e),
        };

        let (synced, done) = if given_up {
            warn!(swimlane = %swimlane, windows = windows.len(), "closed sync windows given up: their alerts are evaluated without the sync's processing");
            (Synced::default(), Done::GivenUp)
        } else {
            let synced = self.record_window_sync(swimlane, windows, &lane).await?;
            (
                synced,
                if lane.adopted {
                    Done::Processed
                } else {
                    Done::DeploymentOnly
                },
            )
        };

        // An unadopted swimlane has no baseline, so nothing in it is drift (D22): nothing is evaluated.
        if lane.adopted {
            if !given_up {
                self.settle(swimlane).await;
            }
            let evaluation = self.alerts.evaluate(swimlane, &spans, &synced.claimed).await?;
            info!(
                swimlane = %swimlane,
                windows = windows.len(),
                c7 = evaluation.c7,
                drift = evaluation.drift,
                notifications = evaluation.notifications,
                sent = evaluation.sent,
                "sync window evaluated"
            );
        }
        self.mark_processed(swimlane, windows).await?;
        Ok(done)
    }

    /// The sync's own processing: the pieces of `record_sync`, with one scan for the swimlane and then each tenant (A33).
    async fn record_window_sync(
        &self,
        swimlane: &SwimlaneId,
        windows: &[ClosedWindow],
        lane: &super::Lane,
    ) -> Result<Synced, SyncError> {
        // The deadline is fixed before anything is asked for, as `record_sync` does.
        let deadline = self
            .clock
            .now()
            .unix_millis()
            .saturating_add(millis(self.config.sync_scan_timeout));

        // What Git has now: the commits the sync deployed (A33).
        let git = Arc::clone(&self.git);
        let heads = blocking(self.config.git_timeout, move || live_heads(&*git)).await?;
        let Some((_, base_commit)) = heads.base.clone() else {
            return Err(SyncError::UnknownCommit);
        };
        let classifier = BranchClassifier::load(&self.db).await?;
        let tenants: Vec<(TenantId, CommitId)> = lane
            .rules
            .tenants
            .iter()
            .filter(|t| classifier.is_deployable(t.as_str()))
            .filter_map(|t| heads.tenants.get(t).map(|c| (t.clone(), c.clone())))
            .collect();
        if tenants.is_empty() {
            // Nothing the swimlane deploys is mirrored (yet): there is no commit to record, and nothing to compare a scan with.
            return if lane.rules.tenants.is_empty() {
                Ok(Synced::default())
            } else {
                Err(SyncError::UnknownCommit)
            };
        }
        let Some(latest) = windows.iter().max_by_key(|w| (w.closed_ms, w.id)) else {
            return Ok(Synced::default());
        };
        // The Job that finished last is the author of the sync's changes.
        let actor = Actor::Job {
            job: latest.job.clone(),
        };
        let source = SyncSource::parse(WINDOW_SOURCE)?;
        let reports: Vec<Report> = tenants
            .into_iter()
            .map(|(tenant, tenant_commit)| Report {
                swimlane: swimlane.clone(),
                tenant,
                tenant_commit,
                base_commit: base_commit.clone(),
                actor: actor.clone(),
                source: source.clone(),
            })
            .collect();

        if !lane.adopted {
            // Before the adoption pass confirms a baseline there is nothing to check a sync against: only the deployments.
            for r in &reports {
                self.record_deployment(r).await?;
            }
            return Ok(Synced::default());
        }

        let Some(first) = reports.first() else {
            return Ok(Synced::default());
        };
        // 1. The states C7 needs, before anything can change them. 2. ONE scan for the whole swimlane.
        let ahead: Vec<Ahead> = super::overwrite::snapshot(&self.db, swimlane).await?;
        self.request_scan(first).await?;
        self.await_scan(first, deadline).await?;

        // 3. Each tenant: C7 first, then its baselines, then its deployment.
        let mut remaining = ahead;
        let mut claimed: BTreeSet<NfsPath> = BTreeSet::new();
        let mut baselined: BTreeSet<NfsPath> = BTreeSet::new();
        for r in &reports {
            let taken = self.claim(r, &lane.rules, remaining.clone()).await?;
            remaining.retain(|a| !taken.contains(&a.path));
            claimed.extend(taken);
            baselined.extend(self.set_baselines(r, &lane.rules).await?);
            self.record_deployment(r).await?;
        }
        self.metrics.sync_overwritten(claimed.len() as u64);
        self.metrics.sync_baselines(baselined.len() as u64);
        if let Some(handle) = &self.pipeline {
            let changed: BTreeSet<NfsPath> = claimed.iter().chain(&baselined).cloned().collect();
            handle.paths_changed(swimlane.clone(), changed.into_iter().collect());
        }
        info!(
            swimlane = %swimlane,
            windows = windows.len(),
            tenants = reports.len(),
            baselines = baselined.len(),
            overwritten = claimed.len(),
            "sync window recorded"
        );
        Ok(Synced { claimed })
    }

    /// Wait until the swimlane's derived state has been quiet for `sync_settle`, at most `sync_settle_max`: the pipeline recomputes what
    /// the scan changed after the scan was applied, and the evaluation reads what it computed. Never fails: a swimlane that cannot be read
    /// is evaluated as it is.
    async fn settle(&self, swimlane: &SwimlaneId) {
        let quiet = self.config.sync_settle;
        if quiet.is_zero() {
            return;
        }
        let poll = self.config.sync_poll_interval.min(quiet);
        let started = Instant::now();
        let Some(mut last) = self.version(swimlane).await else {
            return;
        };
        let mut quiet_since = started;
        loop {
            tokio::time::sleep(poll).await;
            let Some(now) = self.version(swimlane).await else {
                return;
            };
            if now != last {
                last = now;
                quiet_since = Instant::now();
            }
            if quiet_since.elapsed() >= quiet || started.elapsed() >= self.config.sync_settle_max {
                return;
            }
        }
    }

    /// The version of the swimlane's derived state (it moves when the pipeline commits a change).
    async fn version(&self, swimlane: &SwimlaneId) -> Option<i64> {
        self.db
            .run(
                "window_settle",
                sqlx::query_scalar("SELECT version FROM swimlane_registry WHERE swimlane_id = $1")
                    .bind(swimlane.as_str())
                    .fetch_optional(self.db.pool()),
            )
            .await
            .ok()
            .flatten()
    }

    /// The windows are done: they hold nothing any more. Only those that are not processed yet are written, so a second run says nothing.
    async fn mark_processed(&self, swimlane: &SwimlaneId, windows: &[ClosedWindow]) -> Result<(), SyncError> {
        let ids: Vec<i64> = windows.iter().map(|w| w.id).collect();
        let sql = format!(
            "UPDATE sync_windows SET processed_at = {now} WHERE swimlane_id = $1 AND id = ANY($2::bigint[]) AND processed_at IS NULL",
            now = ts("$3")
        );
        self.db
            .run(
                "window_processed",
                sqlx::query(&sql)
                    .bind(swimlane.as_str())
                    .bind(&ids)
                    .bind(self.clock.now().unix_millis())
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }
}

/// The warning for a window that is open too long: the swimlane and how long, never a Job's content.
fn stale_notification(swimlane: &SwimlaneId, minutes: i64) -> Option<Notification> {
    Some(Notification {
        kind: NotificationKind::SyncWindowStale,
        severity: Severity::Medium,
        swimlane: Some(swimlane.clone()),
        title: ShortText::parse("A sync Job has been running for too long").ok()?,
        summary: ShortText::parse(&format!(
            "The sync window of {swimlane} has been open for more than {minutes} minutes. The Job may be stuck; the swimlane's drift alerts are no longer held."
        ))
        .ok()?,
        link: ShortText::parse(&format!("/swimlanes/{swimlane}")).ok(),
    })
}

/// The notice that a swimlane's budget for holding alerts is spent: the swimlane and minutes, never anything else.
fn exhausted_notification(
    swimlane: &SwimlaneId,
    held_minutes: i64,
    budget_minutes: i64,
    within_minutes: i64,
) -> Option<Notification> {
    Some(Notification {
        kind: NotificationKind::SyncWindowStale,
        severity: Severity::Medium,
        swimlane: Some(swimlane.clone()),
        title: ShortText::parse("Drift alerts are no longer held for this swimlane").ok()?,
        summary: ShortText::parse(&format!(
            "Sync windows held the drift alerts of {swimlane} for {held_minutes} of the last {within_minutes} minutes, which is the whole budget of {budget_minutes}. Alerts for the swimlane are sent as they happen, whatever windows are open, until the budget refills."
        ))
        .ok()?,
        link: ShortText::parse(&format!("/swimlanes/{swimlane}")).ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec = SyncService::watch_spec(&crate::config::RegistryConfig::default())
            .expect("the default config gives a valid spec");
        assert_eq!(spec.name(), WATCH_JOB);
        assert_eq!(WATCH_JOB, "registry-sync-watch");
        assert!(ports::is_valid_lease_name(WATCH_JOB));
    }

    #[test]
    fn a_config_the_spec_refuses_is_an_error_and_not_a_panic() {
        let config = crate::config::RegistryConfig {
            sync_watch_interval: Duration::ZERO,
            ..crate::config::RegistryConfig::default()
        };
        assert!(SyncService::watch_spec(&config).is_err());
    }

    #[test]
    fn the_budget_notice_names_the_swimlane_and_minutes_and_nothing_else() {
        let lane = SwimlaneId::parse("sit1").unwrap();
        let n = exhausted_notification(&lane, 240, 240, 1440).unwrap();
        assert_eq!(n.kind, NotificationKind::SyncWindowStale);
        assert_eq!(n.severity, Severity::Medium);
        assert_eq!(n.swimlane, Some(lane));
        assert!(
            n.summary.as_str().contains("240 of the last 1440 minutes"),
            "{:?}",
            n.summary
        );
        assert!(n.summary.as_str().contains("until the budget refills"));
        assert_eq!(n.link.as_ref().map(ShortText::as_str), Some("/swimlanes/sit1"));
    }

    #[test]
    fn the_window_source_is_a_valid_sync_source() {
        assert_eq!(SyncSource::parse(WINDOW_SOURCE).unwrap().as_str(), "window");
    }

    #[test]
    fn the_stale_warning_names_the_swimlane_and_the_time_and_nothing_else() {
        let lane = SwimlaneId::parse("sit1").unwrap();
        let n = stale_notification(&lane, 120).unwrap();
        assert_eq!(n.kind, NotificationKind::SyncWindowStale);
        assert_eq!(n.swimlane, Some(lane));
        assert!(n.summary.as_str().contains("sit1"));
        assert!(n.summary.as_str().contains("120 minutes"));
        assert_eq!(n.link.as_ref().map(ShortText::as_str), Some("/swimlanes/sit1"));
    }
}
