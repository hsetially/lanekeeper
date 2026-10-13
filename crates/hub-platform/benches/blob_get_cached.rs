//! P7: a cached `BlobStore::get` takes under 1 ms (budget `P7.blob_get_cached`, `hub-platform/blob_get_cached`, p95).
//!
//! The real `PgBlobStore` on a real Postgres 16, loaded with the distinct file contents of two target-scale swimlanes (about 4,000
//! blobs, 40 bytes to 1.8 MiB, CRLF and binary among them) and warmed so that every one is in the cache. Each iteration reads the
//! next blob of a fixed pseudo-random order, so the cost is a cache hit on blobs of every size, not one lucky entry. After the
//! measurement the harness checks that no read reached the database: a "cached" number that went to Postgres is not one.
//!
//! Needs Docker (testcontainers) and the fixtures (`cargo xtask gen-fixtures --scale target`); without either this fails rather
//! than measuring something else.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation
)]

#[path = "../tests/support/mod.rs"]
mod support;

mod common;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use domain::ContentHash;
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PgBlobStore};
use ports::BlobStore;
use support::pg::TestDb;

struct Rig {
    runtime: tokio::runtime::Runtime,
    store: PgBlobStore,
    metrics: BlobMetrics,
    hashes: Vec<ContentHash>,
    db: Option<TestDb>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The container is removed by an async drop, which needs the runtime that started it.
        let _guard = self.runtime.enter();
        drop(self.db.take());
    }
}

fn rig() -> Rig {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let blobs = common::fixture_blobs(2);
    let (db, store, metrics, hashes) = runtime.block_on(async {
        let db = TestDb::ready()
            .await
            .expect("blob_get_cached needs Docker for its Postgres");
        let metrics = BlobMetrics::default();
        let store = PgBlobStore::new(db.app_pool().await, BlobSettings::default(), metrics.clone());
        let mut hashes = Vec::with_capacity(blobs.len());
        for blob in blobs {
            hashes.push(store.put(blob).await.unwrap());
        }
        // Warm: every blob read once, so every later read is a hit.
        for hash in &hashes {
            store.get(hash).await.unwrap().expect("a stored blob is found");
        }
        (db, store, metrics, hashes)
    });
    Rig {
        runtime,
        store,
        metrics,
        hashes,
        db: Some(db),
    }
}

fn benches(c: &mut Criterion) {
    let rig = rig();
    let n = rig.hashes.len();
    assert!(n > 1_000, "the fixtures hold {n} blobs, expected thousands");
    let reads_before = rig.metrics.database_reads();
    let mut group = c.benchmark_group("hub-platform");
    let mut next = 0usize;
    group.bench_function("blob_get_cached", |b| {
        b.iter(|| {
            // A stride coprime with the count visits every blob before repeating.
            next = (next + 7919) % n;
            let blob = rig.runtime.block_on(rig.store.get(&rig.hashes[next]));
            black_box(blob.unwrap().expect("warm blob"))
        });
    });
    group.finish();
    assert_eq!(
        rig.metrics.database_reads(),
        reads_before,
        "a cached read reached the database, so the benchmark did not measure the cache"
    );
}

criterion_group!(blob_benches, benches);
criterion_main!(blob_benches);
