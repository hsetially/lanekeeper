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

#[tokio::test]
async fn explain_proposal_expiry_uses_the_expiry_index() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    let (_, sql) = queries.iter().find(|(n, _)| *n == "proposal_expiry").unwrap();
    assert!(plan(&db, sql).await.contains("proposals_expiry_idx"));
    let (_, sql) = queries.iter().find(|(n, _)| *n == "draft_expiry").unwrap();
    assert!(plan(&db, sql).await.contains("drafts_expiry_idx"));
}

#[tokio::test]
async fn explain_proposal_reads_use_the_primary_key_and_the_decision_is_joined_by_its_key() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    for name in ["proposal_get", "proposal_list", "proposal_lock"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        assert!(plan(&db, sql).await.contains("proposals_pkey"), "{name}");
    }
    for name in ["proposal_get", "proposal_list"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        assert!(
            plan(&db, sql).await.contains("approvals_proposal_id_key"),
            "{name} joins the decision through its unique key"
        );
    }
    let (_, sql) = queries.iter().find(|(n, _)| *n == "draft_get").unwrap();
    assert!(plan(&db, sql).await.contains("drafts_pkey"));
}

#[tokio::test]
async fn explain_credential_reads_use_their_keys() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    for name in [
        "credential_load",
        "credential_lock_user",
        "credential_linked_login",
    ] {
        assert!(
            queries.iter().any(|(n, _)| *n == name),
            "{name} is not registered as a hot query"
        );
    }
    let (_, sql) = queries.iter().find(|(n, _)| *n == "credential_load").unwrap();
    assert!(
        plan(&db, sql)
            .await
            .contains("github_credentials_user_tid_user_oid_key"),
        "a credential is found by its owner's unique key"
    );
    for name in ["credential_lock_user", "credential_linked_login"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        assert!(plan(&db, sql).await.contains("users_pkey"), "{name}");
    }
}

/// The nightly job reads the credentials in id order, a bounded page at a time, through the primary key, and finds each user by
/// theirs: no scan of either table, however many rows there are.
#[tokio::test]
async fn explain_credential_cleanup_is_bounded() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    for name in ["credential_stale", "credential_expiring"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        let p = plan(&db, sql).await;
        assert!(p.contains("\"Limit\""), "{name} has no LIMIT: {p}");
        assert!(
            p.contains("github_credentials_pkey"),
            "{name} is not read in id order: {p}"
        );
        assert!(!p.contains("\"Sort\""), "{name} sorts the table: {p}");
    }
    let (_, sql) = queries.iter().find(|(n, _)| *n == "credential_stale").unwrap();
    assert!(
        plan(&db, sql).await.contains("users_pkey"),
        "the user is found by its key"
    );
}
