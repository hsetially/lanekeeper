//! Migration 0008 (prompts 03b and 05, D47, D78, P12): storing a blob that already exists refreshes its age, so that the registry's
//! retention can use a grace period. `PgBlobStore::put` is one statement; it writes nothing for a blob touched within the touch
//! interval (not a tuple, not a row lock, no WAL), refreshes `touched_at` and nothing else of an older one, and gives a new blob its
//! default. Reads never touch. The cache cannot make the refresh unreachable, because `put` never reads it. All of it runs on a real
//! Postgres 16 as the app role, so the column-level grant of 0008 is part of what is tested.
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
use hub_platform::blobs::store::{
    BlobSettings, MAX_PUT_TO_REFERENCE, MIN_RETENTION_GRACE, PUT_SQL, PgBlobStore, TOUCH_INTERVAL,
};
use ports::{BlobStore, content_hash};
use sqlx::{PgPool, Row};
use support::log_capture::LogCapture;
use support::pg::TestDb;

/// Half of the touch interval: a blob stored this long ago is "recent" for `put`.
const WITHIN_THE_INTERVAL_SECS: f64 = 1800.0;
/// A retention-grade age: three days, past the 48-hour grace period.
const THREE_DAYS_SECS: f64 = 3.0 * 24.0 * 3600.0;

async fn store(db: &TestDb) -> (Arc<PgBlobStore>, PgPool) {
    let pool = db.app_pool().await;
    let store = PgBlobStore::new(pool.clone(), BlobSettings::default(), BlobMetrics::default());
    (Arc::new(store), pool)
}

/// What the table says about one blob: the age, the first-stored time, the tuple's identity and lock state.
#[derive(Debug, PartialEq, Eq)]
struct BlobRow {
    touched_at: String,
    created_at: String,
    content: Vec<u8>,
    size: i64,
    xmin: String,
    xmax: String,
    ctid: String,
}

async fn row(pool: &PgPool, hash: &ContentHash) -> Option<BlobRow> {
    sqlx::query(
        "SELECT touched_at::text AS touched_at, created_at::text AS created_at, content::bytea AS content, size, \
                xmin::text AS xmin, xmax::text AS xmax, ctid::text AS ctid FROM blobs WHERE hash = $1::bytea",
    )
    .bind(hash.as_bytes().as_slice())
    .fetch_optional(pool)
    .await
    .unwrap()
    .map(|r| BlobRow {
        touched_at: r.get("touched_at"),
        created_at: r.get("created_at"),
        content: r.get("content"),
        size: r.get("size"),
        xmin: r.get("xmin"),
        xmax: r.get("xmax"),
        ctid: r.get("ctid"),
    })
}

/// Make a blob `secs` old, as the app role can (its one UPDATE is on `touched_at`).
async fn age(pool: &PgPool, hash: &ContentHash, secs: f64) {
    let aged = sqlx::query(
        "UPDATE blobs SET touched_at = now() - make_interval(secs => $2::double precision) WHERE hash = $1::bytea",
    )
    .bind(hash.as_bytes().as_slice())
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(aged.rows_affected(), 1);
}

/// Seconds since `touched_at`, by the server's clock.
async fn touched_secs_ago(pool: &PgPool, hash: &ContentHash) -> f64 {
    sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - touched_at)::double precision FROM blobs WHERE hash = $1::bytea",
    )
    .bind(hash.as_bytes().as_slice())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_blob_is_touched_when_it_is_stored() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let hash = store.put(Bytes::from_static(b"key: value\n")).await.unwrap();
    let stored = row(&pool, &hash).await.unwrap();
    assert_eq!(
        stored.touched_at, stored.created_at,
        "a new blob counts as touched when it is stored"
    );
    assert!(touched_secs_ago(&pool, &hash).await < 60.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_re_put_of_an_aged_blob_refreshes_touched_at_and_nothing_else() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let bytes = Bytes::from_static(b"reverted: to an old version\r\n");
    let hash = store.put(bytes.clone()).await.unwrap();
    age(&pool, &hash, THREE_DAYS_SECS).await;
    let before = row(&pool, &hash).await.unwrap();
    assert!(touched_secs_ago(&pool, &hash).await > THREE_DAYS_SECS - 60.0);

    assert_eq!(store.put(bytes.clone()).await.unwrap(), hash);

    let after = row(&pool, &hash).await.unwrap();
    assert!(
        touched_secs_ago(&pool, &hash).await < 60.0,
        "storing the content again refreshes its age"
    );
    assert_ne!(after.touched_at, before.touched_at);
    assert_eq!(
        after.created_at, before.created_at,
        "first stored is not rewritten"
    );
    assert_eq!(after.content, before.content, "the bytes are not rewritten");
    assert_eq!(after.content, bytes.to_vec());
    assert_eq!(after.size, before.size);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "stored once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_re_put_within_the_interval_writes_nothing_not_even_a_row_lock() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let bytes = Bytes::from_static(b"hot: blob\n");
    let hash = store.put(bytes.clone()).await.unwrap();
    age(&pool, &hash, WITHIN_THE_INTERVAL_SECS).await;
    let before = row(&pool, &hash).await.unwrap();
    assert_eq!(before.xmax, "0", "no one holds or held a lock on the row");

    for _ in 0..20 {
        assert_eq!(store.put(bytes.clone()).await.unwrap(), hash);
    }

    // The same value, the same tuple (no new version: xmin and ctid), and no lock on it (xmax is 0: a lock taken by a
    // statement that updates nothing would set it, as `ON CONFLICT DO UPDATE ... WHERE false` does).
    assert_eq!(row(&pool, &hash).await.unwrap(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cached_blob_that_is_put_again_still_refreshes() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let bytes = Bytes::from_static(b"cached: and put again\n");
    let hash = store.put(bytes.clone()).await.unwrap();
    assert_eq!(store.get(&hash).await.unwrap(), Some(bytes.clone()));
    assert_eq!(store.cache().len(), 1, "the blob is in the cache");
    age(&pool, &hash, THREE_DAYS_SECS).await;

    store.put(bytes).await.unwrap();

    assert!(
        touched_secs_ago(&pool, &hash).await < 60.0,
        "the cache does not stand between a put and the table: `put` never reads it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_never_touch() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let bytes = Bytes::from_static(b"read, not referenced\n");
    let hash = store.put(bytes.clone()).await.unwrap();
    let other = store.put(Bytes::from_static(b"another\n")).await.unwrap();
    age(&pool, &hash, THREE_DAYS_SECS).await;
    age(&pool, &other, THREE_DAYS_SECS).await;
    let before = (
        row(&pool, &hash).await.unwrap(),
        row(&pool, &other).await.unwrap(),
    );

    // Cold, warm and batched reads.
    assert_eq!(store.get(&hash).await.unwrap(), Some(bytes.clone()));
    assert_eq!(store.get(&hash).await.unwrap(), Some(bytes));
    assert_eq!(store.get_many(&[hash, other]).await.unwrap().len(), 2);

    assert_eq!(
        (
            row(&pool, &hash).await.unwrap(),
            row(&pool, &other).await.unwrap()
        ),
        before,
        "reading a blob is not a reference: its age is where it was"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_re_puts_of_one_aged_blob_and_one_new_blob_are_safe() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let aged = Bytes::from(vec![7u8; 5000]);
    let aged_hash = store.put(aged.clone()).await.unwrap();
    age(&pool, &aged_hash, THREE_DAYS_SECS).await;
    let fresh = Bytes::from(vec![9u8; 5000]);
    let created = content_hash(&fresh);

    let mut tasks = Vec::new();
    for _ in 0..32 {
        for bytes in [aged.clone(), fresh.clone()] {
            let store = Arc::clone(&store);
            tasks.push(tokio::spawn(async move { store.put(bytes).await }));
        }
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 2, "each blob once, however many puts met");
    assert!(
        touched_secs_ago(&pool, &aged_hash).await < 60.0,
        "the aged blob was refreshed"
    );
    assert_eq!(row(&pool, &aged_hash).await.unwrap().content, aged.to_vec());
    assert_eq!(row(&pool, &created).await.unwrap().content, fresh.to_vec());
    // The pool of 8 connections never deadlocked or timed out: every put returned Ok above.
}

/// Poll `pg_stat_activity` (a condition, not a delay) until a backend of this database is waiting on a lock in a `put`.
async fn wait_until_a_put_waits_on_a_lock(db: &TestDb) {
    let mut admin = db.admin().await;
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' \
             AND query LIKE 'INSERT INTO blobs%'",
        )
        .fetch_one(&mut admin)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the put never waited on the lock");
}

/// A collector deletes an old blob (as retention does: `touched_at` older than the grace period) and has not committed yet when the
/// content is stored again. The put waits for the delete, and when it commits the put stores the blob again: the blob exists
/// afterwards, with the bytes that were put. Whatever the collector does, a `put` that returned Ok leaves a stored blob.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_that_meets_a_delete_in_progress_leaves_the_blob_stored() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let bytes = Bytes::from_static(b"collected, then needed again\n");
    let hash = store.put(bytes.clone()).await.unwrap();
    age(&pool, &hash, THREE_DAYS_SECS).await;

    let mut collector = pool.begin().await.unwrap();
    let deleted =
        sqlx::query("DELETE FROM blobs WHERE hash = $1::bytea AND touched_at < now() - interval '48 hours'")
            .bind(hash.as_bytes().as_slice())
            .execute(&mut *collector)
            .await
            .unwrap();
    assert_eq!(deleted.rows_affected(), 1);

    let putting = {
        let (store, bytes) = (Arc::clone(&store), bytes.clone());
        tokio::spawn(async move { store.put(bytes).await })
    };
    wait_until_a_put_waits_on_a_lock(&db).await;
    assert!(!putting.is_finished(), "the put is waiting for the collector");
    collector.commit().await.unwrap();

    assert_eq!(putting.await.unwrap().unwrap(), hash);
    let stored = row(&pool, &hash).await.expect("the blob is stored again");
    assert_eq!(stored.content, bytes.to_vec());
    assert!(touched_secs_ago(&pool, &hash).await < 60.0);
}

// ---------------------------------------------------------------------------------------------------------------
// The statement itself: what it writes, how many statements `put` runs, which index it uses, and how the interval relates to the
// grace period.

/// `EXPLAIN (ANALYZE, WAL)` of the put statement for `bytes`, run in a transaction that is rolled back.
async fn explain_put(pool: &PgPool, bytes: &[u8]) -> serde_json::Value {
    let hash = content_hash(bytes);
    let mut tx = pool.begin().await.unwrap();
    let plan: String = sqlx::query(&format!("EXPLAIN (ANALYZE, WAL, FORMAT JSON) {PUT_SQL}"))
        .bind(hash.as_bytes().as_slice())
        .bind(bytes)
        .bind(i64::try_from(bytes.len()).unwrap())
        .bind(i64::try_from(TOUCH_INTERVAL.as_secs()).unwrap())
        .fetch_one(&mut *tx)
        .await
        .unwrap()
        .try_get_unchecked(0)
        .unwrap();
    tx.rollback().await.unwrap();
    serde_json::from_str(&plan).unwrap()
}

/// WAL records the statement wrote (every kind: a row, a new version, a row lock).
fn wal_records(plan: &serde_json::Value) -> i64 {
    plan[0]["Plan"]["WAL Records"]
        .as_i64()
        .expect("EXPLAIN (ANALYZE, WAL, FORMAT JSON) reports the WAL records of the statement")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_put_statement_writes_nothing_for_a_recent_blob_and_something_for_an_aged_or_a_new_one() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let recent = b"recent: blob\n".as_slice();
    let aged = b"aged: blob\n".as_slice();
    for bytes in [recent, aged] {
        store.put(Bytes::copy_from_slice(bytes)).await.unwrap();
    }
    age(&pool, &content_hash(recent), WITHIN_THE_INTERVAL_SECS).await;
    age(&pool, &content_hash(aged), THREE_DAYS_SECS).await;

    assert_eq!(
        wal_records(&explain_put(&pool, recent).await),
        0,
        "a blob touched within the interval: no new tuple, no row lock, no WAL (and so no transaction id and no commit flush)"
    );
    assert!(
        wal_records(&explain_put(&pool, aged).await) > 0,
        "a blob older than the interval is refreshed, which is a write"
    );
    assert!(
        wal_records(&explain_put(&pool, b"brand new\n").await) > 0,
        "a new blob is inserted"
    );
}

/// `put` runs one statement on every path: a new blob, a recent one and an aged one. (An extra round trip on the ingest path would
/// show here, as a second `sqlx::query` event.)
#[tokio::test(flavor = "multi_thread")]
async fn a_put_is_one_statement_whatever_the_blob() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let aged = Bytes::from_static(b"aged\n");
    let recent = Bytes::from_static(b"recent\n");
    store.put(aged.clone()).await.unwrap();
    store.put(recent.clone()).await.unwrap();
    age(&pool, &content_hash(&aged), THREE_DAYS_SECS).await;

    let (logs, _guard) = LogCapture::install();
    for bytes in [Bytes::from_static(b"new\n"), recent, aged] {
        let before = logs.text().lines().filter(|l| l.contains("sqlx::query")).count();
        store.put(bytes).await.unwrap();
        let after = logs.text().lines().filter(|l| l.contains("sqlx::query")).count();
        assert_eq!(after - before, 1, "one statement per put: {}", logs.text());
    }
    assert!(
        logs.text().contains("INSERT INTO blobs"),
        "the capture sees the statements: {}",
        logs.text()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_put_statement_reaches_the_row_by_the_primary_key_and_never_scans_the_table() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
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
    store.put(Bytes::from_static(b"blob-77")).await.unwrap();

    for bytes in [b"blob-77".as_slice(), b"blob-new-one".as_slice()] {
        let plan = explain_put(&pool, bytes).await.to_string();
        assert!(!plan.contains("Seq Scan"), "the put scans the table: {plan}");
        assert!(
            plan.contains("blobs_pkey"),
            "the put does not use the primary key: {plan}"
        );
    }
}

#[test]
fn the_touch_interval_leaves_room_in_the_approved_grace_period() {
    // The relation is also a compile-time assertion in the store; this is the public face of it, for the registry's range check.
    assert!(
        TOUCH_INTERVAL >= Duration::from_secs(60),
        "a refresh that is not rate-limited makes every re-put a write"
    );
    assert_eq!(MIN_RETENTION_GRACE, TOUCH_INTERVAL + MAX_PUT_TO_REFERENCE);
    assert!(
        MIN_RETENTION_GRACE < Duration::from_secs(48 * 3600),
        "the approved grace (48 hours) is longer than the interval plus the longest wait for the reference"
    );
    assert!(
        Duration::from_secs_f64(WITHIN_THE_INTERVAL_SECS) < TOUCH_INTERVAL,
        "the tests' 'recent' is inside the interval"
    );
}

/// Two puts of an aged blob race: the first has refreshed it and not committed when the second, whose probe still sees the old age,
/// reaches the conflict. When the first commits, the second finds the age fresh and writes nothing: the row is still the version
/// the first one wrote (same `xmin`), so a burst of puts of one old blob is one write, not one per put.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_that_loses_the_race_to_another_refresh_writes_nothing() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let bytes = Bytes::from_static(b"refreshed twice at once\n");
    let hash = store.put(bytes.clone()).await.unwrap();
    age(&pool, &hash, THREE_DAYS_SECS).await;

    let mut first = pool.begin().await.unwrap();
    sqlx::query("UPDATE blobs SET touched_at = now() WHERE hash = $1::bytea")
        .bind(hash.as_bytes().as_slice())
        .execute(&mut *first)
        .await
        .unwrap();
    let first_version: String = sqlx::query_scalar("SELECT xmin::text FROM blobs WHERE hash = $1::bytea")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&mut *first)
        .await
        .unwrap();

    let second = {
        let (store, bytes) = (Arc::clone(&store), bytes.clone());
        tokio::spawn(async move { store.put(bytes).await })
    };
    wait_until_a_put_waits_on_a_lock(&db).await;
    first.commit().await.unwrap();
    assert_eq!(second.await.unwrap().unwrap(), hash);

    let stored = row(&pool, &hash).await.unwrap();
    assert_eq!(
        stored.xmin, first_version,
        "the second put found the age already fresh and wrote nothing"
    );
    assert!(touched_secs_ago(&pool, &hash).await < 60.0);
}

// ---------------------------------------------------------------------------------------------------------------
// The contract with the collector (prompt 05, D78): what a retention statement that tests the age does with a blob that was put
// again, and with a blob that was refreshed between the collector's snapshot and its lock on the row.

/// A retention-style delete: among `$1`, the blobs older than the grace period (48 hours) that no `blob_refs` row names. The age
/// test is in the statement that deletes the row.
const RETENTION_DELETE: &str = "DELETE FROM blobs b WHERE b.hash = ANY($1::bytea[]) \
     AND b.touched_at < now() - interval '48 hours' \
     AND NOT EXISTS (SELECT 1 FROM blob_refs r WHERE r.hash = b.hash) \
     RETURNING b.hash::bytea";

#[tokio::test(flavor = "multi_thread")]
async fn a_retention_delete_spares_a_blob_that_was_put_again_and_collects_one_that_was_not() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let mut hashes = Vec::new();
    for text in ["put again\n", "never put again\n", "referenced\n"] {
        let hash = store.put(Bytes::from(text)).await.unwrap();
        age(&pool, &hash, THREE_DAYS_SECS).await;
        hashes.push(hash);
    }
    let (put_again, orphan, referenced) = (hashes[0], hashes[1], hashes[2]);
    sqlx::query("INSERT INTO blob_refs (hash, ref_kind, ref_id) VALUES ($1::bytea, 'draft', 'draft-1')")
        .bind(referenced.as_bytes().as_slice())
        .execute(&pool)
        .await
        .unwrap();

    // All three are older than the grace period. The content of the first comes back: a put refreshes it.
    store.put(Bytes::from("put again\n")).await.unwrap();

    let deleted: Vec<Vec<u8>> = sqlx::query_scalar(RETENTION_DELETE)
        .bind(hashes.iter().map(|h| h.as_bytes().to_vec()).collect::<Vec<_>>())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        deleted,
        vec![orphan.as_bytes().to_vec()],
        "only the old blob that nobody stored again and nothing references is collected"
    );
    assert!(
        row(&pool, &put_again).await.is_some(),
        "a put protects a blob for the grace period"
    );
    assert!(
        row(&pool, &referenced).await.is_some(),
        "and a reference protects it for good"
    );
    assert!(row(&pool, &orphan).await.is_none());
}

/// The key of the advisory lock that holds a collector's statement between its snapshot and its row.
const GATE: i64 = 0x6c6b_0006;

/// A collector's statement with a gate in its WHERE: it takes its snapshot, then waits for the gate before it reaches the row.
#[derive(Clone, Copy)]
enum Collector {
    /// One `DELETE` that tests the age.
    SingleDelete,
    /// `SELECT ... FOR UPDATE SKIP LOCKED` that tests the age, then a `DELETE` of what it locked, in one transaction.
    LockFirst,
}

impl Collector {
    /// Rows deleted.
    async fn run(self, pool: &PgPool, hash: ContentHash) -> u64 {
        match self {
            Collector::SingleDelete => sqlx::query(
                "DELETE FROM blobs WHERE hash = $1::bytea AND touched_at < now() - interval '48 hours' \
                 AND pg_advisory_xact_lock_shared($2)::text = ''",
            )
            .bind(hash.as_bytes().as_slice())
            .bind(GATE)
            .execute(pool)
            .await
            .unwrap()
            .rows_affected(),
            Collector::LockFirst => {
                let mut tx = pool.begin().await.unwrap();
                let locked: Vec<Vec<u8>> = sqlx::query_scalar(
                    "SELECT hash::bytea FROM blobs WHERE hash = $1::bytea AND touched_at < now() - interval '48 hours' \
                     AND pg_advisory_xact_lock_shared($2)::text = '' FOR UPDATE SKIP LOCKED",
                )
                .bind(hash.as_bytes().as_slice())
                .bind(GATE)
                .fetch_all(&mut *tx)
                .await
                .unwrap();
                let deleted = sqlx::query("DELETE FROM blobs WHERE hash = ANY($1::bytea[])")
                    .bind(locked)
                    .execute(&mut *tx)
                    .await
                    .unwrap()
                    .rows_affected();
                tx.commit().await.unwrap();
                deleted
            }
        }
    }
}

/// Poll `pg_stat_activity` (a condition, not a delay) until a backend is waiting on an advisory lock: the collector's statement
/// has its snapshot and is held in front of the row.
async fn wait_until_the_collector_waits_at_the_gate(db: &TestDb) {
    let mut admin = db.admin().await;
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() \
             AND wait_event_type = 'Lock' AND wait_event = 'advisory'",
        )
        .fetch_one(&mut admin)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the collector never reached the gate");
}

/// Store `content`, age it three days, hold a collector at the gate (after its snapshot, before its row), and, if `put_between`,
/// store the content again and let that commit before the collector is released. Returns the rows the collector deleted and whether
/// the blob is still stored.
async fn interleave(
    db: &TestDb,
    store: &Arc<PgBlobStore>,
    pool: &PgPool,
    collector: Collector,
    content: &'static str,
    put_between: bool,
) -> (u64, bool) {
    let hash = store.put(Bytes::from(content)).await.unwrap();
    age(pool, &hash, THREE_DAYS_SECS).await;
    let mut gate = db.admin().await;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(GATE)
        .execute(&mut gate)
        .await
        .unwrap();
    let running = {
        let pool = pool.clone();
        tokio::spawn(async move { collector.run(&pool, hash).await })
    };
    wait_until_the_collector_waits_at_the_gate(db).await;
    assert!(!running.is_finished(), "the collector is held at the gate");
    if put_between {
        // Autocommit: the refresh is committed when this returns, after the collector's snapshot.
        store.put(Bytes::from(content)).await.unwrap();
        assert!(touched_secs_ago(pool, &hash).await < 60.0);
    }
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(GATE)
        .execute(&mut gate)
        .await
        .unwrap();
    let deleted = running.await.unwrap();
    (deleted, row(pool, &hash).await.is_some())
}

/// The refresh commits after the collector's snapshot (which still shows the old age) and before the collector reaches the row.
/// The collector then finds the row changed and tests its own condition again on the new version, which is fresh: it skips the
/// blob. The control runs the same interleaving with no put in between, and the collector collects the blob, so it is the
/// refresh, and not the order of events, that protects it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_collector_delete_that_reaches_the_row_after_a_refresh_commits_skips_it() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let control = interleave(
        &db,
        &store,
        &pool,
        Collector::SingleDelete,
        "control: collected\n",
        false,
    )
    .await;
    assert_eq!(
        control,
        (1, false),
        "with no put in between the aged blob is collected"
    );
    let refreshed = interleave(
        &db,
        &store,
        &pool,
        Collector::SingleDelete,
        "put again in between\n",
        true,
    )
    .await;
    assert_eq!(
        refreshed,
        (0, true),
        "a refresh that commits before the delete reaches the row wins"
    );
}

/// The same for a collector that locks first (`FOR UPDATE SKIP LOCKED` with the age test) and deletes what it locked in the same
/// transaction: a refresh that commits after its snapshot and before its lock leaves nothing to lock, so nothing is deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_collector_that_locks_first_skips_a_row_refreshed_after_its_snapshot() {
    let Some(db) = TestDb::ready().await else { return };
    let (store, pool) = store(&db).await;
    let control = interleave(
        &db,
        &store,
        &pool,
        Collector::LockFirst,
        "control: locked and collected\n",
        false,
    )
    .await;
    assert_eq!(
        control,
        (1, false),
        "with no put in between the aged blob is locked and collected"
    );
    let refreshed = interleave(
        &db,
        &store,
        &pool,
        Collector::LockFirst,
        "put again before the lock\n",
        true,
    )
    .await;
    assert_eq!(
        refreshed,
        (0, true),
        "a refresh that commits before the lock wins"
    );
}
