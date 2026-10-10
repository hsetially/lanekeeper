//! Sync windows on the way to the hub (T10, D72, D75, A21): which events leave the agent, and in what order relative to
//! the deltas they are about, across an outage as well.
//!
//! The scanner, the spool, the pump, the ledger and the announcer are the real ones; the connection is an outbox that the
//! test reads, so the order in which things were queued is exactly what the hub would see. Virtual time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::clock::Clock;
use agent::ops::Metrics;
use agent::scan::Scanner;
use agent::spool::{Spool, SpoolLimits, SpoolOptions, SpoolVolume};
use agent::transport::outbox::{self, OutboxLimits, OutboxReceiver};
use agent::windows::announce::{Told, WindowAnnouncer};
use agent::windows::{WindowLedger, WindowTransition};
use domain::{ClusterReport, JobRef, SyncWindowKind};
use proto::convert::FromAgent;
use support::clock::TestClock;
use support::scripted_source::ScriptedSource;
use support::test_ca::T0_MS;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// What reached the connection, in the order it was queued.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Delta {
        seq: u64,
        job: Option<JobRef>,
        files: usize,
    },
    Opened(JobRef),
    Closed(JobRef),
}

struct Agent {
    clock: Arc<TestClock>,
    source: Arc<ScriptedSource>,
    spool: Spool,
    _spool_dir: TempDir,
    scanner: Scanner,
    ledger: Arc<WindowLedger>,
    announcer: Arc<WindowAnnouncer>,
    scan_task: JoinHandle<()>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.scan_task.abort();
    }
}

fn job(name: &str) -> JobRef {
    JobRef::new(name, &format!("uid-{name}")).unwrap()
}

impl Agent {
    async fn start() -> Self {
        let clock = Arc::new(TestClock::starting_at(T0_MS));
        let source = Arc::new(ScriptedSource::with_clock(clock.clone()));
        source.write("svc/app.yml", b"1");
        let dir = TempDir::new().unwrap();
        let dyn_clock: Arc<dyn Clock> = clock.clone();
        let (spool, _) = Spool::open(
            SpoolVolume::open(dir.path()).unwrap(),
            SpoolOptions::new(SpoolLimits::new(64 * 1024 * 1024, 100_000)).with_io(support::oob::spool_io()),
            dyn_clock.clone(),
        )
        .unwrap();
        let scanner = Scanner::with_seq(
            source.clone(),
            dyn_clock.clone(),
            spool.sink(),
            Metrics::detached(),
            spool.seq(),
        );
        let ledger = Arc::new(WindowLedger::new(dyn_clock));
        scanner.attach_windows(ledger.clone());
        let announcer = Arc::new(WindowAnnouncer::new(
            ledger.clone(),
            scanner.clone(),
            spool.clone(),
        ));
        let scan_task = {
            let scanner = scanner.clone();
            tokio::spawn(async move {
                let never = scanner.run().await;
                match never {}
            })
        };
        sleep(Duration::from_millis(1)).await;
        Self {
            clock,
            source,
            spool,
            _spool_dir: dir,
            scanner,
            ledger,
            announcer,
            scan_task,
        }
    }

    fn job_starts(&self, name: &str) {
        self.ledger.apply(WindowTransition::Running {
            job: job(name),
            started: None,
        });
    }

    fn job_ends(&self, name: &str) {
        self.ledger.apply(WindowTransition::Finished {
            job: job(name),
            finished: None,
        });
    }

    /// A connection that is read as fast as it fills.
    fn connect(&self) -> Connection {
        let (outbox, rx) = outbox::channel(OutboxLimits::default());
        Connection::wire(self, outbox, Reader::Now(rx))
    }

    /// A connection that holds only `messages` and is not read until [`Connection::start_reading`]: the hub is slow.
    fn connect_stalled(&self, messages: usize) -> Connection {
        let (outbox, rx) = outbox::channel(OutboxLimits {
            messages,
            bytes: 8 * 1024 * 1024,
        });
        Connection::wire(self, outbox, Reader::Later(rx))
    }
}

enum Reader {
    Now(OutboxReceiver),
    Later(OutboxReceiver),
}

struct Connection {
    seen: Arc<Mutex<Vec<Seen>>>,
    held_receiver: Option<OutboxReceiver>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn read_into(seen: Arc<Mutex<Vec<Seen>>>, mut rx: OutboxReceiver) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(queued) = rx.recv().await {
            let (message, _permit) = queued.into_parts();
            match FromAgent::from_proto(message).unwrap() {
                Some(FromAgent::Delta(d)) => seen.lock().unwrap().push(Seen::Delta {
                    seq: d.seq,
                    job: d.during_job.clone(),
                    files: d.entries.len(),
                }),
                Some(FromAgent::Cluster(report)) => {
                    for event in report.sync_windows {
                        seen.lock().unwrap().push(match event.kind {
                            SyncWindowKind::Opened => Seen::Opened(event.job),
                            SyncWindowKind::Closed => Seen::Closed(event.job),
                        });
                    }
                }
                _ => {}
            }
        }
    })
}

impl Connection {
    fn wire(agent: &Agent, outbox: agent::transport::Outbox, reader: Reader) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        let mut held_receiver = None;
        match reader {
            Reader::Now(rx) => tasks.push(read_into(seen.clone(), rx)),
            Reader::Later(rx) => held_receiver = Some(rx),
        }
        tasks.push(tokio::spawn(agent.spool.attach(outbox.clone()).run().map_end()));
        let told = Arc::new(Mutex::new(Told::default()));
        let announcer = agent.announcer.clone();
        tasks.push(tokio::spawn(async move {
            announcer.run(&outbox, &told).await;
        }));
        Self {
            seen,
            held_receiver,
            tasks,
        }
    }

    fn start_reading(&mut self) {
        if let Some(rx) = self.held_receiver.take() {
            self.tasks.push(read_into(self.seen.clone(), rx));
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn position(&self, wanted: &Seen) -> Option<usize> {
        self.seen().iter().position(|s| s == wanted)
    }
}

trait MapEnd {
    fn map_end(self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
}

impl<F> MapEnd for F
where
    F: std::future::Future<Output = agent::spool::PumpEnd> + Send + 'static,
{
    fn map_end(self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            let _ = self.await;
        })
    }
}

// ------------------------------------------------------------------------------------------------ the events

#[tokio::test(start_paused = true)]
async fn window_open_and_close_events_sent() {
    let agent = Agent::start().await;
    let conn = agent.connect();
    sleep(Duration::from_millis(100)).await;
    assert!(conn.seen().is_empty(), "no Job, nothing to say");

    agent.job_starts("csp-dataload-1");
    sleep(Duration::from_millis(100)).await;
    assert_eq!(conn.seen(), [Seen::Opened(job("csp-dataload-1"))]);

    agent.job_ends("csp-dataload-1");
    sleep(Duration::from_secs(2)).await;
    assert_eq!(
        conn.seen(),
        [
            Seen::Opened(job("csp-dataload-1")),
            Seen::Closed(job("csp-dataload-1"))
        ],
        "opened, then closed, once each"
    );
    sleep(Duration::from_secs(60)).await;
    assert_eq!(conn.seen().len(), 2, "and not again");
}

#[tokio::test(start_paused = true)]
async fn the_close_does_not_wait_for_the_next_walk_on_the_grid() {
    let agent = Agent::start().await;
    let conn = agent.connect();
    sleep(Duration::from_secs(12)).await; // just after the walk of 10 s
    agent.job_starts("csp-dataload-1");
    sleep(Duration::from_secs(1)).await;
    agent.job_ends("csp-dataload-1");
    let closed = tokio::time::Instant::now();
    while conn.position(&Seen::Closed(job("csp-dataload-1"))).is_none() {
        sleep(Duration::from_millis(100)).await;
        assert!(
            closed.elapsed() < Duration::from_secs(5),
            "the next walk on the grid is 6 s away"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn window_close_follows_last_tagged_delta() {
    let agent = Agent::start().await;
    let conn = agent.connect();
    sleep(Duration::from_secs(1)).await;
    agent.job_starts("csp-dataload-1");
    // The Job copies files for 8 s and the API shows it complete half a second after the last one.
    for i in 0..80 {
        agent.source.write(&format!("tenant/f{i:02}.yml"), b"x");
        sleep(Duration::from_millis(100)).await;
    }
    sleep(Duration::from_millis(500)).await;
    agent.job_ends("csp-dataload-1");
    sleep(Duration::from_secs(30)).await;

    let seen = conn.seen();
    let closed_at = seen
        .iter()
        .position(|s| *s == Seen::Closed(job("csp-dataload-1")))
        .expect("the close was announced");
    let tagged: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s, Seen::Delta { job: Some(_), .. }))
        .map(|(i, _)| i)
        .collect();
    assert!(!tagged.is_empty());
    assert!(
        tagged.iter().all(|i| *i < closed_at),
        "every tagged delta before the close: {seen:?}"
    );
    let files: usize = seen
        .iter()
        .map(|s| {
            if let Seen::Delta { files, .. } = s {
                *files
            } else {
                0
            }
        })
        .sum();
    assert_eq!(
        files, 80,
        "the close came after all of the copy, not after the first delta of it"
    );
    assert_eq!(seen.first(), Some(&Seen::Opened(job("csp-dataload-1"))));
}

#[tokio::test(start_paused = true)]
async fn the_close_waits_for_a_batch_that_is_still_being_held() {
    let agent = Agent::start().await;
    let conn = agent.connect();
    sleep(Duration::from_millis(11_500)).await;
    agent.job_starts("csp-dataload-1");
    sleep(Duration::from_millis(500)).await;
    agent.source.write("tenant/last.yml", b"x"); // at 12 s; the walk of 20 s will find it 8 s later
    agent.job_ends("csp-dataload-1"); // the Job is over; the file is not yet seen by any walk
    sleep(Duration::from_millis(200)).await;
    // The close triggered a walk, which found the file 0.2 s after it was written, and holds it: it is not quiet.
    let seen = conn.seen();
    assert_eq!(
        seen,
        [Seen::Opened(job("csp-dataload-1"))],
        "not closed while a tagged delta may still follow"
    );
    sleep(Duration::from_secs(5)).await;
    let seen = conn.seen();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(matches!(&seen[1], Seen::Delta { job: Some(j), files: 1, .. } if *j == job("csp-dataload-1")));
    assert_eq!(seen[2], Seen::Closed(job("csp-dataload-1")));
}

#[tokio::test(start_paused = true)]
async fn windows_are_not_closed_while_the_connection_still_holds_the_deltas_they_come_after() {
    let agent = Agent::start().await;
    agent.job_starts("csp-dataload-1");
    for round in 0..6 {
        agent.source.write(&format!("tenant/f{round}.yml"), b"x");
        sleep(Duration::from_secs(15)).await; // a delta per round
    }
    agent.job_ends("csp-dataload-1");
    sleep(Duration::from_secs(15)).await;

    // The hub is slow: the connection holds two messages and nothing is read.
    let mut conn = agent.connect_stalled(2);
    sleep(Duration::from_secs(60)).await;
    assert!(
        conn.position(&Seen::Closed(job("csp-dataload-1"))).is_none(),
        "nothing was read, so no close can have been sent"
    );
    conn.start_reading();
    sleep(Duration::from_secs(10)).await;
    let seen = conn.seen();
    let closed_at = seen
        .iter()
        .position(|s| *s == Seen::Closed(job("csp-dataload-1")))
        .unwrap();
    let deltas = seen.iter().filter(|s| matches!(s, Seen::Delta { .. })).count();
    assert!(deltas >= 6, "{seen:?}");
    assert_eq!(
        seen.iter()
            .rposition(|s| matches!(s, Seen::Delta { .. }))
            .unwrap()
            + 1,
        closed_at,
        "the close is the last thing queued, after every delta of the replay: {seen:?}"
    );
}

// ------------------------------------------------------------------------------------------------ after an outage (A21)

#[tokio::test(start_paused = true)]
async fn after_an_outage_the_new_connection_hears_of_the_window_and_the_close_comes_after_the_replay() {
    let agent = Agent::start().await;
    {
        let first = agent.connect();
        sleep(Duration::from_secs(1)).await;
        drop(first); // the hub goes away
    }
    // While it is away a Job runs, changes files, and ends. Everything is spooled.
    agent.job_starts("csp-dataload-1");
    for i in 0..3 {
        agent.source.write(&format!("tenant/f{i}.yml"), b"x");
        sleep(Duration::from_secs(12)).await;
    }
    agent.job_ends("csp-dataload-1");
    sleep(Duration::from_secs(30)).await;
    assert!(agent.spool.stats().entries >= 3);

    let conn = agent.connect();
    sleep(Duration::from_secs(5)).await;
    let seen = conn.seen();
    assert!(
        seen.contains(&Seen::Opened(job("csp-dataload-1"))),
        "the new connection is told of the window: {seen:?}"
    );
    let closed_at = seen
        .iter()
        .position(|s| *s == Seen::Closed(job("csp-dataload-1")))
        .expect("and of its end");
    let last_delta = seen
        .iter()
        .rposition(|s| matches!(s, Seen::Delta { .. }))
        .unwrap();
    assert!(
        last_delta < closed_at,
        "the replay first, then the close: {seen:?}"
    );
    assert!(
        seen.iter()
            .filter(|s| matches!(s, Seen::Delta { .. }))
            .all(|s| matches!(s, Seen::Delta { job: Some(j), .. } if *j == job("csp-dataload-1"))),
        "all tagged: {seen:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_window_that_closed_in_the_last_hour_is_repeated_and_an_older_one_is_not() {
    let agent = Agent::start().await;
    agent.job_starts("old-dataload");
    agent.job_ends("old-dataload");
    sleep(Duration::from_secs(61 * 60)).await;
    agent.job_starts("recent-dataload");
    agent.job_ends("recent-dataload");
    sleep(Duration::from_secs(30)).await;
    agent.job_starts("running-dataload");

    let conn = agent.connect();
    sleep(Duration::from_secs(5)).await;
    let seen = conn.seen();
    assert!(seen.contains(&Seen::Opened(job("recent-dataload"))), "{seen:?}");
    assert!(seen.contains(&Seen::Closed(job("recent-dataload"))), "{seen:?}");
    assert!(seen.contains(&Seen::Opened(job("running-dataload"))), "{seen:?}");
    assert!(!seen.contains(&Seen::Closed(job("running-dataload"))), "{seen:?}");
    assert!(
        seen.iter()
            .all(|s| !matches!(s, Seen::Opened(j) | Seen::Closed(j) if *j == job("old-dataload"))),
        "a window that closed over an hour ago is forgotten: {seen:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn each_connection_is_told_afresh_and_nothing_twice_on_one() {
    let agent = Agent::start().await;
    agent.job_starts("csp-dataload-1");
    let first = agent.connect();
    sleep(Duration::from_secs(5)).await;
    agent.job_ends("csp-dataload-1");
    sleep(Duration::from_secs(30)).await;
    assert_eq!(
        first.seen(),
        [
            Seen::Opened(job("csp-dataload-1")),
            Seen::Closed(job("csp-dataload-1"))
        ]
    );
    let second = agent.connect();
    sleep(Duration::from_secs(30)).await;
    assert_eq!(
        second.seen(),
        [
            Seen::Opened(job("csp-dataload-1")),
            Seen::Closed(job("csp-dataload-1"))
        ],
        "a hub that was away may have missed both, so the next connection hears both"
    );
}

#[tokio::test(start_paused = true)]
async fn the_snapshot_for_a_full_report_has_the_same_rule() {
    let agent = Agent::start().await;
    agent.job_starts("csp-dataload-1");
    // No connection: nothing has been sent, so a close cannot be announced yet.
    let events = agent.announcer.snapshot();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, SyncWindowKind::Opened);
    agent.job_ends("csp-dataload-1");
    sleep(Duration::from_secs(15)).await;
    let events = agent.announcer.snapshot();
    assert_eq!(
        events.len(),
        1,
        "no pump is sending, so the spool is not drained: {events:?}"
    );
    let _conn = agent.connect();
    sleep(Duration::from_secs(2)).await;
    let kinds: Vec<_> = agent.announcer.snapshot().iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [SyncWindowKind::Opened, SyncWindowKind::Closed]);
}

#[tokio::test(start_paused = true)]
async fn the_wire_report_carries_nothing_but_the_windows() {
    // The announcer's reports are deltas of the cluster picture: no deployments, no hints.
    let agent = Agent::start().await;
    let (outbox, mut rx) = outbox::channel(OutboxLimits::default());
    let announcer = agent.announcer.clone();
    let told = Arc::new(Mutex::new(Told::default()));
    let task = tokio::spawn(async move { announcer.run(&outbox, &told).await });
    agent.job_starts("csp-dataload-1");
    let (message, _permit) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the report arrives")
        .unwrap()
        .into_parts();
    let Some(FromAgent::Cluster(ClusterReport {
        full,
        deployments,
        release_hints,
        config_server_started_at,
        sync_windows,
    })) = FromAgent::from_proto(message).unwrap()
    else {
        panic!("a cluster report");
    };
    assert!(!full);
    assert!(deployments.is_empty() && release_hints.is_empty() && config_server_started_at.is_none());
    assert_eq!(sync_windows.len(), 1);
    task.abort();
    let _ = (agent.clock.elapsed(), &agent.scanner);
}
