//! Rules of `AGENTS.md` that a scan of the source can hold the crate to: no unsafe code (S21, rule 1), no panics outside tests
//! (rule 2), nothing unbounded (rule 5), no SQL built from input (S11, A39), every event through the outbox (D80).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn sources() -> Vec<(String, String)> {
    let src = crate_dir().join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    files
        .into_iter()
        .map(|f| {
            let name = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            (name, fs::read_to_string(f).unwrap())
        })
        .collect()
}

/// The production part of a source file: everything before the first `#[cfg(test)]` line (the test module closes each file),
/// without comment lines (doc comments may show `.unwrap()` in prose).
fn production_text(source: &str) -> String {
    source
        .lines()
        .take_while(|l| l.trim() != "#[cfg(test)]")
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn forbid_unsafe_present() {
    let lib = fs::read_to_string(crate_dir().join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![forbid(unsafe_code)]"));
    for (name, text) in sources() {
        assert!(!text.contains("unsafe "), "{name} uses unsafe");
        assert!(!text.contains("allow(unsafe_code)"), "{name} allows unsafe");
    }
}

/// Every construct that rule 2 of `AGENTS.md` denies in non-test code, found by text. `unwrap_or*` and `expect_err` stay legal.
fn panic_sites(production: &str) -> Vec<&'static str> {
    const BANNED: [&str; 12] = [
        ".unwrap()",
        ".expect(",
        "panic!(",
        "todo!(",
        "unimplemented!(",
        "dbg!(",
        "allow(clippy::unwrap_used",
        "allow(clippy::expect_used",
        "allow(clippy::panic",
        "allow(clippy::todo",
        "allow(clippy::unimplemented",
        "allow(clippy::dbg_macro",
    ];
    BANNED
        .into_iter()
        .filter(|banned| production.contains(banned))
        .collect()
}

#[test]
fn no_panic_outside_tests() {
    let manifest = fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("[lints]\nworkspace = true"),
        "the lints come from the workspace"
    );
    let workspace = fs::read_to_string(crate_dir().join("../../Cargo.toml")).unwrap();
    for lint in [
        "unwrap_used",
        "expect_used",
        "panic",
        "todo",
        "unimplemented",
        "dbg_macro",
    ] {
        assert!(
            workspace.contains(&format!("{lint} = \"deny\"")),
            "the workspace must deny clippy::{lint}"
        );
    }
    let mut checked = 0;
    for (name, text) in sources() {
        checked += 1;
        let found = panic_sites(&production_text(&text));
        assert!(
            found.is_empty(),
            "{name} has panicking code outside its test module: {found:?}"
        );
    }
    assert!(
        checked >= 10,
        "the scan must cover the crate, covered {checked} files"
    );
}

#[test]
fn panic_scan_finds_planted_violations() {
    for (plant, expected) in [
        ("fn f(x: Option<u8>) -> u8 { x.unwrap() }", ".unwrap()"),
        ("fn f(x: Option<u8>) -> u8 { x.expect(\"set\") }", ".expect("),
        ("fn f() { panic!(\"no\") }", "panic!("),
        ("fn f() { todo!() }", "todo!("),
        (
            "#[allow(clippy::unwrap_used)]\nfn f() {}",
            "allow(clippy::unwrap_used",
        ),
    ] {
        assert!(
            panic_sites(&production_text(plant)).contains(&expected),
            "the scan missed {expected} in {plant}"
        );
    }
    let clean = "// x.unwrap() in prose\nfn f(x: Option<u8>) -> u8 { x.unwrap_or(0) }\n#[cfg(test)]\nmod tests { fn t() { Some(1).unwrap(); } }";
    assert!(panic_sites(&production_text(clean)).is_empty());
}

/// The text of the `format!` macro call that starts at `at` (the index of `format!(`), up to its closing parenthesis. Parentheses
/// inside string literals do not count.
fn macro_call(text: &str, at: usize) -> &str {
    let bytes = text.as_bytes();
    let mut i = at + "format!(".len();
    let mut depth = 1;
    let mut in_string = false;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'(' if !in_string => depth += 1,
            b')' if !in_string => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    &text[at..i.min(text.len())]
}

/// Splits a `format!(...)` call into its string literal and the arguments after it.
fn literal_and_args(call: &str) -> Option<(&str, &str)> {
    let open = call.find('"')?;
    let bytes = call.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'"' => return Some((&call[open + 1..i], &call[i + 1..])),
            _ => {}
        }
        i += 1;
    }
    None
}

/// SQL is written as a string with binds. The one thing allowed to be formatted into it is the output of the two helpers of
/// `db.rs` (`ts` and `ms`), which turn a placeholder or a column name written in this crate into a time expression, given as
/// `name = ts("$3")` or positionally.
fn sql_formatting_violations(production: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = production[from..].find("format!(") {
        let at = from + found;
        from = at + 1;
        let call = macro_call(production, at);
        let Some((literal, args)) = literal_and_args(call) else {
            continue;
        };
        let upper = literal.to_uppercase();
        if !["SELECT ", "INSERT ", "UPDATE ", "DELETE "]
            .iter()
            .any(|v| upper.contains(v))
        {
            continue;
        }
        let mut args = args.trim_end_matches(')').to_owned();
        for helper in ["ts(", "ms("] {
            while let Some(start) = args.find(helper) {
                let close = args[start..].find(')').map_or(args.len(), |c| start + c + 1);
                args.replace_range(start..close, "");
            }
        }
        // What is left may only be `name =` bindings of the removed helper calls.
        let mut rest: String = args.chars().filter(|c| !c.is_whitespace() && *c != ',').collect();
        while !rest.is_empty() {
            let ident = rest
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                .count();
            if ident > 0 && rest[ident..].starts_with('=') {
                rest = rest[ident + 1..].to_owned();
            } else {
                out.push(call.lines().next().unwrap_or_default().trim().to_owned());
                break;
            }
        }
    }
    out
}

#[test]
fn the_sql_scan_finds_planted_violations() {
    let bad = "fn f(table: &str) -> String { format!(\"SELECT * FROM {table} WHERE a = 1\", table = table) }";
    assert_eq!(sql_formatting_violations(bad).len(), 1);
    let positional = "fn f(x: &str) { let _ = format!(\"DELETE FROM t WHERE x = '{}'\", x); }";
    assert_eq!(sql_formatting_violations(positional).len(), 1);
    let fine = "fn f() { let _ = format!(\"UPDATE t SET at = {} WHERE a = $1\", ts(\"$2\")); }";
    assert!(sql_formatting_violations(fine).is_empty());
    let named = "fn f() { let _ = format!(\"INSERT INTO t (a, b) VALUES ({now}, {t})\", now = ts(\"$3\"), t = ms(\"x\")); }";
    assert!(sql_formatting_violations(named).is_empty());
    let prose = "fn f(x: u8) { let _ = format!(\"{x} files were seen\"); }";
    assert!(
        sql_formatting_violations(prose).is_empty(),
        "text that is not SQL is not the scan's business"
    );
}

#[test]
fn no_offset_no_unbounded_channel_no_format_built_sql_no_direct_publish() {
    for (name, text) in sources() {
        let production = production_text(&text);
        let upper = production.to_uppercase();
        assert!(
            !upper.contains(" OFFSET "),
            "{name}: P12 forbids OFFSET pagination"
        );
        assert!(!production.contains("unbounded_channel"), "{name}: rule 5");
        assert!(
            !production.contains(".publish("),
            "{name}: D80, every event goes through publish_in_tx so that it commits with its change"
        );
        let bad = sql_formatting_violations(&production);
        assert!(
            bad.is_empty(),
            "{name} formats something other than a time helper into SQL: {bad:?}"
        );
    }
}

#[test]
fn every_query_is_bounded_or_keyed() {
    // P12: a SELECT that can return many rows has a LIMIT, or is an aggregate, or reads by key. The scan is deliberately simple:
    // each SELECT ... FROM statement in a string literal must contain LIMIT, count(, sum(, a key comparison (`= $`), or `= ANY(`.
    for (name, text) in sources() {
        let production = production_text(&text);
        let mut rest = production.as_str();
        while let Some(i) = rest.find("SELECT ") {
            let statement: String = rest[i..].chars().take_while(|c| *c != ';').take(900).collect();
            rest = &rest[i + 7..];
            let end = statement.find("\")").unwrap_or(statement.len());
            let s = &statement[..end];
            if !s.contains(" FROM ") {
                continue;
            }
            let lower = s.to_lowercase();
            let bounded = lower.contains(" limit ")
                || lower.contains("count(")
                || lower.contains("sum(")
                || lower.contains("= $")
                || lower.contains("= any(")
                || lower.contains("fetch_one")
                || lower.contains("on conflict");
            assert!(
                bounded,
                "{name}: an unbounded SELECT: {}",
                s.lines().next().unwrap_or_default()
            );
        }
    }
}

/// The text of a source file with every run of whitespace (and every line continuation of a string literal) collapsed to one
/// space and the case folded, so that a statement written over several lines is found by its words.
fn squeezed(production: &str) -> String {
    production
        .replace("\\\n", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A22: the table `git_tree_index` has one writer, hub-platform's `TreeIndexPersist` (lease `git-tree-index`). The registry reads the
/// index through `GitReader::tree_index` and never writes the table, so two writers can never disagree about a commit.
fn writes_git_tree_index(production: &str) -> bool {
    let text = squeezed(production);
    [
        "insert into git_tree_index",
        "update git_tree_index",
        "delete from git_tree_index",
        "truncate git_tree_index",
    ]
    .iter()
    .any(|statement| text.contains(statement))
}

#[test]
fn the_scan_for_git_tree_index_writes_finds_planted_violations() {
    for plant in [
        "sqlx::query(\"INSERT INTO git_tree_index (repo) VALUES ($1)\")",
        "sqlx::query(\"INSERT INTO\n   git_tree_index (repo) VALUES ($1)\")",
        "sqlx::query(\"DELETE   FROM git_tree_index WHERE x\")",
        "sqlx::query(\"insert into \\\n    git_tree_index (repo) values ($1)\")",
    ] {
        assert!(writes_git_tree_index(plant), "the scan missed: {plant}");
    }
    assert!(!writes_git_tree_index(
        "sqlx::query(\"SELECT hash FROM git_tree_index WHERE repo = $1\")"
    ));
}

#[test]
fn no_source_writes_the_git_tree_index() {
    for (name, text) in sources() {
        assert!(
            !writes_git_tree_index(&production_text(&text)),
            "{name}: A22, hub-platform's TreeIndexPersist is the only writer of git_tree_index"
        );
    }
}
