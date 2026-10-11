//! P7: `GET /api/v1/blobs/{hash}` answers in under 30 ms (budget `P7.blob_endpoint`, `hub-platform/blob_endpoint`, p95).
//!
//! The real endpoint behind the real guard (an authenticated Viewer), over the real `PgBlobStore` on a real Postgres 16, loaded with
//! the distinct file contents of two target-scale swimlanes (about 4,000 blobs, 40 bytes to 1.8 MiB). Each iteration is one request
//! for the next blob of a fixed pseudo-random order, read to the last byte of the body, so a 1.8 MiB response is paid for in full.
//! The registered measurement is the warm path (the cache holds the blobs); `blob_endpoint_cold` is the same request with an empty
//! cache every time, a read from Postgres in front of it, and `blob_endpoint_cold_largest` is the same for the largest blob (1.8 MiB);
//! both are reported without a budget.
//!
//! Needs Docker (testcontainers) and the fixtures (`cargo xtask gen-fixtures --scale target`).
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
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use domain::{ContentHash, Role, UserStatus};
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use hub_platform::blobs::endpoint::{BlobPolicy, BlobState, DenyHashes, mount};
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PgBlobStore};
use hub_platform::{FakeSessionAuth, GuardedRouter, TEST_USER_HEADER};
use ports::BlobStore;
use sqlx::PgPool;
use support::pg::TestDb;
use tower::ServiceExt;

struct Rig {
    runtime: tokio::runtime::Runtime,
    pool: PgPool,
    warm: Router,
    metrics: BlobMetrics,
    hashes: Vec<ContentHash>,
    /// The address of the biggest blob.
    largest: ContentHash,
    db: Option<TestDb>,
}

fn router(store: PgBlobStore) -> Router {
    let auth = Arc::new(FakeSessionAuth::new().with_user("viewer", Some(Role::Viewer), UserStatus::Active));
    let policy: Arc<dyn BlobPolicy> = Arc::new(DenyHashes::default());
    let state = BlobState::new(Arc::new(store), policy);
    let (router, _) = mount(GuardedRouter::new(auth, None), state).into_parts().unwrap();
    router
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
    let biggest = blobs.iter().map(bytes::Bytes::len).max().unwrap();
    let (db, pool, warm, metrics, hashes, largest) = runtime.block_on(async {
        let db = TestDb::ready()
            .await
            .expect("blob_endpoint needs Docker for its Postgres");
        let pool = db.app_pool().await;
        let metrics = BlobMetrics::default();
        let store = PgBlobStore::new(pool.clone(), BlobSettings::default(), metrics.clone());
        let mut hashes = Vec::with_capacity(blobs.len());
        let mut largest = None;
        for blob in blobs {
            let is_largest = blob.len() == biggest;
            let hash = store.put(blob).await.unwrap();
            if is_largest {
                largest = Some(hash);
            }
            hashes.push(hash);
        }
        for hash in &hashes {
            store.get(hash).await.unwrap().expect("a stored blob is found");
        }
        (
            db,
            pool,
            router(store),
            metrics,
            hashes,
            largest.expect("a largest blob"),
        )
    });
    Rig {
        runtime,
        pool,
        warm,
        metrics,
        hashes,
        largest,
        db: Some(db),
    }
}

fn request(hash: &ContentHash) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/blobs/{hash}"))
        .header(TEST_USER_HEADER, "viewer")
        .body(Body::empty())
        .unwrap()
}

/// One request, answered and read to the end.
async fn fetch(router: &Router, hash: &ContentHash) -> usize {
    let response = router.clone().oneshot(request(hash)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap().to_bytes().len()
}

fn benches(c: &mut Criterion) {
    let rig = rig();
    let n = rig.hashes.len();
    assert!(n > 1_000, "the fixtures hold {n} blobs, expected thousands");
    let reads_before = rig.metrics.database_reads();
    let mut group = c.benchmark_group("hub-platform");
    let mut next = 0usize;
    group.bench_function("blob_endpoint", |b| {
        b.iter(|| {
            next = (next + 7919) % n;
            black_box(rig.runtime.block_on(fetch(&rig.warm, &rig.hashes[next])))
        });
    });
    assert_eq!(
        rig.metrics.database_reads(),
        reads_before,
        "the warm endpoint reached the database, so the benchmark did not measure the warm path"
    );
    let mut next = 0usize;
    group.bench_function("blob_endpoint_cold", |b| {
        b.iter_batched(
            || {
                // The same blobs and the same table, an empty cache.
                let store =
                    PgBlobStore::new(rig.pool.clone(), BlobSettings::default(), BlobMetrics::default());
                next = (next + 7919) % n;
                (router(store), rig.hashes[next])
            },
            |(router, hash)| black_box(rig.runtime.block_on(fetch(&router, &hash))),
            BatchSize::SmallInput,
        );
    });
    group.bench_function("blob_endpoint_cold_largest", |b| {
        b.iter_batched(
            || {
                let store =
                    PgBlobStore::new(rig.pool.clone(), BlobSettings::default(), BlobMetrics::default());
                router(store)
            },
            |router| black_box(rig.runtime.block_on(fetch(&router, &rig.largest))),
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(blob_benches, benches);
criterion_main!(blob_benches);
