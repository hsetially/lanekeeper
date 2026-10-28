//! The four workers of `hub-writes` as leased jobs (plan 18, T5, G7; D62, D63).
//!
//! `ReconcileWrites`, `ExpireProposals`, `PrJobRunner` and `CredentialCleanup` are bare `run_once(still_held)` methods: the crate
//! says what a run does and that it must be run under a lease, and leaves the lease's name, the interval and the time to the hub.
//! Each wrapper here is a [`Job`] that calls its worker with a closure over [`JobContext::ensure_held`], so a run stops at the next
//! unit of work once the lease is lost, and ends with `LeaseLost` when it was lost during the run (the runner counts that).
//!
//! The lease names are the contract with the other replicas: one replica runs each job at a time, and a new holder runs it at once,
//! so every worker is idempotent (the crate's tests hold that). The intervals are settings within bounds
//! (`LK_WRITES_*_SECS`); the lease times are constants, a few intervals' worth and longer than the longest unit of work between two
//! asks of `still_held`. Nothing is logged but counts: a report holds numbers, never a path or a name (S10).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use hub_writes::prs::PrJobRunner;
use hub_writes::vault::CredentialCleanup;
use hub_writes::{ExpireProposals, ReconcileWrites};
use tracing::debug;

use crate::config::WriteJobTimes;

/// Finishes or clears the writes that landed without being recorded, and sweeps expired idempotency rows.
pub const RECONCILE_JOB: &str = "writes-reconcile";
/// Marks the proposals and drafts that nobody decided as expired.
pub const EXPIRE_JOB: &str = "writes-expire-proposals";
/// Takes up the pull-request jobs a crash or a restart left.
pub const PR_JOBS_JOB: &str = "writes-pr-jobs";
/// Deletes the GitHub credentials of users not seen for 30 days.
pub const CLEANUP_JOB: &str = "writes-credential-cleanup";

/// How long each lease survives without a renewal. The runner renews it every third of this, between runs and during one.
const RECONCILE_LEASE: Duration = Duration::from_secs(90);
const EXPIRE_LEASE: Duration = Duration::from_secs(180);
const PR_JOBS_LEASE: Duration = Duration::from_secs(30);
const CLEANUP_LEASE: Duration = Duration::from_secs(300);

/// A job and its spec, as the runner takes them.
pub type JobEntry = (JobSpec, Arc<dyn Job>);

/// The names of the four jobs, in registration order.
pub const WRITE_JOBS: [&str; 4] = [RECONCILE_JOB, EXPIRE_JOB, PR_JOBS_JOB, CLEANUP_JOB];

/// The four specs. A run is cut off after its `max_run`; none of the workers comes near it, which is the point of a bound.
pub fn specs(times: &WriteJobTimes) -> Result<[JobSpec; 4], SpecError> {
    Ok([
        JobSpec::new(RECONCILE_JOB, times.reconcile, RECONCILE_LEASE)?
            .with_max_run(Duration::from_secs(5 * 60))?,
        JobSpec::new(EXPIRE_JOB, times.expire, EXPIRE_LEASE)?.with_max_run(Duration::from_secs(10 * 60))?,
        JobSpec::new(PR_JOBS_JOB, times.pr_jobs, PR_JOBS_LEASE)?.with_max_run(Duration::from_secs(5 * 60))?,
        JobSpec::new(CLEANUP_JOB, times.cleanup, CLEANUP_LEASE)?
            .with_max_run(Duration::from_secs(30 * 60))?,
    ])
}

macro_rules! opaque_debug {
    ($($name:ident),*) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )*};
}

/// Sets a flag when it is dropped: the job's run was cut off (its `max_run`, or the hub stopping), and the worker on the blocking
/// thread must not go on with another unit of work.
struct StopWhenDropped(Arc<AtomicBool>);

impl Drop for StopWhenDropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Runs a worker's `run_once` and says how it ended.
///
/// **Why a blocking thread.** The four workers take `still_held: &dyn Fn() -> bool`, and a `dyn Fn` that is not also `Sync` makes
/// the future that borrows it across an await point not `Send`: it can be neither a [`Job`] (whose future must be) nor a task. The
/// crate cannot be changed from here, so the future is driven by `Handle::block_on` on a thread of the blocking pool, where it needs
/// no `Send`. It is async all the way down (the pool, the bus and the gateway are driven by the runtime's own threads), and it holds
/// the blocking thread for the length of one run, which `max_run` bounds. If the run is cut off, `stop` is set and the worker
/// returns at its next `still_held`.
async fn run_on_a_thread<R, W>(ctx: &JobContext, what: &'static str, work: W) -> Result<R, JobError>
where
    R: Send + 'static + std::fmt::Debug,
    W: for<'a> FnOnce(&'a dyn Fn() -> bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = R> + 'a>>
        + Send
        + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let _cut_off = StopWhenDropped(Arc::clone(&stop));
    let (context, flag) = (ctx.clone(), Arc::clone(&stop));
    let runtime = tokio::runtime::Handle::current();
    let report = tokio::task::spawn_blocking(move || {
        let held = move || !flag.load(Ordering::SeqCst) && context.ensure_held().is_ok();
        runtime.block_on(work(&held))
    })
    .await
    .map_err(|_| JobError::Failed(what))?;
    debug!(?report, job = what, "a run of a write job");
    // A run that ended with the lease lost did not finish: say so, whatever the report holds.
    ctx.ensure_held()?;
    Ok(report)
}

#[derive(Clone)]
pub struct Reconcile(ReconcileWrites);

#[async_trait]
impl Job for Reconcile {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        let worker = self.0.clone();
        run_on_a_thread(ctx, "writes_reconcile", move |held| {
            Box::pin(async move { worker.run_once(held).await })
        })
        .await
        .map(|_| ())
    }
}

#[derive(Clone)]
pub struct Expire(ExpireProposals);

#[async_trait]
impl Job for Expire {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        let worker = self.0.clone();
        run_on_a_thread(ctx, "writes_expire", move |held| {
            Box::pin(async move { worker.run_once(held).await })
        })
        .await
        .map(|_| ())
    }
}

#[derive(Clone)]
pub struct PrJobs(PrJobRunner);

#[async_trait]
impl Job for PrJobs {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        let worker = self.0.clone();
        run_on_a_thread(ctx, "writes_pr_jobs", move |held| {
            Box::pin(async move { worker.run_once(held).await })
        })
        .await
        .map(|_| ())
    }
}

#[derive(Clone)]
pub struct Cleanup(CredentialCleanup);

#[async_trait]
impl Job for Cleanup {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        let worker = self.0.clone();
        run_on_a_thread(ctx, "writes_cleanup", move |held| {
            Box::pin(async move { worker.run_once(held).await })
        })
        .await
        .map(|_| ())
    }
}

opaque_debug!(Reconcile, Expire, PrJobs, Cleanup);

/// The four jobs with their specs, in the order of [`WRITE_JOBS`].
pub fn jobs(
    times: &WriteJobTimes,
    reconcile: ReconcileWrites,
    expire: ExpireProposals,
    pr_jobs: PrJobRunner,
    cleanup: CredentialCleanup,
) -> Result<Vec<JobEntry>, SpecError> {
    let [a, b, c, d] = specs(times)?;
    Ok(vec![
        (a, Arc::new(Reconcile(reconcile))),
        (b, Arc::new(Expire(expire))),
        (c, Arc::new(PrJobs(pr_jobs))),
        (d, Arc::new(Cleanup(cleanup))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_specs_carry_the_lease_names_and_the_intervals_of_the_settings() {
        let times = WriteJobTimes::default();
        let specs = specs(&times).unwrap();
        let names: Vec<&str> = specs.iter().map(JobSpec::name).collect();
        assert_eq!(names, WRITE_JOBS);
        assert_eq!(
            WRITE_JOBS,
            [
                "writes-reconcile",
                "writes-expire-proposals",
                "writes-pr-jobs",
                "writes-credential-cleanup"
            ]
        );
        for name in WRITE_JOBS {
            assert!(ports::is_valid_lease_name(name), "{name}");
        }
    }

    #[test]
    fn every_interval_the_settings_allow_makes_a_valid_spec() {
        let high = WriteJobTimes {
            reconcile: Duration::from_secs(3600),
            expire: Duration::from_secs(86_400),
            pr_jobs: Duration::from_secs(300),
            cleanup: Duration::from_secs(7 * 86_400),
        };
        let low = WriteJobTimes {
            reconcile: Duration::from_secs(1),
            expire: Duration::from_secs(1),
            pr_jobs: Duration::from_secs(1),
            cleanup: Duration::from_secs(60),
        };
        assert!(specs(&high).is_ok());
        assert!(specs(&low).is_ok());
    }
}
