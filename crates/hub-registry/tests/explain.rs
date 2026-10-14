//! P12: `EXPLAIN (FORMAT JSON)` of the registry's queries on a database seeded to design scale (40 swimlanes of 2,000 files, 80,000
//! distinct blobs) and analysed. A plan is kept as its node types, relations and index names (never costs, which change with the
//! machine), and no query may read a large table with a sequential scan. Each task that adds a query adds its snapshot here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use hub_registry::index::settings::{MARKED_SQL, UNINDEXED_SQL};
use hub_registry::index::tree_hashes::{LOAD_DIRS_SQL, LOAD_FILES_SQL};
use serde_json::Value;
use sqlx::PgPool;
use support::pg::TestDb;

/// Tables that hold thousands of rows at design scale. A sequential scan of one of these is a failure.
const LARGE: [&str; 9] = [
    "file_state",
    "file_observations",
    "settings_index",
    "settings_index_blobs",
    "blobs",
    "effective_files",
    "effective_tree_hashes",
    "git_tree_index",
    "baselines",
];

async fn seeded() -> Option<(TestDb, PgPool)> {
    let db = TestDb::ready().await?;
    let pool = db.app_pool().await;
    let mut admin = db.admin().await;
    for sql in [
        "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1')",
        "INSERT INTO swimlanes (id, project_id, display_name) SELECT 'lane' || n, 'p1', 'lane' FROM generate_series(1, 40) n",
        // 80,000 distinct blobs: the content is the name of its number.
        "INSERT INTO blobs (hash, content, size) \
         SELECT sha256(c), c, length(c) FROM (SELECT ('blob-' || i)::bytea AS c FROM generate_series(1, 80000) i) x",
        // 40 swimlanes of 2,000 files in 50 directories: 60 % structured, 20 % text, 20 % binary.
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, nfs_hash, state, denied) \
         SELECT 'lane' || l, 'dir' || (f % 50) || '/file' || f || '.yml', 'dir' || (f % 50) || '/file' || f || '.yml', 'base', \
                CASE WHEN f % 10 < 6 THEN 'structured' WHEN f % 10 < 8 THEN 'text' ELSE 'binary' END, \
                sha256(('blob-' || ((l - 1) * 2000 + f))::bytea), 'in_sync', false \
         FROM generate_series(1, 40) l, generate_series(1, 2000) f",
        // 19 of 20 structured blobs are indexed already: the sweeper's steady state is a few that are not.
        "INSERT INTO settings_index_blobs (blob_hash, setting_count) \
         SELECT nfs_hash, 3 FROM file_state WHERE class = 'structured' AND hashtext(path || swimlane_id) % 20 <> 0",
        "INSERT INTO effective_tree_hashes (swimlane_id, dir_path, hash) \
         SELECT 'lane' || l, CASE WHEN d = 0 THEN '' ELSE 'dir' || (d - 1) END, sha256(('d' || l || '-' || d)::bytea) \
         FROM generate_series(1, 40) l, generate_series(0, 50) d",
        "INSERT INTO effective_files (swimlane_id, logical_file, hash) \
         SELECT swimlane_id, logical_file, nfs_hash FROM file_state",
        "ANALYZE",
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    Some((db, pool))
}

/// The nodes of a plan, depth first, as `{node, relation?, index?}`.
fn nodes(plan: &Value, out: &mut Vec<Value>) {
    let mut node = serde_json::Map::new();
    node.insert("node".into(), plan["Node Type"].clone());
    if let Some(relation) = plan.get("Relation Name") {
        node.insert("relation".into(), relation.clone());
    }
    if let Some(index) = plan.get("Index Name") {
        node.insert("index".into(), index.clone());
    }
    out.push(Value::Object(node));
    if let Some(children) = plan.get("Plans").and_then(Value::as_array) {
        for child in children {
            nodes(child, out);
        }
    }
}

/// A bound value of a statement under EXPLAIN.
enum Arg<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
    Int(i64),
    ByteaArray(Vec<Vec<u8>>),
}

async fn explain(pool: &PgPool, sql: &str, args: &[Arg<'_>]) -> Value {
    let text = format!("EXPLAIN (FORMAT JSON) {sql}");
    let mut query = sqlx::query_scalar::<_, Value>(&text);
    for arg in args {
        query = match arg {
            Arg::Text(t) => query.bind(*t),
            Arg::Bytes(b) => query.bind(*b),
            Arg::Int(i) => query.bind(*i),
            Arg::ByteaArray(a) => query.bind(a.clone()),
        };
    }
    let row: Value = query.fetch_one(pool).await.unwrap();
    let mut out = Vec::new();
    nodes(&row[0]["Plan"], &mut out);
    Value::Array(out)
}

fn no_large_sequential_scan(name: &str, plan: &Value) {
    for node in plan.as_array().unwrap() {
        let seq = node["node"] == "Seq Scan" || node["node"] == "Parallel Seq Scan";
        let large = node["relation"].as_str().is_some_and(|r| LARGE.contains(&r));
        assert!(
            !(seq && large),
            "{name}: a sequential scan of {}: {plan:#}",
            node["relation"]
        );
    }
}

#[tokio::test]
async fn hot_queries_have_no_sequential_scan_on_large_tables() {
    let Some((_db, pool)) = seeded().await else { return };

    // T2. The sweeper: the current structured blobs of one swimlane that have no marker, in hash order, after a given hash.
    let unindexed = explain(
        &pool,
        UNINDEXED_SQL,
        &[Arg::Text("lane7"), Arg::Bytes(&[]), Arg::Int(32)],
    )
    .await;
    no_large_sequential_scan("unindexed blobs", &unindexed);
    insta::assert_json_snapshot!("unindexed_blobs_of_a_swimlane", unindexed);

    // T2. The markers of a batch of blobs.
    let batch: Vec<Vec<u8>> = (1..=32)
        .map(|i| {
            ports::content_hash(format!("blob-{i}").as_bytes())
                .as_bytes()
                .to_vec()
        })
        .collect();
    let marked = explain(&pool, MARKED_SQL, &[Arg::ByteaArray(batch)]).await;
    no_large_sequential_scan("markers", &marked);
    insta::assert_json_snapshot!("markers_of_a_batch", marked);

    // T2. The stored tree of a swimlane, read by the sync and the nightly repair.
    let dirs = explain(&pool, LOAD_DIRS_SQL, &[Arg::Text("lane7"), Arg::Int(200_000)]).await;
    no_large_sequential_scan("stored directories", &dirs);
    insta::assert_json_snapshot!("stored_directories_of_a_swimlane", dirs);
    let files = explain(&pool, LOAD_FILES_SQL, &[Arg::Text("lane7"), Arg::Int(200_000)]).await;
    no_large_sequential_scan("stored files", &files);
    insta::assert_json_snapshot!("stored_files_of_a_swimlane", files);
}

#[tokio::test]
async fn the_sweeper_finds_the_unindexed_few_among_many() {
    // The plan is only worth its snapshot if the query does what it should at this scale: 1 in 20 structured blobs is unmarked.
    let Some((_db, pool)) = seeded().await else { return };
    let rows: Vec<(Vec<u8>, String)> = sqlx::query_as(UNINDEXED_SQL)
        .bind("lane7")
        .bind(&[][..] as &[u8])
        .bind(10_000_i64)
        .fetch_all(&pool)
        .await
        .unwrap();
    let structured: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM file_state WHERE swimlane_id = 'lane7' AND class = 'structured'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let unmarked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM file_state f WHERE swimlane_id = 'lane7' AND class = 'structured' \
         AND NOT EXISTS (SELECT 1 FROM settings_index_blobs b WHERE b.blob_hash = f.nfs_hash)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(structured, 1200);
    assert!(unmarked > 0 && unmarked < 200, "{unmarked}");
    assert_eq!(
        i64::try_from(rows.len()).unwrap(),
        unmarked,
        "one row per unmarked blob (each blob is in one file here)"
    );
    assert!(rows.windows(2).all(|w| w[0].0 < w[1].0), "in hash order");
}
