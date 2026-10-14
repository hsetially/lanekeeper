//! P15, the registry's half: after a hub outage the agent replays its spool, and the registry must apply it at 1,000 versions per
//! second or more, audit events included, with none lost (budget `P15.registry_ingest_versions_per_s`, bench
//! `hub-registry/ingest_versions_per_s`, statistic `min`). The agent's half is `agent/spool_replay_versions_per_s`, the platform's
//! acknowledgement is `hub-platform/ack_throughput`; this is what the sink behind that acknowledgement does.
//!
//! The workload is the agent's one-hour outage: 360 messages of 50 versions each. A swimlane of the target-scale fixtures has been
//! inventoried first (about 2,000 files, through the real ingestion path), and each version is a new, distinct version of one of
//! its YAML files, so every one is an out-of-band change that writes an observation, a `file_state` row, one audit event and one
//! outbox event, over a real Postgres, in chunks of 50 paths. Three runs, each on a fresh database; the budget is the slowest.
//! Writes `target/perf-results/hub-registry_ingest_versions_per_s.json`.
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
use std::time::Instant;

use bytes::Bytes;
use domain::{NfsPath, ScanEntry, SwimlaneId, Timestamp};
use hub_registry::{AgentIngest, IngestDeps, ManualClock, RegistryConfig, RegistryMetrics};
use ports::{AgentIdentity, ReportSink, content_hash};
use support::fixtures;
use support::perf::write_result;
use support::pg::TestDb;
use support::pg_audit::PgTestAudit;
use support::pg_blobs::PgBlobs;
use support::pg_bus::PgTestBus;
use support::rig::{hello, root};

const MESSAGES: usize = 360;
const VERSIONS_PER_MESSAGE: usize = 50;
const RUNS: usize = 3;
const T: i64 = ManualClock::T0_MS - 4 * 3600 * 1000;

fn is_yaml(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

/// The YAML files of the swimlane, the ones a person edits.
fn yaml_files() -> Vec<(NfsPath, Bytes)> {
    let fx = fixtures::target();
    let lane = fx.swimlanes().into_iter().next().unwrap();
    let all = fx.files(&lane);
    eprintln!(
        "hub-registry/ingest_versions_per_s: {} files in {lane}",
        all.len()
    );
    all
}

fn entry(path: &NfsPath, content: Bytes, at: i64) -> ScanEntry {
    ScanEntry {
        path: path.clone(),
        hash: content_hash(&content),
        size: content.len() as u64,
        mtime: Timestamp::from_unix_millis(at - 1_000),
        observed_at: Timestamp::from_unix_millis(at),
        denied: false,
        bytes: Some(content),
    }
}

/// A new version of a file: the same text with a comment line that says which version it is.
fn version(original: &Bytes, n: u64) -> Bytes {
    let mut v = original.to_vec();
    if !v.ends_with(b"\n") {
        v.push(b'\n');
    }
    v.extend_from_slice(format!("# version {n}\n").as_bytes());
    Bytes::from(v)
}

async fn run(files: &[(NfsPath, Bytes)]) -> Option<f64> {
    let db = TestDb::ready().await?;
    let pool = db.app_pool_of(16).await;
    db.seed_swimlane(&pool, "sl-01").await;
    let ingest = AgentIngest::new(IngestDeps {
        pool: pool.clone(),
        blobs: Arc::new(PgBlobs::new(pool.clone())),
        bus: Arc::new(PgTestBus::new(pool.clone())),
        audit: Arc::new(PgTestAudit::default()),
        clock: Arc::new(ManualClock::at_t0()),
        config: RegistryConfig::default(),
        metrics: RegistryMetrics::default(),
    })
    .unwrap();
    let id = AgentIdentity::new(SwimlaneId::parse("sl-01").unwrap());
    ingest.hello(&id, hello("sl-01")).await.unwrap();

    // The first inventory, in messages of at most 1,000 files and 2.5 MiB.
    let mut seq = 0_u64;
    let mut batch = Vec::new();
    let mut bytes = 0_usize;
    let mut chunks: Vec<Vec<ScanEntry>> = Vec::new();
    for (path, content) in files {
        if !batch.is_empty() && (batch.len() >= 1_000 || bytes + content.len() > 2_500_000) {
            chunks.push(std::mem::take(&mut batch));
            bytes = 0;
        }
        bytes += content.len();
        batch.push(entry(path, content.clone(), T));
    }
    if !batch.is_empty() {
        chunks.push(batch);
    }
    let last = chunks.len();
    for (i, entries) in chunks.into_iter().enumerate() {
        seq += 1;
        let mut d = support::rig::delta(seq, None, root(0), entries);
        d.more = i + 1 < last;
        d.part = u32::try_from(i).unwrap();
        ingest.delta(&id, d).await.unwrap();
    }

    // The spool of an hour: 360 messages of 50 new versions, each of a different file.
    let editable: Vec<&(NfsPath, Bytes)> = files
        .iter()
        .filter(|(p, b)| is_yaml(p.file_name()) && b.len() < 200_000)
        .collect();
    assert!(
        editable.len() >= VERSIONS_PER_MESSAGE,
        "too few YAML files to edit"
    );
    let mut spool = Vec::new();
    let mut n = 0_u64;
    for m in 0..MESSAGES {
        let entries: Vec<ScanEntry> = (0..VERSIONS_PER_MESSAGE)
            .map(|j| {
                let (path, content) = editable[(m * VERSIONS_PER_MESSAGE + j) % editable.len()];
                n += 1;
                entry(
                    path,
                    version(content, n),
                    T + 10_000 * i64::try_from(m + 1).unwrap() + i64::try_from(j).unwrap(),
                )
            })
            .collect();
        spool.push(support::rig::delta(
            seq + 1 + u64::try_from(m).unwrap(),
            Some(root(u8::try_from(m % 200).unwrap())),
            root(u8::try_from((m + 1) % 200).unwrap()),
            entries,
        ));
    }
    // The first message of the spool starts where the inventory ended, so the chain is continuous.
    if let Some(first) = spool.first_mut() {
        first.base_root = Some(root(0));
    }

    let versions = MESSAGES * VERSIONS_PER_MESSAGE;
    let started = Instant::now();
    for d in spool {
        ingest.delta(&id, d).await.unwrap();
    }
    let secs = started.elapsed().as_secs_f64();

    // None lost: every version is in the history, and each has its audit event's path.
    let observed: i64 = sqlx::query_scalar("SELECT count(*) FROM file_observations WHERE NOT initial")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(usize::try_from(observed).unwrap(), versions, "a version was lost");
    let audited: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        usize::try_from(audited).unwrap(),
        versions,
        "an audit event is missing"
    );
    Some(versions as f64 / secs)
}

#[tokio::main]
async fn main() {
    let files = yaml_files();
    let mut samples = Vec::new();
    for r in 0..RUNS {
        let Some(rate) = run(&files).await else {
            eprintln!("hub-registry/ingest_versions_per_s: SKIPPED (no database)");
            return;
        };
        eprintln!(
            "hub-registry/ingest_versions_per_s: run {}: {rate:.0} versions/s ({} versions, audit and outbox included)",
            r + 1,
            MESSAGES * VERSIONS_PER_MESSAGE
        );
        samples.push(rate);
    }
    let path = write_result("hub-registry/ingest_versions_per_s", "versions_per_s", &samples);
    eprintln!(
        "hub-registry/ingest_versions_per_s: min {:.0} versions/s -> {}",
        samples.iter().copied().fold(f64::INFINITY, f64::min),
        path.display()
    );
}
