//! The `verify-06` gate in the Justfile is wired the way the plan says, and its benchmark list matches the registered budgets.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn justfile() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Justfile")).unwrap()
}

#[test]
fn verify_06_runs_every_sub_recipe() {
    let text = justfile();
    let line = text
        .lines()
        .find(|l| l.starts_with("verify-06:"))
        .expect("a verify-06 recipe");
    for dep in [
        "writes-tools-check",
        "writes-lint",
        "writes-test",
        "writes-fuzz",
        "writes-bench",
    ] {
        assert!(line.contains(dep), "verify-06 does not run {dep}");
        assert!(
            text.contains(&format!("\n{dep}:")) || text.contains(&format!("\n{dep}: ")),
            "no recipe {dep}"
        );
    }
    assert!(!text.contains("verify-06 not implemented"));
}

#[test]
fn the_writes_test_recipe_requires_docker_rather_than_skipping() {
    assert!(justfile().contains("LK_REQUIRE_DOCKER=1 cargo test -p hub-writes"));
}

/// The ids of the registered budgets whose benchmark lives in this crate, from `perf/budgets.toml`.
fn registered_writes_budgets() -> BTreeSet<String> {
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml")).unwrap();
    registry
        .split("[[budget]]")
        .skip(1)
        .filter(|block| block.contains("bench = \"hub-writes/") && block.contains("registered = true"))
        .map(|block| {
            block
                .lines()
                .find_map(|l| l.strip_prefix("id = \""))
                .and_then(|l| l.strip_suffix('"'))
                .expect("a budget has an id")
                .to_owned()
        })
        .collect()
}

#[test]
fn writes_bench_judges_exactly_the_registered_writes_budgets() {
    let text = justfile();
    let list = text
        .lines()
        .find_map(|l| l.strip_prefix("writes_budgets := \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("writes_budgets");
    let in_justfile: BTreeSet<String> = list.split_whitespace().map(str::to_owned).collect();
    assert_eq!(in_justfile, registered_writes_budgets());
    assert!(in_justfile.contains("P8") && in_justfile.contains("P12.hub_writes_queries"));

    let recipe: String = text
        .lines()
        .skip_while(|l| !l.starts_with("writes-bench:"))
        .take_while(|l| !l.starts_with("verify-07:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(recipe.contains("cargo bench -p hub-writes"));
    // Earlier numbers are cleared first, so a benchmark that fails cannot pass the check on a previous run's data.
    let cleared = recipe.find("rm -rf target/criterion/hub-writes");
    let benched = recipe.find("cargo bench -p hub-writes");
    assert!(
        cleared.is_some() && cleared < benched,
        "clear stale criterion data before benching"
    );
    assert!(recipe.contains("cargo xtask bench-check --budgets"));
}

#[test]
fn every_writes_benchmark_is_a_registered_budget() {
    let benches = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches");
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml")).unwrap();
    for entry in fs::read_dir(benches).unwrap() {
        let name = entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .trim_end_matches(".rs")
            .to_owned();
        assert!(
            registry.contains(&format!("bench = \"hub-writes/{name}")),
            "benches/{name}.rs has no budget in perf/budgets.toml"
        );
    }
}

#[test]
fn the_fuzz_list_matches_the_fuzz_targets_directory() {
    let text = justfile();
    let list = text
        .lines()
        .find_map(|l| l.strip_prefix("writes_fuzz_targets := \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("writes_fuzz_targets");
    let in_justfile: BTreeSet<String> = list.split_whitespace().map(str::to_owned).collect();
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
    assert_eq!(in_justfile, on_disk);

    // The recipe checks the lockfile (deny, audit) and runs every target on the pinned nightly.
    let recipe: String = text
        .lines()
        .skip_while(|l| !l.starts_with("writes-fuzz:"))
        .take_while(|l| !l.starts_with("# P8"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(recipe.contains("cargo deny --manifest-path crates/hub-writes/fuzz/Cargo.toml"));
    assert!(recipe.contains("cargo audit --file crates/hub-writes/fuzz/Cargo.lock"));
    assert!(recipe.contains("--fuzz-dir crates/hub-writes/fuzz"));
}
