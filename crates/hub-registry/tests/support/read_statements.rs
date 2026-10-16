//! The read API's statements with the binds a real request gives them: ONE list, shared by the P12 benchmark
//! (`benches/steady_state_queries.rs`, which times it at the target scale) and by `tests/read_statements.rs` (which runs it once at a
//! small scale in the normal gate, and checks that it is complete). The statements are the ones the service runs (`pub const` and `pub fn`
//! of `hub_registry::read` and `hub_registry::blob_policy`), not copies, and each entry's label is written from the very path that
//! supplies its SQL (`stmt!`), so a label cannot name a statement that it does not hold.
//!
//! Why one list: a benchmark that listed its own statements and binds drifted from the code. The sixth parameter of the drift statement
//! (`SEVERITY_RANK_TOP`, added by T9) was missing from the benchmark for the whole of T9 and T10 and was found only when the benchmark was
//! run at the gate, and three statements (`AUTO_NOTIFY_SQL`, `DRIFT_BY_PATH_SQL`, `DRIFT_PIVOT_SQL`) were never timed although the
//! benchmark said it timed "every query the read API makes".
//!
//! Binds are plain data ([`Param`]) so that the number of them can be compared with the statement's placeholders without a database, and
//! the values come from the database itself ([`read_world`]): a real swimlane, folder, path, hashes and cursor values, whatever the scale.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use hub_registry::read::effective::app_candidates;
use hub_registry::read::rows::SEVERITY_RANK_TOP;
use hub_registry::read::search_settings::{LANE_BLOBS_SQL, PLACES_PER_STATEMENT};
use hub_registry::read::settings::ROWS_PER_STATEMENT;
use sqlx::Postgres;
use sqlx::postgres::{PgArguments, PgPool};
use sqlx::query::Query;

/// One bind of a statement.
#[derive(Debug, Clone)]
pub enum Param {
    Text(String),
    NullText,
    Int(i64),
    NullInt,
    Int32(i32),
    Float(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    TextList(Vec<String>),
    BytesList(Vec<Vec<u8>>),
}

/// One statement of the read API with its binds.
#[derive(Debug, Clone)]
pub struct ReadStatement {
    /// What the benchmark prints and the test says.
    pub name: &'static str,
    /// Where the SQL comes from, as `read::content::FILE_SQL` or `blob_policy::DENIED_HASH_SQL` (the path below `hub_registry`).
    pub from: String,
    pub sql: String,
    pub params: Vec<Param>,
    /// Whether the statement is prepared and cached by the connection. The settings search plans for its own values, so it is not.
    pub persistent: bool,
}

impl ReadStatement {
    /// The query, with every bind applied.
    pub fn query(&self) -> Query<'_, Postgres, PgArguments> {
        let mut q = sqlx::query(&self.sql).persistent(self.persistent);
        for p in &self.params {
            q = match p {
                Param::Text(s) => q.bind(s.clone()),
                Param::NullText => q.bind(None::<String>),
                Param::Int(n) => q.bind(*n),
                Param::NullInt => q.bind(None::<i64>),
                Param::Int32(n) => q.bind(*n),
                Param::Float(f) => q.bind(*f),
                Param::Bool(b) => q.bind(*b),
                Param::Bytes(b) => q.bind(b.clone()),
                Param::TextList(l) => q.bind(l.clone()),
                Param::BytesList(l) => q.bind(l.clone()),
            };
        }
        q
    }

    /// The number of the highest placeholder (`$7`) in the SQL, which is how many binds it needs.
    pub fn placeholders(&self) -> usize {
        highest_placeholder(&self.sql)
    }
}

/// The highest `$n` in `sql` (0 when there is none).
pub fn highest_placeholder(sql: &str) -> usize {
    let bytes = sql.as_bytes();
    let mut highest = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let digits: String = sql[i + 1..].chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = digits.parse::<usize>() {
                highest = highest.max(n);
            }
            i += 1 + digits.len();
        } else {
            i += 1;
        }
    }
    highest
}

fn label(path: &str) -> String {
    path.chars().filter(|c| !c.is_whitespace()).collect()
}

/// A statement whose SQL is a constant of the crate: `stmt!("name", read::content::FILE_SQL, [binds])`.
macro_rules! stmt {
    ($name:literal, $($seg:ident)::+, [$($param:expr),* $(,)?]) => {
        ReadStatement {
            name: $name,
            from: label(stringify!($($seg)::+)),
            sql: (hub_registry::$($seg)::+).to_owned(),
            params: vec![$($param),*],
            persistent: true,
        }
    };
    ($name:literal, $($seg:ident)::+, [$($param:expr),* $(,)?], not_persistent) => {
        ReadStatement {
            persistent: false,
            ..stmt!($name, $($seg)::+, [$($param),*])
        }
    };
}

/// A statement whose SQL a function of the crate builds: `stmt_fn!("name", read::history::history_sql, [binds])`.
macro_rules! stmt_fn {
    ($name:literal, $($seg:ident)::+, [$($param:expr),* $(,)?]) => {
        ReadStatement {
            name: $name,
            from: label(stringify!($($seg)::+)),
            sql: hub_registry::$($seg)::+(),
            params: vec![$($param),*],
            persistent: true,
        }
    };
}

/// The values the binds are made of, found in the database.
#[derive(Debug, Clone)]
pub struct ReadWorld {
    /// Every swimlane.
    pub lanes: Vec<String>,
    /// The first eight (the most that the settings search lists blobs for).
    pub eight: Vec<String>,
    /// The swimlane most statements are asked about (the eighth, or the last).
    pub lane: String,
    /// A swimlane whose files are drifted: the worst case for the drift list by path, which sorts the whole matching set of a page.
    pub drift_lane: String,
    /// The drift lane's path to resume after, and the `seq` of its row (what a path-ordered cursor holds).
    pub drift_resume_path: String,
    pub drift_resume_seq: i64,
    pub folder: String,
    pub path: String,
    pub page_paths: Vec<String>,
    /// Hashes of structured files of the lane (what a page of places asks about).
    pub hashes: Vec<Vec<u8>>,
    /// The blobs with the most settings, and typical ones (a few dozen settings).
    pub largest: Vec<Vec<u8>>,
    pub typical: Vec<Vec<u8>>,
    /// The blobs the settings search lists for `eight`.
    pub listed: Vec<Vec<u8>>,
    /// A content that many files have had (`popular` observations were added): the worst case for the blob policy's lookup of paths.
    pub popular_hash: Vec<u8>,
}

/// The values for the binds, from the database of a bed or of a test. `drift_lane` is the swimlane that the drift-by-path statement is
/// asked about (the caller made its files drifted); `popular` is how many observations of one content to add (0 for none).
pub async fn read_world(pool: &PgPool, drift_lane: Option<&str>, popular: usize) -> ReadWorld {
    let lanes: Vec<String> = sqlx::query_scalar("SELECT id FROM swimlanes ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap();
    assert!(!lanes.is_empty(), "no swimlane in the database");
    let lane = lanes.get(7).unwrap_or(&lanes[0]).clone();
    let eight: Vec<String> = lanes.iter().take(8).cloned().collect();
    let paths_of = |lane: String| async move {
        sqlx::query_scalar::<_, String>("SELECT path FROM file_state WHERE swimlane_id = $1 ORDER BY path")
            .bind(lane)
            .fetch_all(pool)
            .await
            .unwrap()
    };
    let paths = paths_of(lane.clone()).await;
    assert!(!paths.is_empty(), "the swimlane {lane} has no file");
    let folder = paths
        .iter()
        .find_map(|p| p.split_once('/').map(|(d, _)| d.to_owned()))
        .unwrap_or_default();
    let path = paths[paths.len() / 2].clone();
    let page_paths: Vec<String> = paths
        .iter()
        .step_by((paths.len() / 101).max(1))
        .take(101)
        .cloned()
        .collect();
    let hashes: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT DISTINCT nfs_hash::bytea FROM file_state WHERE swimlane_id = $1 AND class = 'structured' AND nfs_hash IS NOT NULL LIMIT 200",
    )
    .bind(&lane)
    .fetch_all(pool)
    .await
    .unwrap();
    let rows = i64::try_from(ROWS_PER_STATEMENT).unwrap();
    let largest: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT blob_hash::bytea FROM settings_index GROUP BY blob_hash ORDER BY count(*) DESC, blob_hash LIMIT $1",
    )
    .bind(rows)
    .fetch_all(pool)
    .await
    .unwrap();
    let typical: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT blob_hash::bytea FROM settings_index GROUP BY blob_hash HAVING count(*) BETWEEN 30 AND 50 ORDER BY blob_hash LIMIT $1",
    )
    .bind(rows)
    .fetch_all(pool)
    .await
    .unwrap();
    let listed: Vec<Vec<u8>> = sqlx::query_scalar(LANE_BLOBS_SQL)
        .persistent(false)
        .bind(&eight)
        .bind(None::<String>)
        .bind(20_001_i64)
        .fetch_all(pool)
        .await
        .unwrap();
    let drift_lane = drift_lane.map_or_else(|| lane.clone(), str::to_owned);
    let drift_paths = paths_of(drift_lane.clone()).await;
    let drift_resume_path = drift_paths[drift_paths.len() / 2].clone();
    let drift_resume_seq: i64 =
        sqlx::query_scalar("SELECT seq FROM file_state WHERE swimlane_id = $1 AND path = $2")
            .bind(&drift_lane)
            .bind(&drift_resume_path)
            .fetch_one(pool)
            .await
            .unwrap();
    let popular_hash = ports::content_hash(b"a content that a great many files have had\n")
        .as_bytes()
        .to_vec();
    if popular > 0 {
        sqlx::query(
            "INSERT INTO file_observations (swimlane_id, path, hash, source, class, observed_at) \
             SELECT $1, 'popular/f' || n || '.yml', $2, 'scan', 'structured', now() - (n || ' seconds')::interval \
             FROM generate_series(1, $3::int) n",
        )
        .bind(&lane)
        .bind(&popular_hash)
        .bind(i32::try_from(popular).unwrap())
        .execute(pool)
        .await
        .unwrap();
    }
    ReadWorld {
        lanes,
        eight,
        lane,
        drift_lane,
        drift_resume_path,
        drift_resume_seq,
        folder,
        path,
        page_paths,
        hashes,
        largest,
        typical,
        listed,
        popular_hash,
    }
}

/// A world with no data behind it, for what needs only the shape of the list (its labels and the number of its binds).
pub fn dummy_world() -> ReadWorld {
    let one = || "x".to_owned();
    ReadWorld {
        lanes: vec![one()],
        eight: vec![one()],
        lane: one(),
        drift_lane: one(),
        drift_resume_path: one(),
        drift_resume_seq: 1,
        folder: one(),
        path: "a/b.yml".to_owned(),
        page_paths: vec![one()],
        hashes: vec![vec![1; 32]],
        largest: vec![vec![2; 32]],
        typical: vec![vec![3; 32]],
        listed: vec![vec![4; 32]],
        popular_hash: vec![5; 32],
    }
}

/// Every statement the read API runs, with the binds a real request gives it. This is the list the benchmark times and the test runs.
pub fn statements(w: &ReadWorld) -> Vec<ReadStatement> {
    use Param::{Bool, Bytes, BytesList, Float, Int, Int32, NullInt, NullText, Text, TextList};
    let lane = || Text(w.lane.clone());
    let lanes = || TextList(w.lanes.clone());
    let eight = || TextList(w.eight.clone());
    let path = || Text(w.path.clone());
    let like_folder = format!("{}/%", w.folder);
    let folder_chars = i32::try_from(w.folder.chars().count() + 2).unwrap();
    let logical = domain::LogicalFile::parse(&w.path).unwrap();
    let candidates = app_candidates(&logical);
    let roots: Vec<String> = ["application.yml", "application.yaml", "application.properties"]
        .map(str::to_owned)
        .to_vec();
    let names: Vec<String> = (0..12).map(|i| format!("catalog-{i}.yml")).collect();
    let tenant_copy = vec![
        format!("{}/chan/x-sit1.yml", w.folder),
        format!("{}/chan/x.yml", w.folder),
    ];
    let places_blobs: Vec<Vec<u8>> = w.hashes.iter().take(PLACES_PER_STATEMENT).cloned().collect();
    let drift_lane = || Text(w.drift_lane.clone());
    // The worst case of the drift list by path: every state but in sync, every path, the page limit plus one, and the same resumed after a
    // path in the middle of the swimlane (the statement orders the whole matching set before it takes a page).
    let page = Int(501);
    vec![
        stmt!(
            "swimlanes",
            read::swimlanes::SWIMLANES_SQL,
            [NullText, Int(101), Float(30.0)]
        ),
        stmt!("swimlane", read::swimlanes::SWIMLANE_SQL, [lane(), Float(30.0)]),
        stmt!("auto_notify", read::swimlanes::AUTO_NOTIFY_SQL, [lane()]),
        stmt!("existing_swimlanes", read::effective::EXISTING_SQL, [lanes()]),
        stmt!(
            "tree_children_of_the_root",
            read::tree::CHILDREN_SQL,
            [
                lane(),
                Text("%".to_owned()),
                Int32(1),
                NullText,
                Int(200_001),
                NullText,
                Int(101)
            ]
        ),
        stmt!(
            "tree_children_of_a_folder",
            read::tree::CHILDREN_SQL,
            [
                lane(),
                Text(like_folder),
                Int32(folder_chars),
                NullText,
                Int(200_001),
                NullText,
                Int(501)
            ]
        ),
        stmt!(
            "tree_file_rows",
            read::tree::FILE_ROWS_SQL,
            [lane(), TextList(w.page_paths.clone())]
        ),
        // `$6` is the severity rank at which the list resumes: the top one for the first page, as the service binds it (T9 made the list
        // most serious first and added the parameter).
        stmt!(
            "drift_first_page",
            read::tree::DRIFT_SQL,
            [
                lane(),
                Int(0),
                NullText,
                Text("%".to_owned()),
                Int(501),
                Int32(SEVERITY_RANK_TOP)
            ]
        ),
        stmt!(
            "drift_by_path_first_page_of_a_drifted_swimlane",
            read::tree::DRIFT_BY_PATH_SQL,
            [
                drift_lane(),
                NullText,
                Text("%".to_owned()),
                NullText,
                page.clone()
            ]
        ),
        stmt!(
            "drift_by_path_resumed_page_of_a_drifted_swimlane",
            read::tree::DRIFT_BY_PATH_SQL,
            [
                drift_lane(),
                NullText,
                Text("%".to_owned()),
                Text(w.drift_resume_path.clone()),
                page
            ]
        ),
        stmt!(
            "drift_pivot",
            read::tree::DRIFT_PIVOT_SQL,
            [drift_lane(), Int(w.drift_resume_seq)]
        ),
        stmt_fn!(
            "history",
            read::history::history_sql,
            [lane(), path(), NullInt, NullInt, Int(101)]
        ),
        stmt_fn!(
            "findings_of_a_swimlane",
            read::findings::findings_sql,
            [lane(), TextList(Vec::new()), Int32(0), NullInt, Int(501)]
        ),
        stmt_fn!(
            "findings_of_all_swimlanes",
            read::findings::findings_sql,
            [
                NullText,
                TextList(vec!["drift".to_owned()]),
                Int32(2),
                NullInt,
                Int(501)
            ]
        ),
        stmt_fn!(
            "services",
            read::restarts::services_sql,
            [lane(), Int(0), Bool(true), Int(501)]
        ),
        stmt!("content_file", read::content::FILE_SQL, [lane(), path()]),
        stmt!("content_baseline", read::content::BASELINE_SQL, [lane(), path()]),
        stmt!(
            "compare_baseline_blob",
            read::compare::BASELINE_BLOB_SQL,
            [lane(), path()]
        ),
        stmt!("tenants_of_a_swimlane", read::content::TENANTS_SQL, [lane()]),
        stmt!("path_mapping", read::content::MAPPING_SQL, []),
        // The statements that read the settings of blobs take a few blobs each (`ROWS_PER_STATEMENT`): the biggest such statement is that
        // many of the biggest blobs (about 6,000 settings each), a typical one that many blobs of a few dozen settings.
        stmt!(
            "setting_rows_of_the_largest_blobs",
            read::settings::ROWS_SQL,
            [BytesList(w.largest.clone()), Int(1_000)]
        ),
        stmt!(
            "setting_rows_of_typical_blobs",
            read::settings::ROWS_SQL,
            [BytesList(w.typical.clone()), Int(1_000)]
        ),
        stmt!(
            "effective_group_rows_40_swimlanes",
            read::effective::GROUP_ROWS_SQL,
            [lanes(), Text(w.path.clone()), TextList(roots)]
        ),
        stmt!("effective_tenants", read::effective::TENANTS_SQL, [lanes()]),
        stmt!(
            "effective_applications",
            read::effective::APPS_SQL,
            [lanes(), TextList(candidates)]
        ),
        stmt!(
            "resource_rows_by_path",
            read::resource::ROWS_BY_PATH_SQL,
            [lane(), TextList(tenant_copy)]
        ),
        stmt!(
            "resource_property_rows",
            read::resource::PROPERTY_ROWS_SQL,
            [lane(), TextList(names)]
        ),
        stmt!("resource_all_paths", read::resource::ALL_PATHS_SQL, [lane()]),
        stmt!(
            "compare_tree_directory_hash",
            read::compare_tree::DIR_HASH_SQL,
            [lane(), Text(w.folder.clone())]
        ),
        stmt!(
            "compare_tree_child_directories",
            read::compare_tree::CHILD_DIRS_SQL,
            [lane(), TextList(vec![String::new()])]
        ),
        stmt!(
            "compare_tree_files_of_directories",
            read::compare_tree::DIR_FILES_SQL,
            [lane(), TextList(vec![w.folder.clone()])]
        ),
        stmt!(
            "compare_tree_effective_map",
            read::compare_tree::EFFECTIVE_MAP_SQL,
            [lane(), Text("%".to_owned())]
        ),
        stmt!(
            "compare_tree_nfs_map",
            read::compare_tree::NFS_MAP_SQL,
            [lane(), Text("%".to_owned())]
        ),
        stmt!(
            "compare_tree_baseline_map",
            read::compare_tree::BASELINE_MAP_SQL,
            [lane(), Text("%".to_owned())]
        ),
        // Over a few swimlanes (at most eight, the default) the blobs that are current there are listed and the settings read among them;
        // over many the settings are read untested and the places say which are current. Every one of these is planned for its own values.
        stmt!(
            "settings_search_lane_blobs_eight_swimlanes",
            read::search_settings::LANE_BLOBS_SQL,
            [eight(), NullText, Int(20_001)],
            not_persistent
        ),
        stmt!(
            "settings_search_pairs_dense_listed_blobs",
            read::search_settings::PAIRS_SQL,
            [
                Text("%name%".to_owned()),
                BytesList(w.listed.clone()),
                Bytes(Vec::new()),
                Text(String::new()),
                Int(102),
                Int32(255)
            ],
            not_persistent
        ),
        stmt!(
            "settings_search_pairs_absent_listed_blobs",
            read::search_settings::PAIRS_SQL,
            [
                Text("%zzzqqqxx%".to_owned()),
                BytesList(w.listed.clone()),
                Bytes(Vec::new()),
                Text(String::new()),
                Int(102),
                Int32(255)
            ],
            not_persistent
        ),
        stmt!(
            "settings_search_pairs_dense_untested",
            read::search_settings::PAIRS_UNTESTED_SQL,
            [
                Text("%name%".to_owned()),
                Bytes(Vec::new()),
                Text(String::new()),
                Int(102),
                Int32(255)
            ],
            not_persistent
        ),
        stmt!(
            "settings_search_pairs_absent_untested",
            read::search_settings::PAIRS_UNTESTED_SQL,
            [
                Text("%zzzqqqxx%".to_owned()),
                Bytes(Vec::new()),
                Text(String::new()),
                Int(102),
                Int32(255)
            ],
            not_persistent
        ),
        stmt!(
            "settings_search_places",
            read::search_settings::PLACES_SQL,
            [lanes(), BytesList(places_blobs), NullText]
        ),
        stmt!(
            "text_search_files_of_a_swimlane",
            read::search_text::FILES_SQL,
            [lane()]
        ),
        stmt!("text_search_versions", read::search_text::VERSIONS_SQL, [lanes()]),
        // The blob endpoint's policy: the probe of the flag for a hash nothing has had, and the paths of the files that have had a content
        // (one that a typical file has, and the worst case, a content that the limit and one more observations have).
        stmt!(
            "blob_policy_denied_hash",
            blob_policy::DENIED_HASH_SQL,
            [Bytes(vec![7_u8; 32])]
        ),
        stmt!(
            "blob_policy_hash_paths_of_a_typical_content",
            blob_policy::HASH_PATHS_SQL,
            [
                Bytes(w.hashes.first().cloned().unwrap_or_else(|| vec![7_u8; 32])),
                Int(i64::try_from(hub_registry::blob_policy::HASH_PATHS_LIMIT + 1).unwrap())
            ]
        ),
        stmt!(
            "blob_policy_hash_paths_of_the_most_common_content",
            blob_policy::HASH_PATHS_SQL,
            [
                Bytes(w.popular_hash.clone()),
                Int(i64::try_from(hub_registry::blob_policy::HASH_PATHS_LIMIT + 1).unwrap())
            ]
        ),
    ]
}
