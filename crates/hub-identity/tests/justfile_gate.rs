//! The `verify-03a` gate in the Justfile is wired the way the plan says, and its fuzz list matches the crate's targets.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn justfile() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Justfile")).unwrap()
}

#[test]
fn verify_03a_runs_every_sub_recipe() {
    let text = justfile();
    let line = text
        .lines()
        .find(|l| l.starts_with("verify-03a:"))
        .expect("a verify-03a recipe");
    for dep in [
        "identity-tools-check",
        "identity-lint",
        "identity-test",
        "identity-fuzz",
        "identity-bench",
    ] {
        assert!(line.contains(dep), "verify-03a does not run {dep}");
        assert!(
            text.contains(&format!("\n{dep}:")) || text.contains(&format!("\n{dep}: ")),
            "no recipe {dep}"
        );
    }
    assert!(!text.contains("verify-03a not implemented"));
}

#[test]
fn identity_fuzz_list_matches_the_fuzz_targets_directory() {
    let text = justfile();
    let list = text
        .lines()
        .find_map(|l| l.strip_prefix("identity_fuzz_targets := \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("identity_fuzz_targets");
    let in_justfile: BTreeSet<&str> = list.split_whitespace().collect();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/fuzz_targets");
    let on_disk: BTreeSet<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".rs")
                .to_owned()
        })
        .collect();
    assert_eq!(
        in_justfile
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<BTreeSet<_>>(),
        on_disk
    );
}

#[test]
fn the_identity_test_recipe_requires_docker_rather_than_skipping() {
    assert!(justfile().contains("LK_REQUIRE_DOCKER=1 cargo test -p hub-identity"));
}
