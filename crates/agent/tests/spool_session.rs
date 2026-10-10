//! The spool in the running agent (T9, D74, P15): the scanner, the spool, the pump and a real TLS 1.3 gRPC stream to the
//! fake hub, in virtual time. What the unit-level tests in `spool_core.rs` prove about the spool alone is proved here
//! about the whole path a version takes: file system, walk, delta, spool, connection, hub, acknowledgement.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use domain::ScanDelta;
use proto::convert::FromAgent;
use support::fake_hub::ConnHandle;
use support::log_capture::LogCapture;
use support::oob::Harness;
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

fn files(n: usize) -> Arc<ScriptedSource> {
    let source = Arc::new(ScriptedSource::new());
    for i in 0..n {
        source.write(
            &format!("svc-{}/file-{i}.yml", i % 3),
            format!("content {i}").as_bytes(),
        );
    }
    source
}

fn deltas(conn: &ConnHandle) -> Vec<ScanDelta> {
    conn.received()
        .into_iter()
        .filter_map(|m| match m {
            FromAgent::Delta(d) => Some(d),
            _ => None,
        })
        .collect()
}

async fn wait_for_deltas(conn: &ConnHandle, n: usize) -> Vec<ScanDelta> {
    for _ in 0..3_000 {
        let got = deltas(conn);
        if got.len() >= n {
            return got;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("the hub never got {n} delta messages");
}

fn about(deltas: &[ScanDelta], path: &str) -> Vec<(Vec<u8>, i64)> {
    deltas
        .iter()
        .flat_map(|d| d.entries.iter())
        .filter(|e| e.path.as_str() == path)
        .map(|e| (e.bytes.as_ref().unwrap().to_vec(), e.observed_at.unix_millis()))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn hub_down_for_an_hour_while_a_file_goes_a_b_c_and_all_three_arrive_in_order() {
    let source = files(6);
    let harness = Harness::start(source.clone()).await;

    // The hub is gone for an hour and the file changes every twenty minutes.
    harness.rig.server.net.set_reachable(false);
    harness.rig.server.net.kill_connections();
    sleep(Duration::from_secs(5)).await;
    source.write("svc-0/app.yml", b"A");
    sleep(Duration::from_secs(20 * 60)).await;
    source.write("svc-0/app.yml", b"B");
    sleep(Duration::from_secs(20 * 60)).await;
    source.write("svc-0/app.yml", b"C");
    sleep(Duration::from_secs(20 * 60)).await;
    assert_eq!(
        harness.spool.stats().entries,
        3,
        "one version each, kept for the hub"
    );

    harness.rig.server.net.set_reachable(true);
    let conn = harness.rig.server.wait_for_connection(2).await;
    let got = wait_for_deltas(&conn, 3).await;

    let versions = about(&got, "svc-0/app.yml");
    let content: Vec<&[u8]> = versions.iter().map(|(c, _)| c.as_slice()).collect();
    assert_eq!(
        content,
        [b"A".as_slice(), b"B", b"C"],
        "A, B and C, not only the last"
    );
    let seen: Vec<i64> = versions.iter().map(|(_, t)| *t).collect();
    assert!(
        seen[0] < seen[1] && seen[1] < seen[2],
        "in the order they were observed: {seen:?}"
    );
    assert!(
        seen[1] - seen[0] >= 19 * 60 * 1000,
        "observed when they happened, not when they were sent: {seen:?}"
    );
    assert!(got.windows(2).all(|w| w[0].seq < w[1].seq));
    // Each delta continues from where the one before ended, so the hub can follow the chain.
    assert!(got.windows(2).all(|w| w[1].base_root == Some(w[0].new_root)));
    assert_eq!(got.last().unwrap().new_root, source.tree().root_hash());

    // The hub acknowledged them, so nothing is held any more.
    sleep(Duration::from_secs(5)).await;
    harness.spool.maintain().await;
    assert_eq!(harness.spool.stats().entries, 0);
}

#[tokio::test(start_paused = true)]
async fn what_the_hub_did_not_acknowledge_is_sent_again_on_the_next_connection() {
    let source = files(4);
    let harness = Harness::start(source.clone()).await;
    harness.rig.server.set_auto_ack(false);
    source.write("svc-0/a.yml", b"one");
    let first = wait_for_deltas(&harness.conn, 1).await;

    // The connection breaks before the hub acknowledged anything.
    harness.rig.server.net.kill_connections();
    let conn = harness.rig.server.wait_for_connection(2).await;
    let again = wait_for_deltas(&conn, 1).await;
    assert_eq!(again[0].seq, first[0].seq, "the same message, not a new one");
    assert_eq!(again[0].entries[0].path, first[0].entries[0].path);
    assert_eq!(harness.spool.stats().entries, 1, "still held");

    // Now the hub acknowledges it.
    conn.send(proto::convert::ToAgent::Ack(again[0].seq)).await;
    sleep(Duration::from_secs(5)).await;
    harness.spool.maintain().await;
    assert_eq!(harness.spool.stats().entries, 0);
}

#[tokio::test(start_paused = true)]
async fn the_agent_does_not_send_more_than_a_window_to_a_hub_that_never_acknowledges() {
    let source = files(3);
    let harness = Harness::start(source.clone()).await;
    harness.rig.server.set_auto_ack(false);
    for i in 0..100 {
        source.write(&format!("burst/f{i}.yml"), b"x");
        sleep(Duration::from_secs(11)).await;
    }
    let sent = deltas(&harness.conn).len();
    assert!(
        sent > 0 && sent <= 64,
        "{sent} deltas sent with nothing acknowledged"
    );
    assert_eq!(harness.spool.stats().entries, 100, "all of it is held");
    // Acknowledgements open the window again.
    let first = deltas(&harness.conn);
    for d in &first {
        harness.conn.send(proto::convert::ToAgent::Ack(d.seq)).await;
    }
    sleep(Duration::from_secs(5)).await;
    assert!(deltas(&harness.conn).len() > sent);
}

#[tokio::test(start_paused = true)]
async fn the_spool_logs_no_file_content() {
    const MARKER: &str = "SPOOLED-CONTENT-MARKER-77c1";
    let capture = LogCapture::new();
    let _guard = capture.install();
    let source = files(3);
    let harness = Harness::start(source.clone()).await;
    harness.rig.server.net.set_reachable(false);
    harness.rig.server.net.kill_connections();
    sleep(Duration::from_secs(5)).await;
    for i in 0..3 {
        source.write(
            &format!("secret/{i}.yml"),
            format!("password: {MARKER} {i}").as_bytes(),
        );
        sleep(Duration::from_secs(20)).await;
    }
    // Damage the spool under the agent and make it read the damage back.
    let segment = std::fs::read_dir(harness.spool_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "lks"))
        .expect("a segment");
    let mut bytes = std::fs::read(&segment).unwrap();
    let last = bytes.len() - 5;
    bytes[last] ^= 0xFF;
    std::fs::write(&segment, &bytes).unwrap();
    harness.rig.server.net.set_reachable(true);
    let _ = harness.rig.server.wait_for_connection(2).await;
    sleep(Duration::from_secs(30)).await;

    let logs = capture.text();
    assert!(!logs.contains(MARKER), "file content reached the logs");
    assert!(logs.contains("spool"), "the spool did say something: {logs}");
    assert!(
        harness.spool.stats().damaged_total >= 1,
        "the damage was noticed when the record was read back"
    );
}
