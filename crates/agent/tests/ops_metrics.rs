//! `/metrics` (T7, S10, S16): scan durations, files tracked, delta sizes, the connection state and operation counts, all
//! with labels from closed sets.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Counter and gauge values are small whole numbers, which f64 holds exactly.
#![allow(clippy::float_cmp)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use agent::tree::{
    FileRead, MerkleTree, ReadError, ReadRequest, RefreshRequest, Refreshed, ScanError, ScanMode,
    ScanOutcome, TreeSource,
};
use domain::{HubCommand, NfsPath, RequestId};
use proto::convert::{FromAgent, ToAgent};
use support::app_rig::{AppRig, Setup};
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

/// The value of one series in the text exposition, written exactly as it appears (`name{label="value"}`).
fn series(text: &str, name: &str) -> Option<f64> {
    text.lines()
        .find(|l| l.starts_with(name) && l[name.len()..].starts_with(' '))
        .and_then(|l| l[name.len()..].trim().parse().ok())
}

/// A source whose walks take three seconds, so that a duration has something to measure.
#[derive(Debug)]
struct SlowSource(ScriptedSource);

#[async_trait::async_trait]
impl TreeSource for SlowSource {
    fn deny(&self) -> agent::deny::DenyList {
        self.0.deny()
    }

    async fn scan(&self, previous: Option<MerkleTree>, mode: ScanMode) -> Result<ScanOutcome, ScanError> {
        sleep(Duration::from_secs(3)).await;
        self.0.scan(previous, mode).await
    }

    async fn refresh(&self, files: Vec<RefreshRequest>) -> Vec<Refreshed> {
        self.0.refresh(files).await
    }

    async fn read(&self, files: Vec<ReadRequest>) -> Vec<Result<FileRead, ReadError>> {
        self.0.read(files).await
    }
}

#[tokio::test(start_paused = true)]
async fn metrics_cover_scans_deltas_the_connection_and_operations() {
    let source = Arc::new(ScriptedSource::new());
    source.write("svc/a.yml", b"a: 1\n");
    let app = AppRig::start(Setup {
        source: Some(source.clone()),
        ..Setup::default()
    })
    .await;
    let conn = app.connected().await;
    app.wait_until("ready", AppRig::is_ready).await;

    // A change: a delta of one file with 6 bytes.
    source.write("svc/b.yml", b"b: 22\n");
    conn.wait_for(|m| matches!(m, FromAgent::Delta(_))).await;
    // An operation that ends in NOT_FOUND.
    conn.send(ToAgent::Command(HubCommand::ReadFile {
        request_id: rid("read-1"),
        path: NfsPath::parse("svc/missing.yml").unwrap(),
    }))
    .await;
    conn.wait_for(|m| matches!(m, FromAgent::Reply(_))).await;

    let text = app.metrics.render();
    let get = |name: &str| series(&text, name).unwrap_or_else(|| panic!("no series {name} in:\n{text}"));
    assert!(get("lanekeeper_agent_scans_total{kind=\"Full\",result=\"Done\"}") >= 1.0);
    assert!(get("lanekeeper_agent_scans_total{kind=\"Stat\",result=\"Done\"}") >= 1.0);
    assert_eq!(get("lanekeeper_agent_files_tracked"), 2.0);
    assert_eq!(get("lanekeeper_agent_delta_entries_count"), 1.0);
    assert_eq!(get("lanekeeper_agent_delta_bytes_sum"), 6.0);
    assert_eq!(get("lanekeeper_agent_connection_state{state=\"Connected\"}"), 1.0);
    assert_eq!(
        get("lanekeeper_agent_connection_state{state=\"Disconnected\"}"),
        0.0
    );
    assert_eq!(
        get("lanekeeper_agent_connection_attempts_total{result=\"Established\"}"),
        1.0
    );
    assert_eq!(
        get("lanekeeper_agent_operations_total{operation=\"Read\",outcome=\"NotFound\"}"),
        1.0
    );
}

#[tokio::test(start_paused = true)]
async fn the_connection_gauge_follows_the_connection() {
    let app = AppRig::start(Setup::default()).await;
    app.connected().await;
    app.wait_until("ready", AppRig::is_ready).await;
    app.rig.server.net.set_reachable(false);
    app.rig.server.net.kill_connections();
    app.wait_until("not ready", |a| !a.is_ready()).await;
    sleep(Duration::from_secs(10)).await;
    let text = app.metrics.render();
    assert_eq!(
        series(&text, "lanekeeper_agent_connection_state{state=\"Connected\"}"),
        Some(0.0)
    );
    assert!(
        series(
            &text,
            "lanekeeper_agent_connection_attempts_total{result=\"Failed\"}"
        )
        .unwrap_or(0.0)
            >= 1.0
    );
}

#[tokio::test(start_paused = true)]
async fn scan_durations_are_measured_with_the_scanners_clock() {
    let inner = ScriptedSource::new();
    inner.write("svc/a.yml", b"a: 1\n");
    let app = AppRig::start(Setup {
        source: Some(Arc::new(SlowSource(inner))),
        ..Setup::default()
    })
    .await;
    app.connected().await;
    let text = app.metrics.render();
    let sum = series(&text, "lanekeeper_agent_scan_duration_seconds_sum{kind=\"Full\"}").unwrap();
    assert!(
        (3.0..3.5).contains(&sum),
        "the first walk took {sum} s of virtual time"
    );
}

/// The label names and values the exposition may carry.
const LABEL_NAMES: &[&str] = &["kind", "result", "operation", "outcome", "state", "le"];

fn label_pairs(text: &str) -> BTreeSet<(String, String)> {
    let mut pairs = BTreeSet::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let Some(open) = line.find('{') else { continue };
        let Some(close) = line.rfind('}') else { continue };
        for pair in line[open + 1..close].split(',') {
            let (name, value) = pair.split_once('=').unwrap();
            pairs.insert((name.to_owned(), value.trim_matches('"').to_owned()));
        }
    }
    pairs
}

#[tokio::test(start_paused = true)]
async fn metrics_labels_are_low_cardinality() {
    let app = AppRig::start(Setup::default()).await;
    let conn = app.connected().await;
    let ask = |n: usize| {
        let conn = conn.clone();
        async move {
            conn.send(ToAgent::Command(HubCommand::ReadFile {
                request_id: rid(&format!("request-{n}")),
                path: NfsPath::parse(&format!("tenant-{n}/deep/path-{n}/file-{n}.yml")).unwrap(),
            }))
            .await;
        }
    };
    ask(0).await;
    sleep(Duration::from_secs(2)).await;
    let first = app.metrics.render();

    for n in 1..60 {
        ask(n).await;
    }
    sleep(Duration::from_secs(10)).await;
    let after = app.metrics.render();

    // 60 different paths and request ids later the exposition has the same series, and the same label values.
    let names = |text: &str| -> BTreeSet<String> {
        text.lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.rsplit_once(' ').map_or(l, |(series, _)| series).to_owned())
            .collect()
    };
    assert_eq!(names(&first), names(&after), "a request created a series");
    // Every one of the 60 reads was counted, whether it was carried out or (four at a time is the limit) turned away.
    let reads: f64 = ["Ok", "Conflict", "NotFound", "Denied", "Unsupported", "Io"]
        .iter()
        .map(|outcome| {
            series(
                &after,
                &format!("lanekeeper_agent_operations_total{{operation=\"Read\",outcome=\"{outcome}\"}}"),
            )
            .unwrap()
        })
        .sum();
    assert_eq!(reads, 60.0);
    for (name, value) in label_pairs(&after) {
        assert!(LABEL_NAMES.contains(&name.as_str()), "label {name}");
        assert!(
            !value.contains(['/', '.', '-']) || name == "le",
            "{name}={value} looks like a path, an id or a name"
        );
    }
    for forbidden in [
        "tenant-", "request-", "sit1", "svc/", ".yml", "gke-sit1", "bank-sit",
    ] {
        assert!(
            !after.contains(forbidden),
            "{forbidden} in the exposition:\n{after}"
        );
    }
}
