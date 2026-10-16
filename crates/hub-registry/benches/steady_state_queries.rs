//! P12, no steady-state query takes longer than 50 ms (`P12.steady_state_query_ms`, `hub-registry/steady_state_queries`, a harness,
//! p95, 50 ms): every statement of `hub_registry::read` and `hub_registry::blob_policy`, run on the database of the read bed (40
//! swimlanes of about 2,000 files, about 23,000 blobs, every setting of every structured file indexed, every swimlane computed), with
//! the binds a real request has, twenty times each after a first run that is not counted. **Every statement of `read::` is either a named
//! one that is timed here, or an inline one that is on an allow-list, each a bounded lookup with its reason.** The named statements are the
//! ones the service runs (`pub const` and `pub fn` of `hub_registry::read` and `hub_registry::blob_policy`), not copies.
//!
//! **The list is not this file's.** It is `support::read_statements::statements`, shared with `tests/read_statements.rs`, which holds it in
//! the normal gate: every named SQL of the read modules is in it or on an allow-list with a reason (found by a scan of the source), every
//! inline statement (`sqlx::query("...")` and its kin, at its call site) is on an allow-list with the reason it is a bounded lookup that
//! needs no timing of its own (a key or prefix lookup, or the read of the one-row-per-swimlane table under a `LIMIT`; found by a second
//! scan), every entry of the list has as many binds as its highest placeholder, and every entry runs once at a small scale. So a statement
//! added to the read API cannot be left out of this benchmark without a reason on record, and a bind that does not fit its statement fails
//! in `cargo test` and not here at the gate (the sixth parameter of the drift list did, and three statements were never timed).
//!
//! **The worst cases the API allows** are in the list: the drift list by path sorts the whole matching set before it takes a page, so it
//! is asked of a swimlane in which every file is drifted (all of its 2,000 files match) for the first page and for a page in the middle;
//! and the blob policy's lookup of paths is asked of a content that the limit and one more observations have had, the most it ever reads.
//!
//! The samples are every execution of every query. The budget's statistic is the 95th percentile of those, and the benchmark is
//! stricter than that: it fails if the 95th percentile of any one query is over 50 ms, so that a slow query cannot hide among the quick
//! ones. Every query's own 95th percentile is printed, slowest first.
//!
//! After the budget's samples are taken, the drift list by path is also asked of a swimlane ten times the design size (20,000 files, all
//! drifted), and that is printed as information. It is not part of the budget's samples.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod common;

use std::time::Instant;

use common::reads::ReadBed;
use common::support::perf::write_result;
use common::support::read_statements::{ReadStatement, read_world, statements};
use hub_registry::RegistryConfig;
use hub_registry::blob_policy::HASH_PATHS_LIMIT;
use hub_registry::read::settings::ROWS_PER_STATEMENT;
use sqlx::PgPool;

const REPEATS: usize = 20;
const LIMIT_MS: f64 = 50.0;

/// Twenty timed runs of `statement` after one that is not counted.
async fn time(pool: &PgPool, statement: &ReadStatement) -> Vec<f64> {
    let _ = statement.query().fetch_all(pool).await.unwrap();
    let mut ms = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        let rows = statement.query().fetch_all(pool).await.unwrap();
        ms.push(started.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(rows);
    }
    ms
}

fn p95_and_max(ms: &[f64]) -> (f64, f64) {
    let mut sorted = ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    (common::percentile(&sorted, 0.95), *sorted.last().unwrap())
}

#[tokio::main]
async fn main() {
    let Some(bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/steady_state_queries: SKIPPED (no database)");
        return;
    };
    let pool = bed.bed.rig.pool.clone();
    // The swimlane in which every file is drifted. The owner makes it so (the app role may not rewrite the states, and a hub in which the
    // pipeline had found a swimlane all `nfs_ahead` would have the same rows), and analyses the table as autovacuum would.
    let drift_lane = bed.bed.lanes[20].id.to_string();
    let mut admin = bed.bed.rig.db.admin().await;
    sqlx::query("UPDATE file_state SET state = 'nfs_ahead' WHERE swimlane_id = $1")
        .bind(&drift_lane)
        .execute(&mut admin)
        .await
        .unwrap();
    sqlx::raw_sql("ANALYZE file_state")
        .execute(&mut admin)
        .await
        .unwrap();
    let in_sync: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_state WHERE swimlane_id = $1 AND state = 'in_sync'")
            .bind(&drift_lane)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(in_sync, 0, "every file of the drifted swimlane is drifted");
    let drifted: i64 = sqlx::query_scalar("SELECT count(*) FROM file_state WHERE swimlane_id = $1")
        .bind(&drift_lane)
        .fetch_one(&pool)
        .await
        .unwrap();
    // The content that the limit and one more observations have had, added by the app role as ingestion would.
    let world = read_world(&pool, Some(&drift_lane), HASH_PATHS_LIMIT + 1).await;
    // The bed is the design scale, not a toy: the statements that read the settings of blobs have blobs of the size they were budgeted for.
    assert!(
        world.largest.len() == ROWS_PER_STATEMENT && world.typical.len() == ROWS_PER_STATEMENT,
        "the bed has the largest and the typical blobs"
    );
    assert!(
        world.lanes.len() == 40 && drifted >= 1_500,
        "{} swimlanes, {drifted} drifted files",
        world.lanes.len()
    );
    let list = statements(&world);

    let mut results: Vec<(String, Vec<f64>)> = Vec::new();
    for statement in &list {
        let ms = time(&pool, statement).await;
        results.push((statement.name.to_owned(), ms));
    }

    let mut table: Vec<(&str, f64, f64)> = results
        .iter()
        .map(|(name, ms)| {
            let (p95, max) = p95_and_max(ms);
            (name.as_str(), p95, max)
        })
        .collect();
    table.sort_by(|a, b| b.1.total_cmp(&a.1));
    eprintln!(
        "hub-registry/steady_state_queries: {} queries, {REPEATS} runs each; by p95, slowest first:",
        table.len()
    );
    for (name, p95, max) in &table {
        eprintln!("  {name}: p95 {p95:.2} ms, max {max:.2} ms");
    }
    let all: Vec<f64> = results.iter().flat_map(|(_, ms)| ms.iter().copied()).collect();

    // Beyond the design scale, for information: the drift list by path of a swimlane ten times as large, all of it drifted. Not in `all`.
    sqlx::query(
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, nfs_hash, state, denied) \
         SELECT swimlane_id, path || '.copy' || k, logical_file || '.copy' || k, kind, class, nfs_hash, 'nfs_ahead', false \
         FROM file_state, generate_series(1, 9) k WHERE swimlane_id = $1",
    )
    .bind(&drift_lane)
    .execute(&mut admin)
    .await
    .unwrap();
    sqlx::raw_sql("ANALYZE file_state")
        .execute(&mut admin)
        .await
        .unwrap();
    let big: i64 = sqlx::query_scalar("SELECT count(*) FROM file_state WHERE swimlane_id = $1")
        .bind(&drift_lane)
        .fetch_one(&pool)
        .await
        .unwrap();
    for statement in list.iter().filter(|s| s.name.starts_with("drift_by_path")) {
        let ms = time(&pool, statement).await;
        let (p95, max) = p95_and_max(&ms);
        eprintln!(
            "  (information, beyond the design scale) {} on {big} drifted files: p95 {p95:.2} ms, max {max:.2} ms",
            statement.name
        );
    }

    let slow: Vec<_> = table.iter().filter(|(_, p95, _)| *p95 > LIMIT_MS).collect();
    assert!(
        slow.is_empty(),
        "queries whose p95 is over {LIMIT_MS} ms: {slow:?}"
    );
    let path = write_result("hub-registry/steady_state_queries", "ms", &all);
    eprintln!("hub-registry/steady_state_queries: {}", path.display());
}
