//! The scanner on a real connection (T4, S6, P1, P3): the heartbeat, deltas pushed and requested over the stream, what
//! happens across a reconnect, the traffic when nothing changes, and the time from a change to the hub having it.
//!
//! Real TLS 1.3 and gRPC over in-memory pipes, in virtual time.
// The numbers go to stderr on purpose: they are the evidence for P1 and P3.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stderr)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use domain::{ContentHash, HubCommand};
use proto::convert::{FromAgent, ToAgent};
use support::log_capture::LogCapture;
use support::oob::{Harness, idle_bytes_per_minute, measure_oob_latencies, phase_offsets};
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

fn files(n: usize) -> Arc<ScriptedSource> {
    let source = Arc::new(ScriptedSource::new());
    for i in 0..n {
        source.write(
            &format!("svc-{}/file-{i}.yml", i % 7),
            format!("content {i}").as_bytes(),
        );
    }
    source
}

fn heartbeats(conn: &support::fake_hub::ConnHandle) -> Vec<domain::Heartbeat> {
    conn.received()
        .into_iter()
        .filter_map(|m| match m {
            FromAgent::Heartbeat(h) => Some(h),
            _ => None,
        })
        .collect()
}

fn deltas(conn: &support::fake_hub::ConnHandle) -> Vec<domain::ScanDelta> {
    conn.received()
        .into_iter()
        .filter_map(|m| match m {
            FromAgent::Delta(d) => Some(d),
            _ => None,
        })
        .collect()
}

/// Wait (in virtual time) until the hub has `n` delta messages. Not a `wait_for` predicate: those run under the fake
/// hub's lock, and `deltas` takes it.
async fn wait_for_deltas(conn: &support::fake_hub::ConnHandle, n: usize) {
    for _ in 0..600 {
        if deltas(conn).len() >= n {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("the hub never got {n} delta messages");
}

#[tokio::test(start_paused = true)]
async fn the_first_heartbeat_carries_the_root_and_the_file_count() {
    let source = files(30);
    let harness = Harness::start(source.clone()).await;
    let first = &heartbeats(&harness.conn)[0];
    assert_eq!(first.merkle_root, source.tree().root_hash());
    assert_eq!(first.file_count, 30);
    assert!(first.scan_seq >= 1);
    assert!(deltas(&harness.conn).is_empty(), "the baseline is not pushed");
}

#[tokio::test(start_paused = true)]
async fn one_heartbeat_every_ten_seconds_and_nothing_else_while_idle() {
    let harness = Harness::start(files(10)).await;
    let before = harness.conn.received_count();
    sleep(Duration::from_secs(300)).await;
    let received = harness.conn.received();
    let new = &received[before..];
    assert!(
        new.iter().all(|m| matches!(m, FromAgent::Heartbeat(_))),
        "{new:?}"
    );
    assert!(
        (29..=31).contains(&new.len()),
        "{} heartbeats in 5 minutes",
        new.len()
    );
    // The walk number rises with every walk, so a hub can tell a dead scanner from a quiet tree.
    let seqs: Vec<u64> = heartbeats(&harness.conn).iter().map(|h| h.scan_seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] <= w[1]), "{seqs:?}");
    assert!(seqs.last() > seqs.first());
}

#[tokio::test(start_paused = true)]
async fn p3_idle_traffic_under_1kb_per_min() {
    let harness = Harness::start(files(2000)).await;
    // Let the handshake, the Hello and the first heartbeat settle; they are not idle traffic.
    sleep(Duration::from_secs(30)).await;
    let windows = idle_bytes_per_minute(&harness, 10).await;
    eprintln!("idle bytes per minute, both directions, TLS 1.3 ciphertext: {windows:?}");
    assert_eq!(windows.len(), 10);
    for (minute, bytes) in windows.iter().enumerate() {
        assert!(*bytes < 1024, "minute {minute}: {bytes} bytes (budget 1,024)");
    }
    // The measurement is not blind: heartbeats and keepalive really are on the wire.
    assert!(windows.iter().all(|b| *b > 100), "{windows:?}");
}

#[tokio::test(start_paused = true)]
async fn a_change_is_pushed_to_the_hub_over_the_stream() {
    let source = files(5);
    let harness = Harness::start(source.clone()).await;
    let before = harness.scanner.snapshot().unwrap().root;
    source.write("svc-0/new.yml", b"fresh");
    let delta = harness
        .conn
        .wait_for(|m| matches!(m, FromAgent::Delta(d) if d.entries.iter().any(|e| e.path.as_str() == "svc-0/new.yml")))
        .await;
    let FromAgent::Delta(d) = delta else {
        unreachable!()
    };
    assert_eq!(d.base_root, Some(before));
    assert_eq!(d.new_root, source.tree().root_hash());
    assert_eq!(&d.entries[0].bytes.as_ref().unwrap()[..], b"fresh");
    // And the heartbeat after it carries the new root.
    sleep(Duration::from_secs(20)).await;
    assert_eq!(heartbeats(&harness.conn).last().unwrap().merkle_root, d.new_root);
}

#[tokio::test(start_paused = true)]
async fn the_hub_can_ask_for_a_delta_from_a_root_or_for_everything() {
    let source = files(8);
    let harness = Harness::start(source.clone()).await;
    let r0 = harness.scanner.snapshot().unwrap().root;
    source.write("added.yml", b"a");
    harness
        .conn
        .wait_for(
            |m| matches!(m, FromAgent::Delta(d) if d.entries.iter().any(|e| e.path.as_str() == "added.yml")),
        )
        .await;
    let pushed = deltas(&harness.conn).len();

    harness
        .conn
        .send(ToAgent::Command(HubCommand::RequestDelta { since_root: r0 }))
        .await;
    wait_for_deltas(&harness.conn, pushed + 1).await;
    let answer = deltas(&harness.conn).pop().unwrap();
    assert_eq!(answer.base_root, Some(r0));
    assert_eq!(answer.entries.len(), 1);

    harness
        .conn
        .send(ToAgent::Command(HubCommand::RequestDelta {
            since_root: ContentHash::from_bytes([9; 32]),
        }))
        .await;
    wait_for_deltas(&harness.conn, pushed + 2).await;
    let full = deltas(&harness.conn).pop().unwrap();
    assert_eq!(full.base_root, None);
    assert_eq!(full.entries.len(), 9);

    harness
        .conn
        .send(ToAgent::Command(HubCommand::RequestFullScan))
        .await;
    wait_for_deltas(&harness.conn, pushed + 3).await;
    assert_eq!(deltas(&harness.conn).pop().unwrap().entries.len(), 9);
}

#[tokio::test(start_paused = true)]
async fn what_changed_while_disconnected_is_replayed_and_still_found_through_the_heartbeat_root() {
    let source = files(6);
    let harness = Harness::start(source.clone()).await;
    let known_to_hub = harness.scanner.snapshot().unwrap().root;

    // The hub goes away. Files change meanwhile; the agent keeps walking, and every walk that finds a change spools the
    // delta it builds (T9): nobody is listening, and that no longer means nothing is read.
    harness.rig.server.net.set_reachable(false);
    harness.rig.server.net.kill_connections();
    sleep(Duration::from_secs(5)).await;
    source.write("during-outage-1.yml", b"1");
    sleep(Duration::from_secs(12)).await;
    source.write("during-outage-2.yml", b"2");
    sleep(Duration::from_secs(25)).await;
    assert!(
        harness.scanner.stats().deltas_pushed() >= 2,
        "each change was read and spooled although nobody was listening"
    );
    assert_eq!(harness.spool.stats().entries, 2);
    harness.rig.server.net.set_reachable(true);

    // The hub is back. The first heartbeat on the new connection shows a root the hub does not know.
    let conn = harness.rig.server.wait_for_connection(2).await;
    let heartbeat = conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
    let FromAgent::Heartbeat(h) = heartbeat else {
        unreachable!()
    };
    assert_ne!(h.merkle_root, known_to_hub);
    assert_eq!(h.merkle_root, source.tree().root_hash());

    // And the spool replays what it kept, oldest first, each delta with its own file: the hub does not have to guess.
    wait_for_deltas(&conn, 2).await;
    let replayed = deltas(&conn);
    let paths: Vec<_> = replayed
        .iter()
        .map(|d| {
            d.entries
                .iter()
                .map(|e| e.path.as_str().to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(paths, [["during-outage-1.yml"], ["during-outage-2.yml"]]);
    assert_eq!(replayed[0].base_root, Some(known_to_hub));
    assert_eq!(replayed[1].new_root, source.tree().root_hash());
    assert!(replayed[0].seq < replayed[1].seq);

    // The hub can still ask by root, and is answered from the tree, with the same files.
    conn.send(ToAgent::Command(HubCommand::RequestDelta {
        since_root: known_to_hub,
    }))
    .await;
    wait_for_deltas(&conn, 3).await;
    let d = deltas(&conn).pop().unwrap();
    assert_eq!(d.base_root, Some(known_to_hub));
    let mut paths: Vec<_> = d.entries.iter().map(|e| e.path.as_str().to_owned()).collect();
    paths.sort();
    assert_eq!(paths, ["during-outage-1.yml", "during-outage-2.yml"]);
}

#[tokio::test(start_paused = true)]
async fn p1_change_visible_scaled() {
    let source = files(2000);
    let harness = Harness::start(source.clone()).await;
    let offsets = phase_offsets(Duration::from_secs(10), 12);
    let latencies = measure_oob_latencies(&harness, Duration::from_secs(10), &offsets, |i| {
        let path = format!("oob/change-{i}.yml");
        source.write(&path, format!("out of band {i}").as_bytes());
        path
    })
    .await;
    let mut ms: Vec<f64> = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    eprintln!("change to hub, virtual time, ms: {ms:?}");
    let p95 = support::perf::percentile(&ms, 0.95);
    assert!(p95 <= 15_000.0, "p95 {p95} ms against 15,000");
    // The spread is the walk interval: a change just after a walk waits for the next one.
    assert!(
        *ms.last().unwrap() > 8_000.0 && *ms.first().unwrap() < 2_000.0,
        "{ms:?}"
    );
}

/// P1 again, now that changes are held until the tree is quiet (T10): files carry the time they were written, as on a real
/// file system, so a change found 1 s after it was made really waits for the 3 s. No Job is running.
#[tokio::test(start_paused = true)]
async fn p1_holds_without_a_job() {
    let rig = support::rig::Rig::new();
    let source = Arc::new(ScriptedSource::with_clock(rig.clock.clone()));
    for i in 0..2000 {
        source.write(
            &format!("svc-{}/file-{i}.yml", i % 7),
            format!("content {i}").as_bytes(),
        );
    }
    let harness = Harness::start_with(source.clone(), rig).await;
    let offsets = phase_offsets(Duration::from_secs(10), 12);
    let latencies = measure_oob_latencies(&harness, Duration::from_secs(10), &offsets, |i| {
        let path = format!("oob/change-{i}.yml");
        source.write(&path, format!("out of band {i}").as_bytes());
        path
    })
    .await;
    let mut ms: Vec<f64> = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    eprintln!("change to hub with the quiet period, virtual time, ms: {ms:?}");
    let p95 = support::perf::percentile(&ms, 0.95);
    assert!(p95 <= 15_000.0, "p95 {p95} ms against 15,000");
    // Nothing is faster than the quiet period: a change is at least 3 s old when it is reported (a change found at a walk
    // is held until then), and nothing is slower than a walk plus the quiet period.
    assert!(
        ms.iter().all(|m| *m >= 2_900.0),
        "nothing is reported before it has been quiet for 3 s: {ms:?}"
    );
    assert!(*ms.last().unwrap() <= 13_500.0, "{ms:?}");
}

#[tokio::test(start_paused = true)]
async fn log_capture_contains_only_paths_and_hashes() {
    const MARKER: &str = "FILE-CONTENT-MARKER-0e5f";
    let capture = LogCapture::new();
    let _guard = capture.install();

    let source = files(4);
    source.write("secret/payload.yml", format!("password: {MARKER}").as_bytes());
    let harness = Harness::start(source.clone()).await;
    source.write(
        "secret/payload.yml",
        format!("password: changed {MARKER}").as_bytes(),
    );
    source.write("secret/new.yml", format!("token: {MARKER}").as_bytes());
    sleep(Duration::from_secs(15)).await;
    harness
        .conn
        .send(ToAgent::Command(HubCommand::RequestDelta {
            since_root: ContentHash::from_bytes([1; 32]),
        }))
        .await;
    sleep(Duration::from_secs(5)).await;
    // An error path as well: the file system fails once.
    source.fail_next_scans(agent::tree::ScanError::Root(
        std::io::ErrorKind::StaleNetworkFileHandle,
    ));
    sleep(Duration::from_secs(25)).await;
    source.stop_failing_scans();
    sleep(Duration::from_secs(10)).await;

    let log = capture.text();
    assert!(!log.is_empty());
    assert!(!log.contains(MARKER), "file content reached a log line");
    assert!(!log.contains("password:"));
    assert!(log.contains("the scan failed"), "the error path ran");
}
