//! T7: `GitReader::diff_trees` and the cold tree index, on the target-scale fixture repositories (1,400 base files with three
//! tags, 60 tenant branches of about 600 files).
//!
//! `hub-platform/diff_trees` is the diff of two base versions (`v1.0.0` to `v3.0.0`) plus the diff of two tenant branches, which
//! share almost no tree, so the walk cannot skip subtrees: the worst case for the diff. The prompt asks for this benchmark and
//! gives it no budget, so it is reported and not registered. `hub-platform/tree_index_cold` builds the index of one tenant
//! branch from nothing (every blob read and hashed on the pool), the cost the first read of a new commit pays.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use domain::{CommitId, RepoKind};
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
use hub_platform::git::reader::{GixReader, ReaderSettings};
use ports::GitReader;

struct Fixture {
    _dir: tempfile::TempDir,
    _runtime: tokio::runtime::Runtime,
    mirror: Arc<Mirror>,
    v1: CommitId,
    v3: CommitId,
    sit1: CommitId,
    sit2: CommitId,
}

fn fixture() -> Fixture {
    let (base, tenant) = common::fixture_repos();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let settings = MirrorSettings::new(
        dir.path(),
        RemoteUrl::local(&format!("file://{}", base.display())),
        RemoteUrl::local(&format!("file://{}", tenant.display())),
    );
    let mirror = runtime.block_on(async {
        let mirror = Mirror::open(settings, Arc::new(NoCredentials), GitMetrics::default()).unwrap();
        mirror.initialize().await.unwrap();
        mirror
    });
    let reader = reader(&mirror);
    let tag = |name: &str| {
        reader
            .tags(RepoKind::Base)
            .unwrap()
            .into_iter()
            .find(|t| t.name.as_str() == name)
            .unwrap()
            .commit
    };
    let (v1, v3) = (tag("v1.0.0"), tag("v3.0.0"));
    let (sit1, sit2) = (
        reader.head(RepoKind::Tenant, "sit1").unwrap(),
        reader.head(RepoKind::Tenant, "sit2").unwrap(),
    );
    Fixture {
        _dir: dir,
        _runtime: runtime,
        mirror,
        v1,
        v3,
        sit1,
        sit2,
    }
}

fn reader(mirror: &Arc<Mirror>) -> GixReader {
    GixReader::new(
        Arc::clone(mirror),
        &ReaderSettings::default(),
        GitMetrics::default(),
    )
    .unwrap()
}

fn benches(c: &mut Criterion) {
    let f = fixture();
    let reader = reader(&f.mirror);
    // The numbers mean something only if the diffs are not empty.
    assert!(
        !reader
            .diff_trees(RepoKind::Base, &f.v1, &f.v3)
            .unwrap()
            .is_empty()
    );
    assert!(
        reader
            .diff_trees(RepoKind::Tenant, &f.sit1, &f.sit2)
            .unwrap()
            .len()
            > 100
    );

    let mut group = c.benchmark_group("hub-platform");
    group.bench_function("diff_trees", |b| {
        b.iter(|| {
            let base = reader.diff_trees(RepoKind::Base, &f.v1, &f.v3).unwrap();
            let tenant = reader.diff_trees(RepoKind::Tenant, &f.sit1, &f.sit2).unwrap();
            black_box((base.len(), tenant.len()))
        });
    });
    group.bench_function("tree_index_cold", |b| {
        b.iter_batched(
            || reader_for(&f.mirror),
            |reader| {
                black_box(
                    reader
                        .tree_index(RepoKind::Tenant, &f.sit1)
                        .unwrap()
                        .entries()
                        .len(),
                )
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn reader_for(mirror: &Arc<Mirror>) -> GixReader {
    reader(mirror)
}

criterion_group!(git_benches, benches);
criterion_main!(git_benches);
