//! The `registry-pipeline` job (T3, D62, D66, P13): the loop that keeps the swimlanes up to date.
//!
//! One run does this, until its lease is lost or `pipeline_run` has passed:
//!
//! 1. **Subscribe** to `FileObserved` and `GitHeadMoved` (the subscription exists from this call, so nothing published afterwards is
//!    missed), take the signal queue, and, when the last run did not end on its own with the lease still held, **forget the memory**
//!    (what it knew may have been overtaken by another holder).
//! 2. **Reconcile** ([`super::reconcile`]): compare memory, database and mirror, and queue what differs; queue the swimlanes that have an
//!    inventory and were never computed, and, after a takeover, every swimlane, for the warm-up.
//! 3. **Loop**: an event or a signal goes into the [`Coalescer`]; when a swimlane's window closes (200 ms after its first change) the
//!    windows that are due are brought up to date in one batch; a tick every half second checks the lease, and runs the reconcile and
//!    the look for new swimlanes when they are due; and when nothing else is ready the warm-up loads one swimlane that is not in memory.
//!    Events and windows have priority over the warm-up, so a swimlane being verified never delays one that changed by more than the
//!    time of one swimlane's load.
//! 4. **End**: the windows still open are brought up to date, and the run returns with the memory kept, for the next run to start from.
//!
//! A batch that fails (the database or the mirror is down) puts its work back and waits, doubling a pause up to ten seconds, so a
//! failing dependency is not hammered and nothing is lost: the paths stay queued, and what could not be queued is found by the
//! reconcile.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use domain::SwimlaneId;
use futures::StreamExt;
use hub_platform::jobs::{Job, JobContext, JobError};
use tokio::time::{Instant, interval, sleep_until};
use tracing::{debug, info, warn};

use super::affected::{Routed, requeue};
use super::batch::BatchReport;
use super::coalesce::{Coalescer, Work};
use super::events::{Signal, event_filter};
use super::{Inner, Pipeline};

/// How often a run looks at its lease and at what is due, when nothing else wakes it.
const TICK: Duration = Duration::from_millis(500);
/// The longest pause after a failed batch.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct PipelineJob {
    pipeline: Pipeline,
}

impl PipelineJob {
    pub(crate) fn new(pipeline: Pipeline) -> Self {
        Self { pipeline }
    }
}

#[async_trait]
impl Job for PipelineJob {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.pipeline.inner.run_job(ctx).await
    }
}

/// The state of one run of the loop.
struct Run {
    coalescer: Coalescer,
    /// Swimlanes to load when nothing else is waiting.
    warm: VecDeque<SwimlaneId>,
    reconcile_at: Instant,
    inventory_at: Instant,
    /// Do not bring a batch up to date before this: a failed batch is not retried at once.
    not_before: Instant,
    backoff: Duration,
}

impl Inner {
    fn queue_warm(run: &mut Run, lanes: impl IntoIterator<Item = SwimlaneId>, front: bool) {
        for lane in lanes {
            if !run.warm.contains(&lane) {
                if front {
                    run.warm.push_front(lane);
                } else {
                    run.warm.push_back(lane);
                }
            }
        }
    }

    /// Reconcile and put what it found where it belongs.
    async fn reconcile_into(&self, run: &mut Run) {
        match self.reconcile().await {
            Ok(found) => {
                let now = Instant::now();
                if !found.work.is_empty() {
                    debug!(
                        swimlanes = found.work.len(),
                        paths = found.divergent_paths,
                        "reconcile found work"
                    );
                    self.metrics.pipeline_divergences(found.divergent_paths as u64);
                }
                requeue(&mut run.coalescer, found.work, now);
                Self::queue_warm(run, found.cold, true);
            }
            Err(error) => warn!(?error, "reconcile failed; it runs again at its next time"),
        }
        run.reconcile_at = Instant::now() + self.config.reconcile_interval;
    }

    /// The pickup state of the swimlanes a batch brought up to date (T10): their files may have changed what a service has yet to pick up.
    /// It runs after the batch is committed and outside its transaction, so the benchmarked pipeline does not wait for it and a failure here
    /// never fails a batch: the swimlane is counted as failed and the pickup sweeper computes it again.
    async fn pickups_after(&self, report: &BatchReport) {
        let lanes: Vec<SwimlaneId> = report.lanes.iter().map(|l| l.swimlane.clone()).collect();
        if lanes.is_empty() {
            return;
        }
        let done = self.pickups.recompute(&lanes).await;
        if !done.failed.is_empty() {
            warn!(
                swimlanes = done.failed.len(),
                "the pickup state of some swimlanes could not be computed; the sweeper will"
            );
        }
    }

    /// Bring the windows that are due (or all of them) up to date. False when a batch failed.
    async fn flush(&self, run: &mut Run, all: bool) -> bool {
        let now = Instant::now();
        if !all && now < run.not_before {
            return true;
        }
        let due = if all {
            run.coalescer.take_all()
        } else {
            run.coalescer.take_due(now)
        };
        if due.is_empty() {
            return true;
        }
        match self.process(due.clone()).await {
            Ok(report) => {
                run.backoff = Duration::ZERO;
                self.pickups_after(&report).await;
                if report.failed.is_empty() {
                    return true;
                }
                // Their memory is dropped; they are loaded whole at the next window.
                for id in report.failed {
                    run.coalescer.add_whole(&id, Instant::now());
                }
                run.backoff = Duration::from_millis(500);
                run.not_before = Instant::now() + run.backoff;
                false
            }
            Err(error) => {
                warn!(
                    ?error,
                    swimlanes = due.len(),
                    "a batch of the pipeline failed and will be tried again"
                );
                requeue(&mut run.coalescer, due, Instant::now());
                run.backoff = (run.backoff * 2).clamp(Duration::from_millis(500), MAX_BACKOFF);
                run.not_before = Instant::now() + run.backoff;
                false
            }
        }
    }

    /// Load one swimlane that is not in memory.
    async fn warm_one(&self, run: &mut Run) {
        let Some(lane) = run.warm.pop_front() else {
            return;
        };
        if self.exec.lock().await.store.contains(&lane) {
            return;
        }
        match self.process(vec![(lane.clone(), Work::Whole)]).await {
            Ok(report) if !report.failed.is_empty() => run.warm.push_back(lane),
            Ok(report) => self.pickups_after(&report).await,
            Err(error) => {
                warn!(?error, "the warm-up of a swimlane failed and will be tried again");
                run.warm.push_back(lane);
                run.not_before = Instant::now() + Duration::from_secs(1);
            }
        }
    }

    /// One run of the job.
    pub(crate) async fn run_job(&self, ctx: &JobContext) -> Result<(), JobError> {
        let started = Instant::now();
        let mut signals = self.signals.lock().await;
        let mut events = self.bus.subscribe(event_filter());
        let takeover = !self.clean_end.swap(false, Ordering::SeqCst);
        if takeover {
            let mut exec = self.exec.lock().await;
            exec.store.clear();
            self.metrics.pipeline_lanes_in_memory(0);
        }
        self.remember_heads()
            .await
            .map_err(|_| JobError::Failed("registry_pipeline_heads"))?;

        let now = Instant::now();
        let mut run = Run {
            coalescer: Coalescer::new(
                self.config.coalesce_window,
                self.config.max_affected_paths,
                self.config.max_coalesced_lanes,
            ),
            warm: VecDeque::new(),
            reconcile_at: now,
            inventory_at: now + self.config.inventory_poll_interval,
            not_before: now,
            backoff: Duration::ZERO,
        };
        if takeover {
            match self.all_with_inventory().await {
                Ok(all) => Self::queue_warm(&mut run, all, false),
                Err(error) => warn!(?error, "the swimlanes to verify could not be listed"),
            }
        }
        info!(takeover, "the pipeline job started");
        let mut tick = interval(TICK);
        let ends = started + self.config.pipeline_run;

        loop {
            ctx.ensure_held()?;
            let deadline = run.coalescer.next_deadline().map(|d| d.max(run.not_before));
            let idle = !run.warm.is_empty() && run.coalescer.len() == 0;
            tokio::select! {
                biased;
                event = events.next() => match event {
                    Some(event) => {
                        match self.route(event, &mut run.coalescer, Instant::now()).await {
                            Ok(Routed::Resync) => run.reconcile_at = Instant::now(),
                            Ok(_) => {}
                            Err(error) => {
                                // The move is not remembered as handled, so the reconcile finds it again.
                                warn!(?error, "an event could not be routed; the reconcile will find it");
                                run.reconcile_at = Instant::now() + Duration::from_secs(1);
                            }
                        }
                    }
                    // The bus shut down: end the run with the memory kept.
                    None => break,
                },
                signal = signals.recv() => match signal {
                    Some(Signal::Paths { swimlane, paths }) => run.coalescer.add_paths(&swimlane, paths, Instant::now()),
                    Some(Signal::Lane { swimlane }) => run.coalescer.add_whole(&swimlane, Instant::now()),
                    // The pipeline holds a sender itself, so the queue does not close; if it ever does, the run ends.
                    None => break,
                },
                () = async {
                    match deadline {
                        Some(at) => sleep_until(at).await,
                        None => std::future::pending::<()>().await,
                    }
                } => { self.flush(&mut run, false).await; }
                _ = tick.tick() => {
                    let now = Instant::now();
                    if run.coalescer.overflowed() {
                        run.reconcile_at = now;
                    }
                    if now >= run.reconcile_at {
                        self.reconcile_into(&mut run).await;
                    } else if now >= run.inventory_at {
                        match self.never_computed().await {
                            Ok(cold) => Self::queue_warm(&mut run, cold, true),
                            Err(error) => warn!(?error, "the swimlanes never computed could not be listed"),
                        }
                        run.inventory_at = now + self.config.inventory_poll_interval;
                    }
                    if now >= ends {
                        break;
                    }
                },
                () = std::future::ready(()), if idle => self.warm_one(&mut run).await,
            }
        }
        ctx.ensure_held()?;
        // The memory is kept for the next run only when everything that was waiting reached the database.
        let clean = self.flush(&mut run, true).await;
        self.clean_end.store(clean, Ordering::SeqCst);
        info!(clean, "the pipeline job ended its run");
        Ok(())
    }
}
