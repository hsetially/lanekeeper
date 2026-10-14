//! P12, no steady-state query takes longer than 50 ms (`P12.steady_state_query_ms`, `hub-registry/steady_state_queries`, a harness,
//! p95, 50 ms): every query the read API makes, run on the database of the read bed (40 swimlanes of about 2,000 files, about 23,000
//! blobs, every setting of every structured file indexed, every swimlane computed), with the binds a real request has, twenty times
//! each after a first run that is not counted. The statements are the ones the service runs (`pub const` and `pub fn` of
//! `hub_registry::read`), not copies.
//!
//! The samples are every execution of every query. The budget's statistic is the 95th percentile of those, and the benchmark is
//! stricter than that: it fails if the 95th percentile of any one query is over 50 ms, so that a slow query cannot hide among the quick
//! ones. The slowest queries are printed.
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
use hub_registry::RegistryConfig;
use hub_registry::blob_policy::DENIED_HASH_SQL;
use hub_registry::read::compare::BASELINE_BLOB_SQL;
use hub_registry::read::compare_tree::{
    BASELINE_MAP_SQL, CHILD_DIRS_SQL, DIR_FILES_SQL, DIR_HASH_SQL, EFFECTIVE_MAP_SQL, NFS_MAP_SQL,
};
use hub_registry::read::content::{
    BASELINE_SQL as CONTENT_BASELINE_SQL, FILE_SQL as CONTENT_FILE_SQL, MAPPING_SQL,
    TENANTS_SQL as CONTENT_TENANTS_SQL,
};
use hub_registry::read::effective::{APPS_SQL, EXISTING_SQL, GROUP_ROWS_SQL, TENANTS_SQL, app_candidates};
use hub_registry::read::findings::findings_sql;
use hub_registry::read::history::history_sql;
use hub_registry::read::resource::{ALL_PATHS_SQL, PROPERTY_ROWS_SQL, ROWS_BY_PATH_SQL};
use hub_registry::read::restarts::services_sql;
use hub_registry::read::search_settings::{LANE_BLOBS_SQL, PAIRS_SQL, PAIRS_UNTESTED_SQL, PLACES_SQL};
use hub_registry::read::search_text::{FILES_SQL, VERSIONS_SQL};
use hub_registry::read::settings::ROWS_SQL;
use hub_registry::read::swimlanes::{SWIMLANE_SQL, SWIMLANES_SQL};
use hub_registry::read::tree::{CHILDREN_SQL, DRIFT_SQL, FILE_ROWS_SQL};

const REPEATS: usize = 20;
const LIMIT_MS: f64 = 50.0;

#[tokio::main]
async fn main() {
    let Some(bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/steady_state_queries: SKIPPED (no database)");
        return;
    };
    let pool = bed.bed.rig.pool.clone();
    let lanes: Vec<String> = bed.bed.lanes.iter().map(|l| l.id.to_string()).collect();
    let lane = lanes[7].clone();
    // A folder, a file with history, and a page of paths and blobs, all real.
    let files = &bed.bed.lanes[7].files;
    let folder = files
        .iter()
        .find_map(|(p, _)| p.as_str().split_once('/').map(|(d, _)| d.to_owned()))
        .unwrap();
    let path = files[files.len() / 2].0.as_str().to_owned();
    let page_paths: Vec<String> = files
        .iter()
        .step_by(files.len() / 101)
        .take(101)
        .map(|(p, _)| p.as_str().to_owned())
        .collect();
    let hashes: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT DISTINCT nfs_hash::bytea FROM file_state WHERE swimlane_id = $1 AND class = 'structured' AND nfs_hash IS NOT NULL LIMIT 200",
    )
    .bind(&lane)
    .fetch_all(&pool)
    .await
    .unwrap();
    let hashes102: Vec<Vec<u8>> = hashes.iter().take(102).cloned().collect();
    let largest: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT blob_hash::bytea FROM settings_index GROUP BY blob_hash ORDER BY count(*) DESC, blob_hash LIMIT 4",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let typical: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT blob_hash::bytea FROM settings_index GROUP BY blob_hash HAVING count(*) BETWEEN 30 AND 50 ORDER BY blob_hash LIMIT 4",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(largest.len() == 4 && typical.len() == 4);
    let like_folder = format!("{folder}/%");
    let logical = path.clone();
    let candidates = app_candidates(&domain::LogicalFile::parse(&logical).unwrap());
    let roots: Vec<String> = ["application.yml", "application.yaml", "application.properties"]
        .map(str::to_owned)
        .to_vec();
    let names: Vec<String> = (0..12).map(|i| format!("catalog-{i}.yml")).collect();
    let tenant_copy = vec![
        format!("{folder}/chan/x-sit1.yml"),
        format!("{folder}/chan/x.yml"),
    ];

    let mut results: Vec<(&'static str, Vec<f64>)> = Vec::new();
    macro_rules! measure {
        ($name:literal, $query:expr) => {{
            let _ = $query.fetch_all(&pool).await.unwrap();
            let mut ms = Vec::with_capacity(REPEATS);
            for _ in 0..REPEATS {
                let started = Instant::now();
                let rows = $query.fetch_all(&pool).await.unwrap();
                ms.push(started.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(rows);
            }
            results.push(($name, ms));
        }};
    }
    let lane_s = lane.as_str();
    measure!(
        "swimlanes",
        sqlx::query(SWIMLANES_SQL)
            .bind(None::<&str>)
            .bind(101_i64)
            .bind(30.0_f64)
    );
    measure!("swimlane", sqlx::query(SWIMLANE_SQL).bind(lane_s).bind(30.0_f64));
    measure!("existing_swimlanes", sqlx::query(EXISTING_SQL).bind(&lanes));
    measure!(
        "tree_children_of_the_root",
        sqlx::query(CHILDREN_SQL)
            .bind(lane_s)
            .bind("%")
            .bind(1_i32)
            .bind(None::<&str>)
            .bind(200_001_i64)
            .bind(None::<&str>)
            .bind(101_i64)
    );
    measure!(
        "tree_children_of_a_folder",
        sqlx::query(CHILDREN_SQL)
            .bind(lane_s)
            .bind(&like_folder)
            .bind(i32::try_from(folder.chars().count() + 2).unwrap())
            .bind(None::<&str>)
            .bind(200_001_i64)
            .bind(None::<&str>)
            .bind(501_i64)
    );
    measure!(
        "tree_file_rows",
        sqlx::query(FILE_ROWS_SQL).bind(lane_s).bind(&page_paths)
    );
    measure!(
        "drift_first_page",
        sqlx::query(DRIFT_SQL)
            .bind(lane_s)
            .bind(0_i64)
            .bind(None::<&str>)
            .bind("%")
            .bind(501_i64)
    );
    measure!(
        "history",
        sqlx::query(&history_sql())
            .bind(lane_s)
            .bind(&path)
            .bind(None::<i64>)
            .bind(None::<i64>)
            .bind(101_i64)
    );
    measure!(
        "findings_of_a_swimlane",
        sqlx::query(&findings_sql())
            .bind(Some(lane_s))
            .bind(Vec::<String>::new())
            .bind(0_i32)
            .bind(None::<i64>)
            .bind(501_i64)
    );
    measure!(
        "findings_of_all_swimlanes",
        sqlx::query(&findings_sql())
            .bind(None::<&str>)
            .bind(vec!["drift".to_owned()])
            .bind(2_i32)
            .bind(None::<i64>)
            .bind(501_i64)
    );
    measure!(
        "services",
        sqlx::query(&services_sql())
            .bind(lane_s)
            .bind(0_i64)
            .bind(true)
            .bind(501_i64)
    );
    measure!(
        "content_file",
        sqlx::query(CONTENT_FILE_SQL).bind(lane_s).bind(&path)
    );
    measure!(
        "content_baseline",
        sqlx::query(CONTENT_BASELINE_SQL).bind(lane_s).bind(&path)
    );
    measure!(
        "compare_baseline_blob",
        sqlx::query(BASELINE_BLOB_SQL).bind(lane_s).bind(&path)
    );
    measure!(
        "tenants_of_a_swimlane",
        sqlx::query(CONTENT_TENANTS_SQL).bind(lane_s)
    );
    measure!("path_mapping", sqlx::query(MAPPING_SQL));
    // The statements that read the settings of blobs take a few blobs each (`ROWS_PER_STATEMENT`, four): the biggest such statement is
    // four of the biggest blobs (about 6,000 settings each), a typical one four blobs of a few dozen settings.
    measure!(
        "setting_rows_of_4_largest_blobs",
        sqlx::query(ROWS_SQL).bind(&largest).bind(1_000_i64)
    );
    measure!(
        "setting_rows_of_4_typical_blobs",
        sqlx::query(ROWS_SQL).bind(&typical).bind(1_000_i64)
    );
    measure!(
        "effective_group_rows_40_swimlanes",
        sqlx::query(GROUP_ROWS_SQL)
            .bind(&lanes)
            .bind(&logical)
            .bind(&roots)
    );
    measure!("effective_tenants", sqlx::query(TENANTS_SQL).bind(&lanes));
    measure!(
        "effective_applications",
        sqlx::query(APPS_SQL).bind(&lanes).bind(&candidates)
    );
    measure!(
        "resource_rows_by_path",
        sqlx::query(ROWS_BY_PATH_SQL).bind(lane_s).bind(&tenant_copy)
    );
    measure!(
        "resource_property_rows",
        sqlx::query(PROPERTY_ROWS_SQL).bind(lane_s).bind(&names)
    );
    measure!("resource_all_paths", sqlx::query(ALL_PATHS_SQL).bind(lane_s));
    measure!(
        "compare_tree_directory_hash",
        sqlx::query(DIR_HASH_SQL).bind(lane_s).bind(&folder)
    );
    measure!(
        "compare_tree_child_directories",
        sqlx::query(CHILD_DIRS_SQL).bind(lane_s).bind(vec![String::new()])
    );
    measure!(
        "compare_tree_files_of_directories",
        sqlx::query(DIR_FILES_SQL).bind(lane_s).bind(vec![folder.clone()])
    );
    measure!(
        "compare_tree_effective_map",
        sqlx::query(EFFECTIVE_MAP_SQL).bind(lane_s).bind("%")
    );
    measure!(
        "compare_tree_nfs_map",
        sqlx::query(NFS_MAP_SQL).bind(lane_s).bind("%")
    );
    measure!(
        "compare_tree_baseline_map",
        sqlx::query(BASELINE_MAP_SQL).bind(lane_s).bind("%")
    );
    // Over a few swimlanes (at most eight, the default) the blobs that are current there are listed and the settings read among them;
    // over many the settings are read untested and the places say which are current. Every one of these is planned for its own values.
    let eight: Vec<String> = lanes.iter().take(8).cloned().collect();
    measure!(
        "settings_search_lane_blobs_eight_swimlanes",
        sqlx::query(LANE_BLOBS_SQL)
            .persistent(false)
            .bind(&eight)
            .bind(None::<&str>)
            .bind(20_001_i64)
    );
    let listed: Vec<Vec<u8>> = sqlx::query_scalar(LANE_BLOBS_SQL)
        .bind(&eight)
        .bind(None::<&str>)
        .bind(20_001_i64)
        .fetch_all(&pool)
        .await
        .unwrap();
    measure!(
        "settings_search_pairs_dense_listed_blobs",
        sqlx::query(PAIRS_SQL)
            .persistent(false)
            .bind("%name%")
            .bind(&listed)
            .bind(&[][..] as &[u8])
            .bind("")
            .bind(102_i64)
            .bind(255_i32)
    );
    measure!(
        "settings_search_pairs_absent_listed_blobs",
        sqlx::query(PAIRS_SQL)
            .persistent(false)
            .bind("%zzzqqqxx%")
            .bind(&listed)
            .bind(&[][..] as &[u8])
            .bind("")
            .bind(102_i64)
            .bind(255_i32)
    );
    measure!(
        "settings_search_pairs_dense_untested",
        sqlx::query(PAIRS_UNTESTED_SQL)
            .persistent(false)
            .bind("%name%")
            .bind(&[][..] as &[u8])
            .bind("")
            .bind(102_i64)
            .bind(255_i32)
    );
    measure!(
        "settings_search_pairs_absent_untested",
        sqlx::query(PAIRS_UNTESTED_SQL)
            .persistent(false)
            .bind("%zzzqqqxx%")
            .bind(&[][..] as &[u8])
            .bind("")
            .bind(102_i64)
            .bind(255_i32)
    );
    measure!(
        "settings_search_places",
        sqlx::query(PLACES_SQL)
            .bind(&lanes)
            .bind(&hashes102)
            .bind(None::<&str>)
    );
    measure!(
        "text_search_files_of_a_swimlane",
        sqlx::query(FILES_SQL).bind(lane_s)
    );
    measure!("text_search_versions", sqlx::query(VERSIONS_SQL).bind(&lanes));
    measure!(
        "blob_policy_denied_hash",
        sqlx::query(DENIED_HASH_SQL).bind(&[7_u8; 32][..])
    );

    let mut table: Vec<(&str, f64, f64)> = results
        .iter()
        .map(|(name, ms)| {
            let mut sorted = ms.clone();
            sorted.sort_by(f64::total_cmp);
            (*name, common::percentile(&sorted, 0.95), *sorted.last().unwrap())
        })
        .collect();
    table.sort_by(|a, b| b.1.total_cmp(&a.1));
    eprintln!(
        "hub-registry/steady_state_queries: {} queries, {REPEATS} runs each; the slowest by p95:",
        table.len()
    );
    for (name, p95, max) in table.iter().take(8) {
        eprintln!("  {name}: p95 {p95:.2} ms, max {max:.2} ms");
    }
    let all: Vec<f64> = results.iter().flat_map(|(_, ms)| ms.iter().copied()).collect();
    let slow: Vec<_> = table.iter().filter(|(_, p95, _)| *p95 > LIMIT_MS).collect();
    assert!(
        slow.is_empty(),
        "queries whose p95 is over {LIMIT_MS} ms: {slow:?}"
    );
    let path = write_result("hub-registry/steady_state_queries", "ms", &all);
    eprintln!("hub-registry/steady_state_queries: {}", path.display());
}
