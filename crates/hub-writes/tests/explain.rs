//! P12: the hot statements of the write path use their indexes. `EXPLAIN (GENERIC_PLAN)` shows the plan a prepared statement
//! has for any parameter, and `enable_seqscan = off` makes the planner show whether an index could be used at all on a table
//! that is too small to prefer it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use sqlx::{Connection, Row};
use support::pg::TestDb;

async fn plan(db: &TestDb, sql: &str) -> String {
    let mut conn = db.app_pool(1).await.acquire().await.unwrap().detach();
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut conn)
        .await
        .unwrap();
    // The simple protocol: `EXPLAIN` of a statement with parameters cannot be bound.
    let rows = sqlx::raw_sql(&format!("EXPLAIN (GENERIC_PLAN, FORMAT JSON) {sql}"))
        .fetch_all(&mut conn)
        .await
        .unwrap();
    let text: Vec<String> = rows
        .iter()
        .map(|r| r.get::<serde_json::Value, _>(0).to_string())
        .collect();
    conn.close().await.unwrap();
    text.join("\n")
}

#[tokio::test]
async fn every_hot_statement_avoids_a_sequential_scan() {
    let db = db!();
    for (name, sql) in hub_writes::hot_queries() {
        let p = plan(&db, sql).await;
        assert!(!p.contains("Seq Scan"), "{name} scans a table: {p}");
    }
}

#[tokio::test]
async fn explain_idempotency_claim_uses_the_primary_key() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    let (_, lookup) = queries.iter().find(|(n, _)| *n == "idempotency_lookup").unwrap();
    assert!(plan(&db, lookup).await.contains("idempotency_keys_pkey"));
    let (_, claim) = queries.iter().find(|(n, _)| *n == "idempotency_claim").unwrap();
    assert!(
        plan(&db, claim).await.contains("idempotency_keys_pkey"),
        "the conflict arbiter is the key"
    );
}

#[tokio::test]
async fn explain_stale_claim_scan_and_purge_use_the_expiry_index() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    for name in ["idempotency_stale_claims", "idempotency_purge"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        assert!(
            plan(&db, sql).await.contains("idempotency_keys_expires_idx"),
            "{name}"
        );
    }
}

#[tokio::test]
async fn explain_the_lock_check_reads_the_swimlane_by_its_key() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    let (_, sql) = queries.iter().find(|(n, _)| *n == "swimlane_lock").unwrap();
    assert!(plan(&db, sql).await.contains("swimlanes_pkey"));
}
