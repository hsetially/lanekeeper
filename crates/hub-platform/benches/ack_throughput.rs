//! P15: after a hub outage the agent replays its spool, and the hub must take it at 1,000 versions per second or more (budget
//! `P15.hub_ack_versions_per_s`, bench `hub-platform/ack_throughput`, statistic `min`). The agent's own half is
//! `agent/spool_replay_versions_per_s`; this is the hub's: how fast it applies and acknowledges what the agent sends.
//!
//! A real `Agent` service behind the real listener over real mutual TLS (TLS 1.3, HTTP/2, zstd), a real client that sends deltas as
//! fast as flow control lets it and counts acknowledgements, and a sink that takes 1 ms to commit each message, the way a database
//! commit does. The hub applies one agent's deltas one at a time and in order (the sink applies a delta against the root the last
//! one left), so the rate is bounded by the commit time, and the number of versions in each message is what the agent controls.
//!
//! The workload is the agent's one-hour outage (360 messages of 50 versions of 1 KiB, as `agent/spool_replay` writes) with eight
//! maximum-size messages mixed in (three 1 MiB files, 3 MiB, the contract's bound) so that the large-message path is in the
//! number. A "version" is one file entry in a delta, the unit the agent counts. Five runs on fresh streams; the budget is the
//! slowest. It counts versions acknowledged per second from the first message sent to the last ack received. What it does not
//! include: the agent's spool reads (the other half), and a real database (the sink sleeps 1 ms; the real commit is 05's).
//! Writes `target/perf-results/hub-platform_ack_throughput.json`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use domain::{AgentConfig, ClusterReport, Heartbeat, HeartbeatAction, Hello, ScanDelta, TenantId};
use hub_platform::agent_service::StreamLimits;
use ports::{AgentIdentity, ReportSink, SinkError};
use proto::pb;
use support::perf::write_result;
use support::stream_rig::StreamRig;

const SMALL_DELTAS: u64 = 360;
const VERSIONS_PER_SMALL_DELTA: usize = 50;
const BYTES_PER_SMALL_VERSION: usize = 1024;
/// One in this many messages is a maximum-size one.
const LARGE_EVERY: u64 = 45;
const LARGE_FILES: usize = 3;
const LARGE_FILE_BYTES: usize = 1024 * 1024;
const RUNS: usize = 5;
const COMMIT: Duration = Duration::from_millis(1);

/// A sink whose commit takes 1 ms.
struct CommittingSink;

#[async_trait]
impl ReportSink for CommittingSink {
    async fn hello(&self, _: &AgentIdentity, _: Hello) -> Result<AgentConfig, SinkError> {
        Ok(AgentConfig {
            scan_interval_secs: 10,
            heartbeat_interval_secs: 5,
            max_file_bytes: 4 * 1024 * 1024,
            deny_globs: Vec::new(),
            env_allowlist: Vec::new(),
            tenants: vec![TenantId::parse("sit1").unwrap()],
        })
    }

    async fn heartbeat(&self, _: &AgentIdentity, _: Heartbeat) -> Result<HeartbeatAction, SinkError> {
        Ok(HeartbeatAction::None)
    }

    async fn delta(&self, _: &AgentIdentity, _: ScanDelta) -> Result<(), SinkError> {
        tokio::time::sleep(COMMIT).await;
        Ok(())
    }

    async fn cluster(&self, _: &AgentIdentity, _: ClusterReport) -> Result<(), SinkError> {
        Ok(())
    }
}

fn entry(path: String, content: &Bytes, tag: u64) -> pb::ScanEntry {
    let mut hash = [0_u8; 32];
    hash[..8].copy_from_slice(&tag.to_be_bytes());
    pb::ScanEntry {
        path,
        hash: Bytes::from(hash.to_vec()),
        size: content.len() as u64,
        mtime_ms: 1_800_000_000_000,
        observed_at_ms: 1_800_000_000_000,
        denied: false,
        content: Some(content.clone()),
    }
}

/// The messages of the replay, in order, and how many versions they carry.
fn workload() -> (Vec<pb::ScanDelta>, usize) {
    let small = Bytes::from(vec![b'a'; BYTES_PER_SMALL_VERSION]);
    let large = Bytes::from(vec![b'b'; LARGE_FILE_BYTES]);
    let mut messages = Vec::new();
    let mut versions = 0;
    let mut seq = 0_u64;
    for n in 0..SMALL_DELTAS {
        seq += 1;
        messages.push(delta(
            seq,
            (0..VERSIONS_PER_SMALL_DELTA).map(|i| entry(format!("svc-{}/file-{i}.yml", n % 40), &small, seq)),
        ));
        versions += VERSIONS_PER_SMALL_DELTA;
        if (n + 1) % LARGE_EVERY == 0 {
            seq += 1;
            messages.push(delta(
                seq,
                (0..LARGE_FILES).map(|i| entry(format!("report/transform-{i}.xsl"), &large, seq)),
            ));
            versions += LARGE_FILES;
        }
    }
    (messages, versions)
}

fn delta(seq: u64, entries: impl Iterator<Item = pb::ScanEntry>) -> pb::ScanDelta {
    let mut root = [0_u8; 32];
    root[..8].copy_from_slice(&seq.to_be_bytes());
    pb::ScanDelta {
        seq,
        new_root: Bytes::from(root.to_vec()),
        entries: entries.collect(),
        ..Default::default()
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let (messages, versions) = workload();
    let rig = runtime.block_on(StreamRig::agents(
        Arc::new(CommittingSink),
        StreamLimits::default(),
    ));
    let mut rates = Vec::with_capacity(RUNS);
    for run in 0..RUNS {
        let rate = runtime.block_on(async {
            let mut stream = rig.open_agent("sit1").await;
            let total = messages.len();
            let started = Instant::now();
            // The sender runs on its own task: flow control and the hub's pace decide how fast it goes.
            let tx = stream.tx.clone();
            let outgoing = messages.clone();
            let sender = tokio::spawn(async move {
                for message in outgoing {
                    let kind = pb::agent_message::Kind::ScanDelta(message);
                    if tx.send(pb::AgentMessage { kind: Some(kind) }).await.is_err() {
                        break;
                    }
                }
            });
            let mut previous = 0;
            for _ in 0..total {
                let seq = stream
                    .next_ack(Duration::from_secs(60))
                    .await
                    .expect("the hub acknowledged every message");
                assert!(seq > previous, "acks follow the order of the deltas");
                previous = seq;
            }
            let took = started.elapsed();
            sender.abort();
            versions as f64 / took.as_secs_f64()
        });
        eprintln!(
            "run {run}: {versions} versions in {} messages = {rate:.0} versions/s",
            messages.len()
        );
        rates.push(rate);
    }
    let written = write_result("hub-platform/ack_throughput", "versions_per_s", &rates);
    eprintln!(
        "ack throughput, versions/s per run: {rates:.0?}\n-> {}",
        written.display()
    );
}
