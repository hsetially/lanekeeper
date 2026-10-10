//! The scan loop (T4, D63, P1): when it walks, what it sends on its own and what it keeps for the hub to ask for. All of
//! it in virtual time, on an in-memory file system, so ten minutes cost nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::clock::Clock;
use agent::config::Tunables;
use agent::fileops::TreeEdits;
use agent::scan::{ScanRequestError, Scanner};
use agent::tree::{Entry, ScanError, ScanMode};
use agent::windows::{WindowLedger, WindowTransition};
use domain::{ContentHash, JobRef, NfsPath};
use support::clock::TestClock;
use support::collect_sink::CollectSink;
use support::scripted_source::ScriptedSource;
use support::test_ca::T0_MS;
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// A sink with no connection behind it.
#[derive(Debug)]
struct NoConnection;

#[async_trait::async_trait]
impl agent::tree::delta::DeltaSink for NoConnection {
    async fn deliver(&self, _: domain::ScanDelta) -> Result<(), agent::tree::delta::SinkError> {
        Err(agent::tree::delta::SinkError::Closed)
    }

    fn ready(&self) -> bool {
        false
    }
}

struct Rig {
    source: Arc<ScriptedSource>,
    sink: Arc<CollectSink>,
    scanner: Scanner,
    ledger: Arc<WindowLedger>,
    task: Option<JoinHandle<()>>,
}

impl Rig {
    /// Files written through this rig's source look old (2023), as files copied with their times preserved do: a change
    /// is never recent, so it is quiet from the previous walk on.
    fn new() -> Self {
        Self::with_source(
            Arc::new(ScriptedSource::new()),
            Arc::new(TestClock::starting_at(T0_MS)),
        )
    }

    /// Files written through this rig's source are stamped with the rig's clock, as a real file system stamps them.
    fn realistic() -> Self {
        let clock = Arc::new(TestClock::starting_at(T0_MS));
        Self::with_source(Arc::new(ScriptedSource::with_clock(clock.clone())), clock)
    }

    fn with_source(source: Arc<ScriptedSource>, clock: Arc<TestClock>) -> Self {
        let sink = Arc::new(CollectSink::new());
        let clock: Arc<dyn Clock> = clock;
        let ledger = Arc::new(WindowLedger::new(clock.clone()));
        let scanner = Scanner::new(source.clone(), clock, sink.clone());
        scanner.attach_windows(ledger.clone());
        Self {
            source,
            sink,
            scanner,
            ledger,
            task: None,
        }
    }

    fn job(name: &str) -> JobRef {
        JobRef::new(name, &format!("uid-{name}")).unwrap()
    }

    fn job_starts(&self, name: &str) {
        self.ledger.apply(WindowTransition::Running {
            job: Self::job(name),
            started: None,
        });
    }

    fn job_ends(&self, name: &str) {
        self.ledger.apply(WindowTransition::Finished {
            job: Self::job(name),
            finished: None,
        });
    }

    /// Start the loop. The first scan happens at once.
    fn start(&mut self) {
        let scanner = self.scanner.clone();
        self.task = Some(tokio::spawn(async move {
            let never = scanner.run().await;
            match never {}
        }));
    }

    /// Let the loop run for `d` of virtual time.
    async fn run_for(&self, d: Duration) {
        sleep(d).await;
    }

    fn root(&self) -> ContentHash {
        self.scanner.snapshot().expect("a baseline scan").root
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn the_first_scan_is_a_baseline_it_publishes_a_root_and_sends_nothing() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.source.write("d/b.yml", b"2");
    assert!(rig.scanner.snapshot().is_none(), "no root before the first scan");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let snapshot = rig.scanner.snapshot().unwrap();
    assert_eq!(snapshot.root, rig.source.tree().root_hash());
    assert_eq!(snapshot.file_count, 2);
    assert_eq!(snapshot.scan_seq, 1);
    assert!(
        rig.sink.deltas().is_empty(),
        "the hub learns the baseline from the heartbeat, not from a delta"
    );
}

#[tokio::test(start_paused = true)]
async fn a_change_is_pushed_as_a_delta_at_the_next_walk() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let before = rig.root();

    rig.source.write("a.yml", b"changed");
    rig.source.write("new.yml", b"n");
    rig.run_for(Duration::from_secs(10)).await;

    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].base_root, Some(before));
    assert_eq!(deltas[0].new_root, rig.source.tree().root_hash());
    assert_eq!(deltas[0].entries.len(), 2);
    assert_eq!(
        rig.root(),
        deltas[0].new_root,
        "the heartbeat root follows the delta"
    );
}

#[tokio::test(start_paused = true)]
async fn nothing_changing_sends_nothing_and_walks_every_ten_seconds() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    // Five seconds past the last walk, so that the end of the wait does not race with a tick.
    rig.run_for(Duration::from_secs(305)).await;
    assert!(rig.sink.deltas().is_empty());
    // At 0, 10, 20 ... 300 s.
    assert_eq!(rig.source.scans(), 31);
    assert_eq!(rig.scanner.snapshot().unwrap().scan_seq, 31);
}

#[tokio::test(start_paused = true)]
async fn the_walk_interval_follows_the_hubs_configuration() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    rig.scanner.set_tunables(Tunables {
        scan_interval: Duration::from_secs(60),
        ..Tunables::default()
    });
    rig.run_for(Duration::from_secs(300)).await;
    // The first walk, then one every 60 s from the change of setting (at most 6 more, at least 4).
    let scans = rig.source.scans();
    assert!((5..=7).contains(&scans), "{scans} scans");
}

#[tokio::test(start_paused = true)]
async fn everything_is_rehashed_every_fifteen_minutes_and_not_in_between() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_secs(40 * 60)).await;
    let modes = rig.source.modes();
    let full: Vec<usize> = modes
        .iter()
        .enumerate()
        .filter(|(_, m)| **m == ScanMode::Full)
        .map(|(i, _)| i)
        .collect();
    // Scans are 10 s apart: the baseline (0 s), then 900 s and 1,800 s later.
    assert_eq!(full, [0, 90, 180], "{} scans", modes.len());
}

#[tokio::test(start_paused = true)]
async fn unmounted_root_is_not_mass_deletion() {
    let mut rig = Rig::new();
    for i in 0..5 {
        rig.source.write(&format!("d/f{i}.yml"), b"x");
    }
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let before = rig.root();

    // The mount drops: the directory is there and empty.
    rig.source.remove_all();
    rig.run_for(Duration::from_secs(25)).await;
    assert_eq!(rig.root(), before, "an empty root is held, not believed");
    assert!(rig.sink.deltas().is_empty());

    // The mount comes back before the hold is over: nothing happened.
    for i in 0..5 {
        rig.source.write(&format!("d/f{i}.yml"), b"x");
    }
    rig.run_for(Duration::from_secs(60)).await;
    assert_eq!(rig.root(), before);
    assert!(rig.sink.deltas().is_empty());

    // Empty for good: after the 30 s deferral it is accepted as it is.
    rig.source.remove_all();
    rig.run_for(Duration::from_secs(45)).await;
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    assert_eq!(deltas[0].removed.len(), 5);
}

#[tokio::test(start_paused = true)]
async fn a_failed_scan_changes_nothing_and_the_next_one_recovers() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let before = rig.root();

    rig.source
        .fail_next_scans(ScanError::Root(std::io::ErrorKind::StaleNetworkFileHandle));
    rig.source.write("a.yml", b"changed while the mount was down");
    rig.run_for(Duration::from_secs(35)).await;
    assert_eq!(rig.root(), before);
    assert!(rig.sink.deltas().is_empty());
    assert!(rig.scanner.stats().scan_errors() >= 3);

    rig.source.stop_failing_scans();
    rig.run_for(Duration::from_secs(10)).await;
    assert_eq!(rig.sink.take().len(), 1);
    assert_ne!(rig.root(), before);
}

// ------------------------------------------------------------------------------------- what the hub asks for

#[tokio::test(start_paused = true)]
async fn a_root_in_the_ring_gets_a_delta_from_that_root() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let r0 = rig.root();
    rig.source.write("b.yml", b"2");
    rig.run_for(Duration::from_secs(10)).await;
    let r1 = rig.root();
    rig.source.write("c.yml", b"3");
    rig.run_for(Duration::from_secs(10)).await;
    let r2 = rig.root();
    rig.sink.take();

    let answer = CollectSink::new();
    rig.scanner.answer_delta(Some(r0), &answer).await.unwrap();
    let deltas = answer.take();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].base_root, Some(r0));
    assert_eq!(deltas[0].new_root, r2);
    let paths: Vec<_> = deltas[0].entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["b.yml", "c.yml"]);

    rig.scanner.answer_delta(Some(r1), &answer).await.unwrap();
    let paths: Vec<_> = answer.take()[0]
        .entries
        .iter()
        .map(|e| e.path.as_str().to_owned())
        .collect();
    assert_eq!(paths, ["c.yml"]);
    // Asked for on request, not pushed.
    assert!(rig.sink.deltas().is_empty());
}

#[tokio::test(start_paused = true)]
async fn unknown_root_yields_full_listing() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.source.write("d/b.yml", b"2");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;

    let answer = CollectSink::new();
    rig.scanner
        .answer_delta(Some(ContentHash::from_bytes([7; 32])), &answer)
        .await
        .unwrap();
    let deltas = answer.take();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].base_root, None, "a full listing has no base");
    assert_eq!(deltas[0].entries.len(), 2);

    rig.scanner.answer_delta(None, &answer).await.unwrap();
    assert_eq!(answer.take()[0].base_root, None);
}

#[tokio::test(start_paused = true)]
async fn the_current_root_gets_an_empty_delta() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let now = rig.root();
    let answer = CollectSink::new();
    rig.scanner.answer_delta(Some(now), &answer).await.unwrap();
    let deltas = answer.take();
    assert_eq!(deltas.len(), 1);
    assert!(deltas[0].entries.is_empty());
    assert_eq!((deltas[0].base_root, deltas[0].new_root), (Some(now), now));
}

#[tokio::test(start_paused = true)]
async fn nothing_can_be_asked_before_the_first_scan() {
    let rig = Rig::new();
    let answer = CollectSink::new();
    assert!(matches!(
        rig.scanner.answer_delta(None, &answer).await,
        Err(ScanRequestError::NotReady)
    ));
}

#[tokio::test(start_paused = true)]
async fn roots_older_than_an_hour_are_forgotten() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let old = rig.root();
    rig.source.write("b.yml", b"2");
    rig.run_for(Duration::from_secs(10)).await;
    rig.sink.take();

    let answer = CollectSink::new();
    rig.scanner.answer_delta(Some(old), &answer).await.unwrap();
    assert_eq!(answer.take()[0].base_root, Some(old), "still in the ring");

    rig.run_for(Duration::from_secs(61 * 60)).await;
    rig.scanner.answer_delta(Some(old), &answer).await.unwrap();
    assert_eq!(
        answer.take()[0].base_root,
        None,
        "an hour later it is a full listing"
    );
}

#[tokio::test(start_paused = true)]
async fn the_lower_file_limit_from_the_hub_applies_to_deltas() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    rig.scanner.set_tunables(Tunables {
        max_file_bytes: 100,
        ..Tunables::default()
    });
    rig.source.write("big.bin", &[1; 500]);
    rig.source.write("small.bin", &[1; 50]);
    rig.run_for(Duration::from_secs(10)).await;
    let d = &rig.sink.take()[0];
    assert_eq!(
        d.entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
        ["small.bin"]
    );
    assert_eq!(d.skipped[0].reason.as_str(), "too_large");
}

#[tokio::test(start_paused = true)]
async fn sequence_numbers_rise_across_pushed_and_requested_deltas() {
    let mut rig = Rig::new();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let mut seqs = Vec::new();
    for i in 0..3 {
        rig.source.write(&format!("f{i}"), b"x");
        rig.run_for(Duration::from_secs(10)).await;
        seqs.extend(rig.sink.take().iter().map(|d| d.seq));
        let answer = CollectSink::new();
        rig.scanner.answer_delta(None, &answer).await.unwrap();
        seqs.extend(answer.take().iter().map(|d| d.seq));
    }
    assert_eq!(seqs.len(), 6);
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
}

#[tokio::test(start_paused = true)]
async fn with_no_connection_no_changed_file_is_read() {
    let source = Arc::new(ScriptedSource::new());
    source.write("a.yml", b"1");
    let clock: Arc<dyn Clock> = Arc::new(TestClock::starting_at(T0_MS));
    let scanner = Scanner::new(source.clone(), clock, Arc::new(NoConnection));
    let task = {
        let scanner = scanner.clone();
        tokio::spawn(async move {
            let never = scanner.run().await;
            match never {}
        })
    };
    sleep(Duration::from_millis(1)).await;
    let before = scanner.snapshot().unwrap().root;
    for i in 0..5 {
        source.write(&format!("new-{i}.yml"), &[7; 1000]);
        sleep(Duration::from_secs(10)).await;
    }
    // The tree followed every change, and not one file was read for a delta.
    let after = scanner.snapshot().unwrap().root;
    assert_ne!(after, before);
    assert_eq!(after, source.tree().root_hash());
    assert_eq!(source.reads(), 0);
    assert_eq!(scanner.stats().deltas_skipped_offline(), 5);
    assert_eq!(scanner.stats().delta_failures(), 0);
    // And the roots in between are still in the ring for the hub to ask for.
    let answer = CollectSink::new();
    scanner.answer_delta(Some(before), &answer).await.unwrap();
    let deltas = answer.take();
    assert_eq!(deltas[0].base_root, Some(before));
    assert_eq!(deltas[0].entries.len(), 5);
    task.abort();
}

// ------------------------------------------------------------------------------------------------ quiescence (T10)

fn paths_of(delta: &domain::ScanDelta) -> Vec<String> {
    delta.entries.iter().map(|e| e.path.as_str().to_owned()).collect()
}

/// Step through virtual time in 100 ms steps until `sink` holds a delta, and say how long that took from `since`. `None`
/// if nothing came within `limit`.
async fn wait_for_delta(rig: &Rig, since: tokio::time::Instant, limit: Duration) -> Option<Duration> {
    let mut waited = Duration::ZERO;
    while waited <= limit {
        if !rig.sink.deltas().is_empty() {
            return Some(since.elapsed());
        }
        sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }
    None
}

#[tokio::test(start_paused = true)]
async fn quiet_for_3s_before_delta() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;

    // The walks are at 0, 10, 20 s. A file written at 18.5 s is 1.5 s old at the walk of 20 s.
    sleep(Duration::from_millis(18_499)).await;
    rig.source.write("new.yml", b"fresh");
    let written = tokio::time::Instant::now();

    sleep(Duration::from_millis(1_600)).await; // 20.1 s: the walk found it and is waiting
    assert!(rig.sink.deltas().is_empty(), "1.6 s old: not quiet yet");
    sleep(Duration::from_millis(1_300)).await; // 21.4 s: 2.9 s after the write
    assert!(rig.sink.deltas().is_empty(), "2.9 s old: not quiet yet");
    sleep(Duration::from_millis(700)).await; // 22.1 s
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1, "3 s after the write the delta goes out");
    assert_eq!(paths_of(&deltas[0]), ["new.yml"]);
    let took = written.elapsed();
    assert!(took >= Duration::from_secs(3), "{took:?}");
}

#[tokio::test(start_paused = true)]
async fn a_change_that_was_finished_before_the_walk_is_pushed_at_that_walk() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;

    sleep(Duration::from_millis(11_000)).await;
    rig.source.write("early.yml", b"done");
    let written = tokio::time::Instant::now();
    let took = wait_for_delta(&rig, written, Duration::from_secs(20))
        .await
        .unwrap();
    // Written at 11 s, found by the walk of 20 s: nine seconds old, so there is nothing to wait for.
    assert!(
        took >= Duration::from_secs(8) && took <= Duration::from_millis(9_100),
        "{took:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn deferred_at_most_30s_under_constant_change() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    sleep(Duration::from_secs(1)).await;

    // A file every half second, for a minute: the tree is never quiet.
    let writer = {
        let source = rig.source.clone();
        tokio::spawn(async move {
            for i in 0..120 {
                source.write(&format!("busy/f{i:03}.yml"), b"x");
                sleep(Duration::from_millis(500)).await;
            }
        })
    };
    let started = tokio::time::Instant::now();
    let first = wait_for_delta(&rig, started, Duration::from_secs(60))
        .await
        .unwrap();
    assert!(
        first >= Duration::from_secs(30) && first <= Duration::from_millis(31_200),
        "the first delta goes out 30 s after the first change, not before and not much after: {first:?}"
    );
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1);
    let n = deltas[0].entries.len();
    assert!(
        (55..=62).contains(&n),
        "everything written so far is in it: {n} files"
    );

    // The writer is still going: the rest is a second delta, 30 s after the walk that started it.
    writer.await.unwrap();
    sleep(Duration::from_secs(40)).await;
    let rest: usize = rig.sink.take().iter().map(|d| d.entries.len()).sum();
    assert_eq!(n + rest, 120, "no file is lost between the deltas");
}

#[tokio::test(start_paused = true)]
async fn the_scanner_walks_every_second_only_while_changes_are_held() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    sleep(Duration::from_millis(18_499)).await;
    rig.source.write("new.yml", b"fresh");

    // Walks: 0, 10, 20 (finds the change, 1.5 s old), 21 (2.5 s), 22 (3.5 s: released), then back to the 10 s grid.
    sleep(Duration::from_millis(16_500)).await; // 35 s
    assert_eq!(rig.sink.take().len(), 1);
    assert_eq!(rig.source.scans(), 6, "0, 10, 20, 21, 22, 30");
    sleep(Duration::from_secs(60)).await;
    assert_eq!(
        rig.source.scans(),
        12,
        "and a walk every 10 s again while nothing changes"
    );
}

#[tokio::test(start_paused = true)]
async fn the_heartbeat_root_is_the_one_the_hub_was_told_while_changes_are_held() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let told = rig.root();
    sleep(Duration::from_millis(18_499)).await;
    rig.source.write("new.yml", b"fresh");
    sleep(Duration::from_millis(1_600)).await; // held
    assert_eq!(
        rig.root(),
        told,
        "a root the hub has no delta for would only make it ask"
    );
    sleep(Duration::from_secs(3)).await; // released
    assert_eq!(rig.root(), rig.source.tree().root_hash());
    assert_eq!(rig.sink.take()[0].new_root, rig.root());
}

#[tokio::test(start_paused = true)]
async fn a_change_that_is_undone_before_it_is_released_is_never_reported() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"original");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    sleep(Duration::from_millis(18_499)).await;
    rig.source.write("a.yml", b"edited");
    sleep(Duration::from_millis(1_600)).await; // the walk of 20 s sees "edited", and waits
    rig.source.write("a.yml", b"original");
    sleep(Duration::from_secs(30)).await;
    assert!(
        rig.sink.deltas().is_empty(),
        "the hub never knew the edit, and the file is as it was"
    );
}

#[tokio::test(start_paused = true)]
async fn a_tool_write_while_changes_are_held_is_not_reported_as_a_change() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    sleep(Duration::from_millis(18_499)).await;
    rig.source.write("oob.yml", b"out of band");
    sleep(Duration::from_millis(1_600)).await; // held

    // The hub writes a file through the agent while the out-of-band change is waiting.
    rig.source.write("tool.yml", b"from the hub");
    let tree = rig.source.tree();
    let Some(Entry::File(leaf)) = tree.get("tool.yml") else {
        panic!("the file is in the source");
    };
    rig.scanner
        .written(&NfsPath::parse("tool.yml").unwrap(), *leaf)
        .await;

    sleep(Duration::from_secs(5)).await;
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1);
    assert_eq!(
        paths_of(&deltas[0]),
        ["oob.yml"],
        "only what the hub did not do itself"
    );
    assert_eq!(rig.root(), rig.source.tree().root_hash());
}

#[tokio::test(start_paused = true)]
async fn a_hub_request_while_changes_are_held_gets_them_and_ends_the_hold() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    let told = rig.root();
    sleep(Duration::from_millis(18_499)).await;
    rig.source.write("new.yml", b"fresh");
    sleep(Duration::from_millis(1_600)).await; // held
    assert!(rig.sink.deltas().is_empty());

    let answer = CollectSink::new();
    rig.scanner.answer_delta(Some(told), &answer).await.unwrap();
    let deltas = answer.take();
    assert_eq!(
        paths_of(&deltas[0]),
        ["new.yml"],
        "what the hub asked for is answered from the tree"
    );

    // The hub has it now: the held change is not sent again when it would have been released.
    sleep(Duration::from_secs(10)).await;
    assert!(rig.sink.deltas().is_empty());
    assert_eq!(rig.root(), rig.source.tree().root_hash());
}

#[tokio::test(start_paused = true)]
async fn a_delta_that_cannot_be_delivered_leaves_the_root_for_the_hub_to_ask_for() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.sink.fail_after(0);
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    sleep(Duration::from_millis(11_000)).await;
    rig.source.write("new.yml", b"fresh");
    sleep(Duration::from_secs(10)).await;
    assert_eq!(
        rig.scanner.stats().delta_failures(),
        1,
        "tried once, not every second for ever"
    );
    assert_eq!(
        rig.root(),
        rig.source.tree().root_hash(),
        "the heartbeat now shows a root the hub has no delta for, and it asks"
    );
    sleep(Duration::from_secs(30)).await;
    assert_eq!(rig.scanner.stats().delta_failures(), 1);
}

#[tokio::test(start_paused = true)]
async fn kick_walks_at_once_without_moving_the_ten_second_grid() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    sleep(Duration::from_secs(3)).await;
    assert_eq!(rig.source.scans(), 1);
    rig.scanner.kick();
    sleep(Duration::from_millis(10)).await;
    assert_eq!(rig.source.scans(), 2, "a kick is a walk now");
    sleep(Duration::from_secs(8)).await; // 11 s
    assert_eq!(
        rig.source.scans(),
        3,
        "and the walk of 10 s still happened at 10 s"
    );
}

#[tokio::test(start_paused = true)]
async fn the_progress_says_which_walk_has_finished_and_whether_changes_are_held() {
    let mut rig = Rig::realistic();
    rig.source.write("a.yml", b"1");
    let progress = rig.scanner.progress();
    assert_eq!(progress.borrow().walked, None, "no walk yet");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    assert_eq!(progress.borrow().walked, Some(0));
    assert!(!progress.borrow().held);

    rig.job_starts("dataload-1");
    sleep(Duration::from_millis(18_499)).await;
    rig.source.write("new.yml", b"fresh");
    sleep(Duration::from_millis(1_600)).await;
    let now = progress.borrow().clone();
    assert_eq!(
        now.walked,
        Some(1),
        "the walk of 20 s read the ledger at generation 1"
    );
    assert!(now.held);
    sleep(Duration::from_secs(3)).await;
    assert!(!progress.borrow().held, "released");
}

// ------------------------------------------------------------------------------------------------ sync windows (D75)

#[tokio::test(start_paused = true)]
async fn copy_of_600_files_during_job_gives_at_most_two_deltas_all_tagged() {
    let mut rig = Rig::realistic();
    rig.source.write("svc/app.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;

    // The Job starts at 2 s, copies 600 files from 3 s to 11 s, and the API shows it complete at 11.5 s.
    sleep(Duration::from_secs(2)).await;
    rig.job_starts("csp-dataload-1");
    sleep(Duration::from_secs(1)).await;
    for i in 0..600 {
        rig.source
            .write(&format!("tenant/f{i:03}.yml"), format!("{i}").as_bytes());
        sleep(Duration::from_millis(13)).await;
    }
    sleep(Duration::from_millis(3_000 - 600 * 13 % 3_000)).await;
    rig.job_ends("csp-dataload-1");
    sleep(Duration::from_secs(30)).await;

    let deltas = rig.sink.take();
    assert!(deltas.len() <= 2, "{} deltas for one copy", deltas.len());
    let files: usize = deltas.iter().map(|d| d.entries.len()).sum();
    assert_eq!(files, 600);
    let job = Rig::job("csp-dataload-1");
    assert!(
        deltas.iter().all(|d| d.during_job.as_ref() == Some(&job)),
        "every delta says which Job it was seen during: {:?}",
        deltas.iter().map(|d| &d.during_job).collect::<Vec<_>>()
    );
}

#[tokio::test(start_paused = true)]
async fn a_change_found_only_after_the_job_ended_is_still_tagged() {
    let mut rig = Rig::realistic();
    rig.source.write("svc/app.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;

    // The Job runs between two walks and writes its file just before it ends; the next walk comes after the end.
    sleep(Duration::from_secs(12)).await;
    rig.job_starts("csp-dataload-2");
    sleep(Duration::from_secs(2)).await;
    rig.source.write("tenant/late.yml", b"x");
    sleep(Duration::from_millis(500)).await;
    rig.job_ends("csp-dataload-2");
    sleep(Duration::from_secs(20)).await;

    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].during_job, Some(Rig::job("csp-dataload-2")));
}

#[tokio::test(start_paused = true)]
async fn a_change_with_no_job_around_is_not_tagged() {
    let mut rig = Rig::realistic();
    rig.source.write("svc/app.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    // A Job came and went before the walks that matter.
    rig.job_starts("csp-dataload-3");
    sleep(Duration::from_secs(1)).await;
    rig.job_ends("csp-dataload-3");
    sleep(Duration::from_secs(25)).await; // walks at 10 and 20 settle it
    rig.source.write("svc/other.yml", b"oob");
    sleep(Duration::from_secs(20)).await;
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].during_job, None);
}

#[tokio::test(start_paused = true)]
async fn the_oldest_running_job_tags_when_several_run() {
    let mut rig = Rig::realistic();
    rig.source.write("svc/app.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    rig.job_starts("csp-dataload-b");
    rig.job_starts("csp-dataload-a");
    sleep(Duration::from_secs(5)).await;
    rig.source.write("tenant/x.yml", b"x");
    sleep(Duration::from_secs(15)).await;
    assert_eq!(
        rig.sink.take()[0].during_job,
        Some(Rig::job("csp-dataload-b")),
        "the one that opened first"
    );
}

#[tokio::test(start_paused = true)]
async fn an_answer_to_the_hub_while_a_job_runs_is_tagged_too() {
    let mut rig = Rig::realistic();
    rig.source.write("svc/app.yml", b"1");
    rig.start();
    rig.run_for(Duration::from_millis(1)).await;
    rig.job_starts("csp-dataload-4");
    let answer = CollectSink::new();
    rig.scanner.answer_delta(None, &answer).await.unwrap();
    assert_eq!(answer.take()[0].during_job, Some(Rig::job("csp-dataload-4")));
    rig.job_ends("csp-dataload-4");
    rig.scanner.answer_delta(None, &answer).await.unwrap();
    assert_eq!(answer.take()[0].during_job, None);
}
