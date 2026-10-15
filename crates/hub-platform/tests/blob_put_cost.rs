//! A measurement, not a check (migration 0008): what `PgBlobStore::put` costs for a new blob, for a blob that was stored a moment ago
//! and for one stored days ago, with the statement before 0008 (`ON CONFLICT DO NOTHING`), the obvious refresh (`ON CONFLICT DO
//! UPDATE ... WHERE`) and the statement `put` runs now. It prints one table; nothing is asserted about speed. Run it with
//!
//! `LK_TEST_PG_ADMIN_URL=... cargo test -p hub-platform --test blob_put_cost -- --ignored --nocapture`
//!
//! in a debug build over a real Postgres 16 as the app role: the absolute numbers are a debug build's (hashing, the driver and the
//! test itself are slow in it), and the cluster is shared, so the WAL totals include whatever else it wrote. What carries over to a
//! release build is the difference between the rows: the statement time on the server and the WAL a statement writes. "Wall" is
//! one autocommit statement at a time, commit flush included.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::cast_precision_loss
)]

mod support;

use std::fmt::Write as _;
use std::time::Instant;

use bytes::Bytes;
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PUT_SQL, PgBlobStore};
use ports::{BlobStore, content_hash};
use sqlx::PgPool;
use support::pg::TestDb;

const N: usize = 1000;

/// A row of the report: its name, the statement for the wall-clock column (`None`: `PgBlobStore::put` itself), and the statement for
/// the server-side columns with whether it binds the touch interval.
type Variant = (&'static str, Option<&'static str>, Option<(&'static str, bool)>);

/// The statement before 0008.
const BASELINE: &str =
    "INSERT INTO blobs (hash, content, size) VALUES ($1::bytea, $2::bytea, $3) ON CONFLICT (hash) DO NOTHING";
/// The obvious refresh: the conflict path takes the row lock first and then tests the age.
const NAIVE: &str = "INSERT INTO blobs (hash, content, size) VALUES ($1::bytea, $2::bytea, $3) \
                     ON CONFLICT (hash) DO UPDATE SET touched_at = now() WHERE blobs.touched_at < now() - interval '1 hour'";

/// About 8 KiB of YAML-like text, different for every `(tag, i)`.
fn blob(tag: &str, i: usize) -> Bytes {
    let mut text = String::with_capacity(8300);
    let mut line = 0usize;
    while text.len() < 8192 {
        let _ = write!(
            text,
            "{tag}-{i}:\n  key-{line}: value-{}\n  host: svc-{line}.example.test\n",
            line * 7 + i
        );
        line += 1;
    }
    Bytes::from(text)
}

fn set(tag: &str) -> Vec<Bytes> {
    (0..N).map(|i| blob(tag, i)).collect()
}

async fn insert_plain(pool: &PgPool, blobs: &[Bytes], aged: bool) {
    for bytes in blobs {
        sqlx::query("INSERT INTO blobs (hash, content, size) VALUES ($1::bytea, $2::bytea, $3)")
            .bind(content_hash(bytes).as_bytes().as_slice())
            .bind(bytes.as_ref())
            .bind(i64::try_from(bytes.len()).unwrap())
            .execute(pool)
            .await
            .unwrap();
    }
    if aged {
        sqlx::query("UPDATE blobs SET touched_at = now() - interval '3 days' WHERE touched_at > now() - interval '1 hour'")
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn wal_lsn(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wal_bytes_between(pool: &PgPool, from: &str, to: &str) -> f64 {
    sqlx::query_scalar("SELECT pg_wal_lsn_diff($2::pg_lsn, $1::pg_lsn)::double precision")
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// One autocommit statement at a time, as `put` runs them. Returns (wall microseconds per put, WAL bytes per put).
async fn wall(pool: &PgPool, blobs: &[Bytes], sql: Option<&str>, store: &PgBlobStore) -> (f64, f64) {
    let before = wal_lsn(pool).await;
    let started = Instant::now();
    for bytes in blobs {
        match sql {
            Some(sql) => {
                let hash = content_hash(bytes);
                sqlx::query(sql)
                    .bind(hash.as_bytes().as_slice())
                    .bind(bytes.as_ref())
                    .bind(i64::try_from(bytes.len()).unwrap())
                    .execute(pool)
                    .await
                    .unwrap();
            }
            None => {
                store.put(bytes.clone()).await.unwrap();
            }
        }
    }
    let elapsed = started.elapsed();
    let after = wal_lsn(pool).await;
    (
        elapsed.as_micros() as f64 / blobs.len() as f64,
        wal_bytes_between(pool, &before, &after).await / blobs.len() as f64,
    )
}

/// The server's own account of the statements: planning time and execution time (a prepared statement that is reused plans less
/// often than this does, so the execution time is the steady-state cost), and the WAL the statement wrote, summed over the blobs, in
/// one transaction that is rolled back. Returns per statement: (planning us, execution us, WAL records, WAL bytes).
async fn server(pool: &PgPool, blobs: &[Bytes], sql: &str, bind_interval: bool) -> (f64, f64, f64, f64) {
    let mut tx = pool.begin().await.unwrap();
    let (mut planning, mut execution, mut records, mut bytes_written) = (0.0, 0.0, 0.0, 0.0);
    for bytes in blobs {
        let hash = content_hash(bytes);
        let explain = format!("EXPLAIN (ANALYZE, WAL, FORMAT JSON) {sql}");
        let mut query = sqlx::query(&explain)
            .bind(hash.as_bytes().as_slice())
            .bind(bytes.as_ref())
            .bind(i64::try_from(bytes.len()).unwrap());
        if bind_interval {
            query = query.bind(3600_i64);
        }
        let plan: String =
            sqlx::Row::try_get_unchecked(&query.fetch_one(&mut *tx).await.unwrap(), 0).unwrap();
        let plan: serde_json::Value = serde_json::from_str(&plan).unwrap();
        planning += plan[0]["Planning Time"].as_f64().unwrap();
        execution += plan[0]["Execution Time"].as_f64().unwrap();
        records += plan[0]["Plan"]["WAL Records"].as_f64().unwrap_or(0.0);
        bytes_written += plan[0]["Plan"]["WAL Bytes"].as_f64().unwrap_or(0.0);
    }
    tx.rollback().await.unwrap();
    let n = blobs.len() as f64;
    (
        planning * 1000.0 / n,
        execution * 1000.0 / n,
        records / n,
        bytes_written / n,
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: run with --ignored --nocapture"]
async fn put_cost_report() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let store = PgBlobStore::new(pool.clone(), BlobSettings::default(), BlobMetrics::default());
    let rows: Vec<Variant> = vec![
        (
            "before 0008 (DO NOTHING)",
            Some(BASELINE),
            Some((BASELINE, false)),
        ),
        ("DO UPDATE ... WHERE (naive)", Some(NAIVE), Some((NAIVE, false))),
        ("PgBlobStore::put (now)", None, GATED),
    ];
    eprintln!(
        "N = {N} puts per cell, blobs of about 8 KiB; debug build; shared cluster (WAL totals are indicative)"
    );
    eprintln!(
        "{:<28} {:<10} {:>10} {:>12} | {:>8} {:>8} {:>9} {:>9}",
        "statement", "case", "wall us", "WAL B (all)", "plan us", "exec us", "WAL rec", "WAL B"
    );
    for (name, wall_sql, server_sql) in rows {
        for case in ["new", "recent", "aged"] {
            let tag = format!("{name}/{case}");
            let timed = set(&format!("w:{tag}"));
            let explained = set(&format!("s:{tag}"));
            match case {
                "recent" => {
                    insert_plain(&pool, &timed, false).await;
                    insert_plain(&pool, &explained, false).await;
                }
                "aged" => {
                    insert_plain(&pool, &timed, true).await;
                    insert_plain(&pool, &explained, true).await;
                }
                _ => {}
            }
            let (wall_us, wal_all) = wall(&pool, &timed, wall_sql, &store).await;
            let server = match server_sql {
                Some((sql, interval)) => Some(server(&pool, &explained, sql, interval).await),
                None => None,
            };
            let server = server.map_or_else(
                || format!("{:>8} {:>8} {:>9} {:>9}", "-", "-", "-", "-"),
                |(plan, exec, rec, bytes)| format!("{plan:>8.0} {exec:>8.0} {rec:>9.2} {bytes:>9.0}"),
            );
            eprintln!("{name:<28} {case:<10} {wall_us:>10.0} {wal_all:>12.0} | {server}");
        }
    }
}

/// The statement `put` runs now, for the server-side columns (it binds the touch interval too).
const GATED: Option<(&str, bool)> = Some((PUT_SQL, true));
