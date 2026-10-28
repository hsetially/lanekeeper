//! The statements the hub runs on every request or at every start are indexed (plan 18, T4; P12, P7): `EXPLAIN` on a database at
//! design scale shows no sequential scan on a table that grows.
//!
//! The database is the repository's test convention (D124): a Postgres 16 with the `en_US.utf8` collation, migrated by `hub migrate`
//! and used as `lanekeeper_app`, and the plans are of the statements the code runs, taken from the code where it exposes them (the
//! session lookup) and with the key predicate of the code where it does not (the user by key). The plan is snapshotted by shape: the
//! nodes, the indexes and the relations, without costs or row estimates, which move with the machine and the statistics.
//!
//! T4 holds the two statements the authentication path adds to a request (a session by its hash, a user by its key). The tasks
//! after it add theirs here: the registry's and the write paths' (T5) and the seed's (T7, `the_seed_statements_are_indexed`: the
//! lookups by key and the updates of `hub seed`, taken from the constants the code runs). The schema probe at start reads
//! `pg_catalog` only, whose size is the number of migrations and not the data's, so it has no plan to protect.
//! `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::fmt::Write as _;
use std::sync::Arc;

use hub_identity::SystemClock;
use hub_identity::session::cookie::RandomToken;
use hub_identity::session::{SessionPolicy, Sessions};
use sqlx::Executor;
use support::pki::TestCa;

/// Design scale: 100 web users (docs/performance.md), a few thousand with the ones who signed in once and never came back. The
/// session cache holds 50,000 sessions, so that is the most rows `sessions` normally holds.
const USERS: i32 = 5_000;
const SESSIONS_PER_USER: i32 = 4;
const TID: &str = "11111111-1111-4111-8111-111111111111";

/// `cond` with every quoted literal replaced by `?`: the key being looked up is a parameter of the statement, not part of its plan.
fn mask_literals(cond: &str) -> String {
    cond.split('\'')
        .enumerate()
        .map(|(n, part)| if n % 2 == 1 { "?" } else { part })
        .collect::<Vec<_>>()
        .join("")
}

/// A shape of an `EXPLAIN (FORMAT JSON)` plan: one line per node, indented by depth, naming the node, the index it scans and the
/// relation. No cost, no rows, no width.
fn shape(explain: &serde_json::Value) -> String {
    fn walk(node: &serde_json::Value, depth: usize, out: &mut String) {
        let kind = node["Node Type"].as_str().unwrap_or_default();
        let index = node["Index Name"]
            .as_str()
            .map(|i| format!(" using {i}"))
            .unwrap_or_default();
        let relation = node["Relation Name"]
            .as_str()
            .map(|r| format!(" on {r}"))
            .unwrap_or_default();
        let cond = ["Index Cond", "Hash Cond", "Join Filter", "Filter"]
            .into_iter()
            .find_map(|k| node[k].as_str().map(|c| format!(" [{k}: {}]", mask_literals(c))))
            .unwrap_or_default();
        writeln!(out, "{}{kind}{index}{relation}{cond}", "  ".repeat(depth)).unwrap();
        for child in node["Plans"].as_array().into_iter().flatten() {
            walk(child, depth + 1, out);
        }
    }
    let mut out = String::new();
    walk(&explain[0]["Plan"], 0, &mut out);
    out
}

fn scans_a_table(explain: &serde_json::Value) -> bool {
    fn walk(node: &serde_json::Value) -> bool {
        node["Node Type"].as_str().is_some_and(|t| t == "Seq Scan")
            || node["Plans"].as_array().into_iter().flatten().any(walk)
    }
    walk(&explain[0]["Plan"])
}

#[tokio::test]
async fn the_hubs_own_statements_are_indexed() {
    let ca = TestCa::new("lanekeeper-test-ca");
    let Some(pg) = tls_pg::start(&ca).await else {
        return;
    };
    pg.migrate().await;

    // The statistics a database with this many rows has, as the planner would see them after autovacuum.
    let mut admin = pg.superuser_conn().await;
    let collation: String =
        sqlx::query_scalar("SELECT datcollate FROM pg_database WHERE datname = current_database()")
            .fetch_one(&mut admin)
            .await
            .unwrap();
    assert_eq!(
        collation, "en_US.utf8",
        "the plans are those of the test database of D124"
    );
    admin
        .execute(
            sqlx::query(
                "INSERT INTO users (tid, oid, email, display_name, role, status) \
                 SELECT $1, '00000000-0000-4000-8000-' || lpad(to_hex(g + 1000), 12, '0'), 'u' || g || '@example.com', 'U ' || g, 'viewer', 'active' \
                 FROM generate_series(1, $2::int) g",
            )
            .bind(TID)
            .bind(USERS),
        )
        .await
        .unwrap();
    admin
        .execute(
            sqlx::query(
                "INSERT INTO sessions (session_hash, user_tid, user_oid, csrf_hash, expires_at) \
                 SELECT sha256(('s' || g || '-' || k)::bytea), $1, '00000000-0000-4000-8000-' || lpad(to_hex(g + 1000), 12, '0'), \
                        sha256(('c' || g || '-' || k)::bytea), now() + interval '12 hours' \
                 FROM generate_series(1, $2::int) g, generate_series(1, $3::int) k",
            )
            .bind(TID)
            .bind(USERS)
            .bind(SESSIONS_PER_USER),
        )
        .await
        .unwrap();
    admin.execute("ANALYZE users, sessions").await.unwrap();

    let pool = pg.app_pool().await;
    let mut report = String::new();

    // 1. A session by the hash of its cookie: what `AuthLayer` asks on a cache miss, once in 30 seconds per session.
    let sessions = Sessions::new(pool.clone(), Arc::new(SystemClock), &SessionPolicy::default());
    let probe = RandomToken::generate().unwrap().hash();
    let plan = sessions.store().explain_lookup(&probe).await.unwrap();
    assert!(
        !scans_a_table(&plan),
        "a session lookup must not scan a table:\n{}",
        shape(&plan)
    );
    assert!(
        plan.to_string().contains("\"Index Name\":\"sessions_pkey\""),
        "{plan}"
    );
    writeln!(report, "session by hash\n{}", shape(&plan)).unwrap();

    // 2. A user by its key: what the guard's adapter asks on a user-cache miss, and what `/me` asks.
    let plan: (serde_json::Value,) =
        sqlx::query_as("EXPLAIN (FORMAT JSON) SELECT * FROM users WHERE tid = $1 AND oid = $2")
            .bind(TID)
            .bind("00000000-0000-4000-8000-0000000003e9")
            .fetch_one(&pool)
            .await
            .unwrap();
    let plan = plan.0;
    assert!(
        !scans_a_table(&plan),
        "a user lookup must not scan a table:\n{}",
        shape(&plan)
    );
    assert!(
        plan.to_string().contains("\"Index Name\":\"users_pkey\""),
        "{plan}"
    );
    writeln!(report, "user by key\n{}", shape(&plan)).unwrap();

    insta::assert_snapshot!("the_hubs_own_statements", report);
}

/// The statements `hub seed` runs (T7, P12), at a scale no deployment reaches (5,000 swimlanes and sentinels, 10,000 tenant rows, 5,000
/// projects): the lookups of the rows a file names go by the primary key, the reads of "rows not in the file" walk the key in order and
/// stop at their limit, and the updates find their row by the key. The seed runs once, by hand or as a Job, so this is not about speed:
/// it is that no statement of it can ever be the one that reads a whole table because a table grew.
///
/// **Why sequential scans are switched off for the plans.** These tables hold tens of rows in a deployment, and at five thousand
/// rows (about thirty pages) the planner still, rightly, reads a table whole rather than look up forty keys. So the test asks the
/// question that matters: *is there an index the statement can use*. With `enable_seqscan = off` a statement that has no usable index
/// still plans a sequential scan (at a prohibitive cost), and one that has one plans the index. The snapshot is those plans.
#[tokio::test]
async fn the_seed_statements_are_indexed() {
    use hub::seed::apply;

    let ca = TestCa::new("lanekeeper-test-ca");
    let Some(pg) = tls_pg::start(&ca).await else {
        return;
    };
    pg.migrate().await;

    let mut admin = pg.superuser_conn().await;
    for sql in [
        "INSERT INTO projects (id, display_name) SELECT 'p' || g, 'P ' || g FROM generate_series(1, 5000) g",
        "INSERT INTO swimlanes (id, project_id, display_name) SELECT 'sl-' || g, 'p1', 'S ' || g FROM generate_series(1, 5000) g",
        "INSERT INTO swimlane_tenants (swimlane_id, tenant_id) SELECT 'sl-' || g, 'sit' || k FROM generate_series(1, 5000) g, generate_series(1, 2) k",
        "INSERT INTO sentinels (vm, swimlane_id, export_root) SELECT 'vm-' || g, 'sl-' || g, '/export' FROM generate_series(1, 5000) g",
        "ANALYZE projects, swimlanes, swimlane_tenants, sentinels",
    ] {
        admin.execute(sql).await.unwrap();
    }

    let pool = pg.app_pool().await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let ids = |prefix: &str| -> Vec<String> { (1..=40).map(|n| format!("{prefix}{n}")).collect() };
    let mut report = String::new();
    // A file of 40 swimlanes (the design scale) asks for the rows of its ids.
    let lookups: [(&str, &str, Vec<String>, Option<i64>); 7] = [
        ("projects by id", apply::SELECT_PROJECTS, ids("p"), None),
        ("swimlanes by id", apply::SELECT_SWIMLANES, ids("sl-"), None),
        (
            "tenants of the swimlanes",
            apply::SELECT_TENANTS,
            ids("sl-"),
            Some(12_801),
        ),
        ("sentinels by vm", apply::SELECT_SENTINELS, ids("vm-"), None),
        (
            "projects not in the file",
            apply::ABSENT_PROJECTS,
            ids("p"),
            Some(51),
        ),
        (
            "swimlanes not in the file",
            apply::ABSENT_SWIMLANES,
            ids("sl-"),
            Some(51),
        ),
        (
            "sentinels not in the file",
            apply::ABSENT_SENTINELS,
            ids("vm-"),
            Some(51),
        ),
    ];
    for (name, sql, ids, limit) in lookups {
        let explain = format!("EXPLAIN (FORMAT JSON) {sql}");
        let mut query = sqlx::query_as::<_, (serde_json::Value,)>(&explain).bind(ids);
        if let Some(limit) = limit {
            query = query.bind(limit);
        }
        let plan = query.fetch_one(&mut *tx).await.unwrap().0;
        assert!(
            !scans_a_table(&plan),
            "{name} must not scan a table:\n{}",
            shape(&plan)
        );
        writeln!(report, "{name}\n{}", shape(&plan)).unwrap();
    }
    // The writes find their row by the key.
    let writes: [(&str, &str, [&str; 2]); 3] = [
        ("project update", apply::UPDATE_PROJECT, ["p1", "P"]),
        ("swimlane update", apply::UPDATE_SWIMLANE, ["sl-1", "S"]),
        ("sentinel update", apply::UPDATE_SENTINEL, ["vm-1", "/export"]),
    ];
    for (name, sql, [key, value]) in writes {
        let explain = format!("EXPLAIN (FORMAT JSON) {sql}");
        let mut query = sqlx::query_as::<_, (serde_json::Value,)>(&explain).bind(key);
        // The updates take the key and then one value per column they may set; every other bind is NULL.
        let columns = if name == "swimlane update" {
            8
        } else if name == "sentinel update" {
            3
        } else {
            1
        };
        for position in 0..columns {
            query = query.bind(if position == 1 { Some(value) } else { None });
        }
        let plan = query.fetch_one(&mut *tx).await.unwrap().0;
        assert!(
            !scans_a_table(&plan),
            "{name} must not scan a table:\n{}",
            shape(&plan)
        );
        writeln!(report, "{name}\n{}", shape(&plan)).unwrap();
    }
    insta::assert_snapshot!("the_seed_statements", report);
}
