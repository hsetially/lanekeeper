//! P5, the base cost: an incremental recompute of one changed file in one swimlane finishes within 50 ms (`P5`,
//! `hub-registry/incremental_recompute_base`, criterion, p95), as the hub does it: the rows of the path read from Postgres, the path
//! rebuilt against Git, the content of its group read from the blob store, `engine::recompute_paths`, and the difference committed in one
//! transaction (the derived columns, the findings, the effective tables, the registry row and the events), over the target-scale
//! fixtures. The engine's kernel alone is `P5.engine_base`.
//!
//! The file is the one of median size among the swimlane's structured tenant files that Git also has, so its group (the tenant file, its
//! base file and the other tenants' copies) is a typical one. Each iteration is a new version of it, sent through ingestion first
//! (not timed): the pipeline is asked about a change that really happened, and is checked to have recomputed the path and named it in
//! `DriftChanged`, so the number is not for work that was skipped. Before anything is timed, the incremental state is checked to be
//! what a full recompute gives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use hub_registry::metrics::LaneResult;
use hub_registry::{RegistryConfig, Work};

use common::Bed;

fn bench(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let Some(mut bed) = rt.block_on(Bed::new(1, RegistryConfig::default())) else {
        eprintln!("hub-registry/incremental_recompute_base: SKIPPED (no database)");
        return;
    };
    let lane = bed.lanes[0].id.clone();
    let first = rt
        .block_on(bed.pipeline.process(vec![(lane.clone(), Work::Whole)]))
        .unwrap();
    assert_eq!(first.lanes[0].result, LaneResult::Whole);
    let index = bed.median_tenant_file(0);
    eprintln!(
        "hub-registry/incremental_recompute_base: {} ({} bytes), {} paths in the swimlane",
        bed.lanes[0].files[index].0,
        bed.lanes[0].files[index].1.len(),
        first.lanes[0].paths
    );

    // The incremental result is what a full recompute gives, and the change is news.
    let paths = rt.block_on(bed.edit(0, &[index]));
    let report = rt
        .block_on(
            bed.pipeline
                .process(vec![(lane.clone(), Work::Paths(paths.into_iter().collect()))]),
        )
        .unwrap();
    assert_eq!(report.lanes[0].result, LaneResult::Incremental);
    assert_eq!(report.lanes[0].drift_paths, 1, "{report:?}");
    let incremental = rt.block_on(bed.pipeline.lane_state(&lane)).unwrap();
    let (fresh, _) = bed
        .rig
        .pipeline(std::sync::Arc::new(bed.git.clone()), RegistryConfig::default());
    let repaired = rt.block_on(fresh.refresh_lane(&lane)).unwrap();
    assert_eq!(
        repaired.lanes[0].rows_written + repaired.lanes[0].rows_deleted,
        0,
        "nothing to repair: {repaired:?}"
    );
    assert!(
        rt.block_on(fresh.lane_state(&lane)).unwrap() == incremental,
        "the incremental state is not the full recompute"
    );

    let mut group = c.benchmark_group("hub-registry");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(10));
    group.bench_function("incremental_recompute_base", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let paths = rt.block_on(bed.edit(0, &[index]));
                let work = vec![(lane.clone(), Work::Paths(paths.into_iter().collect()))];
                let started = Instant::now();
                let report = rt.block_on(bed.pipeline.process(black_box(work))).unwrap();
                total += started.elapsed();
                assert_eq!(report.lanes.len(), 1);
                assert_eq!(report.lanes[0].result, LaneResult::Incremental);
                assert_eq!(report.lanes[0].drift_paths, 1, "the change was named: {report:?}");
            }
            total
        });
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
