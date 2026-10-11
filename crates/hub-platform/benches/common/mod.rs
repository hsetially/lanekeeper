//! Shared by the benchmarks (T7, T8): where the target-scale fixtures are.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bytes::Bytes;

/// `$CARGO_TARGET_DIR`, or the workspace's `target`.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    )
}

/// The two bare repositories of the target-scale fixtures: `$LK_FIXTURES_DIR/repos`, defaulting to
/// `target/fixtures/target/repos`. The benchmarks measure at design scale, never on toy data (`lanekeeper-performance`), so a
/// missing fixture is an error with the command that makes it, not a skip.
pub fn fixture_repos() -> (PathBuf, PathBuf) {
    let base = std::env::var_os("LK_FIXTURES_DIR")
        .map_or_else(|| target_dir().join("fixtures/target"), PathBuf::from)
        .join("repos");
    let (b, t) = (
        base.join("configuration-base-saas.git"),
        base.join("csp-tenant-data.git"),
    );
    for repo in [&b, &t] {
        assert!(
            repo.join("HEAD").is_file(),
            "no fixture repository at {}. Run `cargo xtask gen-fixtures --scale target` first.",
            repo.display()
        );
    }
    (b, t)
}

/// The target-scale NFS fixture tree: `$LK_FIXTURES_DIR/nfs`, defaulting to `target/fixtures/target/nfs`.
pub fn fixture_nfs() -> PathBuf {
    let nfs = std::env::var_os("LK_FIXTURES_DIR")
        .map_or_else(|| target_dir().join("fixtures/target"), PathBuf::from)
        .join("nfs");
    assert!(
        nfs.is_dir(),
        "no fixture files at {}. Run `cargo xtask gen-fixtures --scale target` first.",
        nfs.display()
    );
    nfs
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The distinct file contents of the first `swimlanes` swimlanes of the target-scale fixtures, in a fixed order: about 2,000 files
/// per swimlane from 40 bytes to the 1.8 MiB report transform, CRLF and binary files among them. Real shapes, never toy data.
pub fn fixture_blobs(swimlanes: usize) -> Vec<Bytes> {
    let nfs = fixture_nfs();
    let mut lanes: Vec<PathBuf> = std::fs::read_dir(&nfs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    lanes.sort();
    let mut files = Vec::new();
    for lane in lanes.into_iter().take(swimlanes) {
        walk(&lane, &mut files);
    }
    let mut seen = HashSet::new();
    let mut blobs = Vec::new();
    for file in files {
        let content = std::fs::read(&file).unwrap();
        if seen.insert(ports::content_hash(&content)) {
            blobs.push(Bytes::from(content));
        }
    }
    blobs
}
