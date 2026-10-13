//! P5: an incremental recompute after a change to N files finishes within 50 ms + 2 ms x N x S for S affected swimlanes.
//! Two budgets measure it, both on the engine's kernel `recompute_paths` over the target-scale fixtures:
//!
//! - `P5.engine_base` (`engine/incremental_recompute_base`, criterion): one changed file in one swimlane, the whole cost of
//!   bringing that swimlane's state up to date. The file is the one of median size among the swimlane's structured tenant files
//!   that Git also has, so the group (the tenant file, its base file and the other tenants' copies) is a typical one. Two more
//!   benchmarks, not budgets, show the extremes: the largest structured file (`..._largest_file`, a 6,000-line file with its
//!   copies) and a change to the root `application.yml`, which every `spring_merge` group depends on (`..._root_file`).
//! - `P5.engine_per_file_per_swimlane` (`engine/incremental_recompute_per_file_per_swimlane`, a harness): 100 changed files in
//!   each of 10 swimlanes, 1,000 (swimlane, path) pairs, on a pool of two threads (2 vCPU). The time of the whole call divided
//!   by 1,000 is one sample; ten samples go to `target/perf-results/` for `cargo xtask bench-check`. The base cost is inside
//!   the figure, so it is above the per-pair cost the budget asks for.
//!
//! Reading the fixtures is set-up. Before anything is timed, the incremental result is checked to be the full recompute of the
//! changed swimlanes, and different from the state it started from, so the numbers are not for work that was skipped.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;
#[path = "../tests/support/mod.rs"]
mod support;

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bytes::Bytes;
use criterion::{Criterion, criterion_group};
use domain::{AppName, FileClass, NfsPath, SwimlaneId};
use engine::{
    BatchRules, HostRules, LaneFile, LaneState, Limits, NfsContent, recompute_all, recompute_paths,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use support::lanes::{LaneData, load_lanes, sample_app_names};

const SWIMLANES: usize = 10;
const FILES_PER_SWIMLANE: usize = 100;
const SAMPLES: usize = 10;
/// The harness's name below `engine/` in `perf/budgets.toml` (`P5.engine_per_file_per_swimlane`).
const HARNESS: &str = "incremental_recompute_per_file_per_swimlane";

/// Some files are named like an application, so some are property sources: the hub's roles (D82), at a realistic count.
fn app_names(lanes: &[LaneData]) -> Vec<AppName> {
    sample_app_names(lanes, 300)
}

fn edit(file: &LaneFile, round: usize) -> LaneFile {
    let mut text = file.nfs.as_ref().unwrap().bytes.to_vec();
    if !text.ends_with(b"\n") {
        text.push(b'\n');
    }
    text.extend_from_slice(format!("bench_edit_{round}: {round}\n").as_bytes());
    let bytes = Bytes::from(text);
    LaneFile {
        nfs: Some(NfsContent {
            blob: blob_hash(&bytes),
            bytes,
        }),
        ..file.clone()
    }
}

fn blob_hash(data: &[u8]) -> domain::ContentHash {
    domain::ContentHash::from_bytes(Sha256::digest(data).into())
}

fn is_editable(f: &LaneFile) -> bool {
    f.class == FileClass::Structured
        && f.nfs.is_some()
        && matches!(
            f.path.as_str().rsplit('.').next(),
            Some("yml" | "yaml" | "properties")
        )
}

fn copy_of(lane: &LaneData) -> LaneData {
    LaneData {
        id: lane.id.clone(),
        tenants: lane.tenants.clone(),
        files: lane.files.clone(),
    }
}

/// A copy of the swimlane with the files at `indices` edited.
fn with_edits(lane: &LaneData, indices: &[usize], round: usize) -> LaneData {
    let mut out = copy_of(lane);
    for &i in indices {
        out.files[i] = edit(&lane.files[i], round);
    }
    out
}

fn affected(lane: &LaneData, indices: &[usize]) -> Vec<(SwimlaneId, NfsPath)> {
    indices
        .iter()
        .map(|&i| (lane.id.clone(), lane.files[i].path.clone()))
        .collect()
}

/// The index of the file of median size among the swimlane's structured tenant files that Git also has.
fn median_tenant_file(lane: &LaneData) -> usize {
    let mut candidates: Vec<(usize, usize)> = lane
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| is_editable(f) && f.facts.tenant.is_some() && f.git.is_some())
        .map(|(i, f)| (f.nfs.as_ref().unwrap().bytes.len(), i))
        .collect();
    candidates.sort_unstable();
    candidates[candidates.len() / 2].1
}

fn largest_file(lane: &LaneData) -> usize {
    lane.files
        .iter()
        .enumerate()
        .filter(|(_, f)| is_editable(f))
        .max_by_key(|(_, f)| f.nfs.as_ref().unwrap().bytes.len())
        .unwrap()
        .0
}

criterion_group!(benches, base_benches);

fn base_benches(c: &mut Criterion) {
    let lanes = load_lanes(&common::fixtures(), 1);
    let lane = &lanes[0];
    let apps = app_names(&lanes);
    let hosts = HostRules::default();
    let limits = Limits::default();
    let rules = BatchRules {
        application_names: &apps,
        host_rules: &hosts,
        limits: &limits,
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let state: Vec<LaneState> = recompute_all(&pool, &[lane.input()], &rules).unwrap();

    let root = lane
        .files
        .iter()
        .position(|f| f.path.as_str() == "application.yml")
        .expect("the fixtures have a root application.yml");
    let cases = [
        ("incremental_recompute_base", median_tenant_file(lane)),
        ("incremental_recompute_largest_file", largest_file(lane)),
        ("incremental_recompute_root_file", root),
    ];

    let mut group = c.benchmark_group("engine");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));
    for (name, index) in cases {
        let changed = with_edits(lane, &[index], 1);
        let paths = affected(lane, &[index]);
        // The kernel gives what a full recompute gives, and it is not the state it started from.
        let once = recompute_paths(&pool, &state, &[changed.input()], &paths, &rules).unwrap();
        let full = recompute_all(&pool, &[changed.input()], &rules).unwrap();
        assert!(
            once == full,
            "{name}: the incremental result is not the full recompute"
        );
        assert!(once[0] != state[0], "{name}: the change changed nothing");
        let size = lane.files[index].nfs.as_ref().unwrap().bytes.len();
        eprintln!("{name}: {} ({size} bytes)", lane.files[index].path);
        group.bench_function(name, |b| {
            b.iter(|| {
                let next =
                    recompute_paths(&pool, black_box(&state), &[changed.input()], &paths, &rules).unwrap();
                black_box(next)
            });
        });
    }
    group.finish();
}

/// Nearest-rank percentile of an ascending slice, the rule `bench-check` applies to its own samples.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let n = sorted.len() as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rank = ((p * n) - 1e-9).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn results_dir() -> PathBuf {
    common::target_dir().join("perf-results")
}

/// 100 files in each of 10 swimlanes, changed together: one call, 1,000 pairs.
fn per_file_per_swimlane() {
    let lanes = load_lanes(&common::fixtures(), SWIMLANES);
    assert_eq!(lanes.len(), SWIMLANES);
    let apps = app_names(&lanes);
    let hosts = HostRules::default();
    let limits = Limits::default();
    let rules = BatchRules {
        application_names: &apps,
        host_rules: &hosts,
        limits: &limits,
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let inputs: Vec<_> = lanes.iter().map(LaneData::input).collect();
    let states = recompute_all(&pool, &inputs, &rules).unwrap();

    // The files to change: evenly spread over the editable ones of each swimlane.
    let changed: Vec<LaneData> = lanes
        .iter()
        .map(|lane| {
            let editable: Vec<usize> = (0..lane.files.len())
                .filter(|&i| is_editable(&lane.files[i]))
                .collect();
            assert!(
                editable.len() >= FILES_PER_SWIMLANE,
                "{} editable files",
                editable.len()
            );
            let step = editable.len() / FILES_PER_SWIMLANE;
            let picked: Vec<usize> = (0..FILES_PER_SWIMLANE).map(|k| editable[k * step]).collect();
            with_edits(lane, &picked, 1)
        })
        .collect();
    let paths: Vec<(SwimlaneId, NfsPath)> = lanes
        .iter()
        .zip(&changed)
        .flat_map(|(before, after)| {
            before
                .files
                .iter()
                .zip(&after.files)
                .filter(|(a, b)| a.nfs.as_ref().unwrap().blob != b.nfs.as_ref().unwrap().blob)
                .map(|(a, _)| (before.id.clone(), a.path.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(paths.len(), SWIMLANES * FILES_PER_SWIMLANE);
    let changed_inputs: Vec<_> = changed.iter().map(LaneData::input).collect();

    let once = recompute_paths(&pool, &states, &changed_inputs, &paths, &rules).unwrap();
    let full = recompute_all(&pool, &changed_inputs, &rules).unwrap();
    assert!(once == full, "the incremental result is not the full recompute");
    assert!(
        once.iter().zip(&states).all(|(a, b)| a != b),
        "a swimlane did not change"
    );

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let next = recompute_paths(&pool, black_box(&states), &changed_inputs, &paths, &rules).unwrap();
        let elapsed = started.elapsed();
        black_box(next);
        #[allow(clippy::cast_precision_loss)]
        samples.push(elapsed.as_secs_f64() * 1000.0 / paths.len() as f64);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    #[allow(clippy::cast_precision_loss)]
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let bench = format!("engine/{HARNESS}");
    let body = json!({
        "bench": bench,
        "unit": "ms",
        "values": {
            "min": percentile(&sorted, 0.0),
            "p50": percentile(&sorted, 0.50),
            "p95": percentile(&sorted, 0.95),
            "p99": percentile(&sorted, 0.99),
            "max": percentile(&sorted, 1.0),
            "mean": mean,
        },
        "samples": samples,
    });
    let dir = results_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.json", bench.replace('/', "_")));
    std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    eprintln!(
        "{bench}: {} pairs per call, ms per pair {samples:.3?}\n-> {}",
        paths.len(),
        path.display()
    );
}

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
    per_file_per_swimlane();
}
