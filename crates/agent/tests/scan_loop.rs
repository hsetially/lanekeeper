//! The scan loop (T4, D63, P1): when it walks, what it sends on its own and what it keeps for the hub to ask for. All of
//! it in virtual time, on an in-memory file system, so ten minutes cost nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::clock::Clock;
use agent::config::Tunables;
use agent::scan::{ScanRequestError, Scanner};
use agent::tree::{ScanError, ScanMode};
use domain::ContentHash;
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
    task: Option<JoinHandle<()>>,
}

impl Rig {
    fn new() -> Self {
        let source = Arc::new(ScriptedSource::new());
        let sink = Arc::new(CollectSink::new());
        let clock: Arc<dyn Clock> = Arc::new(TestClock::starting_at(T0_MS));
        let scanner = Scanner::new(source.clone(), clock, sink.clone());
        Self {
            source,
            sink,
            scanner,
            task: None,
        }
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
