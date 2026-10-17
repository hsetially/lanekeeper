//! P12: the list of statements that `benches/steady_state_queries.rs` times is complete, and it runs (05 fix round 1, round 2).
//!
//! The benchmark says it times every query the read API makes, with the binds a real request has. Nothing held it to that: its own list
//! and its own binds drifted from the code. The drift statement's sixth parameter (T9) was missing from the benchmark through T9 and T10 and
//! panicked when the gate ran it, and three statements were never timed at all. The list is now one module of the test support
//! (`support::read_statements`) that the benchmark and these tests share, and the gate holds it four ways, in `cargo test`:
//!
//! 1. every `pub const`, `pub static` and `pub fn` of the SQL of `src/read/*.rs` and of `src/blob_policy.rs` is in the list, or is on an
//!    explicit allow-list with the reason it is not a statement of its own (found by a scan of the source, like the other source scans);
//! 2. every statement in the list has as many binds as the highest placeholder in its SQL (no database needed);
//! 3. every statement in the list runs once, with its real binds, over a real Postgres at a small scale, so a bind that does not fit its
//!    statement (a wrong count, a wrong type) fails here and not when somebody runs the benchmark;
//! 4. **every statement of `read::` and of `blob_policy` is either a named one that is timed, or an inline one on an explicit allow-list.**
//!    The scan of (1) sees only names (`pub const *_SQL`, `pub fn *_sql`), so a statement written inline at its call site
//!    (`sqlx::query("...")`, `query_as`, `query_scalar`, `raw_sql`, their macros, or a built `format!`) was invisible to it. The scan of
//!    (4) finds every one of those in `src/read/**` and `src/blob_policy.rs`, and each must be in [`INLINE_ALLOWED`], with the reason it
//!    is a bounded lookup that needs no timing of its own (a key or prefix lookup, or the read of a table that has one row per swimlane
//!    under a `LIMIT`). An inline statement that is not allowed, and an allowance whose statement is gone, fail the test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

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

// ------------------------------------------------------------------------------------------------ inline statements

/// The statements that the read modules and the blob policy run **inline** (a string at the call site, not a named constant), and why each
/// is not in the P12 list. Each entry is the module, the start of the statement's text (whitespace collapsed, as [`inline_statements`]
/// gives it) and the reason. The statements cannot be listed without moving them into constants, which is a change of the production
/// code that a statement of the list must not need: the list's entries are the service's own constants and not copies of them.
/// Timings are p95 over 200 runs at the target scale (40 swimlanes of 2,000 files), measured once with the plans in review round 2.
const INLINE_ALLOWED: [(&str, &str, &str); 4] = [
    (
        "read::search_settings",
        "SELECT id FROM swimlanes ORDER BY id LIMIT 1000",
        "the ids of all swimlanes, for a search that names none: not a lookup by key but the read of one small table that has one row per \
         swimlane (40 at the design scale, a page of it), in primary key order and bounded by LIMIT 1000; the plan is a sort of a seq scan \
         of that page, p95 0.7 ms",
    ),
    (
        "read::search_text",
        "SELECT id FROM swimlanes ORDER BY id LIMIT 1000",
        "the same statement as the settings search's: all swimlane ids, one row per swimlane, in primary key order, bounded by LIMIT 1000; \
         the table is a page at the design scale, p95 0.7 ms",
    ),
    (
        "read::swimlanes",
        "SELECT EXISTS (SELECT 1 FROM swimlanes WHERE id = $1)",
        "whether one swimlane exists: a primary key lookup of the swimlanes table (the planner reads the single page of its 40 rows whole, \
         where a larger table would take the index), p95 0.5 ms",
    ),
    (
        "read::history",
        "SELECT EXISTS (SELECT 1 FROM file_observations WHERE swimlane_id = $1 AND path = $2",
        "`read_is_version`: three EXISTS, each a key or prefix lookup of one path and one hash (an index only scan of \
         `file_observations_replay_key`, `file_state_prefix_idx`, `baselines_pkey`: the EXPLAIN at the target scale shows no scan of a \
         table), each stops at its first match, p95 0.5 ms",
    ),
];

/// The files the scan reads: every `.rs` under `src/read` (at any depth) and `src/blob_policy.rs`, each with its module path
/// (`read::swimlanes`, `read` for `read/mod.rs`, `blob_policy`).
fn scanned_files() -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, module: &str, out: &mut Vec<(String, PathBuf)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                walk(&path, &format!("{module}::{stem}"), out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = if stem == "mod" {
                    module.to_owned()
                } else {
                    format!("{module}::{stem}")
                };
                out.push((name, path));
            }
        }
    }
    let mut files = Vec::new();
    walk(&src().join("read"), "read", &mut files);
    files.push(("blob_policy".to_owned(), src().join("blob_policy.rs")));
    files
}

/// Collapses whitespace runs to one space and trims.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of the string literal that starts at `rest` (a plain `"..."` with `\` escapes and line continuations, or a raw `r"..."` /
/// `r#"..."#`), whitespace collapsed, or `None` if `rest` does not start with one.
fn string_literal(rest: &str) -> Option<String> {
    if let Some(raw) = rest.strip_prefix('r') {
        let hashes = raw.chars().take_while(|c| *c == '#').count();
        let body = raw[hashes..].strip_prefix('"')?;
        let close = format!("\"{}", "#".repeat(hashes));
        let end = body.find(&close)?;
        return Some(squash(&body[..end]));
    }
    let body = rest.strip_prefix('"')?;
    let mut text = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(squash(&text)),
            '\\' => match chars.next()? {
                // A line continuation: the newline and the whitespace after it are not part of the string.
                '\n' => while chars.next_if(|n| n.is_whitespace()).is_some() {},
                other => text.push(other),
            },
            other => text.push(other),
        }
    }
    None
}

/// Every statement that `source` hands to sqlx as something other than a name: a string literal (plain or raw) or a built one
/// (`format!`, `concat!`, `include_str!`), after `sqlx::query`, `query_as`, `query_scalar`, `query_with`, `raw_sql` or their macros, with
/// or without a turbofish. The text is the literal's, whitespace collapsed (a built one is `built: ` and its first literal, or `built`).
/// A call whose argument is a constant, a variable or a function call (`FILE_SQL`, `sql`, `&history_sql()`) is a named statement, which
/// the scan of the names covers. A call in a comment is not a call.
fn inline_statements(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = source[from..].find("sqlx::") {
        let start = from + at;
        from = start + "sqlx::".len();
        let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
        if source[line_start..start].contains("//") {
            continue;
        }
        let ident: String = source[from..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !(ident.starts_with("query") || ident == "raw_sql") {
            continue;
        }
        let mut rest = &source[from + ident.len()..];
        rest = rest.strip_prefix('!').unwrap_or(rest);
        if let Some(turbofish) = rest.strip_prefix("::<") {
            // Balanced angle brackets of the generic arguments.
            let mut depth = 1;
            let mut end = None;
            for (i, c) in turbofish.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { continue };
            rest = &turbofish[end..];
        }
        let Some(args) = rest.trim_start().strip_prefix('(') else {
            continue; // a path (`sqlx::query::Query`), not a call
        };
        let arg = args.trim_start();
        let arg = arg.strip_prefix('&').map_or(arg, str::trim_start);
        if let Some(text) = string_literal(arg) {
            found.push(text);
        } else if let Some(built) = ["format!(", "concat!(", "include_str!("]
            .iter()
            .find_map(|m| arg.strip_prefix(m))
        {
            found.push(match string_literal(built.trim_start()) {
                Some(text) => format!("built: {text}"),
                None => "built".to_owned(),
            });
        }
    }
    found
}

/// The imports in `source` that bring `query`, `query_as`, `query_scalar`, `query_with` or `raw_sql` into scope by name, which the scan
/// of the calls (`sqlx::query(`) could not see.
fn unqualified_sqlx_imports(source: &str) -> Vec<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("use sqlx::"))
        .filter(|line| {
            let names = line.trim_start_matches("use sqlx::").trim_end_matches(';');
            names
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|word| word.starts_with("query") || word == "raw_sql")
        })
        .map(str::to_owned)
        .collect()
}

/// The problems with the inline statements `found` (module, text) against `allow` (module, start of the text, reason): a statement that
/// is not allowed, an allowance for a statement that is not there, and an allowance without a reason.
fn inline_problems(found: &[(String, String)], allow: &[(&str, &str, &str)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (module, text) in found {
        let allowed = allow
            .iter()
            .any(|(m, start, _)| m == module && text.starts_with(&squash(start)));
        if !allowed {
            problems.push(format!(
                "`{module}` runs an inline statement `{text}` that is neither a named constant of the P12 list nor in INLINE_ALLOWED: make it a `pub const *_SQL` and add it to support::read_statements::statements, or add it to INLINE_ALLOWED with the reason it is a bounded lookup"
            ));
        }
    }
    for (module, start, reason) in allow {
        if !found
            .iter()
            .any(|(m, text)| m == module && text.starts_with(&squash(start)))
        {
            problems.push(format!(
                "INLINE_ALLOWED names `{start}` in `{module}`, which the source no longer runs inline: remove the entry"
            ));
        }
        if reason.trim().len() < 40 {
            problems.push(format!(
                "the reason for `{start}` in INLINE_ALLOWED says nothing about why it is bounded"
            ));
        }
    }
    problems
}

/// Every inline statement of the scanned files, with its module, and the unqualified imports (as problems).
fn scanned_inline() -> (Vec<(String, String)>, Vec<String>) {
    let mut found = Vec::new();
    let mut imports = Vec::new();
    for (module, path) in scanned_files() {
        let source = fs::read_to_string(&path).unwrap();
        for text in inline_statements(&source) {
            found.push((module.clone(), text));
        }
        for import in unqualified_sqlx_imports(&source) {
            imports.push(format!(
                "`{module}` imports `{import}`: the scan of inline statements sees `sqlx::query(` and not a bare `query(`, so write the call with its path"
            ));
        }
    }
    (found, imports)
}

#[test]
fn every_inline_statement_of_the_read_modules_and_the_blob_policy_is_allowed_with_a_reason() {
    let (found, imports) = scanned_inline();
    assert!(imports.is_empty(), "{imports:#?}");
    // The scan sees what it is meant to see (a scan that found nothing would pass every check): the four call sites that the scan of the
    // names could not see, and not the call sites of the named statements.
    assert!(
        found.len() >= INLINE_ALLOWED.len(),
        "the inline statements of src/read and src/blob_policy.rs: {found:#?}"
    );
    let problems = inline_problems(&found, &INLINE_ALLOWED);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_scan_of_inline_statements_finds_what_it_must_and_nothing_else() {
    let found = |source: &str| inline_statements(source);
    // A plain literal, a multi-line one with a line continuation, and the shapes of the call.
    assert_eq!(found("sqlx::query(\"SELECT 1\")"), ["SELECT 1"]);
    assert_eq!(
        found("sqlx::query_scalar::<_, i64>(\n    \"SELECT 2 \\\n         FROM t\"\n)"),
        ["SELECT 2 FROM t"]
    );
    assert_eq!(
        found("sqlx::query_as::<_, (String, Vec<u8>)>(\"SELECT 3\")"),
        ["SELECT 3"]
    );
    assert_eq!(found("sqlx::raw_sql(r#\"SELECT \"4\"\"#)"), ["SELECT \"4\""]);
    assert_eq!(found("sqlx::query!(\"SELECT 5\", a)"), ["SELECT 5"]);
    assert_eq!(found("sqlx::query_with(\"SELECT 6\", args)"), ["SELECT 6"]);
    assert_eq!(found("sqlx::query(&\"SELECT 7\")"), ["SELECT 7"]);
    // A statement that is built at the call site is found too.
    assert_eq!(
        found("sqlx::query(&format!(\"SELECT {}\", x))"),
        ["built: SELECT {}"]
    );
    assert_eq!(found("sqlx::query(&concat!(a, b))"), ["built"]);
    // Named statements are the other scan's: a constant, a variable, a function call.
    assert!(found("sqlx::query(FILE_SQL)").is_empty());
    assert!(found("sqlx::query_scalar(BASELINE_SQL).bind(1)").is_empty());
    assert!(found("sqlx::query(sql)").is_empty());
    assert!(found("sqlx::query(&history_sql())").is_empty());
    assert!(found("sqlx::query_as::<_, Row>(&findings_sql())").is_empty());
    // Not calls: a type path, other items of sqlx, a comment.
    assert!(found("let q: sqlx::query::Query<'_, Postgres, PgArguments> = x;").is_empty());
    assert!(found("use sqlx::{PgPool, Row};\nlet p: sqlx::PgPool = x;").is_empty());
    assert!(found("// sqlx::query(\"SELECT 1\")\n/// sqlx::query(\"SELECT 2\")").is_empty());
    assert!(found("let x = 1; // sqlx::query(\"SELECT 1\")").is_empty());
    // Two calls in one source, in order, and a call after a test module is still a call.
    assert_eq!(
        found("sqlx::query(\"A\")\n#[cfg(test)]\nmod tests {}\nfn later() { sqlx::query_scalar(\"B\"); }"),
        ["A", "B"]
    );
    // The imports that would hide a call from the scan.
    assert_eq!(unqualified_sqlx_imports("use sqlx::query;"), ["use sqlx::query;"]);
    assert_eq!(
        unqualified_sqlx_imports("use sqlx::{PgPool, query_scalar};").len(),
        1
    );
    assert!(unqualified_sqlx_imports("use sqlx::{PgPool, Row};\nuse sqlx::postgres::PgRow;").is_empty());
}

#[test]
fn the_inline_check_finds_a_statement_nobody_allowed_and_an_allowance_nobody_needs() {
    let real = scanned_inline().0;
    assert!(
        inline_problems(&real, &INLINE_ALLOWED).is_empty(),
        "the real source passes"
    );
    // A new inline statement in a read module.
    let mut with_new = real.clone();
    with_new.push((
        "read::tree".to_owned(),
        "SELECT count(*) FROM file_state".to_owned(),
    ));
    let problems = inline_problems(&with_new, &INLINE_ALLOWED);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(problems[0].contains("read::tree") && problems[0].contains("SELECT count(*)"));
    // The same text in another module is not allowed by an entry for this one.
    let mut elsewhere = real.clone();
    elsewhere.push((
        "read::tree".to_owned(),
        "SELECT id FROM swimlanes ORDER BY id LIMIT 1000".to_owned(),
    ));
    assert_eq!(inline_problems(&elsewhere, &INLINE_ALLOWED).len(), 1);
    // An allowance whose statement is gone from the source.
    let without: Vec<(String, String)> = real
        .iter()
        .filter(|(m, _)| m != "read::swimlanes")
        .cloned()
        .collect();
    assert!(
        inline_problems(&without, &INLINE_ALLOWED)
            .iter()
            .any(|p| p.contains("no longer runs inline"))
    );
    // An allowance with no reason.
    let silent = [(
        "read::swimlanes",
        "SELECT EXISTS (SELECT 1 FROM swimlanes WHERE id = $1)",
        "fine",
    )];
    let only_swimlanes: Vec<(String, String)> = real
        .iter()
        .filter(|(m, _)| m == "read::swimlanes")
        .cloned()
        .collect();
    assert!(
        inline_problems(&only_swimlanes, &silent)
            .iter()
            .any(|p| p.contains("says nothing"))
    );
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
