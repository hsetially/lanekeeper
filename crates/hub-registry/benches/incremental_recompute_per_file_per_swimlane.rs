//! P5, the per-file part: an incremental recompute after a change to N files finishes within 50 ms + 2 ms x N x S for S affected
//! swimlanes (`P5.per_file_per_swimlane`, `hub-registry/incremental_recompute_per_file_per_swimlane`, a harness, p95).
//!
//! 100 changed files in each of 10 swimlanes, 1,000 (swimlane, path) pairs, in one batch, over the target-scale fixtures and a real
//! Postgres as the hub runs it: the rows read for all the swimlanes together, the paths rebuilt, the content fetched once, the engine
//! called once, and one transaction for each swimlane. The time of the whole call divided by 1,000 is one sample; ten samples go to
//! `target/perf-results/hub-registry_incremental_recompute_per_file_per_swimlane.json` for `cargo xtask bench-check`. The base cost is
//! inside the figure, so it is above the per-pair cost the budget asks for. The engine's own figure is `P5.engine_per_file_per_swimlane`.
//!
//! Each sample is made of new versions sent through ingestion (not timed) and checked: every swimlane was recomputed incrementally,
//! all its 100 paths, and named in `DriftChanged`. The files are structured YAML, spread over the swimlane's editable files (a different
//! set every sample), and not the root `application.yml`, which every property source depends on (the engine's benchmark has it
//! separately).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss
)]

mod common;

use std::time::Instant;

use hub_registry::metrics::LaneResult;
use hub_registry::{RegistryConfig, Work};

use common::Bed;
use common::support::perf::write_result;

const LANES: usize = 10;
const FILES: usize = 100;
const SAMPLES: usize = 10;

#[tokio::main]
async fn main() {
    let Some(mut bed) = Bed::new(LANES, RegistryConfig::default()).await else {
        eprintln!("hub-registry/incremental_recompute_per_file_per_swimlane: SKIPPED (no database)");
        return;
    };
    let whole = bed.pipeline.refresh_all().await.unwrap();
    assert_eq!(whole.lanes, LANES);
    eprintln!(
        "hub-registry/incremental_recompute_per_file_per_swimlane: {} swimlanes computed whole in {:.1} s",
        whole.lanes,
        whole.took.as_secs_f64()
    );

    let editable: Vec<Vec<usize>> = (0..LANES)
        .map(|l| {
            bed.editable(l)
                .into_iter()
                .filter(|&i| bed.lanes[l].files[i].0.as_str() != "application.yml")
                .collect()
        })
        .collect();
    let mut samples = Vec::new();
    for sample in 0..SAMPLES {
        let mut work = Vec::new();
        for (lane, pool) in editable.iter().enumerate() {
            assert!(
                pool.len() >= FILES,
                "too few editable files in {}",
                bed.lanes[lane].id
            );
            // Spread over the swimlane's files, a different set each sample.
            let picks: Vec<usize> = (0..FILES)
                .map(|j| pool[(j * pool.len() / FILES + sample) % pool.len()])
                .collect();
            let paths = bed.edit(lane, &picks).await;
            assert_eq!(paths.len(), FILES);
            work.push((
                bed.lanes[lane].id.clone(),
                Work::Paths(paths.into_iter().collect()),
            ));
        }
        let started = Instant::now();
        let report = bed.pipeline.process(work).await.unwrap();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(report.lanes.len(), LANES, "{report:?}");
        for lane in &report.lanes {
            assert_eq!(lane.result, LaneResult::Incremental, "{lane:?}");
            assert_eq!(lane.paths, FILES);
            assert_eq!(lane.drift_paths, FILES, "every changed path was named: {lane:?}");
        }
        let per_pair = ms / (LANES * FILES) as f64;
        eprintln!(
            "hub-registry/incremental_recompute_per_file_per_swimlane: sample {}: {ms:.0} ms for {} pairs = {per_pair:.3} ms per pair",
            sample + 1,
            LANES * FILES
        );
        samples.push(per_pair);
    }
    let path = write_result(
        "hub-registry/incremental_recompute_per_file_per_swimlane",
        "ms",
        &samples,
    );
    eprintln!(
        "hub-registry/incremental_recompute_per_file_per_swimlane: {}",
        path.display()
    );
}
