//! P15: after a one-hour hub outage the spool replays at 1,000 versions per second or more (budget
//! `P15.spool_replay_versions_per_s`, bench `agent/spool_replay_versions_per_s`, statistic `min`).
//!
//! The outage: one delta per 10 s walk for an hour, 360 deltas of 50 versions of 1 KiB each (18,000 versions, about 18
//! MiB), written through the spool's own append path with its real fsyncs. Then the agent restarts, which is the harsher
//! case: the timer starts at `Spool::open`, so it includes reading every segment once to check its checksums and rebuild
//! the index, and ends when the last message has been sent to a hub that acknowledges each as it arrives. The file
//! operations run on the blocking pool, as in the agent. Five runs, each on a fresh directory; the budget is the slowest.
//!
//! What this does not include: the network (the hub is a task that drains the outbox), and a cold page cache (the
//! segments were written a moment ago; the evidence says so). Writes `target/perf-results/agent_spool_replay_versions_per_s.json`.
// The counts are small (thousands), so the casts between integer sizes and to `f64` are exact.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Instant;

use agent::clock::SystemClock;
use agent::spool::{Spool, SpoolLimits, SpoolOptions, SpoolVolume};
use agent::transport::outbox::{self, OutboxLimits};
use bytes::Bytes;
use domain::{ContentHash, NfsPath, ScanDelta, ScanEntry, Timestamp};
use proto::pb::agent_message::Kind;
use sha2::{Digest, Sha256};

const DELTAS: u64 = 360;
const VERSIONS_PER_DELTA: usize = 50;
const BYTES_PER_VERSION: usize = 1024;
const RUNS: usize = 5;

fn delta(seq: u64) -> ScanDelta {
    let entries = (0..VERSIONS_PER_DELTA)
        .map(|i| {
            // Distinct content for every version, so nothing is collapsed and every byte is really written.
            let mut content = vec![b'a' + (seq % 26) as u8; BYTES_PER_VERSION];
            content[..16].copy_from_slice(format!("{seq:08}{i:08}").as_bytes());
            ScanEntry {
                path: NfsPath::parse(&format!("svc-{}/file-{i}.yml", seq % 40)).unwrap(),
                hash: ContentHash::from_bytes(Sha256::digest(&content).into()),
                size: content.len() as u64,
                mtime: Timestamp::from_unix_millis(1_800_000_000_000 + seq as i64 * 10_000),
                observed_at: Timestamp::from_unix_millis(1_800_000_000_000 + seq as i64 * 10_000),
                denied: false,
                bytes: Some(Bytes::from(content)),
            }
        })
        .collect();
    ScanDelta {
        seq,
        base_root: Some(ContentHash::from_bytes([seq as u8; 32])),
        new_root: ContentHash::from_bytes([seq as u8 + 1; 32]),
        entries,
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more: false,
        part: 0,
        gap: None,
    }
}

fn limits() -> SpoolLimits {
    SpoolLimits::new(512 * 1024 * 1024, 100_000)
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mut rates = Vec::with_capacity(RUNS);
    for run in 0..RUNS {
        let dir = tempfile::TempDir::new().unwrap();
        let clock = Arc::new(SystemClock);
        runtime.block_on(async {
            // The outage.
            let (spool, _) = Spool::open(
                SpoolVolume::open(dir.path()).unwrap(),
                SpoolOptions::new(limits()),
                clock.clone(),
            )
            .unwrap();
            for seq in 1..=DELTAS {
                spool.append(delta(seq)).await.unwrap();
            }
            assert_eq!(spool.stats().entries, DELTAS * VERSIONS_PER_DELTA as u64);
        });

        // The restart and the replay.
        let started = Instant::now();
        let versions = runtime.block_on(async {
            let (spool, recovery) = Spool::open(
                SpoolVolume::open(dir.path()).unwrap(),
                SpoolOptions::new(limits()),
                clock.clone(),
            )
            .unwrap();
            assert_eq!(
                recovery.groups, DELTAS as usize,
                "everything survived the restart"
            );
            let (outbox, mut rx) = outbox::channel(OutboxLimits::default());
            let pump = tokio::spawn(spool.attach(outbox).run());
            let mut versions = 0_usize;
            let mut messages = 0_u64;
            let mut previous = 0;
            while messages < DELTAS {
                let queued = rx.recv().await.expect("the pump keeps the connection open");
                let (message, _permit) = queued.into_parts();
                let Some(Kind::ScanDelta(d)) = message.kind else {
                    panic!("only deltas are replayed");
                };
                assert!(d.seq > previous, "in order");
                previous = d.seq;
                versions += d.entries.len();
                messages += 1;
                // The hub acknowledges what it received.
                spool.ack(d.seq);
            }
            pump.abort();
            versions
        });
        let took = started.elapsed();
        let rate = versions as f64 / took.as_secs_f64();
        eprintln!("run {run}: {versions} versions in {took:?} = {rate:.0} versions/s");
        assert_eq!(versions, DELTAS as usize * VERSIONS_PER_DELTA);
        rates.push(rate);
    }
    let written = support::perf::write_result("agent/spool_replay_versions_per_s", "versions_per_s", &rates);
    eprintln!(
        "spool replay, versions/s per run: {rates:.0?}\n-> {}",
        written.display()
    );
}
