//! Rules about the source itself (S17, S21), checked by reading `src/`.
//!
//! These are cheap guards that fail the moment someone reaches for the wrong API: a test that only covers today's
//! code would miss the next file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// Files that may open something with ambient authority. Each is a startup reader that runs before any hub command:
/// the NFS root (`root.rs`). T3 adds the hub CA file and T9 the spool directory.
const AMBIENT_ALLOWED: &[&str] = &["root.rs"];

/// APIs that reach the filesystem without a cap-std `Dir`.
const FORBIDDEN_ALWAYS: &[&str] = &[
    "std::fs",
    "tokio::fs",
    "File::open",
    "File::create",
    "File::options",
];

/// APIs that create a capability from ambient authority.
const AMBIENT_ONLY_IN_ALLOWED: &[&str] = &[
    "ambient_authority",
    "open_ambient",
    "from_std_file",
    "from_raw_fd",
];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `dir`, as (path relative to `src/`, contents).
fn sources(dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// True when `token` appears in `line` as a whole path, so `std::fs` matches but `cap_std::fs` does not.
fn has_token(line: &str, token: &str) -> bool {
    line.match_indices(token).any(|(at, _)| {
        line[..at]
            .chars()
            .next_back()
            .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
    })
}

/// Lines of code that break the file-access rule. Comment lines are skipped.
fn file_access_violations(rel: &str, source: &str, ambient_allowed: &[&str]) -> Vec<String> {
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let may_open_ambient = ambient_allowed.contains(&file_name);
    let mut found = Vec::new();
    for (n, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let uses_std_fs_in_a_group = line.trim_start().starts_with("use std::{") && has_token(line, "fs");
        let forbidden = FORBIDDEN_ALWAYS.iter().any(|t| has_token(line, t)) || uses_std_fs_in_a_group;
        let ambient = !may_open_ambient && AMBIENT_ONLY_IN_ALLOWED.iter().any(|t| has_token(line, t));
        if forbidden || ambient {
            found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }
    found
}

#[test]
fn all_file_access_via_cap_std() {
    let files = sources(&src_dir());
    assert!(files.len() >= 4, "the scan found only {} files", files.len());
    let violations: Vec<String> = files
        .iter()
        .flat_map(|(rel, source)| file_access_violations(rel, source, AMBIENT_ALLOWED))
        .collect();
    assert!(
        violations.is_empty(),
        "file access outside cap-std:\n{}",
        violations.join("\n")
    );
    // The allow-list is not a dead entry: root.rs really is the one place the root is opened.
    let root = files.iter().find(|(rel, _)| rel == "root.rs").unwrap();
    assert!(root.1.contains("open_ambient_dir"));
}

#[test]
fn file_access_scan_rejects_planted_violations() {
    let planted = [
        "let b = std::fs::read(p)?;",
        "let f = tokio::fs::File::open(p).await?;",
        "let f = File::open(p)?;",
        "use std::{fs, io};",
        "let d = Dir::open_ambient_dir(p, ambient_authority())?;",
    ];
    for line in planted {
        assert_eq!(
            file_access_violations("tree/walk.rs", line, AMBIENT_ALLOWED).len(),
            1,
            "{line}"
        );
    }
    // The ambient open is allowed in an allow-listed file, and comments are not code.
    let ambient = "let d = Dir::open_ambient_dir(p, ambient_authority())?;";
    assert!(file_access_violations("root.rs", ambient, AMBIENT_ALLOWED).is_empty());
    assert!(
        file_access_violations("tree/walk.rs", "// std::fs::read is forbidden", AMBIENT_ALLOWED).is_empty()
    );
    assert!(file_access_violations("tree/walk.rs", "/// see `std::fs`", AMBIENT_ALLOWED).is_empty());
    // A cap-std path that merely contains `fs::` is fine.
    assert!(file_access_violations("tree/walk.rs", "use cap_std::fs::Dir;", AMBIENT_ALLOWED).is_empty());
}

#[test]
fn forbid_unsafe_present() {
    let src = src_dir();
    for file in ["lib.rs", "main.rs"] {
        let text = fs::read_to_string(src.join(file)).unwrap();
        assert!(
            text.contains("#![forbid(unsafe_code)]"),
            "{file} lacks #![forbid(unsafe_code)]"
        );
    }
}

#[test]
fn lints_inherit_the_workspace_policy() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let lints = manifest.split("[lints]").nth(1).expect("a [lints] table");
    let first = lints.lines().find(|l| !l.trim().is_empty()).unwrap();
    assert_eq!(first.trim(), "workspace = true");
}
