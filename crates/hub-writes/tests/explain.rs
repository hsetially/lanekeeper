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

/// A draft's effects read the swimlane's Deployments through the key `(swimlane, namespace, name)`, in the order they are listed in,
/// and its severity reads the (few) rules by their key: neither is a scan, and both are bounded.
#[tokio::test]
async fn explain_a_drafts_services_and_rules_are_bounded_and_read_through_keys() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    let (_, services) = queries.iter().find(|(n, _)| *n == "draft_services").unwrap();
    let p = plan(&db, services).await;
    assert!(p.contains("services_swimlane_id_namespace_name_key"), "{p}");
    assert!(p.contains("\"Limit\""), "no LIMIT: {p}");
    let (_, rules) = queries.iter().find(|(n, _)| *n == "draft_rules").unwrap();
    let p = plan(&db, rules).await;
    assert!(p.contains("severity_rules_pkey"), "{p}");
    assert!(p.contains("\"Limit\""), "no LIMIT: {p}");
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

/// T9 (D77): the one-open-pull-request check joins the (path, hash) pairs of a request to the partial unique index made for it, and
/// the scan for jobs that need a runner reads the index of the unfinished states, a bounded page at a time.
#[tokio::test]
async fn explain_open_pr_lookup_uses_pr_links_one_open() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    let (_, sql) = queries.iter().find(|(n, _)| *n == "pr_link_open").unwrap();
    let p = plan(&db, sql).await;
    assert!(p.contains("pr_links_one_open"), "{p}");
    assert!(p.contains("\"Limit\""), "{p}");
    // The list of links is read in id order, newest first, through the primary key; the watcher finds a link by it.
    for name in ["pr_link_list", "pr_link_place"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        assert!(plan(&db, sql).await.contains("pr_links_pkey"), "{name}");
    }
}

#[tokio::test]
async fn explain_pr_job_resume_uses_pr_jobs_state_idx() {
    let db = db!();
    let queries = hub_writes::hot_queries();
    for name in ["pr_job_resume", "pr_job_active"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        let p = plan(&db, sql).await;
        assert!(p.contains("pr_jobs_state_idx"), "{name}: {p}");
        assert!(p.contains("\"Limit\""), "{name} has no LIMIT: {p}");
    }
    for name in ["pr_job_get", "pr_job_lock"] {
        let (_, sql) = queries.iter().find(|(n, _)| *n == name).unwrap();
        assert!(plan(&db, sql).await.contains("pr_jobs_pkey"), "{name}");
    }
}

/// The statements the benchmark adds in T9 run against the rows it seeds: the lookup finds a link that is there and none that is
/// not, and the scan finds exactly the unfinished jobs.
#[tokio::test]
async fn the_pull_request_probes_of_the_benchmark_read_what_it_seeds() {
    let db = db!();
    let rig = support::rig::Rig::new(db).await;
    for i in 0..40 {
        rig.swimlane(&format!("sit{i}"), true).await;
    }
    let editor = rig.user(1, domain::Role::Editor).await;
    rig.seed_pr_rows(&editor.id).await;
    assert_eq!(rig.count("pr_links").await, 20_000);
    assert_eq!(rig.count("pr_jobs").await, 5_000);
    let probe = rig.hub.probe();
    assert!(probe.open_pr_lookup(1).await);
    assert!(probe.open_pr_lookup(19_999).await);
    // The scan skips the finished jobs (they are 4,990 of the 5,000) and a job touched an hour ago needs a runner.
    assert_eq!(probe.pr_job_resume_scan().await, 10);
}
