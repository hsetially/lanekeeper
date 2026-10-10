//! P4: a full rehash of 2,000 files takes under 20 s (budget `P4`, `agent/full_rehash_2000_files`).
//!
//! The real walker over swimlane 1 of the target-scale fixtures, with every file read and hashed on the worker pool.
//! The fixtures sit on local disk, not NFS (assumption A15): this is the CPU and system-call cost of the walk; the
//! latency of a real mount is a pilot measurement (Q11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::hint::black_box;
use std::time::Duration;

use agent::root::NfsRoot;
use agent::tree::Pool;
use agent::tree::walk::{ScanMode, WalkConfig, walk};
use criterion::{Criterion, criterion_group, criterion_main};

fn full_rehash(c: &mut Criterion) {
    let dir = common::swimlane_one();
    let files = common::count_files(&dir);
    assert!(
        (1900..=2200).contains(&files),
        "{files} files: not a 2,000-file swimlane"
    );
    let root = NfsRoot::open(&dir).unwrap();
    let pool = Pool::new(4).unwrap();
    let cfg = WalkConfig::default();

    let mut group = c.benchmark_group("agent");
    group.sample_size(20).measurement_time(Duration::from_secs(10));
    group.bench_function("full_rehash_2000_files", |b| {
        b.iter(|| {
            let out = walk(&pool, root.dir(), None, ScanMode::Full, &cfg).unwrap();
            assert_eq!(usize::try_from(out.stats.hashed), Ok(files));
            black_box(out.tree.root_hash())
        });
    });
    group.finish();
}

criterion_group!(benches, full_rehash);
criterion_main!(benches);
