//! T6: the runner of singleton jobs (D62, D63).
//!
//! These tests run on the paused tokio clock against the `ports` fake leases, which expire on that clock, so a lease's ttl, a
//! job's interval and the renewal schedule can be driven without sleeping. The leases on a real database are in `lease_race.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use hub_platform::jobs::{
    AcquireOutcome, Job, JobContext, JobError, JobMetrics, JobOutcome, JobRunner, JobSpec, MAX_JOBS,
    RegisterError,
};
use ports::fakes::FakeLeases;
use ports::{LeaseBackend, LeaseError, LeaseGuard, Leases};
use prometheus_client::encoding::text;
use prometheus_client::registry::Registry;
use tokio::time::{Instant, sleep};

const SECOND: Duration = Duration::from_secs(1);

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn spec(name: &str, interval: u64, ttl: u64) -> JobSpec {
    JobSpec::new(name, secs(interval), secs(ttl)).unwrap()
}

#[derive(Default)]
struct Counting {
    runs: AtomicU32,
}

#[async_trait]
impl Job for Counting {
    async fn run(&self, _ctx: &JobContext) -> Result<(), JobError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl Counting {
    fn runs(&self) -> u32 {
        self.runs.load(Ordering::SeqCst)
    }
}

fn runner(leases: Arc<dyn Leases>, metrics: &JobMetrics) -> JobRunner {
    JobRunner::new(leases, metrics.clone())
}

// ---------------------------------------------------------------------------------------------------- who runs

#[tokio::test(start_paused = true)]
async fn only_the_lease_holder_runs_the_job() {
    let leases = FakeLeases::new();
    let (m1, m2) = (JobMetrics::default(), JobMetrics::default());
    let (j1, j2) = (Arc::new(Counting::default()), Arc::new(Counting::default()));
    let (mut r1, mut r2) = (
        runner(Arc::new(leases.clone()), &m1),
        runner(Arc::new(leases.clone()), &m2),
    );
    r1.register(spec("job.tick", 10, 30), j1.clone()).unwrap();
    r2.register(spec("job.tick", 10, 30), j2.clone()).unwrap();
    let (h1, h2) = (r1.start(), r2.start());

    sleep(secs(35)).await;

    assert!(
        (j1.runs() > 0) != (j2.runs() > 0),
        "exactly one replica ran the job: {} and {}",
        j1.runs(),
        j2.runs()
    );
    let total = j1.runs() + j2.runs();
    assert!(
        (3..=4).contains(&total),
        "one run per 10 s interval over 35 s, got {total}"
    );
    h1.shutdown().await;
    h2.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_holder_keeps_its_lease_between_runs_by_renewing_it() {
    let leases = FakeLeases::new();
    let (m1, m2) = (JobMetrics::default(), JobMetrics::default());
    let (j1, j2) = (Arc::new(Counting::default()), Arc::new(Counting::default()));
    let (mut r1, mut r2) = (
        runner(Arc::new(leases.clone()), &m1),
        runner(Arc::new(leases.clone()), &m2),
    );
    r1.register(spec("job.tick", 100, 30), j1.clone()).unwrap();
    r2.register(spec("job.tick", 100, 30), j2.clone()).unwrap();
    let (h1, h2) = (r1.start(), r2.start());

    // Ten times the ttl, with the job due only every 100 s: the lease would have run out many times over without renewal.
    sleep(secs(300)).await;

    let (winner, loser) = if j1.runs() > 0 { (&j1, &j2) } else { (&j2, &j1) };
    assert!(
        winner.runs() >= 3,
        "the holder ran every interval: {}",
        winner.runs()
    );
    assert_eq!(loser.runs(), 0, "nobody else took the lease while it was renewed");
    assert!(
        leases.try_acquire("job.tick", secs(30)).await.unwrap().is_none(),
        "still held"
    );
    h1.shutdown().await;
    h2.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_lease_is_taken_over_after_the_holder_dies() {
    let leases = FakeLeases::new();
    let (m1, m2) = (JobMetrics::default(), JobMetrics::default());
    let (j1, j2) = (Arc::new(Counting::default()), Arc::new(Counting::default()));
    let mut r1 = runner(Arc::new(leases.clone()), &m1);
    r1.register(spec("job.tick", 10, 30), j1.clone()).unwrap();
    let h1 = r1.start();
    sleep(secs(5)).await;
    assert_eq!(j1.runs(), 1);

    // The replica dies: its tasks vanish without releasing anything.
    drop(h1);

    let mut r2 = runner(Arc::new(leases.clone()), &m2);
    r2.register(spec("job.tick", 10, 30), j2.clone()).unwrap();
    let h2 = r2.start();
    sleep(secs(20)).await;
    assert_eq!(j2.runs(), 0, "the dead holder's lease has not run out yet");
    sleep(secs(30)).await;
    assert!(j2.runs() >= 1, "taken over after the ttl: {}", j2.runs());
    assert_eq!(j1.runs(), 1, "the dead replica ran nothing more");
    h2.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_releases_the_lease_for_the_next_replica() {
    let leases = FakeLeases::new();
    let (m1, m2) = (JobMetrics::default(), JobMetrics::default());
    let (j1, j2) = (Arc::new(Counting::default()), Arc::new(Counting::default()));
    let mut r1 = runner(Arc::new(leases.clone()), &m1);
    r1.register(spec("job.tick", 10, 30), j1.clone()).unwrap();
    let h1 = r1.start();
    sleep(SECOND).await;
    assert_eq!(j1.runs(), 1);

    h1.shutdown().await;

    let mut r2 = runner(Arc::new(leases.clone()), &m2);
    r2.register(spec("job.tick", 10, 30), j2.clone()).unwrap();
    let h2 = r2.start();
    sleep(SECOND).await;
    assert_eq!(
        j2.runs(),
        1,
        "a released lease is taken at once, not after its ttl"
    );
    h2.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn each_job_has_its_own_lease() {
    let leases = FakeLeases::new();
    let m = JobMetrics::default();
    let (first, second) = (Arc::new(Counting::default()), Arc::new(Counting::default()));
    let mut r = runner(Arc::new(leases.clone()), &m);
    r.register(spec("job.a", 10, 30), first.clone()).unwrap();
    r.register(spec("job.b", 10, 30), second.clone()).unwrap();
    let h = r.start();
    sleep(secs(15)).await;
    assert!(
        first.runs() >= 1 && second.runs() >= 1,
        "{} and {}",
        first.runs(),
        second.runs()
    );
    h.shutdown().await;
}

// ---------------------------------------------------------------------------------------- a store that can misbehave

const RENEW_OK: u8 = 0;
const RENEW_LOST: u8 = 1;
const RENEW_UNAVAILABLE: u8 = 2;

/// State of a lease store the test controls: what renewal answers, and whether anything can be acquired.
struct Scripted {
    renew_mode: AtomicU8,
    acquirable: AtomicBool,
    acquire_fails: AtomicBool,
    acquires: AtomicU32,
    renews: AtomicU32,
    releases: AtomicU32,
}

impl Scripted {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            renew_mode: AtomicU8::new(RENEW_OK),
            acquirable: AtomicBool::new(true),
            acquire_fails: AtomicBool::new(false),
            acquires: AtomicU32::new(0),
            renews: AtomicU32::new(0),
            releases: AtomicU32::new(0),
        })
    }
}

/// The backend of the guards `ScriptedStore` hands out: they share its counters and its renewal mode.
struct Shared(Arc<Scripted>);

#[async_trait]
impl LeaseBackend for Shared {
    async fn renew(&self, _name: &str, _token: u64, ttl: Duration) -> Result<Instant, LeaseError> {
        self.0.renews.fetch_add(1, Ordering::SeqCst);
        match self.0.renew_mode.load(Ordering::SeqCst) {
            RENEW_OK => Ok(Instant::now() + ttl),
            RENEW_LOST => Err(LeaseError::Lost),
            _ => Err(LeaseError::Unavailable),
        }
    }

    async fn release(&self, _name: &str, _token: u64) -> Result<(), LeaseError> {
        self.0.releases.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ScriptedStore(Arc<Scripted>);

#[async_trait]
impl Leases for ScriptedStore {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError> {
        let s = &self.0;
        if s.acquire_fails.load(Ordering::SeqCst) {
            return Err(LeaseError::Unavailable);
        }
        if !s.acquirable.load(Ordering::SeqCst) {
            return Ok(None);
        }
        s.acquires.fetch_add(1, Ordering::SeqCst);
        Ok(Some(LeaseGuard::new(
            name.to_owned(),
            1,
            Instant::now() + ttl,
            Arc::new(Shared(s.clone())),
        )))
    }
}

/// Works in units of one second, asking whether the lease is still held before each.
#[derive(Default)]
struct Units {
    done: AtomicU32,
    stopped_by: std::sync::Mutex<Vec<JobError>>,
}

#[async_trait]
impl Job for Units {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        for _ in 0..1000 {
            if let Err(e) = ctx.ensure_held() {
                self.stopped_by.lock().unwrap().push(e.clone());
                return Err(e);
            }
            sleep(SECOND).await;
            self.done.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn a_job_stops_when_renewal_says_the_lease_is_lost() {
    let store = Scripted::new();
    let m = JobMetrics::default();
    let job = Arc::new(Units::default());
    let mut r = runner(Arc::new(ScriptedStore(store.clone())), &m);
    r.register(spec("job.units", 1000, 9), job.clone()).unwrap();
    let h = r.start();

    sleep(secs(4)).await;
    assert!(job.done.load(Ordering::SeqCst) >= 3, "the job is working");
    // Someone else takes the lease: the next renewal (at 3 s, 6 s, ...) fails with Lost, and nothing can be acquired again.
    store.renew_mode.store(RENEW_LOST, Ordering::SeqCst);
    store.acquirable.store(false, Ordering::SeqCst);
    sleep(secs(5)).await;

    let done = job.done.load(Ordering::SeqCst);
    sleep(secs(60)).await;
    assert_eq!(
        job.done.load(Ordering::SeqCst),
        done,
        "no unit of work after the loss"
    );
    assert_eq!(job.stopped_by.lock().unwrap().as_slice(), &[JobError::LeaseLost]);
    assert_eq!(m.runs_total("job.units", JobOutcome::LeaseLost), 1);
    assert_eq!(m.lost_total("job.units"), 1);
    assert_eq!(m.held("job.units"), 0);
    assert_eq!(
        store.releases.load(Ordering::SeqCst),
        0,
        "a lost lease is not released"
    );
    h.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_job_checks_the_guard_expiry_before_each_unit_even_if_renewal_cannot_reach_the_store() {
    let store = Scripted::new();
    let m = JobMetrics::default();
    let job = Arc::new(Units::default());
    let mut r = runner(Arc::new(ScriptedStore(store.clone())), &m);
    r.register(spec("job.units", 1000, 9), job.clone()).unwrap();
    // Renewal fails without saying the lease is lost: the expiry the store granted is all the job can rely on.
    store.renew_mode.store(RENEW_UNAVAILABLE, Ordering::SeqCst);
    let h = r.start();
    sleep(SECOND).await;
    // The first acquire has happened; once the guard is dropped nothing can be acquired again.
    store.acquirable.store(false, Ordering::SeqCst);
    sleep(secs(30)).await;

    let done = job.done.load(Ordering::SeqCst);
    assert!(
        (8..=9).contains(&done),
        "units ran until the 9 s expiry and no longer: {done}"
    );
    assert_eq!(job.stopped_by.lock().unwrap().as_slice(), &[JobError::LeaseLost]);
    assert!(
        store.renews.load(Ordering::SeqCst) >= 2,
        "renewal was tried, at a third of the ttl"
    );
    h.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_lease_is_renewed_at_one_third_of_its_ttl() {
    let store = Scripted::new();
    let m = JobMetrics::default();
    let job = Arc::new(Counting::default());
    let mut r = runner(Arc::new(ScriptedStore(store.clone())), &m);
    r.register(spec("job.slow", 1000, 30), job.clone()).unwrap();
    let h = r.start();
    sleep(secs(65)).await;
    assert_eq!(store.acquires.load(Ordering::SeqCst), 1, "acquired once and kept");
    let renews = store.renews.load(Ordering::SeqCst);
    assert!(
        (5..=7).contains(&renews),
        "a renewal every 10 s over 65 s, got {renews}"
    );
    assert_eq!(m.held("job.slow"), 1);
    h.shutdown().await;
    assert_eq!(store.releases.load(Ordering::SeqCst), 1, "shutdown releases it");
    assert_eq!(m.held("job.slow"), 0);
}

#[tokio::test(start_paused = true)]
async fn the_lease_is_renewed_while_a_long_unit_of_work_runs() {
    let store = Scripted::new();
    let m = JobMetrics::default();
    let job = Arc::new(Units::default());
    let mut r = runner(Arc::new(ScriptedStore(store.clone())), &m);
    r.register(spec("job.long", 1000, 9), job.clone()).unwrap();
    let h = r.start();
    sleep(secs(100)).await;
    assert!(
        job.done.load(Ordering::SeqCst) >= 99,
        "ten times the ttl and the job never lost its lease"
    );
    assert!(job.stopped_by.lock().unwrap().is_empty());
    h.shutdown().await;
}

// ---------------------------------------------------------------------------------------------- bad jobs

struct Slow;

#[async_trait]
impl Job for Slow {
    async fn run(&self, _ctx: &JobContext) -> Result<(), JobError> {
        sleep(secs(100_000)).await;
        Ok(())
    }
}

struct Failing {
    runs: AtomicU32,
}

#[async_trait]
impl Job for Failing {
    async fn run(&self, _ctx: &JobContext) -> Result<(), JobError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Err(JobError::Failed("git_fetch_failed"))
    }
}

struct Panicking {
    runs: AtomicU32,
}

#[async_trait]
impl Job for Panicking {
    async fn run(&self, _ctx: &JobContext) -> Result<(), JobError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        panic!("a job that panics must not stop the runner");
    }
}

#[tokio::test(start_paused = true)]
async fn a_job_that_runs_too_long_is_cut_off_and_keeps_its_lease() {
    let leases = FakeLeases::new();
    let m = JobMetrics::default();
    let mut r = runner(Arc::new(leases.clone()), &m);
    r.register(
        spec("job.slow", 10, 30).with_max_run(secs(5)).unwrap(),
        Arc::new(Slow),
    )
    .unwrap();
    let h = r.start();
    sleep(secs(7)).await;
    assert_eq!(m.runs_total("job.slow", JobOutcome::TimedOut), 1);
    sleep(secs(10)).await;
    assert_eq!(
        m.runs_total("job.slow", JobOutcome::TimedOut),
        2,
        "it is tried again at the next interval"
    );
    sleep(secs(200)).await;
    assert!(
        leases.try_acquire("job.slow", secs(30)).await.unwrap().is_none(),
        "the lease was renewed throughout"
    );
    h.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_failing_job_is_counted_and_tried_again_at_the_next_interval() {
    let leases = FakeLeases::new();
    let m = JobMetrics::default();
    let job = Arc::new(Failing {
        runs: AtomicU32::new(0),
    });
    let mut r = runner(Arc::new(leases.clone()), &m);
    r.register(spec("job.fail", 10, 30), job.clone()).unwrap();
    let h = r.start();
    sleep(secs(25)).await;
    assert_eq!(job.runs.load(Ordering::SeqCst), 3);
    assert_eq!(m.runs_total("job.fail", JobOutcome::Failed), 3);
    assert_eq!(m.runs_total("job.fail", JobOutcome::Ok), 0);
    assert!(
        leases.try_acquire("job.fail", secs(30)).await.unwrap().is_none(),
        "a failure does not give up the lease"
    );
    h.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_panicking_job_is_counted_and_the_runner_carries_on() {
    let leases = FakeLeases::new();
    let m = JobMetrics::default();
    let job = Arc::new(Panicking {
        runs: AtomicU32::new(0),
    });
    let mut r = runner(Arc::new(leases.clone()), &m);
    r.register(spec("job.panic", 10, 30), job.clone()).unwrap();
    let h = r.start();
    sleep(secs(25)).await;
    assert_eq!(
        job.runs.load(Ordering::SeqCst),
        3,
        "it ran again after each panic"
    );
    assert_eq!(m.runs_total("job.panic", JobOutcome::Panicked), 3);
    h.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_unavailable_lease_store_runs_nothing_and_is_retried() {
    let store = Scripted::new();
    store.acquire_fails.store(true, Ordering::SeqCst);
    let m = JobMetrics::default();
    let job = Arc::new(Counting::default());
    let mut r = runner(Arc::new(ScriptedStore(store.clone())), &m);
    r.register(spec("job.tick", 10, 30), job.clone()).unwrap();
    let h = r.start();
    sleep(secs(35)).await;
    assert_eq!(job.runs(), 0, "no lease, no work");
    assert!(
        m.acquires_total("job.tick", AcquireOutcome::Failed) >= 2,
        "it keeps trying"
    );

    store.acquire_fails.store(false, Ordering::SeqCst);
    sleep(secs(15)).await;
    assert!(job.runs() >= 1, "and runs once the store is back");
    assert_eq!(m.acquires_total("job.tick", AcquireOutcome::Acquired), 1);
    h.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_replica_that_does_not_hold_the_lease_polls_at_a_bounded_rate() {
    let leases = FakeLeases::new();
    let _held = leases.try_acquire("job.tick", secs(3600)).await.unwrap().unwrap();
    let m = JobMetrics::default();
    let job = Arc::new(Counting::default());
    let mut r = runner(Arc::new(leases.clone()), &m);
    r.register(spec("job.tick", 10, 30), job.clone()).unwrap();
    let h = r.start();
    sleep(secs(100)).await;
    let busy = m.acquires_total("job.tick", AcquireOutcome::Busy);
    assert!((7..=14).contains(&busy), "about one attempt per 10 s, got {busy}");
    assert_eq!(job.runs(), 0);
    h.shutdown().await;
}

// ------------------------------------------------------------------------------------------ registration and metrics

#[tokio::test(start_paused = true)]
async fn registration_is_validated_and_bounded() {
    let leases = FakeLeases::new();
    let m = JobMetrics::default();
    let mut r = runner(Arc::new(leases), &m);
    r.register(spec("job.a", 10, 30), Arc::new(Counting::default()))
        .unwrap();
    assert_eq!(
        r.register(spec("job.a", 10, 30), Arc::new(Counting::default()))
            .unwrap_err(),
        RegisterError::Duplicate
    );
    for i in 1..MAX_JOBS {
        r.register(spec(&format!("job.{i}"), 10, 30), Arc::new(Counting::default()))
            .unwrap();
    }
    assert_eq!(
        r.register(spec("job.one-too-many", 10, 30), Arc::new(Counting::default()))
            .unwrap_err(),
        RegisterError::TooMany
    );
}

#[test]
fn job_specs_are_validated() {
    assert!(JobSpec::new("job.ok", secs(1), secs(3)).is_ok());
    assert!(JobSpec::new("", secs(1), secs(3)).is_err(), "empty name");
    assert!(
        JobSpec::new("Bad Name", secs(1), secs(3)).is_err(),
        "not a lease name"
    );
    assert!(
        JobSpec::new(&"x".repeat(129), secs(1), secs(3)).is_err(),
        "too long"
    );
    assert!(
        JobSpec::new("job.ok", Duration::ZERO, secs(3)).is_err(),
        "zero interval"
    );
    assert!(
        JobSpec::new("job.ok", secs(1), Duration::ZERO).is_err(),
        "zero ttl"
    );
    assert!(
        JobSpec::new("job.ok", secs(1), secs(3601)).is_err(),
        "ttl over the hour"
    );
    assert!(
        JobSpec::new("job.ok", secs(1), secs(3))
            .unwrap()
            .with_max_run(Duration::ZERO)
            .is_err()
    );
    assert!(
        JobSpec::new("job.ok", secs(1), secs(3))
            .unwrap()
            .with_max_run(secs(7200))
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn metrics_are_labelled_by_job_and_outcome_only() {
    let leases = FakeLeases::new();
    let m = JobMetrics::default();
    let mut registry = Registry::with_prefix("lanekeeper");
    m.register(&mut registry);
    let mut r = runner(Arc::new(leases), &m);
    r.register(spec("job.tick", 10, 30), Arc::new(Counting::default()))
        .unwrap();
    let h = r.start();
    sleep(secs(15)).await;

    let mut out = String::new();
    text::encode(&mut out, &registry).unwrap();
    assert!(
        out.contains(r#"lanekeeper_job_runs_total{job="job.tick",outcome="Ok"} 2"#),
        "{out}"
    );
    assert!(
        out.contains(r#"lanekeeper_job_acquires_total{job="job.tick",outcome="Acquired"} 1"#),
        "{out}"
    );
    assert!(
        out.contains(r#"lanekeeper_job_lease_held{job="job.tick"} 1"#),
        "{out}"
    );
    h.shutdown().await;
}
