//! P12: `EXPLAIN (FORMAT JSON)` of the registry's queries on a database seeded to design scale (40 swimlanes of 2,000 files, 80,000
//! distinct blobs) and analysed. A plan is kept as its node types, relations and index names (never costs, which change with the
//! machine), and no query may read a large table with a sequential scan. Each task that adds a query adds its snapshot here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use hub_registry::adoption::{ADOPT_BASELINES_SQL, ADOPT_MARKS_SQL, CURRENT_NFS_SQL, NFS_FILES_SQL};
use hub_registry::attribution::statements::{
    ACK_SQL, ADMIN_MAP_SQL, EMAIL_SQL, FROM_BATCH_SQL, FROM_OBSERVATIONS_SQL, HELLO_SQL, INSERT_RECORDS_SQL,
    LANES_SQL as ATTRIBUTION_LANES_SQL, NFS_CLIENT_SQL, SENTINEL_SQL, UPGRADE_SQL,
};
use hub_registry::baselines::{CURRENT_BLOBS_SQL, DROP_REFS_SQL, INSERT_REFS_SQL};
use hub_registry::blob_policy::{DENIED_HASH_SQL, HASH_PATHS_LIMIT, HASH_PATHS_SQL};
use hub_registry::index::settings::{MARKED_SQL, UNINDEXED_SQL};
use hub_registry::index::tree_hashes::{LOAD_DIRS_SQL, LOAD_FILES_SQL};
use hub_registry::pickup::sweeper::{INVENTORIED_LANES_SQL, PENDING_LANES_SQL};
use hub_registry::pickup::{
    MAPPINGS_SQL, changed_files_sql, new_folders_sql, services_sql as pickup_services_sql, times_sql,
    write_sql,
};
use hub_registry::pipeline::c9_c11::inputs::{
    ENV_NAMES_SQL, FILES_SQL as STRUCTURE_FILES_SQL, FIRST_OBSERVATION_SQL,
    SERVICES_SQL as STRUCTURE_SERVICES_SQL, folders_sql, gates_sql, newest_observation_sql,
    observation_at_sql,
};
use hub_registry::pipeline::findings_store::{OPEN_OF_KINDS_SQL, registry_row_sql};
use hub_registry::pipeline::lane_loader::{
    BASELINES_OF_LANE_SQL, BASELINES_OF_PATHS_SQL, MARKS_OF_LANE_SQL, MARKS_OF_PATHS_SQL, ROWS_OF_LANE_SQL,
    ROWS_OF_PATHS_SQL,
};
use hub_registry::pipeline::persist::{
    DELETE_ROWS_SQL, OPEN_FINDINGS_SQL, REGISTRY_SQL, RESOLVE_FINDINGS_SQL, STAMP_OBSERVATIONS_SQL,
    UPSERT_BASELINES_SQL, UPSERT_FINDINGS_SQL, UPSERT_ROWS_SQL,
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
use hub_registry::read::search_settings::{LANE_BLOBS_SQL, PAIRS_SQL, PAIRS_UNTESTED_SQL, PLACES_SQL};
use hub_registry::read::search_text::{FILES_SQL, VERSIONS_SQL};
use hub_registry::read::settings::ROWS_SQL;
use hub_registry::read::swimlanes::{AUTO_NOTIFY_SQL, SWIMLANE_SQL, SWIMLANES_SQL};
use hub_registry::read::tree::{CHILDREN_SQL, DRIFT_BY_PATH_SQL, DRIFT_PIVOT_SQL, DRIFT_SQL, FILE_ROWS_SQL};
use hub_registry::tenants::STORED_SQL as TENANTS_STORED_SQL;
use hub_registry::tool_writes::{
    ATTRIBUTE_SQL, DROP_SQL, LANE_PENDING_SQL, LANES_SQL, OBSERVATIONS_SQL, PENDING_OF_PATHS_SQL,
    REGISTER_SQL, SWEEP_SQL,
};
use serde_json::Value;
use sqlx::{PgPool, Row as _};
use support::pg::TestDb;

/// Tables that hold thousands of rows at design scale. A sequential scan of one of these is a failure.
const LARGE: [&str; 12] = [
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
    "tool_write_registrations",
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

/// D124: the plan snapshots of this file are plans of one collation, `en_US.utf8`, the default of the image CI runs on. Under `C` the
/// `text_pattern_ops` indexes can supply an `ORDER BY` and the planner picks other indexes (`effective_files_prefix_idx` for
/// `effective_files_pkey`, and so on), so the same snapshots do not hold. The harness creates every database on that locale whatever the
/// server's default is; this test reads it back from the catalog and from the order the database sorts text in, so that a harness that
/// stopped pinning it fails here by name and not as a plan that differs.
#[tokio::test]
async fn the_explain_database_is_on_the_collation_of_the_pinned_image() {
    let Some(db) = TestDb::ready().await else { return };
    let (collate, ctype) = db.locale().await;
    assert_eq!(collate, support::pg::COLLATION, "datcollate");
    assert_eq!(ctype, support::pg::COLLATION, "datctype");
    // The order of strings, not only the name: `C` puts every capital before every small letter, `en_US` sorts by letter first.
    let mut admin = db.admin().await;
    let capital_before_small: bool = sqlx::query_scalar("SELECT 'B'::text < 'a'::text")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    assert!(
        !capital_before_small,
        "the database sorts text by byte value, as `C` does"
    );
}

/// The generic plan of a statement (the one a prepared statement is given after it has run a few times), or the error when the server
/// cannot make one without the values.
async fn explain_generic(pool: &PgPool, sql: &str) -> Result<Value, sqlx::Error> {
    let text = format!("EXPLAIN (GENERIC_PLAN, FORMAT JSON) {sql}");
    // The simple protocol: the statement keeps its `$n`, and the server plans it for no values at all.
    let row = sqlx::raw_sql(&text).fetch_one(pool).await?;
    let plan: Value = row.try_get(0)?;
    let mut out = Vec::new();
    nodes(&plan[0]["Plan"], &mut out);
    Ok(Value::Array(out))
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
    /// A `NULL` of the type `text[]` (not an array of nulls).
    NullTextArray,
    OptTextArray(Vec<Option<String>>),
    OptByteaArray(Vec<Option<Vec<u8>>>),
    IntArray(Vec<i64>),
    OptIntArray(Vec<Option<i64>>),
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
            Arg::NullTextArray => query.bind(None::<Vec<String>>),
            Arg::OptTextArray(a) => query.bind(a.clone()),
            Arg::OptByteaArray(a) => query.bind(a.clone()),
            Arg::IntArray(a) => query.bind(a.clone()),
            Arg::OptIntArray(a) => query.bind(a.clone()),
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
        // The partitions of `sentinel_records` are named after their month: any of them is the large table.
        let large = node["relation"]
            .as_str()
            .is_some_and(|r| LARGE.contains(&r) || r.starts_with("sentinel_records"));
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
            Arg::OptTextArray(vec![Some("medium".to_owned()); n]),
        ],
    )
    .await;
    no_large_sequential_scan("upsert rows", &upsert_rows);
    insta::assert_json_snapshot!("pipeline_upsert_rows", upsert_rows);
    // The observations the rows point at are rated by their primary keys, never by a scan of the history (T9).
    let stamp = explain(
        &pool,
        STAMP_OBSERVATIONS_SQL,
        &[Arg::Text("lane7"), Arg::TextArray(paths.clone())],
    )
    .await;
    no_large_sequential_scan("stamp observations", &stamp);
    insta::assert_json_snapshot!("pipeline_stamp_observations", stamp);
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
            // A prepared statement that has run a few times is given one plan for all values: that one reads no large table whole either.
            let generic = explain_generic(&pool, $sql)
                .await
                .unwrap_or_else(|e| panic!("{}: no generic plan to look at: {e}", $name));
            no_large_sequential_scan(concat!($name, " (generic plan)"), &generic);
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
            Arg::Int(101),
            Arg::Int32(4)
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
            Arg::Int(101),
            Arg::Int32(1)
        ]
    );

    // The port's drift list, by path: the first page, a state in a folder resumed after a path, and the lookup that resumes it.
    check!(
        "drift_by_path_first_page",
        DRIFT_BY_PATH_SQL,
        [
            Arg::Text(lane),
            Arg::OptText(None),
            Arg::Text("%"),
            Arg::OptText(None),
            Arg::Int(101)
        ]
    );
    check!(
        "drift_by_path_of_a_state_in_a_folder",
        DRIFT_BY_PATH_SQL,
        [
            Arg::Text(lane),
            Arg::OptText(Some("git_ahead")),
            Arg::Text("dir3/%"),
            Arg::OptText(Some("dir3/file100.yml")),
            Arg::Int(101)
        ]
    );
    check!("drift_pivot", DRIFT_PIVOT_SQL, [Arg::Text(lane), Arg::Int(1500)]);
    check!("auto_notify", AUTO_NOTIFY_SQL, [Arg::Text(lane)]);

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
    check!(
        "effective_applications",
        APPS_SQL,
        [
            Arg::TextArray(lanes(40)),
            Arg::TextArray(vec![
                "catalog".to_owned(),
                "catalog-big".to_owned(),
                "catalog-big-01.yml".to_owned()
            ])
        ]
    );
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
        "settings_search_lane_blobs_eight_swimlanes",
        LANE_BLOBS_SQL,
        [Arg::TextArray(lanes(8)), Arg::OptText(None), Arg::Int(20_001)]
    );
    check!(
        "settings_search_lane_blobs_one_file",
        LANE_BLOBS_SQL,
        [
            Arg::TextArray(lanes(1)),
            Arg::OptText(Some("dir3/file3.yml")),
            Arg::Int(20_001)
        ]
    );
    check!(
        "settings_search_pairs_listed_blobs",
        PAIRS_SQL,
        [
            Arg::Text("%port%"),
            Arg::ByteaArray(blob_hashes(2_000)),
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
            Arg::ByteaArray(blob_hashes(25)),
            Arg::OptText(None)
        ]
    );

    // Text search: the readable files of a swimlane and the versions of the swimlanes.
    check!("text_search_files_of_a_swimlane", FILES_SQL, [Arg::Text(lane)]);
    check!("text_search_versions", VERSIONS_SQL, [Arg::TextArray(lanes(40))]);

    // The blob policy: is this hash a denied file's?
    check!("blob_policy_denied_hash", DENIED_HASH_SQL, [Arg::Bytes(&[7; 32])]);
    // And the files that have had it, for the hub's own globs and for files that were deleted: by the hash index of the observations, a
    // probe of `file_state` for each, bounded by the limit.
    check!(
        "blob_policy_hash_paths",
        HASH_PATHS_SQL,
        [
            Arg::Bytes(&[7; 32]),
            Arg::Int(i64::try_from(HASH_PATHS_LIMIT + 1).unwrap())
        ]
    );
}

/// T10: the pickup computation, over a database with 300 Deployments in each of the 40 swimlanes (12,000 services and as many state rows),
/// 50 folders each, and a confirmed mapping for each of 50 folders. None of its statements reads `file_state`, `file_observations`,
/// `services` or `swimlane_folders` by a sequential scan, and every one has a limit. The one scan there is, the sweeper's look at the
/// states that are not live, is of `service_state` (no index serves `pickup_state`, the table is a contract) and runs every 30 seconds,
/// never for a request: its plan is a snapshot so that a change of it is seen.
#[tokio::test]
async fn pickup_queries_are_bounded_and_read_no_large_table_by_a_scan() {
    let Some((db, pool)) = seeded().await else { return };
    let mut admin = db.admin().await;
    for table in [
        "services",
        "service_state",
        "swimlane_folders",
        "folder_mappings",
        "swimlane_registry",
    ] {
        sqlx::raw_sql(&format!("ALTER TABLE {table} SET (autovacuum_enabled = false)"))
            .execute(&mut admin)
            .await
            .unwrap();
    }
    for sql in [
        "INSERT INTO swimlane_registry (swimlane_id, first_inventory_at) SELECT id, now() FROM swimlanes",
        "INSERT INTO services (swimlane_id, namespace, name, env_values, env_names) \
         SELECT 'lane' || l, 'csp', 'svc-' || s, \
                jsonb_build_object('SPRING_APPLICATION_NAME', 'app-' || s, 'CONFIG_CLIENT_CACHE_TTL', '1200'), '{}' \
         FROM generate_series(1, 40) l, generate_series(1, 300) s",
        // One service in forty is waiting for its cache.
        "INSERT INTO service_state (service_id, pods, oldest_pod_started_at, pickup_state) \
         SELECT id, '[]', now() - interval '1 hour', CASE WHEN id % 40 = 0 THEN 'live_within_ttl' ELSE 'live' END FROM services",
        "INSERT INTO swimlane_folders (swimlane_id, dir_path, first_seen_at, initial) \
         SELECT 'lane' || l, 'dir' || d, now(), d % 10 <> 0 FROM generate_series(1, 40) l, generate_series(0, 49) d",
        "INSERT INTO folder_mappings (folder_path, namespace, deployment, confirmed) \
         SELECT 'dir' || d, 'csp', 'svc-' || d, true FROM generate_series(0, 49) d",
        "ANALYZE",
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    let lane = "lane7";
    let strict = ["file_state", "file_observations", "services", "swimlane_folders"];
    let no_scan = |name: &str, plan: &Value| {
        for node in plan.as_array().unwrap() {
            let seq = node["node"] == "Seq Scan" || node["node"] == "Parallel Seq Scan";
            let relation = node["relation"].as_str().unwrap_or_default();
            assert!(
                !(seq && (strict.contains(&relation) || LARGE.contains(&relation))),
                "{name}: a sequential scan of {relation}: {plan:#}"
            );
        }
    };

    let times = explain(&pool, &times_sql(), &[Arg::Text(lane)]).await;
    no_scan("times", &times);
    insta::assert_json_snapshot!("pickup_times", times);

    let services = explain(&pool, &pickup_services_sql(), &[Arg::Text(lane), Arg::Int(5_000)]).await;
    no_scan("services", &services);
    insta::assert_json_snapshot!("pickup_services", services);

    let changed = explain(
        &pool,
        &changed_files_sql(),
        &[Arg::Text(lane), Arg::Int(1_800_000_000_000), Arg::Int(20_001)],
    )
    .await;
    no_scan("changed files", &changed);
    insta::assert_json_snapshot!("pickup_changed_files", changed);

    let folders = explain(
        &pool,
        &new_folders_sql(),
        &[Arg::Text(lane), Arg::Int(1_800_000_000_000), Arg::Int(50_000)],
    )
    .await;
    no_scan("new folders", &folders);
    insta::assert_json_snapshot!("pickup_new_folders", folders);

    let mappings = explain(&pool, MAPPINGS_SQL, &[Arg::Text(lane), Arg::Int(10_000)]).await;
    insta::assert_json_snapshot!("pickup_mappings", mappings);

    // The write: the services of one swimlane that are or were pending, by key, with the comparison in the statement.
    let ids: Vec<i64> = (1..=60).collect();
    let n = ids.len();
    let write = explain(
        &pool,
        &write_sql(),
        &[
            Arg::IntArray(ids),
            Arg::TextArray(vec!["live_within_ttl".to_owned(); n]),
            Arg::OptIntArray(vec![Some(1_800_000_000_000); n]),
            Arg::OptIntArray(vec![Some(1_799_999_000_000); n]),
            Arg::TextArray(vec!["[\"a.yml\"]".to_owned(); n]),
            Arg::Int(1_800_000_000_000),
        ],
    )
    .await;
    no_scan("write", &write);
    insta::assert_json_snapshot!("pickup_write", write);

    // The sweeper's two looks (every 30 seconds, and every ten minutes).
    let pending = explain(&pool, PENDING_LANES_SQL, &[Arg::Int(1_001)]).await;
    insta::assert_json_snapshot!("pickup_sweep_pending_lanes", pending);
    let inventoried = explain(&pool, INVENTORIED_LANES_SQL, &[Arg::Int(1_001)]).await;
    insta::assert_json_snapshot!("pickup_sweep_inventoried_lanes", inventoried);

    // The tenants of the swimlane (a computation that reads files reads them too).
    let tenants = explain(&pool, TENANTS_STORED_SQL, &[Arg::Text(lane), Arg::Int(129)]).await;
    insta::assert_json_snapshot!("tenants_stored", tenants);
}

/// T10 (A45): the statements of the structure checks, over a database with the files of 40 swimlanes (80,000), 300 Deployments and 50 folders
/// in each, and the observations of every file. A computation reads one swimlane's rows by key (`file_state`, `swimlane_folders`, `services`,
/// `file_observations` through the history index) and the gates of all of them (40 rows of `swimlane_registry`); none reads a large table by a
/// sequential scan, and every one has a limit. The write's row statement counts the swimlane's open findings through the open-fingerprint index.
#[tokio::test]
async fn structure_queries_are_bounded_and_read_no_large_table_by_a_scan() {
    let Some((db, pool)) = seeded().await else { return };
    let mut admin = db.admin().await;
    for table in ["services", "swimlane_folders", "swimlane_registry"] {
        sqlx::raw_sql(&format!("ALTER TABLE {table} SET (autovacuum_enabled = false)"))
            .execute(&mut admin)
            .await
            .unwrap();
    }
    for sql in [
        "INSERT INTO swimlane_registry (swimlane_id, first_inventory_at, version, rules_fingerprint) \
         SELECT id, now(), 5, sha256('rules'::bytea) FROM swimlanes",
        "INSERT INTO services (swimlane_id, namespace, name, env_values, env_names) \
         SELECT 'lane' || l, 'csp', 'svc-' || s, jsonb_build_object('SPRING_APPLICATION_NAME', 'app-' || s), \
                ARRAY['A_' || s, 'B_' || s, 'C_' || s] \
         FROM generate_series(1, 40) l, generate_series(1, 300) s",
        "INSERT INTO swimlane_folders (swimlane_id, dir_path, first_seen_at, initial) \
         SELECT 'lane' || l, 'dir' || d, now(), d % 10 <> 0 FROM generate_series(1, 40) l, generate_series(0, 49) d",
        // The findings of the structure checks of a swimlane: a few hundred unresolved placeholders (one per key of a file) and a dozen
        // ambiguous names, next to the 300 findings of the engine's kinds the seeding made.
        "INSERT INTO findings (swimlane_id, kind, path, severity, message, fingerprint) \
         SELECT 'lane' || l, 'unresolved_placeholder', 'svc/file' || i || '.yml', 'medium', 'm', 'up-' || i \
         FROM generate_series(1, 40) l, generate_series(1, 500) i",
        "INSERT INTO findings (swimlane_id, kind, path, severity, message, fingerprint) \
         SELECT 'lane' || l, 'ambiguous_file_name', 'svc/name' || i || '.yml', 'medium', 'm', 'af-' || i \
         FROM generate_series(1, 40) l, generate_series(1, 12) i",
        "ANALYZE",
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    let lane = "lane7";
    let strict = [
        "file_state",
        "file_observations",
        "services",
        "swimlane_folders",
        "findings",
    ];
    macro_rules! check {
        ($name:literal, $sql:expr, $args:expr) => {{
            let plan = explain(&pool, $sql, &$args).await;
            for node in plan.as_array().unwrap() {
                let seq = node["node"] == "Seq Scan" || node["node"] == "Parallel Seq Scan";
                let relation = node["relation"].as_str().unwrap_or_default();
                assert!(
                    !(seq && (strict.contains(&relation) || LARGE.contains(&relation))),
                    "{}: a sequential scan of {relation}: {plan:#}",
                    $name
                );
            }
            insta::assert_json_snapshot!(concat!("structure_", $name), plan);
        }};
    }
    check!(
        "gates_of_some",
        &gates_sql(),
        [Arg::TextArray(vec![lane.to_owned()]), Arg::Int(2)]
    );
    check!(
        "gates_of_all",
        &gates_sql(),
        [Arg::NullTextArray, Arg::Int(100_001)]
    );
    check!("files", STRUCTURE_FILES_SQL, [Arg::Text(lane), Arg::Int(200_001)]);
    check!("folders", &folders_sql(), [Arg::Text(lane), Arg::Int(100_000)]);
    check!(
        "services",
        STRUCTURE_SERVICES_SQL,
        [Arg::Text(lane), Arg::Int(100_000)]
    );
    check!(
        "env_names",
        ENV_NAMES_SQL,
        [
            Arg::Text(lane),
            Arg::IntArray((1..=40).collect()),
            Arg::Int(100_000)
        ]
    );
    check!(
        "channels_newest_observation",
        &newest_observation_sql(),
        [Arg::Text(lane), Arg::Text("channels.yml")]
    );
    check!(
        "channels_observation_at_start",
        &observation_at_sql(),
        [
            Arg::Text(lane),
            Arg::Text("channels.yml"),
            Arg::Int(1_800_000_000_000)
        ]
    );
    check!(
        "channels_first_observation",
        FIRST_OBSERVATION_SQL,
        [Arg::Text(lane), Arg::Text("channels.yml")]
    );
    check!(
        "open_findings_of_the_structure_kinds",
        OPEN_OF_KINDS_SQL,
        [
            Arg::Text(lane),
            Arg::TextArray(vec![
                "ambiguous_file_name".to_owned(),
                "unresolved_placeholder".to_owned(),
                "config_server_restart_required".to_owned()
            ]),
            Arg::Int(100_000)
        ]
    );
    check!(
        "findings_registry_row",
        &registry_row_sql(),
        [Arg::Text(lane), Arg::Int(1_800_000_000_000)]
    );
    check!(
        "pipeline_registry_row",
        REGISTRY_SQL,
        [
            Arg::Text(lane),
            Arg::Int(1),
            Arg::Text("{}"),
            Arg::Bytes(&[7; 32]),
            Arg::Int(1_800_000_000_000)
        ]
    );
}

/// T1 (A10): the statements of the tool-write registrations, on a database with the observations of 40 swimlanes (24,000) and a table of
/// 10,000 registrations of which 120 are younger than the TTL: the state of a registry whose expiry sweep is behind (the age index is what
/// keeps the bound's count off the expired rows), and no statement may read a large table with a sequential scan.
#[tokio::test]
async fn tool_write_queries_have_no_sequential_scan_on_large_tables() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let mut admin = db.admin().await;
    for table in [
        "projects",
        "swimlanes",
        "file_observations",
        "tool_write_registrations",
    ] {
        sqlx::raw_sql(&format!("ALTER TABLE {table} SET (autovacuum_enabled = false)"))
            .execute(&mut admin)
            .await
            .unwrap();
    }
    for sql in [
        "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1')",
        "INSERT INTO swimlanes (id, project_id, display_name) SELECT 'lane' || n, 'p1', 'lane' FROM generate_series(1, 40) n",
        // Three observations of each of 200 files of each swimlane; a deletion every tenth.
        "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, recorded_at) \
         SELECT 'lane' || l, 'dir' || (f % 50) || '/file' || f || '.yml', \
                CASE WHEN k = 3 AND f % 10 = 0 THEN NULL ELSE sha256(('v' || l || '-' || f || '-' || k)::bytea) END, \
                'scan', now() - (k || ' minutes')::interval, now() - (k || ' minutes')::interval \
         FROM generate_series(1, 40) l, generate_series(1, 200) f, generate_series(1, 3) k",
        // 250 registrations for each swimlane: the first three are live, the rest are a day old and not swept yet; one in ten is a delete.
        "INSERT INTO tool_write_registrations (swimlane_id, path, hash, is_delete, user_tid, user_oid, request_id, registered_at) \
         SELECT 'lane' || l, 'dir' || (f % 50) || '/file' || f || '.yml', sha256(('r' || l || '-' || f)::bytea), f % 10 = 0, \
                '72f988bf-86f1-41af-91ab-2d7cd011db47', '00000000-0000-4000-8000-000000000001', 'req-' || l || '-' || f, \
                CASE WHEN f <= 3 THEN now() ELSE now() - interval '1 day' END \
         FROM generate_series(1, 40) l, generate_series(1, 250) f",
        "ANALYZE",
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    let paths = some_paths(50);
    let cutoff = 0_i64; // replaced below by the database's own notion of "a TTL ago", in Unix ms
    let _ = cutoff;
    let now_ms: i64 = sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM now()) * 1000)::bigint")
        .fetch_one(&pool)
        .await
        .unwrap();
    let cutoff = now_ms - 15 * 60 * 1000;
    let hash = ports::content_hash(b"registered").as_bytes().to_vec();

    // `register`: the repeat check, the count up to the bound, and the insert, in one statement.
    let register = explain(
        &pool,
        REGISTER_SQL,
        &[
            Arg::Text("lane7"),
            Arg::Text("dir10/file10.yml"),
            Arg::Bytes(&hash),
            Arg::Text("req-new"),
            Arg::Bool(false),
            Arg::Int(cutoff),
            Arg::Int(10_000),
            Arg::Text("72f988bf-86f1-41af-91ab-2d7cd011db47"),
            Arg::Text("00000000-0000-4000-8000-000000000001"),
            Arg::Int(now_ms),
        ],
    )
    .await;
    no_large_sequential_scan("register", &register);
    insta::assert_json_snapshot!("tool_write_register", register);

    // Ingestion's read of a chunk: the registrations of its paths, and the state before a deletion for the delete registrations.
    let pending = explain(
        &pool,
        PENDING_OF_PATHS_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(paths.clone()),
            Arg::Int(cutoff),
            Arg::Int(1_100_000),
            Arg::Int(i64::MAX),
        ],
    )
    .await;
    no_large_sequential_scan("pending of a chunk's paths", &pending);
    insta::assert_json_snapshot!("tool_write_pending_of_paths", pending);

    // The matcher.
    let lanes = explain(&pool, LANES_SQL, &[Arg::Int(cutoff), Arg::Int(201)]).await;
    no_large_sequential_scan("swimlanes with registrations", &lanes);
    insta::assert_json_snapshot!("tool_write_lanes", lanes);
    let lane_pending = explain(
        &pool,
        LANE_PENDING_SQL,
        &[Arg::Text("lane7"), Arg::Int(cutoff), Arg::Int(1_100_000)],
    )
    .await;
    no_large_sequential_scan("registrations of a swimlane", &lane_pending);
    insta::assert_json_snapshot!("tool_write_lane_pending", lane_pending);
    let observations = explain(
        &pool,
        OBSERVATIONS_SQL,
        &[
            Arg::Text("lane7"),
            Arg::TextArray(paths),
            Arg::Int(cutoff),
            Arg::Int(1_100_000),
        ],
    )
    .await;
    no_large_sequential_scan("observations of the paths", &observations);
    insta::assert_json_snapshot!("tool_write_observations", observations);
    let attribute = explain(
        &pool,
        ATTRIBUTE_SQL,
        &[
            Arg::Text("lane7"),
            Arg::IntArray(vec![1, 2, 3]),
            Arg::TextArray(vec!["{}".to_owned(); 3]),
        ],
    )
    .await;
    no_large_sequential_scan("attribute", &attribute);
    insta::assert_json_snapshot!("tool_write_attribute", attribute);

    // Dropping, and the expiry sweep.
    let drop = explain(
        &pool,
        DROP_SQL,
        &[Arg::Text("lane7"), Arg::IntArray(vec![1, 2, 3])],
    )
    .await;
    no_large_sequential_scan("drop", &drop);
    insta::assert_json_snapshot!("tool_write_drop", drop);
    let sweep = explain(&pool, SWEEP_SQL, &[Arg::Int(cutoff), Arg::Int(1_000)]).await;
    no_large_sequential_scan("sweep", &sweep);
    insta::assert_json_snapshot!("tool_write_sweep", sweep);
}

/// T7 (A17, A19, P14): the statements of the sentinel's sink, the correlator and the grace job, on a database with the observations of 40
/// swimlanes (80,000), 300,000 records of the sentinels spread over every partition (the months' and the default one),
/// 2,000 users and 100 mapped logins, and no statement may read a large table with a sequential scan.
#[tokio::test]
async fn attribution_queries_have_no_sequential_scan_on_large_tables() {
    // `file_observations` has two indexes that begin with the same three columns (`file_observations_history_idx`, for the history of a
    // file, and the unique `file_observations_replay_key`, which makes a replay idempotent), and which of them the planner takes for a
    // lookup by (swimlane, path, time) depends on a random sample of the table that `ANALYZE` takes: the plan is the same and the name is
    // not. Both are written as the first, so that a snapshot is of the plan and not of the dice.
    fn same_index(mut plan: Value) -> Value {
        for node in plan.as_array_mut().into_iter().flatten() {
            if node["index"] == "file_observations_replay_key" {
                node["index"] = Value::from("file_observations_history_idx");
            }
        }
        plan
    }
    async fn explain(pool: &PgPool, sql: &str, args: &[Arg<'_>]) -> Value {
        same_index(crate::explain(pool, sql, args).await)
    }
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let mut admin = db.admin().await;
    for table in [
        "projects",
        "swimlanes",
        "file_observations",
        "sentinels",
        "users",
        "os_login_user_map",
    ] {
        sqlx::raw_sql(&format!("ALTER TABLE {table} SET (autovacuum_enabled = false)"))
            .execute(&mut admin)
            .await
            .unwrap();
    }
    for sql in [
        "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1')",
        "INSERT INTO swimlanes (id, project_id, display_name, nfs_export) SELECT 'lane' || n, 'p1', 'lane', '/export' FROM generate_series(1, 40) n",
        "INSERT INTO sentinels (vm, swimlane_id, export_root, last_seen_at) \
         SELECT 'vm' || n, 'lane' || n, '/export', now() FROM generate_series(1, 40) n",
        // 500 files of each swimlane, four observations of each over the last hour, the newest of each with no attribution beyond unknown
        // (the state the job is for) and the others already attributed (the state history is mostly in).
        "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, recorded_at, attribution_source, attribution_confidence) \
         SELECT 'lane' || l, 'dir' || (f % 50) || '/file' || f || '.yml', sha256(('v' || l || '-' || f || '-' || k)::bytea), 'scan', \
                now() - (k * 15 || ' minutes')::interval, now() - (k * 15 || ' minutes')::interval, \
                CASE WHEN k = 1 THEN 'unknown' ELSE 'sync_job' END, CASE WHEN k = 1 THEN 'low' ELSE 'high' END \
         FROM generate_series(1, 40) l, generate_series(1, 500) f, generate_series(1, 4) k",
        // 300,000 records: 7,500 for each sentinel, over 150 days around now, so that every month's partition and the default one hold
        // rows (an empty partition is read by a sequential scan, rightly, and says nothing about the table).
        "INSERT INTO sentinel_records (sentinel_id, batch_seq, record_index, observed_at, path, operation, success, login_user, effective_user, exe, comm) \
         SELECT s.id, (r / 50) + 1, r % 50, now() + ((r * 7919) % 216000 - 108000) * interval '1 minute', \
                'dir' || (r % 50) || '/file' || (r % 500) || '.yml', 'write', true, 'ada_example_com', 'root', '/usr/bin/vim', 'vim' \
         FROM sentinels s, generate_series(1, 7500) r",
        "INSERT INTO users (tid, oid, email, display_name, role, status) \
         SELECT '72f988bf-86f1-41af-91ab-2d7cd011db47', lpad(n::text, 36, '0'), 'user' || n || '@example.com', 'User', 'viewer', \
                CASE WHEN n % 10 = 0 THEN 'disabled' ELSE 'active' END FROM generate_series(1, 2000) n",
        "INSERT INTO os_login_user_map (os_login, user_tid, user_oid) \
         SELECT 'local_' || n, '72f988bf-86f1-41af-91ab-2d7cd011db47', lpad(n::text, 36, '0') FROM generate_series(1, 100) n",
        "ANALYZE",
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    let now_ms: i64 = sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM now()) * 1000)::bigint")
        .fetch_one(&pool)
        .await
        .unwrap();
    let window = 60_000_i64;
    let weak = || Arg::TextArray(vec!["low".to_owned(), "medium".to_owned()]);

    // The sink: the sentinel's row (locked and stamped), the hello, the records, the acknowledgement.
    let sentinel = explain(&pool, SENTINEL_SQL, &[Arg::Text("vm7"), Arg::Int(now_ms)]).await;
    no_large_sequential_scan("the sentinel's row", &sentinel);
    insta::assert_json_snapshot!("attribution_sentinel_row", sentinel);
    let hello = explain(
        &pool,
        HELLO_SQL,
        &[
            Arg::Text("vm7"),
            Arg::Text("0.1.0"),
            Arg::Int(now_ms),
            Arg::Text("/export"),
        ],
    )
    .await;
    no_large_sequential_scan("hello", &hello);
    insta::assert_json_snapshot!("attribution_hello", hello);
    let n = 50_usize;
    let insert = explain(
        &pool,
        INSERT_RECORDS_SQL,
        &[
            Arg::Int(7),
            Arg::Int(9_999),
            Arg::Int(now_ms),
            Arg::IntArray((0..50).collect()),
            Arg::IntArray(vec![now_ms; n]),
            Arg::TextArray((0..n).map(|i| format!("dir{i}/file{i}.yml")).collect()),
            Arg::TextArray(vec!["write".to_owned(); n]),
            Arg::BoolArray(vec![true; n]),
            Arg::OptTextArray(vec![Some("ada_example_com".to_owned()); n]),
            Arg::OptTextArray(vec![Some("root".to_owned()); n]),
            Arg::OptTextArray(vec![Some("/usr/bin/vim".to_owned()); n]),
            Arg::OptTextArray(vec![Some("vim".to_owned()); n]),
        ],
    )
    .await;
    no_large_sequential_scan("the records of a batch", &insert);
    insta::assert_json_snapshot!("attribution_insert_records", insert);
    let ack = explain(&pool, ACK_SQL, &[Arg::Int(7), Arg::Int(9_999)]).await;
    no_large_sequential_scan("the acknowledgement", &ack);
    insta::assert_json_snapshot!("attribution_ack", ack);

    // The correlator, from the records of a batch to the observations they explain.
    let from_batch = explain(
        &pool,
        FROM_BATCH_SQL,
        &[
            Arg::Int(7),
            Arg::Text("lane7"),
            Arg::Int(3),
            weak(),
            Arg::Int(5_000),
            Arg::Int(window),
        ],
    )
    .await;
    no_large_sequential_scan("observations from a batch", &from_batch);
    insta::assert_json_snapshot!("attribution_from_batch", from_batch);
    // The job: from the observations of a swimlane to the records stored for them, and the NFS clients.
    let from_observations = explain(
        &pool,
        FROM_OBSERVATIONS_SQL,
        &[
            Arg::Text("lane7"),
            Arg::IntArray(vec![7]),
            Arg::Int(now_ms - 2 * 3600 * 1000),
            Arg::TextArray(vec!["low".to_owned()]),
            Arg::Int(5_000),
            Arg::Int(window),
        ],
    )
    .await;
    no_large_sequential_scan("observations from the records", &from_observations);
    insta::assert_json_snapshot!("attribution_from_observations", from_observations);
    let nfs = explain(
        &pool,
        NFS_CLIENT_SQL,
        &[
            Arg::Text("lane7"),
            Arg::Int(now_ms - 10 * 60 * 1000),
            Arg::Int(now_ms - 30 * 1000),
            Arg::TextArray(vec!["low".to_owned()]),
            Arg::Int(5_000),
            Arg::Int(window),
        ],
    )
    .await;
    no_large_sequential_scan("the NFS clients", &nfs);
    insta::assert_json_snapshot!("attribution_nfs_client", nfs);
    let upgrade = explain(
        &pool,
        UPGRADE_SQL,
        &[
            Arg::Text("lane7"),
            Arg::IntArray(vec![1, 2, 3]),
            Arg::TextArray(vec!["sentinel_login".to_owned(); 3]),
            Arg::TextArray(vec!["high".to_owned(); 3]),
            Arg::OptTextArray(vec![None; 3]),
            Arg::TextArray(vec!["{}".to_owned(); 3]),
            weak(),
        ],
    )
    .await;
    no_large_sequential_scan("the upgrade", &upgrade);
    insta::assert_json_snapshot!("attribution_upgrade", upgrade);
    let lanes = explain(&pool, ATTRIBUTION_LANES_SQL, &[Arg::Int(201)]).await;
    no_large_sequential_scan("the swimlanes with a sentinel", &lanes);
    insta::assert_json_snapshot!("attribution_lanes", lanes);

    // The OS Login map: the admin's rows by key, then the active users by the form of their email (a table of thousands, not of
    // millions: it is read by a scan, once for the logins of a run that the admin's map did not answer).
    let logins = vec!["local_7".to_owned(), "ada_example_com".to_owned()];
    let admin_map = explain(&pool, ADMIN_MAP_SQL, &[Arg::TextArray(logins.clone())]).await;
    no_large_sequential_scan("the admin's map", &admin_map);
    insta::assert_json_snapshot!("attribution_admin_map", admin_map);
    let email = explain(&pool, EMAIL_SQL, &[Arg::TextArray(logins)]).await;
    no_large_sequential_scan("the email match", &email);
    insta::assert_json_snapshot!("attribution_email_match", email);
}
