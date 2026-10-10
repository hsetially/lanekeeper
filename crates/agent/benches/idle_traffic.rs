//! P3: an idle agent sends under 1 KB a minute (budget `P3`, `agent/idle_traffic_bytes_per_min`, p95).
//!
//! The whole agent against the fake hub on a 2,000-file tree, over real TLS 1.3 on counting pipes, in virtual time:
//! ten one-minute windows after the connection has settled. The bytes are the ciphertext in both directions
//! (decision A14): heartbeats, HTTP/2 keepalive and the framing of both, not TCP or IP headers. Writes
//! `target/perf-results/agent_idle_traffic_bytes_per_min.json`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use support::oob::{Harness, idle_bytes_per_minute};
use support::scripted_source::ScriptedSource;

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let source = Arc::new(ScriptedSource::new());
        for i in 0..2000 {
            source.write(
                &format!("svc-{:03}/file-{i:04}.yml", i % 150),
                format!("content {i}").as_bytes(),
            );
        }
        let harness = Harness::start(source).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        let windows = idle_bytes_per_minute(&harness, 10).await;
        // A minute of idle traffic is a few hundred bytes: far inside f64's exact integers.
        #[allow(clippy::cast_precision_loss)]
        let samples: Vec<f64> = windows.iter().map(|b| *b as f64).collect();
        let written =
            support::perf::write_result("agent/idle_traffic_bytes_per_min", "bytes_per_min", &samples);
        eprintln!("idle bytes per minute: {windows:?}\n-> {}", written.display());
    });
}
