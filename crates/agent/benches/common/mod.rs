//! Shared by the agent's benchmarks (T4): where the target-scale fixtures are, and how to copy one swimlane.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// `$CARGO_TARGET_DIR`, or the workspace's `target`.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    )
}

/// Swimlane 1 of the target-scale fixtures (about 2,000 files): `$LK_FIXTURES_DIR/nfs/<first swimlane>`, with the
/// directory defaulting to `target/fixtures/target`. The benchmarks measure at design scale, never on toy data
/// (`lanekeeper-performance`), so a missing fixture is an error with the command that makes it, not a skip.
pub fn swimlane_one() -> PathBuf {
    let base = std::env::var_os("LK_FIXTURES_DIR")
        .map_or_else(|| target_dir().join("fixtures/target"), PathBuf::from);
    let nfs = base.join("nfs");
    let mut swimlanes: Vec<PathBuf> = fs::read_dir(&nfs)
        .unwrap_or_else(|e| {
            panic!(
                "no fixtures at {}: {e}. Run `cargo xtask gen-fixtures --scale target` first.",
                nfs.display()
            )
        })
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    swimlanes.sort();
    swimlanes
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{} has no swimlane", nfs.display()))
}

/// Copy a directory tree (regular files and directories only).
pub fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Number of regular files below `dir`.
pub fn count_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                count_files(&e.path())
            } else {
                usize::from(e.file_type().unwrap().is_file())
            }
        })
        .sum()
}
