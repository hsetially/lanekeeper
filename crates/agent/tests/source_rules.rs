//! Rules about the source itself (S17, S21), checked by reading `src/`.
//!
//! These are cheap guards that fail the moment someone reaches for the wrong API: a test that only covers today's
//! code would miss the next file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// Files that may open something with ambient authority. Each is a startup reader that runs before any hub command:
/// the NFS root (`root.rs`) and the pinned hub CA file (`tls.rs`, read once, with a size limit, through a cap-std `Dir`
/// opened on its directory). T9 adds the spool directory.
const AMBIENT_ALLOWED: &[&str] = &["root.rs", "tls.rs"];

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

/// The SAN prefix is written once, in `identity/cert.rs`, and everything else refers to the constant (S5, decision A5).
#[test]
fn agent_san_defined_in_one_constant() {
    const PREFIX: &str = "spiffe://lanekeeper/swimlane/";
    const OLD_FORM: &str = "spiffe://lanekeeper/agent/";
    let files = sources(&src_dir());
    let mut spelled_out = Vec::new();
    for (rel, source) in &files {
        for (n, line) in source.lines().enumerate() {
            // Comments may describe the form; only code can define it.
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(!line.contains(OLD_FORM), "{rel}:{}: the old SAN form (A5)", n + 1);
            if line.contains(PREFIX) {
                spelled_out.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert_eq!(
        spelled_out.len(),
        1,
        "the SAN prefix must be defined once:\n{}",
        spelled_out.join("\n")
    );
    assert!(
        spelled_out[0].starts_with("identity/cert.rs:") && spelled_out[0].contains("AGENT_SAN_PREFIX"),
        "{}",
        spelled_out[0]
    );
    // And the one place that checks a certificate uses the constant.
    let cert = files.iter().find(|(rel, _)| rel == "identity/cert.rs").unwrap();
    assert!(cert.1.contains("strip_prefix(AGENT_SAN_PREFIX)"));
}

/// Lines that name the generated wire types outside the transport directory. Comments are skipped.
fn generated_type_uses(rel: &str, source: &str) -> Vec<String> {
    if rel.starts_with("transport/") {
        return Vec::new();
    }
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| has_token(line, "proto::pb") || has_token(line, "pb::"))
        .map(|(n, line)| format!("{rel}:{}: {}", n + 1, line.trim()))
        .collect()
}

/// S11: everything the hub sends is validated by `proto::convert` before the agent acts on it. The generated messages
/// are used in `transport/` only, so no other module can read an unvalidated path, hash or length.
#[test]
fn generated_wire_types_stay_in_the_transport_directory() {
    let files = sources(&src_dir());
    let violations: Vec<String> = files
        .iter()
        .flat_map(|(rel, source)| generated_type_uses(rel, source))
        .collect();
    assert!(
        violations.is_empty(),
        "generated wire types outside transport/:\n{}",
        violations.join("\n")
    );
    // The scan is not blind: the transport does use them.
    assert!(
        files
            .iter()
            .any(|(rel, source)| rel.starts_with("transport/") && has_token(source, "pb::HubMessage"))
    );
    // And it catches a planted use.
    assert_eq!(
        generated_type_uses("scan.rs", "fn f(m: proto::pb::HubMessage) {}").len(),
        1
    );
    assert_eq!(
        generated_type_uses("dispatch.rs", "use proto::pb;\nlet m: pb::ReadFile = x;").len(),
        2
    );
    assert!(generated_type_uses("transport/wire.rs", "use proto::pb;").is_empty());
}

/// Rule 5: every channel is bounded.
#[test]
fn no_unbounded_channels() {
    for (rel, source) in sources(&src_dir()) {
        for (n, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("unbounded_channel") && !line.contains("UnboundedSender"),
                "{rel}:{}: an unbounded channel (code rule 5)",
                n + 1
            );
        }
    }
}

/// Decision A1 / D89: the walker is the agent's own, over cap-std. The `ignore` crate walks by path and would follow a
/// directory swapped for a symlink, so it must not come back as a dependency.
#[test]
fn the_agent_does_not_depend_on_the_ignore_crate() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for line in manifest.lines().filter(|l| !l.trim_start().starts_with('#')) {
        let name = line.split(['=', '.']).next().unwrap_or("").trim();
        assert_ne!(
            name, "ignore",
            "the agent walks with its own cap-std walker (D89): {line}"
        );
    }
    for (rel, source) in sources(&src_dir()) {
        for (n, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("WalkParallel") && !line.contains("ignore::"),
                "{rel}:{}: the `ignore` crate's walker (D89)",
                n + 1
            );
        }
    }
}
