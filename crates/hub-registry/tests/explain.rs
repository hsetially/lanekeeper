//! P12: `EXPLAIN (FORMAT JSON)` of the registry's queries on a database seeded to design scale (40 swimlanes of 2,000 files, 80,000
//! distinct blobs) and analysed. A plan is kept as its node types, relations and index names (never costs, which change with the
//! machine), and no query may read a large table with a sequential scan. Each task that adds a query adds its snapshot here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use hub_registry::adoption::{ADOPT_BASELINES_SQL, ADOPT_MARKS_SQL, CURRENT_NFS_SQL, NFS_FILES_SQL};
use hub_registry::baselines::{CURRENT_BLOBS_SQL, DROP_REFS_SQL, INSERT_REFS_SQL};
use hub_registry::blob_policy::DENIED_HASH_SQL;
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
use hub_registry::read::compare::BASELINE_BLOB_SQL;
use hub_registry::read::compare_tree::{
    BASELINE_MAP_SQL, CHILD_DIRS_SQL, DIR_FILES_SQL, DIR_HASH_SQL, EFFECTIVE_MAP_SQL, NFS_MAP_SQL,
};
use hub_registry::read::content::{
    BASELINE_SQL as CONTENT_BASELINE_SQL, FILE_SQL as CONTENT_FILE_SQL, MAPPING_SQL,
    TENANTS_SQL as CONTENT_TENANTS_SQL,
};
use hub_registry::read::effective::{APPS_SQL, EXISTING_SQL, GROUP_ROWS_SQL, TENANTS_SQL};
use hub_registry::read::findings::findings_sql;
use hub_registry::read::history::history_sql;
use hub_registry::read::resource::{ALL_PATHS_SQL, PROPERTY_ROWS_SQL, ROWS_BY_PATH_SQL};
use hub_registry::read::restarts::services_sql;
use hub_registry::read::search_settings::{PAIRS_SQL, PAIRS_UNTESTED_SQL, PLACES_SQL};
use hub_registry::read::search_text::{FILES_SQL, VERSIONS_SQL};
use hub_registry::read::settings::ROWS_SQL;
use hub_registry::read::swimlanes::{SWIMLANE_SQL, SWIMLANES_SQL};
use hub_registry::read::tree::{CHILDREN_SQL, DRIFT_SQL, FILE_ROWS_SQL};
use serde_json::Value;
use sqlx::PgPool;
use support::pg::TestDb;

/// Tables that hold thousands of rows at design scale. A sequential scan of one of these is a failure.
const LARGE: [&str; 11] = [
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
    "blob_refs",
];

/// The tables the seeding fills. Autovacuum is switched off for them, because a vacuum that runs between the seeding and the
/// EXPLAIN sets their visibility maps, and the planner then reads a plan with an index-only scan where it read a bitmap heap scan a
/// moment before: the plan snapshots below would be right or wrong by the timing of a background worker, as they were (one run in
/// three failed on `markers_of_a_batch`). With no vacuum the maps stay empty and the plans are those of the snapshots every time.
const SEEDED: [&str; 12] = [
    "projects",
    "swimlanes",
    "blobs",
    "file_state",
    "settings_index_blobs",
    "effective_tree_hashes",
    "effective_files",
    "file_observations",
    "baselines",
    "blob_refs",
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
        "INSERT INTO baselines (swimlane_id, path, hash, repo, commit_id, blob_hash) \
         SELECT swimlane_id, path, nfs_hash, 'base', 'c1', nfs_hash FROM file_state WHERE hashtext(path || swimlane_id) % 2 = 0",
        // Each of those baselines holds a reference to the raw version behind it (A24, D78).
        "INSERT INTO blob_refs (hash, ref_kind, ref_id) \
         SELECT nfs_hash, 'baseline', swimlane_id || ':' || path FROM file_state WHERE hashtext(path || swimlane_id) % 2 = 0",
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
    Int32(i32),
    Float(f64),
    Bool(bool),
    OptText(Option<&'a str>),
    OptInt(Option<i64>),
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
            Arg::Int32(i) => query.bind(*i),
            Arg::Float(f) => query.bind(*f),
            Arg::Bool(b) => query.bind(*b),
            Arg::OptText(t) => query.bind(*t),
            Arg::OptInt(i) => query.bind(*i),
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
    // The references that keep the raw versions behind baselines (A24, D78): the raw versions that the baselines of some paths stand on
    // now, the references dropped for the versions they replaced, and the ones added for the new versions.
    let current_blobs = explain(
        &pool,
        CURRENT_BLOBS_SQL,
        &[Arg::Text("lane7"), Arg::TextArray(paths.clone())],
    )
    .await;
    no_large_sequential_scan("baseline blobs", &current_blobs);
    insta::assert_json_snapshot!("baselines_current_blobs", current_blobs);
    // The blobs of the first fifty of lane7's files that `some_paths` names: file number `f` of lane 7 is blob `6 * 2000 + f`.
    let blob_hashes: Vec<Vec<u8>> = (1..=n)
        .map(|i| {
            ports::content_hash(format!("blob-{}", 12_000 + i * 10).as_bytes())
                .as_bytes()
                .to_vec()
        })
        .collect();
    let ref_ids: Vec<String> = paths.iter().map(|p| format!("lane7:{p}")).collect();
    let drop_refs = explain(
        &pool,
        DROP_REFS_SQL,
        &[
            Arg::ByteaArray(blob_hashes.clone()),
            Arg::TextArray(ref_ids.clone()),
        ],
    )
    .await;
    no_large_sequential_scan("drop baseline refs", &drop_refs);
    insta::assert_json_snapshot!("baselines_drop_refs", drop_refs);
    let insert_refs = explain(
        &pool,
        INSERT_REFS_SQL,
        &[Arg::ByteaArray(blob_hashes), Arg::TextArray(ref_ids)],
    )
    .await;
    no_large_sequential_scan("insert baseline refs", &insert_refs);
    insta::assert_json_snapshot!("baselines_insert_refs", insert_refs);
    // T5. What an adoption confirmation reads and writes: the swimlane's files, the planned files as they are under the lock, the
    // baselines (the pipeline's statement and the admin who adopted them) and the marks.
    let nfs_files = explain(&pool, NFS_FILES_SQL, &[Arg::Text("lane7"), Arg::Int(200_001)]).await;
    no_large_sequential_scan("adoption files", &nfs_files);
    insta::assert_json_snapshot!("adoption_nfs_files", nfs_files);
    let current_nfs = explain(
        &pool,
        CURRENT_NFS_SQL,
        &[Arg::Text("lane7"), Arg::TextArray(paths.clone())],
    )
    .await;
    no_large_sequential_scan("adoption current hashes", &current_nfs);
    insta::assert_json_snapshot!("adoption_current_nfs", current_nfs);
    let adopt_baselines = explain(
        &pool,
        ADOPT_BASELINES_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(paths.clone()),
            Arg::ByteaArray(vec![vec![7; 32]; n]),
            Arg::TextArray(vec!["base".to_owned(); n]),
            Arg::OptTextArray(vec![None; n]),
            Arg::TextArray(vec!["c1".to_owned(); n]),
            Arg::OptByteaArray(vec![None; n]),
            Arg::Int(1_700_000_000_000),
            Arg::Text("tid"),
            Arg::Text("oid"),
        ],
    )
    .await;
    no_large_sequential_scan("adoption baselines", &adopt_baselines);
    insta::assert_json_snapshot!("adoption_baselines", adopt_baselines);
    let adopt_marks = explain(
        &pool,
        ADOPT_MARKS_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(paths.clone()),
            Arg::ByteaArray(vec![vec![7; 32]; n]),
            Arg::ByteaArray(vec![vec![8; 32]; n]),
            Arg::TextArray(vec!["on purpose".to_owned(); n]),
            Arg::Text("tid"),
            Arg::Text("oid"),
            Arg::Int(1_700_000_000_000),
        ],
    )
    .await;
    no_large_sequential_scan("adoption marks", &adopt_marks);
    insta::assert_json_snapshot!("adoption_marks", adopt_marks);
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

// ------------------------------------------------------------------------------------------------ T6: the read API

/// The design-scale database with what the reads add: the settings of every structured blob (three each), the tenants and the
/// registry row of each swimlane, a few services, and the path mapping.
async fn seeded_for_reads() -> Option<(TestDb, PgPool)> {
    let (db, pool) = seeded().await?;
    let mut admin = db.admin().await;
    for table in [
        "settings_index",
        "swimlane_tenants",
        "swimlane_registry",
        "services",
        "service_state",
    ] {
        sqlx::raw_sql(&format!("ALTER TABLE {table} SET (autovacuum_enabled = false)"))
            .execute(&mut admin)
            .await
            .unwrap();
    }
    for sql in [
        "INSERT INTO settings_index (blob_hash, setting_path, value_text, value_type, start_line, end_line) \
         SELECT b.h, p.path, 'v', 'string', 1, 1 \
         FROM (SELECT DISTINCT nfs_hash AS h FROM file_state WHERE class = 'structured') b, \
              (VALUES ('server.port'), ('server.name'), ('app.timeout')) AS p(path)",
        "INSERT INTO swimlane_tenants (swimlane_id, tenant_id) SELECT id, 'sit' || substr(id, 5) FROM swimlanes",
        "INSERT INTO swimlane_registry (swimlane_id, version, state_counts, finding_count) \
         SELECT id, 5, '{\"in_sync\": 1900, \"git_ahead\": 100}', 300 FROM swimlanes",
        "INSERT INTO services (swimlane_id, namespace, name, env_values) \
         SELECT id, 'ns', 'svc' || n, '{\"SPRING_APPLICATION_NAME\": \"core\"}' FROM swimlanes, generate_series(1, 5) n",
        "INSERT INTO service_state (service_id, pickup_state) SELECT id, 'live' FROM services",
        "INSERT INTO path_mappings (repo, repo_root) VALUES ('base', 'config'), ('tenant', 'data/config')",
        "ANALYZE",
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    Some((db, pool))
}

fn blob_hashes(n: usize) -> Vec<Vec<u8>> {
    (1..=n)
        .map(|i| {
            ports::content_hash(format!("blob-{i}").as_bytes())
                .as_bytes()
                .to_vec()
        })
        .collect()
}

#[tokio::test]
async fn read_queries_have_no_sequential_scan_on_large_tables() {
    let Some((_db, pool)) = seeded_for_reads().await else {
        return;
    };
    let lane = "lane7";
    let lanes = |n: usize| -> Vec<String> { (1..=n).map(|i| format!("lane{i}")).collect() };
    let page_paths: Vec<String> = (1..=101)
        .map(|i| format!("dir3/file{}.yml", 3 + 50 * (i % 40)))
        .collect();

    macro_rules! check {
        ($name:literal, $sql:expr, $args:expr) => {{
            let plan = explain(&pool, $sql, &$args).await;
            no_large_sequential_scan($name, &plan);
            insta::assert_json_snapshot!(concat!("read_", $name), plan);
        }};
    }

    // The swimlane list and one swimlane.
    check!(
        "swimlanes",
        SWIMLANES_SQL,
        [Arg::OptText(None), Arg::Int(101), Arg::Float(30.0)]
    );
    check!("swimlane", SWIMLANE_SQL, [Arg::Text(lane), Arg::Float(30.0)]);
    check!("existing_swimlanes", EXISTING_SQL, [Arg::TextArray(lanes(40))]);

    // The tree: the children of the root and of a folder, the rows of a page, the drift list.
    check!(
        "tree_children_of_the_root",
        CHILDREN_SQL,
        [
            Arg::Text(lane),
            Arg::Text("%"),
            Arg::Int32(1),
            Arg::OptText(None),
            Arg::Int(200_001),
            Arg::OptText(None),
            Arg::Int(101)
        ]
    );
    check!(
        "tree_children_of_a_folder",
        CHILDREN_SQL,
        [
            Arg::Text(lane),
            Arg::Text("dir3/%"),
            Arg::Int32(6),
            Arg::OptText(Some("in_sync")),
            Arg::Int(200_001),
            Arg::OptText(Some("file1")),
            Arg::Int(101)
        ]
    );
    check!(
        "tree_file_rows",
        FILE_ROWS_SQL,
        [Arg::Text(lane), Arg::TextArray(page_paths.clone())]
    );
    check!(
        "drift_first_page",
        DRIFT_SQL,
        [
            Arg::Text(lane),
            Arg::Int(0),
            Arg::OptText(None),
            Arg::Text("%"),
            Arg::Int(101)
        ]
    );
    check!(
        "drift_of_a_state_in_a_folder",
        DRIFT_SQL,
        [
            Arg::Text(lane),
            Arg::Int(1500),
            Arg::OptText(Some("git_ahead")),
            Arg::Text("dir3/%"),
            Arg::Int(101)
        ]
    );

    // History, findings, services.
    check!(
        "history",
        &history_sql(),
        [
            Arg::Text(lane),
            Arg::Text("dir3/file3.yml"),
            Arg::OptInt(None),
            Arg::OptInt(None),
            Arg::Int(101)
        ]
    );
    check!(
        "history_second_page",
        &history_sql(),
        [
            Arg::Text(lane),
            Arg::Text("dir3/file3.yml"),
            Arg::OptInt(Some(1_700_000_000_000)),
            Arg::OptInt(Some(5)),
            Arg::Int(101)
        ]
    );
    check!(
        "findings_of_a_swimlane",
        &findings_sql(),
        [
            Arg::OptText(Some(lane)),
            Arg::TextArray(vec![]),
            Arg::Int32(0),
            Arg::OptInt(None),
            Arg::Int(101)
        ]
    );
    check!(
        "findings_of_all_swimlanes_by_kind_and_severity",
        &findings_sql(),
        [
            Arg::OptText(None),
            Arg::TextArray(vec!["drift".to_owned()]),
            Arg::Int32(2),
            Arg::OptInt(Some(9_000)),
            Arg::Int(101)
        ]
    );
    check!(
        "services_pending",
        &services_sql(),
        [Arg::Text(lane), Arg::Int(0), Arg::Bool(false), Arg::Int(101)]
    );

    // Content: one file by its key, a baseline, the tenants and the mapping.
    check!(
        "content_file",
        CONTENT_FILE_SQL,
        [Arg::Text(lane), Arg::Text("dir3/file3.yml")]
    );
    check!(
        "content_baseline",
        CONTENT_BASELINE_SQL,
        [Arg::Text(lane), Arg::Text("dir3/file3.yml")]
    );
    check!(
        "compare_baseline_blob",
        BASELINE_BLOB_SQL,
        [Arg::Text(lane), Arg::Text("dir3/file3.yml")]
    );
    check!("tenants_of_a_swimlane", CONTENT_TENANTS_SQL, [Arg::Text(lane)]);
    check!("path_mapping", MAPPING_SQL, []);

    // The setting rows of a page of blobs, read once per blob.
    check!(
        "setting_rows_of_blobs",
        ROWS_SQL,
        [Arg::ByteaArray(blob_hashes(200)), Arg::Int(1_000)]
    );

    // Effective config: the group of a logical file in many swimlanes, their tenants and applications, a resource's candidates.
    check!(
        "effective_group_rows",
        GROUP_ROWS_SQL,
        [
            Arg::TextArray(lanes(40)),
            Arg::Text("dir3/file3.yml"),
            Arg::TextArray(vec![
                "application.yml".to_owned(),
                "application.yaml".to_owned(),
                "application.properties".to_owned()
            ])
        ]
    );
    check!("effective_tenants", TENANTS_SQL, [Arg::TextArray(lanes(40))]);
    check!("effective_applications", APPS_SQL, [Arg::TextArray(lanes(40))]);
    check!(
        "resource_rows_by_path",
        ROWS_BY_PATH_SQL,
        [
            Arg::Text(lane),
            Arg::TextArray(vec![
                "dir3/chan/file3-sit7.yml".to_owned(),
                "dir3/chan/file3.yml".to_owned()
            ])
        ]
    );
    check!(
        "resource_property_rows",
        PROPERTY_ROWS_SQL,
        [
            Arg::Text(lane),
            Arg::TextArray((0..12).map(|i| format!("core-{i}.yml")).collect())
        ]
    );
    check!("resource_all_paths", ALL_PATHS_SQL, [Arg::Text(lane)]);

    // The whole-tree compare.
    check!(
        "compare_tree_directory_hash",
        DIR_HASH_SQL,
        [Arg::Text(lane), Arg::Text("dir3")]
    );
    check!(
        "compare_tree_child_directories",
        CHILD_DIRS_SQL,
        [Arg::Text(lane), Arg::TextArray(vec![String::new()])]
    );
    check!(
        "compare_tree_files_of_directories",
        DIR_FILES_SQL,
        [
            Arg::Text(lane),
            Arg::TextArray(vec!["dir3".to_owned(), "dir4".to_owned()])
        ]
    );
    check!(
        "compare_tree_effective_map",
        EFFECTIVE_MAP_SQL,
        [Arg::Text(lane), Arg::Text("dir3/%")]
    );
    check!(
        "compare_tree_nfs_map",
        NFS_MAP_SQL,
        [Arg::Text(lane), Arg::Text("dir3/%")]
    );
    check!(
        "compare_tree_baseline_map",
        BASELINE_MAP_SQL,
        [Arg::Text(lane), Arg::Text("dir3/%")]
    );

    // Settings search: a part of a path across all swimlanes (read untested, the places say which are current), in a few swimlanes
    // (tested in SQL), in one file, and the places of the blobs found.
    check!(
        "settings_search_pairs_all_swimlanes",
        PAIRS_UNTESTED_SQL,
        [
            Arg::Text("%port%"),
            Arg::Bytes(&[]),
            Arg::Text(""),
            Arg::Int(102),
            Arg::Int32(255)
        ]
    );
    check!(
        "settings_search_pairs_few_swimlanes",
        PAIRS_SQL,
        [
            Arg::Text("%port%"),
            Arg::TextArray(lanes(2)),
            Arg::OptText(None),
            Arg::Bytes(&[]),
            Arg::Text(""),
            Arg::Int(102),
            Arg::Int32(255)
        ]
    );
    check!(
        "settings_search_pairs_one_swimlane_and_file",
        PAIRS_SQL,
        [
            Arg::Text("%timeout%"),
            Arg::TextArray(lanes(1)),
            Arg::OptText(Some("dir3/file3.yml")),
            Arg::Bytes(&[]),
            Arg::Text(""),
            Arg::Int(102),
            Arg::Int32(255)
        ]
    );
    check!(
        "settings_search_places",
        PLACES_SQL,
        [
            Arg::TextArray(lanes(40)),
            Arg::ByteaArray(blob_hashes(102)),
            Arg::OptText(None)
        ]
    );

    // Text search: the readable files of a swimlane and the versions of the swimlanes.
    check!("text_search_files_of_a_swimlane", FILES_SQL, [Arg::Text(lane)]);
    check!("text_search_versions", VERSIONS_SQL, [Arg::TextArray(lanes(40))]);

    // The blob policy: is this hash a denied file's?
    check!("blob_policy_denied_hash", DENIED_HASH_SQL, [Arg::Bytes(&[7; 32])]);
}
