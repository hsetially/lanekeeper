//! The opt-in run against the real repositories (plan decision A4, acceptance 4). It needs two working-tree checkouts that
//! this repository never holds (AGENTS.md rule 12):
//!
//! - `LK_REAL_BASE_DIR`: `configuration-base-saas`, default branch (the `config/` folder, or the checkout itself);
//! - `LK_REAL_TENANT_DIR`: `csp-tenant-data`, branch `sit1` (the `data/config/` folder, or the checkout itself).
//!
//! Run it with `just engine-real-snapshot`. Without the two variables the recipe says `real-snapshot: NOT RUN` and the gate
//! does not count it as passed; the test itself refuses to run, so it can never pass vacuously.
//!
//! What it prints is counts, file names and setting paths. Never a value and never a line of a file: the files may hold
//! secrets, and a log is not a place for them (S10, S13).
//!
//! T3 adds the first expectation: on every structured file of both checkouts, tree-sitter and the parser agree on where every
//! setting is. Later tasks add the rest (C1, C2 and C8 of `sit1` in T6, the 13 ambiguous names of C9 in T10).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use engine::{Limits, LocationSource, ParseWarning, Syntax, index_settings};

/// The checkouts named by the environment. Panics when they are not set: a run that cannot happen is not a run that passed.
fn real_dirs() -> (PathBuf, PathBuf) {
    let read = |name: &str| -> PathBuf {
        let value = std::env::var_os(name).unwrap_or_else(|| {
            panic!("real-snapshot: NOT RUN ({name} is not set; see tests/real_snapshot.rs)")
        });
        let dir = PathBuf::from(value);
        assert!(dir.is_dir(), "real-snapshot: {name} is not a directory");
        dir
    };
    (read("LK_REAL_BASE_DIR"), read("LK_REAL_TENANT_DIR"))
}

fn syntax_of(name: &str) -> Option<Syntax> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "yml" | "yaml" => Some(Syntax::Yaml),
        "json" => Some(Syntax::Json),
        "properties" => Some(Syntax::Properties),
        _ => None,
    }
}

/// Every regular file below `dir` (not following links, not entering `.git`), sorted, as (path relative to `dir` with `/`,
/// path on disk).
fn files_below(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("cannot read a directory below {}: {}", base.display(), e.kind()))
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let Ok(kind) = entry.file_type() else { continue };
            if entry.file_name() == ".git" {
                continue;
            }
            if kind.is_dir() {
                walk(base, &entry.path(), out);
            } else if kind.is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, entry.path()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

/// What a run over one checkout found.
#[derive(Debug, Default)]
struct Report {
    files: usize,
    settings: usize,
    yaml_files: usize,
    /// Structured files the engine refused (too big, not valid): not a disagreement, but listed.
    refused: Vec<String>,
    /// Places where tree-sitter and the parser disagree, or tree-sitter could not read a file. Names only.
    disagreements: Vec<String>,
}

/// Indexes every structured file below `root` and records where tree-sitter and the parser do not agree.
fn check_locations(root: &Path) -> Report {
    let limits = Limits::default();
    let mut report = Report::default();
    for (rel, path) in files_below(root) {
        let Some(syntax) = syntax_of(&rel) else { continue };
        let Ok(bytes) = fs::read(&path) else {
            report.refused.push(format!("{rel}: unreadable"));
            continue;
        };
        let index = match index_settings(&bytes, syntax, &limits) {
            Ok(index) => index,
            Err(error) => {
                // The error says what was wrong and where, never the text.
                report.refused.push(format!("{rel}: {error}"));
                continue;
            }
        };
        report.files += 1;
        report.settings += index.rows.len();
        if syntax == Syntax::Yaml {
            report.yaml_files += 1;
            if index.located_by != LocationSource::TreeSitter {
                report
                    .disagreements
                    .push(format!("{rel}: tree-sitter could not read the file"));
            }
            for warning in &index.warnings {
                if let ParseWarning::LocationMismatch { path: Some(setting) } = warning {
                    report.disagreements.push(format!("{rel}: {setting}"));
                }
            }
        }
    }
    report
}

fn summary(label: &str, report: &Report) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "real-snapshot: {label}: {} files indexed ({} YAML), {} settings, {} refused, {} disagreements",
        report.files,
        report.yaml_files,
        report.settings,
        report.refused.len(),
        report.disagreements.len()
    )
    .unwrap();
    for line in &report.refused {
        writeln!(out, "real-snapshot: {label}: refused {line}").unwrap();
    }
    for line in &report.disagreements {
        writeln!(out, "real-snapshot: {label}: DISAGREES {line}").unwrap();
    }
    out
}

/// Acceptance 4, first part (T3): on every real file, the tree-sitter location agrees with the parser's.
#[test]
#[ignore = "needs LK_REAL_BASE_DIR and LK_REAL_TENANT_DIR: just engine-real-snapshot"]
fn locations_agree_on_every_real_file() {
    let (base, tenant) = real_dirs();
    let mut failed = false;
    for (label, dir) in [("base", base), ("tenant", tenant)] {
        let report = check_locations(&dir);
        print!("{}", summary(label, &report));
        assert!(
            report.files > 0,
            "real-snapshot: {label}: no structured file found below the directory"
        );
        failed |= !report.disagreements.is_empty();
    }
    assert!(
        !failed,
        "real-snapshot: tree-sitter and the parser disagree (see above)"
    );
    println!("real-snapshot: locations: PASS");
}

// --- the run itself is tested on made-up files, so that it cannot pass by doing nothing -------------------------------

mod harness {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_checkout_where_the_readers_agree_is_reported_clean() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "svc/app.yml", "a: 1\nb:\n  c: |\n    x\n  d: 2\n");
        write(dir.path(), "svc/app-sit1.yaml", "a: 2\r\n");
        write(dir.path(), "svc/app.properties", "k=v\n");
        write(dir.path(), "svc/data.json", "{\"a\": 1}");
        write(dir.path(), "svc/logo.bmp", "BM not text");
        write(dir.path(), ".git/config.yml", "ignored: [");
        let report = check_locations(dir.path());
        assert_eq!(
            (report.files, report.yaml_files, report.settings),
            (4, 2, 6),
            "{report:?}"
        );
        assert!(
            report.disagreements.is_empty() && report.refused.is_empty(),
            "{report:?}"
        );
    }

    #[test]
    fn a_disagreement_and_a_refusal_are_named_and_never_show_a_value() {
        let dir = tempfile::tempdir().unwrap();
        // An explicit block scalar key: tree-sitter does not decode it, so it does not find the setting.
        write(
            dir.path(),
            "svc/odd.yml",
            "? |\n  key\n: S3CR3T-VALUE-ONE\nok: 1\n",
        );
        write(dir.path(), "svc/broken.yml", "a: [S3CR3T-VALUE-TWO\n");
        let report = check_locations(dir.path());
        assert_eq!(report.disagreements.len(), 1, "{report:?}");
        assert!(report.disagreements[0].starts_with("svc/odd.yml: "));
        assert_eq!(report.refused.len(), 1, "{report:?}");
        assert!(report.refused[0].starts_with("svc/broken.yml: "));
        let text = summary("base", &report);
        assert!(!text.contains("S3CR3T"), "a value reached the report: {text}");
        assert!(text.contains("DISAGREES svc/odd.yml"), "{text}");
        assert!(text.contains("1 disagreements"), "{text}");
    }
}
