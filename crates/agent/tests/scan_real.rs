//! The scanner on a real directory, in real time (T4, P1, S10): files are written, the walker finds them through
//! cap-std, and the bytes reach a fake hub over TLS exactly as they were.
//!
//! The walk is set to its fastest (5 s) so that the test does not take a minute; the full 10 s timing is the ignored
//! test at the end, which `just agent-slow-test` runs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stderr)]

mod support;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent::root::NfsRoot;
use agent::tree::{FsSource, Pool, WalkConfig};
use proto::convert::FromAgent;
use proto::pb;
use support::oob::{Harness, measure_oob_latencies, phase_offsets};
use support::rig::Rig;
use tempfile::TempDir;

fn source_over(dir: &Path) -> Arc<FsSource> {
    Arc::new(FsSource::new(
        NfsRoot::open(dir).unwrap(),
        Pool::new(4).unwrap(),
        WalkConfig::default(),
    ))
}

fn fast_walks(rig: &Rig) {
    rig.server.set_config(pb::AgentConfig {
        scan_interval_secs: 5,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: Vec::new(),
        env_allowlist: Vec::new(),
        tenants: vec!["sit1".to_owned()],
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_written_on_disk_reaches_the_hub_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("svc")).unwrap();
    fs::write(dir.path().join("svc/existing.yml"), b"a: 1\n").unwrap();
    let rig = Rig::new();
    fast_walks(&rig);
    let harness = Harness::start_with(source_over(dir.path()), rig).await;

    let crlf: &[u8] = b"\xef\xbb\xbfserver:\r\n  port: 8080\r\n";
    let binary: Vec<u8> = (0..=255).collect();
    fs::write(dir.path().join("svc/new.yml"), crlf).unwrap();
    fs::write(dir.path().join("svc/logo.bmp"), &binary).unwrap();
    fs::write(dir.path().join(".nfs0000dead"), b"silly rename leftover").unwrap();
    std::os::unix::fs::symlink("existing.yml", dir.path().join("svc/link.yml")).unwrap();

    let delta = tokio::time::timeout(
        Duration::from_secs(30),
        harness
            .conn
            .wait_for(|m| matches!(m, FromAgent::Delta(d) if d.entries.iter().any(|e| e.path.as_str() == "svc/new.yml"))),
    )
    .await
    .expect("the delta arrives within a few walks");
    let FromAgent::Delta(d) = delta else {
        unreachable!()
    };
    let by_path: std::collections::BTreeMap<_, _> = d
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.bytes.clone().unwrap()))
        .collect();
    assert_eq!(
        &by_path["svc/new.yml"][..],
        crlf,
        "CRLF and the BOM are untouched"
    );
    assert_eq!(&by_path["svc/logo.bmp"][..], &binary[..]);
    assert!(!by_path.contains_key(".nfs0000dead"));
    assert!(
        d.skipped
            .iter()
            .any(|s| s.path.as_str() == "svc/link.yml" && s.reason.as_str() == "symlink")
    );
    // The hub's picture and the agent's agree.
    assert_eq!(d.new_root, harness.scanner.snapshot().unwrap().root);
}

/// The budget is P1: an out-of-band change visible within 15 s at the 95th percentile. This is the real thing: real
/// files in a directory of 2,000, real hashing on the worker pool, the real 10 s walk, and a TLS stream. It takes about
/// two minutes, so it only runs on request (`cargo test --release -- --ignored`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "takes two minutes; run by `just agent-slow-test`"]
async fn p1_change_visible_real_timings() {
    let dir = TempDir::new().unwrap();
    support::workload::populate(dir.path(), 2000);
    let harness = Harness::start(source_over(dir.path())).await;
    let interval = Duration::from_secs(10);
    let offsets = phase_offsets(interval, 12);
    let latencies = measure_oob_latencies(&harness, interval, &offsets, |i| {
        let path = format!("oob/change-{i}.yml");
        fs::create_dir_all(dir.path().join("oob")).unwrap();
        fs::write(dir.path().join(&path), format!("out of band {i}\r\n")).unwrap();
        path
    })
    .await;
    let mut ms: Vec<f64> = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    eprintln!("change to hub, real time, ms: {ms:?}");
    let p95 = support::perf::percentile(&ms, 0.95);
    assert!(p95 <= 15_000.0, "p95 {p95} ms against the 15,000 ms budget");
}
