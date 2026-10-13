//! T8 (P7, P12, S11, S21): `PgBlobStore` on a real Postgres 16 as the app role: the port's conformance suite, the table's own
//! content address, the cache in front of it, batched reads, and what a failing database does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::ContentHash;
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PgBlobStore};
use ports::{BlobError, BlobStore, MAX_BLOB_BYTES, MAX_GET_MANY, content_hash};
use sqlx::{PgPool, Row};
use support::log_capture::LogCapture;
use support::pg::TestDb;

const MIB: u64 = 1024 * 1024;

fn settings(cache_mib: u64) -> BlobSettings {
    BlobSettings {
        cache_bytes: cache_mib * MIB,
        ..BlobSettings::default()
    }
}

async fn store(db: &TestDb, cache_mib: u64) -> (PgBlobStore, PgPool, BlobMetrics) {
    let pool = db.app_pool().await;
    let metrics = BlobMetrics::default();
    let store = PgBlobStore::new(pool.clone(), settings(cache_mib), metrics.clone());
    (store, pool, metrics)
}

async fn rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn blob_store_conformance() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, _, _) = store(&db, 64).await;
    ports::conformance::blob_store(&store).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn put_stores_the_bytes_once_under_their_address_and_the_row_checks_out() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool, _) = store(&db, 64).await;
    let bytes = Bytes::from_static(b"key: value\r\nother: 2\r\n");
    let hash = store.put(bytes.clone()).await.unwrap();
    assert_eq!(hash, content_hash(&bytes));
    assert_eq!(store.put(bytes.clone()).await.unwrap(), hash, "idempotent");
    assert_eq!(rows(&pool).await, 1, "stored once");

    let row = sqlx::query(
        "SELECT content::bytea AS content, size, hash::bytea AS hash, hash = sha256(content) AS addressed FROM blobs",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row.get::<Vec<u8>, _>("content"),
        bytes.to_vec(),
        "CRLF is stored as given"
    );
    assert_eq!(row.get::<i64, _>("size"), i64::try_from(bytes.len()).unwrap());
    assert_eq!(row.get::<Vec<u8>, _>("hash"), hash.as_bytes().to_vec());
    assert!(row.get::<bool, _>("addressed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_table_itself_refuses_a_row_under_the_wrong_address() {
    let Some(db) = TestDb::ready().await else { return };
    let (_, pool, _) = store(&db, 1).await;
    let wrong = content_hash(b"something else");
    let result = sqlx::query("INSERT INTO blobs (hash, content, size) VALUES ($1::bytea, $2::bytea, 5)")
        .bind(wrong.as_bytes().as_slice())
        .bind(b"hello".as_slice())
        .execute(&pool)
        .await;
    assert!(
        result.is_err(),
        "the CHECK keeps the table content-addressed even if a writer is wrong"
    );
    assert_eq!(rows(&pool).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn put_does_not_fill_the_cache_but_the_first_read_does() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, _, metrics) = store(&db, 64).await;
    let hash = store
        .put(Bytes::from_static(b"written, maybe never read"))
        .await
        .unwrap();
    assert_eq!(store.cache().len(), 0, "writes are often never read again");
    assert_eq!(metrics.database_reads(), 0);

    assert!(store.get(&hash).await.unwrap().is_some());
    assert_eq!(store.cache().len(), 1);
    assert_eq!(metrics.database_reads(), 1);

    assert!(store.get(&hash).await.unwrap().is_some());
    assert_eq!(metrics.database_reads(), 1, "the second read is a hit");
    assert_eq!((metrics.cache_hits(), metrics.cache_misses()), (1, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cached_blob_is_served_without_the_database() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool, _) = store(&db, 64).await;
    let bytes = Bytes::from(vec![9u8; 100_000]);
    let hash = store.put(bytes.clone()).await.unwrap();
    assert_eq!(store.get(&hash).await.unwrap(), Some(bytes.clone()));
    // With no database at all, the cached read still answers: that is what "cached under 1 ms" rests on.
    pool.close().await;
    assert_eq!(store.get(&hash).await.unwrap(), Some(bytes.clone()));
    assert_eq!(store.get_many(&[hash]).await.unwrap(), vec![(hash, bytes)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_hash_is_none_and_is_not_cached() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, _, metrics) = store(&db, 64).await;
    let unknown = content_hash(b"never stored");
    assert_eq!(store.get(&unknown).await.unwrap(), None);
    assert_eq!(store.get(&unknown).await.unwrap(), None);
    assert_eq!(store.cache().len(), 0, "negative results are not cached");
    assert_eq!(metrics.database_reads(), 2, "so each ask reaches the database");

    // And once it is stored, the next read finds it: nothing stale was remembered.
    let stored = store.put(Bytes::from_static(b"never stored")).await.unwrap();
    assert_eq!(stored, unknown);
    assert_eq!(
        store.get(&unknown).await.unwrap(),
        Some(Bytes::from_static(b"never stored"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn get_many_keeps_first_request_order_skips_unknown_and_mixes_cache_with_database() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, _, metrics) = store(&db, 64).await;
    let mut hashes = Vec::new();
    for n in 0..20u8 {
        hashes.push(
            store
                .put(Bytes::from(vec![n; 10 + usize::from(n)]))
                .await
                .unwrap(),
        );
    }
    // Half of them are warm.
    for hash in hashes.iter().step_by(2) {
        store.get(hash).await.unwrap().unwrap();
    }
    let reads_before = metrics.database_reads();
    let unknown = content_hash(b"unknown");
    let mut ask: Vec<ContentHash> = hashes.iter().rev().copied().collect();
    ask.insert(3, unknown);
    ask.push(hashes[0]);
    ask.push(unknown);

    let got = store.get_many(&ask).await.unwrap();
    let order: Vec<ContentHash> = got.iter().map(|(h, _)| *h).collect();
    let expected: Vec<ContentHash> = hashes.iter().rev().copied().collect();
    assert_eq!(order, expected, "first-request order, unknown skipped, each once");
    for (hash, bytes) in &got {
        assert_eq!(*hash, content_hash(bytes));
    }
    assert_eq!(
        metrics.database_reads() - reads_before,
        1,
        "the ten that were cold came from one batched query"
    );
    // Everything that exists is warm now, so only the unknown hash (never cached, so asked again) reaches the database.
    let again = metrics.database_reads();
    store.get_many(&ask).await.unwrap();
    assert_eq!(
        metrics.database_reads(),
        again + 1,
        "one read, for the hash that does not exist"
    );
    let known: Vec<ContentHash> = ask.iter().copied().filter(|h| *h != unknown).collect();
    let warm = metrics.database_reads();
    assert_eq!(store.get_many(&known).await.unwrap().len(), hashes.len());
    assert_eq!(metrics.database_reads(), warm, "all warm: no read at all");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_many_with_nothing_to_ask_or_too_much_never_reaches_the_database() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool, _) = store(&db, 1).await;
    pool.close().await;
    assert!(store.get_many(&[]).await.unwrap().is_empty());
    let too_many = vec![content_hash(b"x"); MAX_GET_MANY + 1];
    assert_eq!(store.get_many(&too_many).await, Err(BlobError::TooMany));
    // Exactly the limit is accepted (and here fails only because the pool is closed).
    let at_limit = vec![content_hash(b"x"); MAX_GET_MANY];
    assert_eq!(store.get_many(&at_limit).await, Err(BlobError::Unavailable));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blob_over_4_mib_is_refused_before_it_is_hashed_or_stored() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool, _) = store(&db, 1).await;
    pool.close().await;
    let big = Bytes::from(vec![1u8; MAX_BLOB_BYTES + 1]);
    assert_eq!(store.put(big).await, Err(BlobError::TooLarge));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_database_that_is_gone_is_unavailable_never_a_panic_or_a_wrong_answer() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool, metrics) = store(&db, 1).await;
    let hash = store.put(Bytes::from_static(b"x")).await.unwrap();
    pool.close().await;
    assert_eq!(store.get(&hash).await, Err(BlobError::Unavailable));
    assert_eq!(store.get_many(&[hash]).await, Err(BlobError::Unavailable));
    assert_eq!(
        store.put(Bytes::from_static(b"y")).await,
        Err(BlobError::Unavailable)
    );
    assert_eq!(store.cache().len(), 0, "a failure caches nothing");
    assert!(metrics.store_errors() >= 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stampede_for_one_cold_blob_reads_the_database_once() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, _, metrics) = store(&db, 64).await;
    let store = Arc::new(store);
    let bytes = Bytes::from(vec![5u8; 2 * 1024 * 1024]);
    let hash = store.put(bytes.clone()).await.unwrap();
    let mut tasks = Vec::new();
    for _ in 0..64 {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move { store.get(&hash).await }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap(), Some(bytes.clone()));
    }
    assert_eq!(
        metrics.database_reads(),
        1,
        "64 users opening the same big file must cost one read of it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_read_does_not_poison_the_next_one() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, _, _) = store(&db, 64).await;
    let store = Arc::new(store);
    let bytes = Bytes::from(vec![3u8; 3 * 1024 * 1024]);
    let hash = store.put(bytes.clone()).await.unwrap();
    for _ in 0..20 {
        // Start a read and drop it at once, as a client that disconnects does.
        let s = Arc::clone(&store);
        let handle = tokio::spawn(async move { s.get(&hash).await });
        tokio::task::yield_now().await;
        handle.abort();
        let _ = handle.await;
    }
    assert_eq!(
        store.get(&hash).await.unwrap(),
        Some(bytes),
        "the next read is whole and correct"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lookup_queries_use_the_primary_key_index_and_never_scan_the_table() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    // 20,000 small blobs: enough that the planner's choice is the index's, not the table's.
    sqlx::query(
        "INSERT INTO blobs (hash, content, size) \
         SELECT sha256(c), c, length(c) FROM (SELECT convert_to('blob-' || g, 'UTF8') AS c FROM generate_series(1, 20000) g) t",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut admin = db.admin().await;
    sqlx::query("ANALYZE blobs").execute(&mut admin).await.unwrap();

    let one = hub_platform::blobs::store::GET_ONE_SQL;
    let many = hub_platform::blobs::store::GET_MANY_SQL;
    // `EXPLAIN (FORMAT JSON)` answers one `json` value; its wire form is its text.
    let one_plan: String = sqlx::query(&format!("EXPLAIN (FORMAT JSON) {one}"))
        .bind(content_hash(b"blob-77").as_bytes().as_slice())
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get_unchecked(0)
        .unwrap();
    let hashes: Vec<Vec<u8>> = (70..90)
        .map(|n| content_hash(format!("blob-{n}").as_bytes()).as_bytes().to_vec())
        .collect();
    let many_plan: String = sqlx::query(&format!("EXPLAIN (FORMAT JSON) {many}"))
        .bind(hashes)
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get_unchecked(0)
        .unwrap();
    for (what, plan) in [("get", one_plan), ("get_many", many_plan)] {
        assert!(!plan.contains("Seq Scan"), "{what} scans the table: {plan}");
        assert!(
            plan.contains("blobs_pkey"),
            "{what} does not use the primary key: {plan}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn log_capture_has_no_secret_content_or_env_value_blob_store() {
    let Some(db) = TestDb::ready().await else { return };
    let (logs, _guard) = LogCapture::install();
    let (store, pool, _) = store(&db, 1).await;
    let secret = Bytes::from_static(b"password: PLANTED-STORE-SECRET-31bd\n");
    let hash = store.put(secret).await.unwrap();
    store.get(&hash).await.unwrap();
    store.get_many(&[hash, content_hash(b"unknown")]).await.unwrap();
    pool.close().await;
    let _ = store.get(&content_hash(b"cold after close")).await;
    let _ = store
        .put(Bytes::from_static(b"token: PLANTED-STORE-SECRET-2"))
        .await;
    let text = logs.text();
    assert!(
        !text.contains("PLANTED-STORE-SECRET"),
        "the log holds blob content: {text}"
    );
    assert!(!text.contains("password"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_database_is_cut_off_by_the_query_timeout() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let metrics = BlobMetrics::default();
    let store = PgBlobStore::new(
        pool.clone(),
        BlobSettings {
            query_timeout: Duration::from_millis(200),
            ..settings(1)
        },
        metrics.clone(),
    );
    let hash = store.put(Bytes::from_static(b"slow")).await.unwrap();
    // Hold an exclusive lock on the table from another connection, so that a read of it has to wait.
    let mut admin = db.admin().await;
    sqlx::query("BEGIN").execute(&mut admin).await.unwrap();
    sqlx::query("LOCK TABLE blobs IN ACCESS EXCLUSIVE MODE")
        .execute(&mut admin)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let result = store.get(&hash).await;
    assert_eq!(result, Err(BlobError::Unavailable));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    sqlx::query("ROLLBACK").execute(&mut admin).await.unwrap();
    assert_eq!(
        store.get(&hash).await.unwrap(),
        Some(Bytes::from_static(b"slow")),
        "and it recovers"
    );
}
