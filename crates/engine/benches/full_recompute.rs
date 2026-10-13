//! P6: the nightly full recompute at design scale takes 3 minutes or less (`P6`, `engine/full_recompute_target_scale`, p95).
//!
//! The input is what the hub holds at target scale: 40 swimlanes with about 80,000 files, read from the fixtures the way the
//! hub reads a swimlane (`tests/support/lanes.rs`): every file classified against all the deployable branches, with the hash
//! of its counterpart in Git and a baseline. Reading is set-up and is not timed. The recompute is: every distinct blob decoded,
//! hashed and parsed once, then for every swimlane the drift state of every file, the effective hash of every logical file, the
//! tree hash of every directory and the findings of checks C1 to C8. It runs on a pool of two threads, which stands in for the
//! 2 vCPU of a hub replica.
//!
//! The merge rules make every YAML file a `spring_merge` property source, which is the more expensive of the two models (the
//! layers are merged and hashed), so the number is not flattered by the cheap one.
//!
//! The result is checked on every iteration (the same states as the first run), so a change that makes the work disappear shows
//! up as a failure, not as a fast number.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;
#[path = "../tests/support/mod.rs"]
mod support;

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use domain::{DriftState, MergeMode};
use engine::{BatchRules, HostRules, Limits, MergeRules, recompute_all};
use support::lanes::{LaneData, load_lanes};

/// A line of `/proc/self/status` (Linux), for the report: the resident size now and at its peak (P11 is the hub's budget; the
/// batch's table of parsed blobs lives only for one call, and this shows how big it gets).
fn memory(label: &str) {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        let line = |key: &str| {
            status
                .lines()
                .find(|l| l.starts_with(key))
                .unwrap_or("")
                .to_owned()
        };
        eprintln!("{label}: {} | {}", line("VmRSS"), line("VmHWM"));
    }
}

fn full_recompute_bench(c: &mut Criterion) {
    let lanes = load_lanes(&common::fixtures(), usize::MAX);
    assert_eq!(lanes.len(), 40, "the target scale has 40 swimlanes");
    let files: usize = lanes.iter().map(|l| l.files.len()).sum();
    assert!(files > 70_000, "{files} files: not the target-scale fixtures");
    let inputs: Vec<_> = lanes.iter().map(LaneData::input).collect();
    memory("after reading the fixtures");

    let merge = MergeRules::new(
        MergeMode::WholeFile,
        &[
            ("*.yml", MergeMode::SpringMerge),
            ("*.yaml", MergeMode::SpringMerge),
            ("*.properties", MergeMode::SpringMerge),
        ],
    )
    .unwrap();
    let hosts = HostRules::default();
    let limits = Limits::default();
    let rules = BatchRules {
        merge_rules: &merge,
        host_rules: &hosts,
        limits: &limits,
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();

    let expected = recompute_all(&pool, &inputs, &rules).unwrap();
    assert_eq!(expected.len(), 40);
    let drifting = expected
        .iter()
        .flat_map(engine::LaneState::files)
        .filter(|f| f.drift.state != DriftState::InSync)
        .count();
    let findings: usize = expected.iter().map(|s| s.findings().len()).sum();
    assert!(
        expected.iter().all(|s| s.tree_root().is_some()) && drifting > 0 && findings > 0,
        "{drifting} drifting files, {findings} findings: the fixtures must have both kinds"
    );
    eprintln!("full recompute: {files} files, {drifting} drifting, {findings} findings");
    memory("after the first recompute");

    let mut group = c.benchmark_group("engine");
    // One recompute per sample: the p95 is over single recomputes.
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(1));
    group.measurement_time(Duration::from_secs(1));
    group.bench_function("full_recompute_target_scale", |b| {
        b.iter(|| {
            let states = recompute_all(&pool, black_box(&inputs), &rules).unwrap();
            assert!(states == expected, "a recompute gave another answer");
            black_box(states)
        });
    });
    group.finish();
}

criterion_group!(benches, full_recompute_bench);
criterion_main!(benches);
