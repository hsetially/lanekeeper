//! P14: a sentinel-based attribution is attached within 30 seconds of the change being observed (`P14`, `hub-registry/sentinel_attribution`, a
//! harness, p95, 30,000 ms).
//!
//! The change is observed when the hub applies the agent's scan delta; the attribution is attached when `file_observations` says
//! `sentinel_login` for it. What the hub controls in between is measured, on a database at the target scale of one swimlane (about
//! 2,000 files with their inventory, 40,000 older observations of them, 300,000 records of the sentinel over the weeks before), with the
//! leased job `registry-attribution-grace` running at its real interval (5 s), as the hub's runner calls it:
//!
//! - **the record was first** (the usual order: the sentinel flushes every second and the agent walks every ten): the sample is the time
//!   from the scan delta being applied until the job's next run has attributed it, so it holds up to one interval of waiting;
//! - **the scan was first**: the sample is the time from the scan delta being applied until the batch that carries the record has been
//!   applied by the sink, in whose transaction the observation is attributed, the record being handed over at once (the time the record
//!   takes to exist is the sentinel's, not the hub's).
//!
//! Thirty samples, half of each, each a new version of a different file, sent after the job started, and a pause between samples of 0.3 to
//! 5.3 s from a fixed pseudo-random sequence, so that the samples fall on every phase of the job's clock (a fixed pause locks on to it: a
//! sample that waits for the job's next run ends on a tick, and the next one starts a fixed time after that). Every sample ends with the observation attributed
//! to the application user the record's login maps to; one that is not is a failure of the benchmark and not a number.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::too_many_lines
)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hub_platform::jobs::JobContext;
use hub_registry::{AttributionGrace, ManualClock, RegistryConfig};
use ports::ReportSink;

use common::Bed;
use common::support::perf::write_result;
use common::support::sentinel::{batch, record};

const SAMPLES: usize = 30;

/// The older observations of every file of the swimlane (as history that was recorded over the weeks), and the records of the sentinel.
async fn preload(bed: &Bed) {
    let lane = bed.lanes[0].id.as_str();
    let t0 = ManualClock::T0_MS;
    // 20 older versions of each file: 40,000 rows, attributed already (a sync job's), in the weeks before the clock of the benchmark.
    sqlx::query(
        "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, recorded_at, attribution_source, attribution_confidence) \
         SELECT fs.swimlane_id, fs.path, sha256((fs.path || k::text)::bytea), 'scan', \
                (timestamptz 'epoch' + (($2::bigint - k * 86400000)::text || ' milliseconds')::interval), \
                (timestamptz 'epoch' + (($2::bigint - k * 86400000)::text || ' milliseconds')::interval), 'sync_job', 'high' \
         FROM file_state fs, generate_series(2, 21) k WHERE fs.swimlane_id = $1",
    )
    .bind(lane)
    .bind(t0)
    .execute(&bed.rig.pool)
    .await
    .unwrap();
    // 300,000 records: 150 for each of the lane's files, between two and 62 days before the clock, in batches of 100.
    sqlx::query(
        "INSERT INTO sentinel_records (sentinel_id, batch_seq, record_index, observed_at, path, operation, success, login_user, effective_user, exe, comm) \
         SELECT s.id, 1 + (n.n / 100), (n.n % 100)::int, \
                (timestamptz 'epoch' + (($2::bigint - (172800000 + (n.n % 5184000) * 1000))::text || ' milliseconds')::interval), \
                n.path, 'write', true, 'someone_example_com', 'root', '/usr/bin/vim', 'vim' \
         FROM sentinels s, \
              (SELECT row_number() OVER () AS n, p.path FROM (SELECT path FROM file_state WHERE swimlane_id = $1) p, generate_series(1, 150)) n \
         WHERE s.swimlane_id = $1",
    )
    .bind(lane)
    .bind(t0)
    .execute(&bed.rig.pool)
    .await
    .unwrap();
    // The sentinel has acknowledged what it has sent.
    sqlx::query(
        "UPDATE sentinels SET last_acked_seq = (SELECT max(batch_seq) FROM sentinel_records WHERE sentinel_id = sentinels.id) WHERE swimlane_id = $1",
    )
    .bind(lane)
    .execute(&bed.rig.pool)
    .await
    .unwrap();
    sqlx::raw_sql("ANALYZE file_observations; ANALYZE sentinel_records")
        .execute(&bed.rig.pool)
        .await
        .ok();
}

#[tokio::main]
async fn main() {
    let config = RegistryConfig::default();
    let Some(mut bed) = Bed::new(1, config.clone()).await else {
        eprintln!("hub-registry/sentinel_attribution: SKIPPED (no database)");
        return;
    };
    bed.rig.sentinel_for_the_swimlane().await;
    let ada = bed.rig.add_user(1, "someone@example.com", "active").await;
    preload(&bed).await;
    bed.rig.sentinel_hello().await.unwrap();
    let acked: i64 = bed.rig.last_acked().await;
    eprintln!("hub-registry/sentinel_attribution: bed ready, the sentinel has acknowledged {acked} batches");

    // The job at its real interval, through the context a runner gives it.
    let spec = AttributionGrace::spec(&config).unwrap();
    let job = bed.rig.sentinel.grace().job();
    let (ctx, _expiry) = JobContext::held_until_told_otherwise(spec.name());
    let ctx = Arc::new(ctx);
    let interval = config.attribution_interval;
    let runner = tokio::spawn({
        let ctx = Arc::clone(&ctx);
        async move {
            loop {
                if job.run(&ctx).await.is_err() {
                    break;
                }
                tokio::time::sleep(interval).await;
            }
        }
    });

    let editable = bed.editable(0);
    assert!(editable.len() >= SAMPLES);
    let mut seq = u64::try_from(acked).unwrap() + 1;
    let mut pause_state = 0x9E37_79B9_7F4A_7C15_u64;
    let (mut first_record, mut first_scan, mut all) = (Vec::new(), Vec::new(), Vec::new());
    for (sample, &file) in editable.iter().enumerate().take(SAMPLES) {
        let record_first = sample % 2 == 0;
        let (identity, delta, paths) = bed.edit_delta(0, &[file]);
        let path = paths[0].as_str().to_owned();
        let seen_ms = delta.entries[0].observed_at.unix_millis();
        let record = record(&path, seen_ms - 3_000);
        let mine = |login: &str| {
            let mut r = record.clone();
            r.login_user = common::support::sentinel::text(login);
            r
        };
        if record_first {
            bed.rig
                .records(batch(seq, vec![mine("someone_example_com")]))
                .await
                .unwrap();
            seq += 1;
        }
        bed.rig.ingest.delta(&identity, delta).await.unwrap();
        let started = Instant::now();
        if !record_first {
            bed.rig
                .records(batch(seq, vec![mine("someone_example_com")]))
                .await
                .unwrap();
            seq += 1;
        }
        loop {
            let (source, actor): (String, Option<serde_json::Value>) = sqlx::query_as(
                "SELECT attribution_source, attribution_actor FROM file_observations \
                 WHERE swimlane_id = $1 AND path = $2 ORDER BY id DESC LIMIT 1",
            )
            .bind(bed.lanes[0].id.as_str())
            .bind(&path)
            .fetch_one(&bed.rig.pool)
            .await
            .unwrap();
            if source == "sentinel_login" {
                assert_eq!(
                    actor.as_ref().and_then(|a| a["user"].as_str()),
                    Some(ada.to_string().as_str()),
                    "sample {sample}: the attribution names the user"
                );
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "sample {sample} was not attributed within a minute (it is {source})"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "hub-registry/sentinel_attribution: sample {}: {} first, {ms:.0} ms",
            sample + 1,
            if record_first { "record" } else { "scan" }
        );
        all.push(ms);
        if record_first {
            &mut first_record
        } else {
            &mut first_scan
        }
        .push(ms);
        // A pause of 0.3 to 5.3 s from a fixed pseudo-random sequence (a linear congruential generator), so that the samples fall on every
        // phase of the job's clock: a fixed pause locks on to it, because a sample that waits for the job ends on a tick.
        pause_state = pause_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        tokio::time::sleep(Duration::from_millis(300 + (pause_state >> 33) % 5_000)).await;
    }
    runner.abort();
    let stats = |name: &str, v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        eprintln!(
            "hub-registry/sentinel_attribution: {name}: p50 {:.0} ms, p95 {:.0} ms, max {:.0} ms ({} samples)",
            common::percentile(v, 0.5),
            common::percentile(v, 0.95),
            common::percentile(v, 1.0),
            v.len()
        );
    };
    stats("record first", &mut first_record);
    stats("scan first", &mut first_scan);
    let path = write_result("hub-registry/sentinel_attribution", "ms", &all);
    eprintln!("hub-registry/sentinel_attribution: {}", path.display());
}
