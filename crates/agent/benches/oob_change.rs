//! P1: an out-of-band change reaches the hub within 15 s (budget `P1`, `agent/oob_change_visible`, p95).
//!
//! Twelve changes, spread over the 10 s walk interval, on a copy of swimlane 1 of the target-scale fixtures, with the
//! real walker, real hashing on the worker pool and a TLS 1.3 stream to the fake hub, all in real time. The time from
//! writing the file to the hub holding its delta is the measurement. About two minutes. Writes
//! `target/perf-results/agent_oob_change_visible.json`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;
#[path = "../tests/support/mod.rs"]
mod support;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use agent::root::NfsRoot;
use agent::tree::{FsSource, Pool, WalkConfig};
use support::oob::{Harness, measure_oob_latencies, phase_offsets};

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let work = tempfile::TempDir::new().unwrap();
        common::copy_tree(&common::swimlane_one(), work.path());
        let files = common::count_files(work.path());
        let source = Arc::new(FsSource::new(
            NfsRoot::open(work.path()).unwrap(),
            Pool::new(4).unwrap(),
            WalkConfig::default(),
        ));
        let harness = Harness::start(source).await;
        let interval = Duration::from_secs(10);
        let offsets = phase_offsets(interval, 12);
        let latencies = measure_oob_latencies(&harness, interval, &offsets, |i| {
            let path = format!("oob/change-{i}.yml");
            fs::create_dir_all(work.path().join("oob")).unwrap();
            fs::write(work.path().join(&path), format!("out of band {i}\r\n")).unwrap();
            path
        })
        .await;
        let ms: Vec<f64> = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        let written = support::perf::write_result("agent/oob_change_visible", "ms", &ms);
        eprintln!(
            "oob_change_visible over {files} files, ms: {ms:.0?}\n-> {}",
            written.display()
        );
    });
}
