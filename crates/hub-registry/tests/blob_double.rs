//! The registry's blob test double stores a blob the way the hub does (T9, migration 0008): a blob that is stored again has its
//! `touched_at` refreshed, once it is older than the touch interval, and nothing else of the row changes. Retention's tests rely on
//! it (a put of an old blob protects it for the grace period), and so do the ingestion tests and the P15 bench, which now run the real
//! `PUT_SQL`. A copy of `put` that stored with `ON CONFLICT DO NOTHING` would leave every old blob collectable as soon as it was
//! stored again, and these tests would fail.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use bytes::Bytes;
use domain::ContentHash;
use ports::{BlobStore, content_hash};
use sqlx::PgPool;
use support::pg::TestDb;
use support::pg_blobs::PgBlobs;

const THREE_DAYS_SECS: f64 = 3.0 * 24.0 * 3600.0;
const HALF_AN_HOUR_SECS: f64 = 1800.0;

async fn age(pool: &PgPool, hash: &ContentHash, secs: f64) {
    sqlx::query("UPDATE blobs SET touched_at = now() - make_interval(secs => $2::double precision) WHERE hash = $1::bytea")
        .bind(hash.as_bytes().as_slice())
        .bind(secs)
        .execute(pool)
        .await
        .unwrap();
}

async fn touched_secs_ago(pool: &PgPool, hash: &ContentHash) -> f64 {
    sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - touched_at)::double precision FROM blobs WHERE hash = $1::bytea",
    )
    .bind(hash.as_bytes().as_slice())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_blob_that_is_stored_again_after_the_touch_interval_is_refreshed() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let blobs = PgBlobs::new(pool.clone());
    let bytes = Bytes::from_static(b"a: reverted to an old version\n");
    let hash = blobs.put(bytes.clone()).await.unwrap();
    assert_eq!(hash, content_hash(&bytes));
    age(&pool, &hash, THREE_DAYS_SECS).await;
    assert!(touched_secs_ago(&pool, &hash).await > THREE_DAYS_SECS - 60.0);

    assert_eq!(blobs.put(bytes.clone()).await.unwrap(), hash);

    assert!(
        touched_secs_ago(&pool, &hash).await < 60.0,
        "the double's put is the real one: storing the content again refreshes its age"
    );
    assert_eq!(blobs.stored().await, 1, "stored once");
    assert_eq!(
        blobs.get(&hash).await.unwrap(),
        Some(bytes),
        "the bytes are the ones that were put"
    );
}

#[tokio::test]
async fn a_blob_touched_within_the_interval_is_left_as_it_is() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let blobs = PgBlobs::new(pool.clone());
    let bytes = Bytes::from_static(b"hot: blob\n");
    let hash = blobs.put(bytes.clone()).await.unwrap();
    age(&pool, &hash, HALF_AN_HOUR_SECS).await;

    blobs.put(bytes).await.unwrap();

    let ago = touched_secs_ago(&pool, &hash).await;
    assert!(
        (HALF_AN_HOUR_SECS - 60.0..HALF_AN_HOUR_SECS + 60.0).contains(&ago),
        "a re-put within the interval writes nothing (the real statement's rate limit), so the age is where it was: {ago}"
    );
}

#[tokio::test]
async fn the_double_still_counts_fails_and_refuses_what_it_did_before() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let blobs = PgBlobs::new(pool);
    blobs.put(Bytes::from_static(b"one\n")).await.unwrap();
    blobs.put(Bytes::from_static(b"one\n")).await.unwrap();
    assert_eq!(blobs.puts(), 2, "calls to put, new or not");
    blobs.set_failing(true);
    assert!(blobs.put(Bytes::from_static(b"two\n")).await.is_err());
    assert_eq!(blobs.puts(), 3, "a failing put is counted too");
    blobs.set_failing(false);
    assert_eq!(
        blobs
            .put(Bytes::from(vec![0_u8; ports::MAX_BLOB_BYTES + 1]))
            .await,
        Err(ports::BlobError::TooLarge),
        "the real store's size limit"
    );
    assert_eq!(blobs.stored().await, 1);
}
