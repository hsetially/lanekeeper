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

/// The ids of the registered budgets whose benchmark lives in this crate, from `perf/budgets.toml`.
fn registered_identity_budgets() -> BTreeSet<String> {
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml")).unwrap();
    registry
        .split("[[budget]]")
        .skip(1)
        .filter(|block| block.contains("bench = \"hub-identity/") && block.contains("registered = true"))
        .map(|block| {
            let id = block
                .lines()
                .find_map(|l| l.strip_prefix("id = \""))
                .and_then(|l| l.strip_suffix('"'))
                .expect("a budget has an id");
            id.to_owned()
        })
        .collect()
}

#[test]
fn identity_bench_judges_exactly_the_registered_identity_budgets() {
    let text = justfile();
    let list = text
        .lines()
        .find_map(|l| l.strip_prefix("identity_budgets := \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("identity_budgets");
    let in_justfile: BTreeSet<String> = list.split_whitespace().map(str::to_owned).collect();
    assert_eq!(in_justfile, registered_identity_budgets());
    assert!(!in_justfile.is_empty());

    // The recipe runs the benchmarks and then judges the filtered registry, not the whole one (which needs the other
    // prompts' results) and not nothing.
    let recipe: String = text
        .lines()
        .skip_while(|l| !l.starts_with("identity-bench:"))
        .take_while(|l| !l.starts_with("verify-03b:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(recipe.contains("cargo bench -p hub-identity"));
    // Earlier numbers are cleared first, so a benchmark that fails cannot pass the check on a previous run's data.
    let cleared = recipe.find("rm -rf target/criterion/hub-identity");
    let benched = recipe.find("cargo bench -p hub-identity");
    assert!(
        cleared.is_some() && cleared < benched,
        "clear stale criterion data before benching"
    );
    assert!(recipe.contains("cargo xtask bench-check --budgets"));
}

#[test]
fn every_identity_benchmark_is_a_registered_budget() {
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
            registry.contains(&format!("bench = \"hub-identity/{name}")),
            "benches/{name}.rs has no budget in perf/budgets.toml"
        );
    }
}
