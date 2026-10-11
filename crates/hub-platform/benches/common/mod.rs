//! Shared by the Git benchmarks (T7): where the target-scale fixture repositories are.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

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
