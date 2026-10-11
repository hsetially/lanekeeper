//! Singleton jobs (D62, D63): work that exactly one replica does at a time, such as the Git poll, the tree-index persistence,
//! the outbox prune and the nightly and checkpoint jobs that later prompts register.
//!
//! Every replica runs a [`JobRunner`] with the same jobs registered. For each job it does this:
//!
//! 1. **Acquire.** Until it holds the job's lease it asks for it, about every `min(interval, ttl / 3)` (jittered, so replicas do
//!    not ask in step). A replica that does not get it does nothing else.
//! 2. **Keep.** The holder keeps its lease between runs by renewing it every third of the ttl, so a lease with a ttl of three
//!    intervals survives two missed renewals, and one that is not renewed (the holder died) runs out and passes to another
//!    replica within one ttl. It also renews while a run is in progress, so a long job keeps its lease.
//! 3. **Run.** When the job is due, the runner calls it with a [`JobContext`]. The job asks [`JobContext::ensure_held`] before
//!    each unit of work that writes: it fails once the lease is lost or the expiry the store granted has passed, and the job
//!    stops. Runs of one job never overlap, and a run is cut off after its `max_run`.
//! 4. **Stop.** On a `Lost` from renewal the runner drops the guard (it is not released: it is not ours) and goes back to
//!    step 1. On shutdown it releases what it holds, so the next replica takes over at once.
//!
//! A new holder runs the job right after it acquires, because it cannot know when the last holder ran it. Jobs are therefore
//! idempotent: that is a rule for what is registered here, not something the runner can check.
//!
//! The runner never trusts its own clock for the lease: a renewal gives back the expiry the store granted, and the context's
//! answer is that expiry compared with the monotonic clock.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::FutureExt;
use ports::{LeaseError, LeaseGuard, Leases, check_ttl, is_valid_lease_name};
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tracing::{debug, info, warn};

use crate::events::source::wait_closed;

pub mod outbox_prune;

/// The most jobs one runner holds. Job names come from code, so this also bounds the metric labels.
pub const MAX_JOBS: usize = 64;
/// What a run may take unless the spec says otherwise.
pub const DEFAULT_MAX_RUN: Duration = Duration::from_secs(15 * 60);
/// The longest `max_run` a spec may ask for.
pub const MAX_RUN_LIMIT: Duration = Duration::from_secs(3600);
/// The longest interval a spec may ask for.
pub const MAX_INTERVAL: Duration = Duration::from_secs(7 * 24 * 3600);

/// One call to the lease store.
const LEASE_CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How soon a renewal that could not reach the store is tried again.
const RENEW_RETRY: Duration = Duration::from_secs(1);
/// How long shutdown waits for the jobs to stop and release their leases.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// Polling for a lease is spread over up to this much more than the base period.
const POLL_JITTER: f64 = 0.1;

// ------------------------------------------------------------------------------------------------------- the job

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// The lease is no longer held: stop now. Returned by [`JobContext::ensure_held`].
    #[error("the job's lease is no longer held")]
    LeaseLost,
    /// The run failed. The code is a stable word for the log and never carries internals.
    #[error("the job failed: {0}")]
    Failed(&'static str),
}

/// What a running job sees of its lease.
#[derive(Debug, Clone)]
pub struct JobContext {
    name: Arc<str>,
    expiry: watch::Receiver<Instant>,
}

#[cfg(any(test, feature = "fakes"))]
impl JobContext {
    /// A context whose lease is held until the returned sender's value says otherwise, for tests that run one job directly
    /// without a runner. Send an instant in the past to lose the lease. Keep the sender alive for as long as the context is used.
    #[must_use]
    pub fn held_until_told_otherwise(name: &str) -> (Self, watch::Sender<Instant>) {
        let (tx, rx) = watch::channel(Instant::now() + Duration::from_secs(24 * 3600));
        (
            Self {
                name: Arc::from(name),
                expiry: rx,
            },
            tx,
        )
    }
}

impl JobContext {
    /// The job's name, which is also its lease's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Call before each unit of work. Fails with [`JobError::LeaseLost`] once the lease was lost, the shutdown began, or the
    /// expiry the store granted has passed (renewal could not reach the store and nothing can be assumed beyond it).
    pub fn ensure_held(&self) -> Result<(), JobError> {
        if Instant::now() < self.held_until() {
            Ok(())
        } else {
            Err(JobError::LeaseLost)
        }
    }

    /// The instant until which the lease is held as far as this replica knows.
    #[must_use]
    pub fn held_until(&self) -> Instant {
        *self.expiry.borrow()
    }
}

/// Work done by the holder of the job's lease.
#[async_trait]
pub trait Job: Send + Sync + 'static {
    /// One run. Idempotent, bounded by the spec's `max_run`, and careful to call [`JobContext::ensure_held`] before each unit
    /// of work that writes.
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError>;
}

// ------------------------------------------------------------------------------------------------------- the spec

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("the job name is not a valid lease name")]
    InvalidName,
    #[error("the interval must be more than zero and at most a week")]
    InvalidInterval,
    #[error("the lease ttl must be more than zero and at most one hour")]
    InvalidTtl,
    #[error("the longest run must be more than zero and at most one hour")]
    InvalidMaxRun,
}

/// When a job runs and how its lease is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    name: String,
    interval: Duration,
    lease_ttl: Duration,
    max_run: Duration,
}

impl JobSpec {
    /// `name` is the lease's name (1-128 of `[a-z0-9._:-]`). The holder runs the job every `interval`; `lease_ttl` is how long
    /// the lease survives without renewal, so choose a few intervals' worth.
    pub fn new(name: &str, interval: Duration, lease_ttl: Duration) -> Result<Self, SpecError> {
        if !is_valid_lease_name(name) {
            return Err(SpecError::InvalidName);
        }
        if interval.is_zero() || interval > MAX_INTERVAL {
            return Err(SpecError::InvalidInterval);
        }
        check_ttl(lease_ttl).map_err(|_| SpecError::InvalidTtl)?;
        Ok(Self {
            name: name.to_owned(),
            interval,
            lease_ttl,
            max_run: DEFAULT_MAX_RUN,
        })
    }

    /// The longest one run may take before it is cut off.
    pub fn with_max_run(mut self, max_run: Duration) -> Result<Self, SpecError> {
        if max_run.is_zero() || max_run > MAX_RUN_LIMIT {
            return Err(SpecError::InvalidMaxRun);
        }
        self.max_run = max_run;
        Ok(self)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    fn renew_every(&self) -> Duration {
        self.lease_ttl / 3
    }

    /// How often a replica without the lease asks for it.
    fn poll_every(&self) -> Duration {
        self.interval.min(self.renew_every())
    }
}

// ----------------------------------------------------------------------------------------------------- metrics

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum JobOutcome {
    Ok,
    Failed,
    /// The job stopped because its lease was lost or ran out.
    LeaseLost,
    /// The run took longer than `max_run` and was cut off.
    TimedOut,
    Panicked,
    /// The hub is shutting down and the job was asked to stop.
    Interrupted,
}

/// How a try for a lease ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum AcquireOutcome {
    Acquired,
    /// Another replica holds it.
    Busy,
    /// The store did not answer.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct JobLabel {
    job: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RunLabels {
    job: String,
    outcome: JobOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct AcquireLabels {
    job: String,
    outcome: AcquireOutcome,
}

/// Cloning shares the series. The only labels are the job (a name from code, at most [`MAX_JOBS`] of them) and an outcome from
/// a closed set.
#[derive(Debug, Clone, Default)]
pub struct JobMetrics {
    runs: Family<RunLabels, Counter>,
    acquires: Family<AcquireLabels, Counter>,
    held: Family<JobLabel, Gauge>,
    lost: Family<JobLabel, Counter>,
}

impl JobMetrics {
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "job_runs",
            "Runs of singleton jobs, by job and how they ended",
            self.runs.clone(),
        );
        registry.register(
            "job_acquires",
            "Tries for a singleton job's lease, by job and outcome",
            self.acquires.clone(),
        );
        registry.register(
            "job_lease_held",
            "1 while this replica holds the job's lease",
            self.held.clone(),
        );
        registry.register(
            "job_lease_lost",
            "Times this replica lost a lease it held (taken over, or ran out)",
            self.lost.clone(),
        );
    }

    fn run(&self, job: &str, outcome: JobOutcome) {
        self.runs
            .get_or_create(&RunLabels {
                job: job.to_owned(),
                outcome,
            })
            .inc();
    }

    fn acquire(&self, job: &str, outcome: AcquireOutcome) {
        self.acquires
            .get_or_create(&AcquireLabels {
                job: job.to_owned(),
                outcome,
            })
            .inc();
    }

    fn set_held(&self, job: &str, held: bool) {
        self.held
            .get_or_create(&JobLabel { job: job.to_owned() })
            .set(i64::from(held));
    }

    fn lost_one(&self, job: &str) {
        self.lost.get_or_create(&JobLabel { job: job.to_owned() }).inc();
    }

    #[must_use]
    pub fn runs_total(&self, job: &str, outcome: JobOutcome) -> u64 {
        self.runs
            .get_or_create(&RunLabels {
                job: job.to_owned(),
                outcome,
            })
            .get()
    }

    #[must_use]
    pub fn acquires_total(&self, job: &str, outcome: AcquireOutcome) -> u64 {
        self.acquires
            .get_or_create(&AcquireLabels {
                job: job.to_owned(),
                outcome,
            })
            .get()
    }

    /// 1 while this replica holds the job's lease.
    #[must_use]
    pub fn held(&self, job: &str) -> i64 {
        self.held.get_or_create(&JobLabel { job: job.to_owned() }).get()
    }

    #[must_use]
    pub fn lost_total(&self, job: &str) -> u64 {
        self.lost.get_or_create(&JobLabel { job: job.to_owned() }).get()
    }
}

// ----------------------------------------------------------------------------------------------------- the runner

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    #[error("a job with this name is already registered")]
    Duplicate,
    #[error("too many jobs are registered")]
    TooMany,
}

struct Registered {
    spec: JobSpec,
    job: Arc<dyn Job>,
}

/// The singleton jobs of one replica.
pub struct JobRunner {
    leases: Arc<dyn Leases>,
    jobs: Vec<Registered>,
    names: HashSet<String>,
    metrics: JobMetrics,
}

impl std::fmt::Debug for JobRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRunner")
            .field("jobs", &self.jobs.len())
            .finish_non_exhaustive()
    }
}

impl JobRunner {
    #[must_use]
    pub fn new(leases: Arc<dyn Leases>, metrics: JobMetrics) -> Self {
        Self {
            leases,
            jobs: Vec::new(),
            names: HashSet::new(),
            metrics,
        }
    }

    /// Add a job. Later prompts call this for their nightly and checkpoint jobs before [`JobRunner::start`].
    pub fn register(&mut self, spec: JobSpec, job: Arc<dyn Job>) -> Result<(), RegisterError> {
        if self.names.contains(spec.name()) {
            return Err(RegisterError::Duplicate);
        }
        if self.jobs.len() >= MAX_JOBS {
            return Err(RegisterError::TooMany);
        }
        self.names.insert(spec.name().to_owned());
        self.jobs.push(Registered { spec, job });
        Ok(())
    }

    /// Start one task per job. Dropping the handle stops them at once without releasing anything (a replica that died);
    /// [`JobRunnerHandle::shutdown`] stops them and releases what they hold.
    #[must_use]
    pub fn start(self) -> JobRunnerHandle {
        let (closing, closed) = watch::channel(false);
        let mut tasks = JoinSet::new();
        for Registered { spec, job } in self.jobs {
            let driver = Driver {
                leases: self.leases.clone(),
                spec,
                job,
                metrics: self.metrics.clone(),
                closed: closed.clone(),
            };
            tasks.spawn(driver.drive());
        }
        JobRunnerHandle { tasks, closing }
    }
}

/// The running jobs. Field order matters: the tasks are aborted first when this is dropped.
#[derive(Debug)]
pub struct JobRunnerHandle {
    tasks: JoinSet<()>,
    closing: watch::Sender<bool>,
}

impl JobRunnerHandle {
    /// Ask every job to stop, wait (a few seconds at most) for them to release their leases, then stop what is left.
    pub async fn shutdown(mut self) {
        self.closing.send_replace(true);
        let all_done = async { while self.tasks.join_next().await.is_some() {} };
        if timeout(SHUTDOWN_GRACE, all_done).await.is_err() {
            warn!(
                code = "jobs_shutdown_timeout",
                "jobs did not stop in time and were cut off"
            );
            self.tasks.abort_all();
        }
    }
}

// ----------------------------------------------------------------------------------------------------- one job

/// A lease held, and when to renew it and run the job next.
struct Held {
    guard: LeaseGuard,
    next_renew: Instant,
    next_run: Instant,
}

struct Driver {
    leases: Arc<dyn Leases>,
    spec: JobSpec,
    job: Arc<dyn Job>,
    metrics: JobMetrics,
    closed: watch::Receiver<bool>,
}

/// `d` plus up to [`POLL_JITTER`] of itself.
fn jittered(d: Duration) -> Duration {
    d.mul_f64(1.0 + POLL_JITTER * fastrand::f64())
}

impl Driver {
    fn name(&self) -> &str {
        self.spec.name()
    }

    async fn drive(self) {
        let mut held: Option<Held> = None;
        loop {
            if *self.closed.borrow() {
                break;
            }
            held = self.step(held).await;
            let wait = match &held {
                None => jittered(self.spec.poll_every()),
                Some(h) => h
                    .next_renew
                    .min(h.next_run)
                    .saturating_duration_since(Instant::now()),
            };
            let mut closed = self.closed.clone();
            tokio::select! {
                () = sleep(wait) => {}
                () = wait_closed(&mut closed) => break,
            }
        }
        if let Some(h) = held {
            self.release(h).await;
        }
    }

    /// One pass: acquire, or renew and run if due. Returns what is held afterwards.
    async fn step(&self, held: Option<Held>) -> Option<Held> {
        let Some(mut h) = held else {
            return self.try_acquire().await;
        };
        let now = Instant::now();
        if now >= h.next_renew {
            match self.renew(&mut h.guard).await {
                Ok(()) => h.next_renew = Instant::now() + self.spec.renew_every(),
                Err(LeaseError::Lost) => return self.dropped(),
                Err(error) => {
                    warn!(code = "job_lease_renew_failed", job = self.name(), %error, "renewing a job's lease failed");
                    h.next_renew = Instant::now() + RENEW_RETRY.min(self.spec.renew_every());
                }
            }
        }
        if !h.guard.is_valid() {
            // The store has not confirmed the lease for as long as the expiry it granted: nothing can be assumed beyond it.
            return self.dropped();
        }
        if Instant::now() >= h.next_run {
            let started = Instant::now();
            if !self.run(&mut h).await {
                return self.dropped();
            }
            h.next_run = (started + self.spec.interval).max(Instant::now());
        }
        Some(h)
    }

    fn dropped(&self) -> Option<Held> {
        warn!(
            code = "job_lease_lost",
            job = self.name(),
            "the lease of a job was lost"
        );
        self.metrics.lost_one(self.name());
        self.metrics.set_held(self.name(), false);
        None
    }

    async fn try_acquire(&self) -> Option<Held> {
        let call = self.leases.try_acquire(self.name(), self.spec.lease_ttl);
        match timeout(LEASE_CALL_TIMEOUT, call).await {
            Ok(Ok(Some(guard))) => {
                info!(
                    code = "job_lease_acquired",
                    job = self.name(),
                    "took the lease of a job"
                );
                self.metrics.acquire(self.name(), AcquireOutcome::Acquired);
                self.metrics.set_held(self.name(), true);
                let now = Instant::now();
                Some(Held {
                    guard,
                    next_renew: now + self.spec.renew_every(),
                    next_run: now,
                })
            }
            Ok(Ok(None)) => {
                self.metrics.acquire(self.name(), AcquireOutcome::Busy);
                None
            }
            Ok(Err(error)) => {
                warn!(code = "job_lease_acquire_failed", job = self.name(), %error, "asking for a job's lease failed");
                self.metrics.acquire(self.name(), AcquireOutcome::Failed);
                None
            }
            Err(_) => {
                warn!(
                    code = "job_lease_acquire_timeout",
                    job = self.name(),
                    "asking for a job's lease timed out"
                );
                self.metrics.acquire(self.name(), AcquireOutcome::Failed);
                None
            }
        }
    }

    async fn renew(&self, guard: &mut LeaseGuard) -> Result<(), LeaseError> {
        timeout(LEASE_CALL_TIMEOUT, guard.renew(self.spec.lease_ttl))
            .await
            .unwrap_or(Err(LeaseError::Unavailable))
    }

    async fn release(&self, h: Held) {
        match timeout(LEASE_CALL_TIMEOUT, h.guard.release()).await {
            Ok(Ok(())) => debug!(
                code = "job_lease_released",
                job = self.name(),
                "released the lease of a job"
            ),
            Ok(Err(error)) => {
                debug!(code = "job_lease_release_failed", job = self.name(), %error, "could not release a job's lease");
            }
            Err(_) => debug!(
                code = "job_lease_release_timeout",
                job = self.name(),
                "releasing a job's lease timed out"
            ),
        }
        self.metrics.set_held(self.name(), false);
    }

    /// Run the job once, renewing the lease while it runs. False when the lease was lost during the run.
    async fn run(&self, h: &mut Held) -> bool {
        let (expiry_tx, expiry_rx) = watch::channel(h.guard.expires_at());
        let ctx = JobContext {
            name: Arc::from(self.name()),
            expiry: expiry_rx,
        };
        let work = timeout(
            self.spec.max_run,
            AssertUnwindSafe(self.job.run(&ctx)).catch_unwind(),
        );
        tokio::pin!(work);

        let mut closed = self.closed.clone();
        let mut lost = false;
        let mut stopping = false;
        let result = loop {
            let renewal = async {
                if lost {
                    std::future::pending::<()>().await;
                }
                sleep_until(h.next_renew).await;
                self.renew(&mut h.guard).await
            };
            let closing = async {
                if stopping {
                    std::future::pending::<()>().await;
                }
                wait_closed(&mut closed).await;
            };
            tokio::select! {
                res = &mut work => break res,
                renewed = renewal => match renewed {
                    Ok(()) => {
                        h.next_renew = Instant::now() + self.spec.renew_every();
                        expiry_tx.send_replace(h.guard.expires_at());
                    }
                    Err(LeaseError::Lost) => {
                        // The job learns at its next `ensure_held`.
                        lost = true;
                        expiry_tx.send_replace(Instant::now());
                    }
                    Err(error) => {
                        warn!(code = "job_lease_renew_failed", job = self.name(), %error, "renewing a job's lease failed");
                        h.next_renew = Instant::now() + RENEW_RETRY.min(self.spec.renew_every());
                    }
                },
                () = closing => {
                    stopping = true;
                    expiry_tx.send_replace(Instant::now());
                }
            }
        };

        let outcome = match result {
            Err(_elapsed) => {
                warn!(
                    code = "job_timed_out",
                    job = self.name(),
                    "a job run was cut off after its longest run"
                );
                JobOutcome::TimedOut
            }
            Ok(Err(_panic)) => {
                warn!(
                    code = "job_panicked",
                    job = self.name(),
                    "a job panicked; it will run again at its next interval"
                );
                JobOutcome::Panicked
            }
            Ok(Ok(Ok(()))) => JobOutcome::Ok,
            Ok(Ok(Err(JobError::LeaseLost))) if stopping => JobOutcome::Interrupted,
            Ok(Ok(Err(JobError::LeaseLost))) => JobOutcome::LeaseLost,
            Ok(Ok(Err(JobError::Failed(code)))) => {
                warn!(code, job = self.name(), "a job run failed");
                JobOutcome::Failed
            }
        };
        self.metrics.run(self.name(), outcome);
        !lost
    }
}
