//! The `verify-05` gate in the Justfile is wired the way the plan says, its fuzz list matches the crate's targets, and its
//! benchmark list matches the budgets that have a benchmark in this crate (registered or not: an unregistered one is listed as
//! UNMET with its number, as `engine-bench` does).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn justfile() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Justfile")).unwrap()
}

fn list(text: &str, name: &str) -> BTreeSet<String> {
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{name} := \"")))
        .and_then(|l| l.strip_suffix('"'))
        .unwrap_or_else(|| panic!("no `{name}` in the Justfile"))
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[test]
fn verify_05_runs_every_sub_recipe() {
    let text = justfile();
    let line = text
        .lines()
        .find(|l| l.starts_with("verify-05:"))
        .expect("a verify-05 recipe");
    for dep in [
        "registry-tools-check",
        "registry-lint",
        "registry-test",
        "registry-slow-test",
        "registry-fuzz",
        "registry-bench",
    ] {
        assert!(line.contains(dep), "verify-05 does not run {dep}");
        assert!(
            text.contains(&format!("\n{dep}:")) || text.contains(&format!("\n{dep}: ")),
            "no recipe {dep}"
        );
    }
    assert!(!text.contains("verify-05 not implemented"));
}

#[test]
fn the_gate_lists_every_fuzz_target_and_every_registry_budget() {
    let text = justfile();
    let in_justfile = list(&text, "registry_fuzz_targets");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz");
    let on_disk: BTreeSet<String> = fs::read_dir(dir.join("fuzz_targets"))
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
        in_justfile, on_disk,
        "the Justfile and fuzz_targets/ must name the same targets"
    );

    // Every target has a [[bin]] in the fuzz manifest, a corpus directory and a row in the README.
    let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let readme = fs::read_to_string(dir.join("README.md")).unwrap();
    for target in &in_justfile {
        assert!(
            manifest.contains(&format!("name = \"{target}\"")),
            "no [[bin]] for {target}"
        );
        assert!(dir.join("corpus").join(target).is_dir(), "no corpus for {target}");
        assert!(
            readme.contains(&format!("| `{target}` |")),
            "no README row for {target}"
        );
    }

    // The benchmark recipe judges exactly the budgets whose benchmark is in this crate.
    let budgets = registry_benchmark_budgets();
    assert_eq!(list(&text, "registry_budgets"), budgets);
    assert!(!budgets.is_empty());
}

#[test]
fn the_registry_test_recipe_requires_docker_rather_than_skipping() {
    assert!(justfile().contains("LK_REQUIRE_DOCKER=1 cargo test -p hub-registry"));
}

/// The ids of the budgets whose benchmark has a file in `benches/`, from `perf/budgets.toml`. A budget that is not registered
/// yet is in the list: `bench-check` shows it as UNMET with the number the benchmark measured, and registering it later changes
/// nothing here. A budget whose benchmark is written by a later task is not: it has no number to show.
fn registry_benchmark_budgets() -> BTreeSet<String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let registry = fs::read_to_string(manifest.join("../../perf/budgets.toml")).unwrap();
    registry
        .split("[[budget]]")
        .skip(1)
        .filter_map(|block| {
            let bench = block
                .lines()
                .find_map(|l| l.strip_prefix("bench = \"hub-registry/"))
                .and_then(|l| l.strip_suffix('"'))?;
            if !manifest.join("benches").join(format!("{bench}.rs")).is_file() {
                return None;
            }
            block
                .lines()
                .find_map(|l| l.strip_prefix("id = \""))
                .and_then(|l| l.strip_suffix('"'))
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn registry_bench_runs_the_benchmarks_and_judges_the_filtered_registry() {
    let text = justfile();
    let recipe: String = text
        .lines()
        .skip_while(|l| !l.starts_with("registry-bench:"))
        .take_while(|l| !l.starts_with("verify-06:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(recipe.contains("cargo bench -p hub-registry"));
    // Earlier numbers are cleared first, so a benchmark that fails cannot pass the check on a previous run's data.
    let cleared = recipe.find("rm -rf target/criterion/hub-registry");
    let harness_cleared = recipe.find("rm -f target/perf-results/hub-registry_");
    let benched = recipe.find("cargo bench -p hub-registry");
    assert!(
        cleared.is_some() && cleared < benched,
        "clear stale criterion data before benching"
    );
    assert!(
        harness_cleared.is_some() && harness_cleared < benched,
        "clear stale harness results before benching"
    );
    assert!(recipe.contains("cargo xtask bench-check --budgets"));
}

/// The recipe cuts a registry with the budgets it judges, using an `awk` program written in the Justfile. A block of
/// `perf/budgets.toml` may start with comment lines (the contract PR put one before most of this prompt's budgets), so the
/// program has to find `[[budget]]` inside a paragraph, not only at its start. Run here, as the recipe runs it.
#[test]
fn the_awk_filter_of_registry_bench_finds_every_budget_it_names() {
    let text = justfile();
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("awk -v ids=\"{{registry_budgets}}\""))
        .expect("an awk line in registry-bench");
    let program = line
        .split('\'')
        .nth(1)
        .expect("the awk program is quoted with single quotes");
    let ids = list(&text, "registry_budgets");
    assert!(!ids.is_empty());
    let budgets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml");
    let output = std::process::Command::new("awk")
        .arg("-v")
        .arg(format!(
            "ids={}",
            ids.iter().cloned().collect::<Vec<_>>().join(" ")
        ))
        .arg(program)
        .arg(&budgets)
        .output()
        .expect("awk runs");
    assert!(output.status.success());
    let cut = String::from_utf8(output.stdout).unwrap();
    let blocks = cut.lines().filter(|l| l.starts_with("[[budget]]")).count();
    assert_eq!(
        blocks,
        ids.len(),
        "the filtered registry holds one block for each id: {cut}"
    );
    for id in &ids {
        assert!(
            cut.contains(&format!("\nid = \"{id}\"")),
            "{id} is missing from the filtered registry"
        );
    }
}

#[test]
fn every_registry_benchmark_is_a_registered_budget() {
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
        if name == "common" {
            continue;
        }
        assert!(
            registry.contains(&format!("bench = \"hub-registry/{name}\"")),
            "benches/{name}.rs has no budget in perf/budgets.toml"
        );
    }
}
