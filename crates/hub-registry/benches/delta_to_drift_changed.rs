//! P1, the registry's share: an out-of-band change on NFS appears in the UI within 15 seconds, of which the agent spends 10 s walking
//! and 3 s waiting for quiet, so the hub has 2 s from a scan delta arriving to `DriftChanged` in the outbox (`P1.hub_delta_to_drift`,
//! `hub-registry/delta_to_drift_changed`, a harness, p95).
//!
//! The pipeline's job runs as it does in the hub: ingestion publishes `FileObserved` through the real event bus (the outbox, NOTIFY, the
//! relay), the job hears it, waits out its 200 ms window, recomputes the path, and commits with `DriftChanged` in the same transaction.
//! A sample is the time from the call that hands a one-file scan delta to ingestion until a `DriftChanged` that names the file is
//! visible in the outbox, polled every 2 ms; the delta is built beforehand. Thirty samples, each a new version of the median tenant file
//! of a target-scale swimlane, sent after the job has loaded the swimlane (a change to a swimlane the hub has not loaded is a load, the
//! job's takeover, not a change; the load is `P6`'s). Every sample is a real change: the file's row moves, and the event is the one
//! that names it after the delta.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::time::{Duration, Instant};

use hub_registry::RegistryConfig;
use hub_registry::metrics::LaneResult;
use ports::ReportSink;

use common::Bed;
use common::support::perf::write_result;
use common::support::world::{RunningJob, wait_until};

const SAMPLES: usize = 30;

#[tokio::main]
async fn main() {
    let Some(mut bed) = Bed::new(1, RegistryConfig::default()).await else {
        eprintln!("hub-registry/delta_to_drift_changed: SKIPPED (no database)");
        return;
    };
    let computed = bed.pipeline.refresh_all().await.unwrap();
    assert_eq!(computed.lanes, 1);
    // The job takes over (it forgets what a replica that did not hold the lease knew) and loads the swimlane.
    let wholes = bed.rig.metrics.pipeline_lane_count(LaneResult::Whole);
    let job = RunningJob::start(&bed.pipeline);
    wait_until(
        "the takeover to load the swimlane",
        Duration::from_secs(120),
        || async {
            bed.rig.metrics.pipeline_lane_count(LaneResult::Whole) > wholes
                && !bed.pipeline.lanes_in_memory().await.is_empty()
        },
    )
    .await;

    let index = bed.median_tenant_file(0);
    let mut samples = Vec::new();
    for sample in 0..SAMPLES {
        let (identity, delta, paths) = bed.edit_delta(0, &[index]);
        let path = paths[0].as_str().to_owned();
        let after: i64 = sqlx::query_scalar("SELECT COALESCE(max(id), 0) FROM outbox")
            .fetch_one(&bed.rig.pool)
            .await
            .unwrap();
        let started = Instant::now();
        bed.rig.ingest.delta(&identity, delta).await.unwrap();
        loop {
            let seen: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM outbox WHERE id > $1 AND event #>> '{event,type}' = 'drift_changed' \
                 AND (event #> '{event,paths}') @> to_jsonb($2::text)",
            )
            .bind(after)
            .bind(&path)
            .fetch_one(&bed.rig.pool)
            .await
            .unwrap();
            if seen > 0 {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "no DriftChanged for sample {sample}"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if sample % 5 == 0 {
            eprintln!(
                "hub-registry/delta_to_drift_changed: sample {}: {ms:.0} ms",
                sample + 1
            );
        }
        samples.push(ms);
        // A moment between samples, so that one does not start in the quiet of the previous one's window.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = job.lose_lease().await;
    let path = write_result("hub-registry/delta_to_drift_changed", "ms", &samples);
    eprintln!("hub-registry/delta_to_drift_changed: {}", path.display());
}
