//! P12: `EXPLAIN (FORMAT JSON)` of the registry's queries on a database seeded to design scale (40 swimlanes of 2,000 files, 80,000
//! distinct blobs) and analysed. A plan is kept as its node types, relations and index names (never costs, which change with the
//! machine), and no query may read a large table with a sequential scan. Each task that adds a query adds its snapshot here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use hub_registry::index::settings::{MARKED_SQL, UNINDEXED_SQL};
use hub_registry::index::tree_hashes::{LOAD_DIRS_SQL, LOAD_FILES_SQL};
use hub_registry::pipeline::lane_loader::{
    BASELINES_OF_LANE_SQL, BASELINES_OF_PATHS_SQL, MARKS_OF_LANE_SQL, MARKS_OF_PATHS_SQL, ROWS_OF_LANE_SQL,
    ROWS_OF_PATHS_SQL,
};
use hub_registry::pipeline::persist::{
    DELETE_ROWS_SQL, OPEN_FINDINGS_SQL, RESOLVE_FINDINGS_SQL, UPSERT_BASELINES_SQL, UPSERT_FINDINGS_SQL,
    UPSERT_ROWS_SQL,
};
use serde_json::Value;
use sqlx::PgPool;
use support::pg::TestDb;

/// Tables that hold thousands of rows at design scale. A sequential scan of one of these is a failure.
const LARGE: [&str; 10] = [
    "findings",
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

/// The tables the seeding fills. Autovacuum is switched off for them, because a vacuum that runs between the seeding and the
/// EXPLAIN sets their visibility maps, and the planner then reads a plan with an index-only scan where it read a bitmap heap scan a
/// moment before: the plan snapshots below would be right or wrong by the timing of a background worker, as they were (one run in
/// three failed on `markers_of_a_batch`). With no vacuum the maps stay empty and the plans are those of the snapshots every time.
const SEEDED: [&str; 11] = [
    "projects",
    "swimlanes",
    "blobs",
    "file_state",
    "settings_index_blobs",
    "effective_tree_hashes",
    "effective_files",
    "file_observations",
    "baselines",
    "findings",
    "intentional_divergence",
];

async fn seeded() -> Option<(TestDb, PgPool)> {
    let db = TestDb::ready().await?;
    let pool = db.app_pool().await;
    let mut admin = db.admin().await;
    for table in SEEDED {
        sqlx::raw_sql(&format!("ALTER TABLE {table} SET (autovacuum_enabled = false)"))
            .execute(&mut admin)
            .await
            .unwrap();
    }
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
        // T3. Two observations of every file (the history keeps every version); the newest is the one the row points at, and one in ten
        // of those moved a state, so the row has a previous state.
        "INSERT INTO file_observations (swimlane_id, path, hash, source, class, observed_at, previous_state, previous_hash) \
         SELECT swimlane_id, path, nfs_hash, 'scan', class, now() - (k || ' minutes')::interval, \
                CASE WHEN k = 1 AND hashtext(path || swimlane_id) % 10 = 0 THEN 'in_sync' END, \
                CASE WHEN k = 1 AND hashtext(path || swimlane_id) % 10 = 0 THEN nfs_hash END \
         FROM file_state, generate_series(1, 2) k",
        "UPDATE file_state fs SET last_observation_id = \
           (SELECT max(o.id) FROM file_observations o WHERE o.swimlane_id = fs.swimlane_id AND o.path = fs.path)",
        // Half of the files have a baseline (an adopted swimlane); 300 findings per swimlane; 1 file in 100 is a deliberate divergence.
        "INSERT INTO baselines (swimlane_id, path, hash, repo, commit_id) \
         SELECT swimlane_id, path, nfs_hash, 'base', 'c1' FROM file_state WHERE hashtext(path || swimlane_id) % 2 = 0",
        "INSERT INTO findings (swimlane_id, kind, path, severity, message, fingerprint) \
         SELECT 'lane' || l, 'drift', 'dir' || (f % 50) || '/file' || f || '.yml', 'low', 'a finding', 'fp' || f \
         FROM generate_series(1, 40) l, generate_series(1, 300) f",
        // The users the marks name are not seeded: the foreign key is not what is measured.
        "SET session_replication_role = replica",
        "INSERT INTO intentional_divergence (swimlane_id, path, nfs_hash, git_hash, marked_by_tid, marked_by_oid) \
         SELECT swimlane_id, path, nfs_hash, nfs_hash, 't', 'o' FROM file_state WHERE hashtext(path || swimlane_id) % 100 = 0",
        "SET session_replication_role = DEFAULT",
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
    // The index an INSERT ... ON CONFLICT decides on.
    if let Some(arbiters) = plan.get("Conflict Arbiter Indexes") {
        node.insert("arbiters".into(), arbiters.clone());
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
    TextArray(Vec<String>),
    OptTextArray(Vec<Option<String>>),
    OptByteaArray(Vec<Option<Vec<u8>>>),
    BoolArray(Vec<bool>),
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
            Arg::TextArray(a) => query.bind(a.clone()),
            Arg::OptTextArray(a) => query.bind(a.clone()),
            Arg::OptByteaArray(a) => query.bind(a.clone()),
            Arg::BoolArray(a) => query.bind(a.clone()),
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

/// The first `n` paths of the swimlane `lane7` (every tenth of its files, so that they are spread over the table).
fn some_paths(n: usize) -> Vec<String> {
    (1..=n)
        .map(|i| format!("dir{}/file{}.yml", (i * 10) % 50, i * 10))
        .collect()
}

#[tokio::test]
async fn pipeline_queries_have_no_sequential_scan_on_large_tables() {
    let Some((_db, pool)) = seeded().await else { return };
    let paths = some_paths(50);
    let lanes: Vec<String> = vec!["lane7".to_owned(); paths.len()];
    let paths_or_lanes = || {
        [
            Arg::TextArray(lanes.clone()),
            Arg::TextArray(paths.clone()),
            Arg::Int(200_000),
        ]
    };

    // T3. The load of a whole swimlane (a takeover, the nightly run, a rules change): its rows with the observation they point at.
    let rows = explain(&pool, ROWS_OF_LANE_SQL, &[Arg::Text("lane7"), Arg::Int(200_001)]).await;
    no_large_sequential_scan("rows of a swimlane", &rows);
    insta::assert_json_snapshot!("pipeline_rows_of_a_swimlane", rows);
    let baselines = explain(
        &pool,
        BASELINES_OF_LANE_SQL,
        &[Arg::Text("lane7"), Arg::Int(200_001)],
    )
    .await;
    no_large_sequential_scan("baselines of a swimlane", &baselines);
    insta::assert_json_snapshot!("pipeline_baselines_of_a_swimlane", baselines);
    let marks = explain(&pool, MARKS_OF_LANE_SQL, &[Arg::Text("lane7"), Arg::Int(200_001)]).await;
    no_large_sequential_scan("marks of a swimlane", &marks);
    insta::assert_json_snapshot!("pipeline_marks_of_a_swimlane", marks);

    // T3. An incremental batch: the rows, baselines and marks of a few paths, by key.
    let rows = explain(&pool, ROWS_OF_PATHS_SQL, &paths_or_lanes()).await;
    no_large_sequential_scan("rows of paths", &rows);
    insta::assert_json_snapshot!("pipeline_rows_of_paths", rows);
    let baselines = explain(&pool, BASELINES_OF_PATHS_SQL, &paths_or_lanes()).await;
    no_large_sequential_scan("baselines of paths", &baselines);
    insta::assert_json_snapshot!("pipeline_baselines_of_paths", baselines);
    let marks = explain(&pool, MARKS_OF_PATHS_SQL, &paths_or_lanes()).await;
    no_large_sequential_scan("marks of paths", &marks);
    insta::assert_json_snapshot!("pipeline_marks_of_paths", marks);

    // T3. The open findings of the engine's kinds, compared with a computation.
    let kinds = vec!["drift".to_owned(), "orphan_file".to_owned()];
    let findings = explain(
        &pool,
        OPEN_FINDINGS_SQL,
        &[Arg::Text("lane7"), Arg::TextArray(kinds), Arg::Int(100_000)],
    )
    .await;
    no_large_sequential_scan("open findings", &findings);
    insta::assert_json_snapshot!("pipeline_open_findings", findings);

    // T3. What a batch writes: rows by their key, findings by their open fingerprint.
    let n = paths.len();
    let upsert_rows = explain(
        &pool,
        UPSERT_ROWS_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(paths.clone()),
            Arg::TextArray(paths.clone()),
            Arg::TextArray(vec!["base".to_owned(); n]),
            Arg::TextArray(vec!["structured".to_owned(); n]),
            Arg::OptTextArray(vec![None; n]),
            Arg::OptByteaArray(vec![Some(vec![7; 32]); n]),
            Arg::OptByteaArray(vec![None; n]),
            Arg::TextArray(vec!["unknown".to_owned(); n]),
            Arg::Int(1_700_000_000_000),
            Arg::BoolArray(vec![false; n]),
        ],
    )
    .await;
    no_large_sequential_scan("upsert rows", &upsert_rows);
    insta::assert_json_snapshot!("pipeline_upsert_rows", upsert_rows);
    let delete_rows = explain(
        &pool,
        DELETE_ROWS_SQL,
        &[Arg::Text("lane7"), Arg::TextArray(paths.clone())],
    )
    .await;
    no_large_sequential_scan("delete rows", &delete_rows);
    insta::assert_json_snapshot!("pipeline_delete_rows", delete_rows);
    let upsert_baselines = explain(
        &pool,
        UPSERT_BASELINES_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(paths.clone()),
            Arg::ByteaArray(vec![vec![7; 32]; n]),
            Arg::TextArray(vec!["base".to_owned(); n]),
            Arg::OptTextArray(vec![None; n]),
            Arg::TextArray(vec!["c1".to_owned(); n]),
            Arg::OptByteaArray(vec![None; n]),
            Arg::Int(1_700_000_000_000),
        ],
    )
    .await;
    no_large_sequential_scan("upsert baselines", &upsert_baselines);
    insta::assert_json_snapshot!("pipeline_upsert_baselines", upsert_baselines);
    let fingerprints: Vec<String> = (1..=50).map(|f| format!("fp{f}")).collect();
    let upsert_findings = explain(
        &pool,
        UPSERT_FINDINGS_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(vec!["drift".to_owned(); n]),
            Arg::TextArray(paths.clone()),
            Arg::TextArray(vec!["low".to_owned(); n]),
            Arg::TextArray(vec!["[]".to_owned(); n]),
            Arg::TextArray(vec!["a finding".to_owned(); n]),
            Arg::TextArray(vec!["[]".to_owned(); n]),
            Arg::TextArray(vec!["{}".to_owned(); n]),
            Arg::TextArray(fingerprints.clone()),
            Arg::Int(1_700_000_000_000),
        ],
    )
    .await;
    no_large_sequential_scan("upsert findings", &upsert_findings);
    insta::assert_json_snapshot!("pipeline_upsert_findings", upsert_findings);
    let resolve_findings = explain(
        &pool,
        RESOLVE_FINDINGS_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(vec!["drift".to_owned(); n]),
            Arg::TextArray(fingerprints),
            Arg::Int(1_700_000_000_000),
        ],
    )
    .await;
    no_large_sequential_scan("resolve findings", &resolve_findings);
    insta::assert_json_snapshot!("pipeline_resolve_findings", resolve_findings);
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
