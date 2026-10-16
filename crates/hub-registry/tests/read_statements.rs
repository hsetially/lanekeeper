//! P12: the list of statements that `benches/steady_state_queries.rs` times is complete, and it runs (05 fix round 1).
//!
//! The benchmark says it times every query the read API makes, with the binds a real request has. Nothing held it to that: its own list
//! and its own binds drifted from the code. The drift statement's sixth parameter (T9) was missing from the benchmark through T9 and T10 and
//! panicked when the gate ran it, and three statements were never timed at all. The list is now one module of the test support
//! (`support::read_statements`) that the benchmark and these tests share, and the gate holds it three ways, in `cargo test`:
//!
//! 1. every `pub const`, `pub static` and `pub fn` of the SQL of `src/read/*.rs` and of `src/blob_policy.rs` is in the list, or is on an
//!    explicit allow-list with the reason it is not a statement of its own (found by a scan of the source, like the other source scans);
//! 2. every statement in the list has as many binds as the highest placeholder in its SQL (no database needed);
//! 3. every statement in the list runs once, with its real binds, over a real Postgres at a small scale, so a bind that does not fit its
//!    statement (a wrong count, a wrong type) fails here and not when somebody runs the benchmark.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use hub_registry::blob_policy::HASH_PATHS_LIMIT;
use support::read_statements::{ReadStatement, dummy_world, highest_placeholder, read_world, statements};
use support::reads::Reads;

/// What the scanned modules hold under an `_SQL` name that is not a statement of its own, and why. Each entry needs a reason, and an entry
/// that is not in the source (or is in the list) is a mistake.
const NOT_STATEMENTS: [(&str, &str); 2] = [
    (
        "read::rows::STATE_RANK_SQL",
        "a CASE expression that `tree.rs` embeds in its statements (`state_rank_sql!`); it is never run on its own",
    ),
    (
        "read::rows::SEVERITY_RANK_SQL",
        "a CASE expression that `tree.rs` embeds in its statements (`severity_rank_sql!`); it is never run on its own",
    ),
];

fn src() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The names that `source` declares as SQL: `pub const NAME_SQL`, `pub static NAME_SQL` and `pub fn name_sql(`.
fn sql_names(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = line.strip_prefix("pub ")?;
            let (kind, rest) = rest.split_once(' ')?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            match kind {
                "const" | "static" if name.ends_with("_SQL") => Some(name),
                "fn" if name.ends_with("_sql") => Some(name),
                _ => None,
            }
        })
        .collect()
}

/// `read::content::FILE_SQL`, `blob_policy::DENIED_HASH_SQL`, ...: every SQL name of the modules that the read API's statements live in.
fn declared() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut files: Vec<(String, std::path::PathBuf)> = fs::read_dir(src().join("read"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            (stem != "mod").then(|| (format!("read::{stem}"), p.clone()))
        })
        .collect();
    files.push(("blob_policy".to_owned(), src().join("blob_policy.rs")));
    for (module, path) in files {
        for name in sql_names(&fs::read_to_string(&path).unwrap()) {
            found.insert(format!("{module}::{name}"));
        }
    }
    found
}

/// The problems with `declared` against what is `listed` and what is explained: a statement that nothing times, an explanation that is
/// not needed, and an explanation without a reason.
fn coverage_problems(
    declared: &BTreeSet<String>,
    listed: &BTreeSet<String>,
    allow: &BTreeMap<&str, &str>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for name in declared {
        if !listed.contains(name) && !allow.contains_key(name.as_str()) {
            problems.push(format!(
                "`{name}` is a statement of the read API that the P12 benchmark does not time: add it to support::read_statements::statements, or to NOT_STATEMENTS with the reason"
            ));
        }
    }
    for (name, reason) in allow {
        if !declared.contains(*name) {
            problems.push(format!(
                "NOT_STATEMENTS names `{name}`, which the source no longer declares"
            ));
        }
        if listed.contains(*name) {
            problems.push(format!(
                "`{name}` is in the list and in NOT_STATEMENTS: remove it from one"
            ));
        }
        if reason.trim().len() < 20 {
            problems.push(format!("the reason for `{name}` in NOT_STATEMENTS says nothing"));
        }
    }
    problems
}

fn listed() -> BTreeSet<String> {
    statements(&dummy_world()).into_iter().map(|s| s.from).collect()
}

#[test]
fn every_sql_of_the_read_modules_and_the_blob_policy_is_in_the_list_or_explained() {
    let declared = declared();
    assert!(
        declared.len() > 35,
        "the scan of the source found only {} statements: {declared:?}",
        declared.len()
    );
    let allow: BTreeMap<&str, &str> = NOT_STATEMENTS.into_iter().collect();
    let problems = coverage_problems(&declared, &listed(), &allow);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_coverage_check_finds_a_statement_nobody_times() {
    let allow: BTreeMap<&str, &str> = NOT_STATEMENTS.into_iter().collect();
    let real = declared();
    let listed = listed();
    assert!(
        coverage_problems(&real, &listed, &allow).is_empty(),
        "the real source passes"
    );
    // A new statement in a read module that the list does not have.
    let mut with_ghost = real.clone();
    with_ghost.extend(
        sql_names(
            "pub const GHOST_SQL: &str = \"SELECT 1\";\npub fn ghost_sql() -> String { String::new() }\n",
        )
        .into_iter()
        .map(|n| format!("read::ghost::{n}")),
    );
    let problems = coverage_problems(&with_ghost, &listed, &allow);
    assert_eq!(problems.len(), 2, "{problems:#?}");
    assert!(problems.iter().all(|p| p.contains("read::ghost::")));
    // A statement dropped from the list.
    let mut without = listed.clone();
    assert!(without.remove("read::tree::DRIFT_BY_PATH_SQL"));
    assert!(
        coverage_problems(&real, &without, &allow)
            .iter()
            .any(|p| p.contains("read::tree::DRIFT_BY_PATH_SQL")),
        "a statement that is no longer timed is found"
    );
    // An allow-list entry that is stale, in the list, or has no reason.
    let stale: BTreeMap<&str, &str> =
        [("read::rows::GONE_SQL", "a reason that is long enough to count")].into();
    assert!(
        coverage_problems(&real, &listed, &stale)
            .iter()
            .any(|p| p.contains("no longer declares"))
    );
    let in_list: BTreeMap<&str, &str> =
        [("read::tree::DRIFT_SQL", "a reason that is long enough to count")].into();
    assert!(
        coverage_problems(&real, &listed, &in_list)
            .iter()
            .any(|p| p.contains("in the list and in"))
    );
    let silent: BTreeMap<&str, &str> = [("read::rows::STATE_RANK_SQL", "-")].into();
    assert!(
        coverage_problems(&real, &listed, &silent)
            .iter()
            .any(|p| p.contains("says nothing"))
    );
}

#[test]
fn sql_names_finds_constants_statics_and_functions_and_nothing_else() {
    let source = "\
pub const A_SQL: &str = \"x\";
pub static B_SQL: LazyLock<String> = x;
pub fn c_sql() -> String { x }
pub(crate) const D_SQL: &str = \"x\";
const E_SQL: &str = \"x\";
pub const NOT_A_QUERY: usize = 1;
pub fn helper() {}
";
    assert_eq!(sql_names(source), ["A_SQL", "B_SQL", "c_sql"]);
}

#[test]
fn every_statement_has_as_many_binds_as_its_highest_placeholder() {
    let world = dummy_world();
    let list = statements(&world);
    assert!(list.len() > 40, "{} statements", list.len());
    let wrong: Vec<String> = list
        .iter()
        .filter(|s| s.placeholders() != s.params.len())
        .map(|s| {
            format!(
                "{} ({}): {} placeholders, {} binds",
                s.name,
                s.from,
                s.placeholders(),
                s.params.len()
            )
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
    let names: BTreeSet<&str> = list.iter().map(|s| s.name).collect();
    assert_eq!(
        names.len(),
        list.len(),
        "every statement of the list has a name of its own"
    );
}

#[test]
fn the_highest_placeholder_is_found() {
    assert_eq!(highest_placeholder("SELECT 1"), 0);
    assert_eq!(highest_placeholder("a = $1 AND b = $12 AND c = $2::text"), 12);
    assert_eq!(highest_placeholder("price in dollars: $ or $x"), 0);
}

/// Statements that must come back with rows at the small scale of the test, so that their binds are known to select real data and not
/// nothing (a statement that returns no rows could be given any bind that has the right type).
const RETURNS_ROWS: [&str; 17] = [
    "swimlanes",
    "swimlane",
    "auto_notify",
    "existing_swimlanes",
    "tree_children_of_the_root",
    "tree_children_of_a_folder",
    "tree_file_rows",
    "drift_first_page",
    "drift_by_path_first_page_of_a_drifted_swimlane",
    "drift_pivot",
    "history",
    "content_file",
    "resource_all_paths",
    "compare_tree_nfs_map",
    "text_search_files_of_a_swimlane",
    "blob_policy_hash_paths_of_a_typical_content",
    "blob_policy_hash_paths_of_the_most_common_content",
];

#[tokio::test]
async fn every_statement_of_the_list_runs_once_with_its_binds_over_a_real_database() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.world
        .inventory(&[
            ("core.yml", "port: 1\nname: core\n"),
            ("conf/a.yml", "name: a\nlist:\n  - x\n"),
            ("conf/b.yml", "name: b\n"),
            ("conf/chan/x.yml", "k: v\n"),
            ("deep/er/c.properties", "name=c\n"),
            ("notes.txt", "plain text\n"),
        ])
        .await;
    r.world.whole().await;
    r.index_all().await;
    let pool = r.world.rig.pool.clone();
    let world = read_world(&pool, None, HASH_PATHS_LIMIT + 1).await;
    let list = statements(&world);

    let mut returned: BTreeMap<&'static str, usize> = BTreeMap::new();
    for s in &list {
        let rows = s.query().fetch_all(&pool).await.unwrap_or_else(|e| {
            panic!(
                "the statement `{}` ({}) does not run with its binds: {e}",
                s.name, s.from
            )
        });
        returned.insert(s.name, rows.len());
    }
    let empty: Vec<&str> = RETURNS_ROWS
        .iter()
        .copied()
        .filter(|name| returned.get(name).copied().unwrap_or(0) == 0)
        .collect();
    assert!(
        empty.is_empty(),
        "these statements returned nothing, so their binds select nothing: {empty:?} ({returned:?})"
    );
    for name in RETURNS_ROWS {
        assert!(list.iter().any(|s| s.name == name), "{name} is not in the list");
    }
    // The worst case of the policy's lookup returns what it is bounded to, and one more than the limit is what tells it from a content that
    // fits.
    assert_eq!(
        returned["blob_policy_hash_paths_of_the_most_common_content"],
        HASH_PATHS_LIMIT + 1
    );
}

#[test]
fn a_statement_with_a_bind_too_few_is_what_the_second_check_catches() {
    // The control of the second check: the sixth parameter of the drift statement, left out as it was before the gate found it.
    let mut list = statements(&dummy_world());
    let drift: &mut ReadStatement = list.iter_mut().find(|s| s.name == "drift_first_page").unwrap();
    assert_eq!(drift.placeholders(), drift.params.len());
    drift.params.pop();
    assert_ne!(drift.placeholders(), drift.params.len());
}
