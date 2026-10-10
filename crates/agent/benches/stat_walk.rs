//! P4: a stat walk of 2,000 files takes under a second (`P4.stat_walk`, `agent/stat_walk_2000_files`).
//!
//! The walk every 10 s: list and `lstat` every entry, compare with the previous tree, read nothing. Swimlane 1 of the
//! target-scale fixtures, on local disk (assumption A15; real NFS numbers come from the pilot).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::hint::black_box;

use agent::root::NfsRoot;
use agent::tree::Pool;
use agent::tree::walk::{ScanMode, WalkConfig, walk};
use criterion::{Criterion, criterion_group, criterion_main};

fn stat_walk(c: &mut Criterion) {
    let dir = common::swimlane_one();
    let files = common::count_files(&dir);
    assert!(
        (1900..=2200).contains(&files),
        "{files} files: not a 2,000-file swimlane"
    );
    let root = NfsRoot::open(&dir).unwrap();
    let pool = Pool::new(4).unwrap();
    let cfg = WalkConfig::default();
    let baseline = walk(&pool, root.dir(), None, ScanMode::Full, &cfg).unwrap().tree;

    let mut group = c.benchmark_group("agent");
    group.bench_function("stat_walk_2000_files", |b| {
        b.iter(|| {
            let out = walk(&pool, root.dir(), Some(&baseline), ScanMode::Stat, &cfg).unwrap();
            // Nothing changed, so nothing is read: that is what makes the walk cheap.
            assert_eq!(out.stats.hashed, 0);
            black_box(out.tree.root_hash())
        });
    });
    group.finish();
}

criterion_group!(benches, stat_walk);
criterion_main!(benches);
