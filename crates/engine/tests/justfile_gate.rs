//! The `verify-04` gate in the Justfile is wired the way the plan says: it runs every sub-recipe, its fuzz list matches the
//! crate's targets, and its benchmark check judges exactly the engine's registered budgets.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn justfile() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Justfile")).unwrap()
}

/// The recipe `name`: from its line to the next blank line.
fn recipe(text: &str, name: &str) -> String {
    text.lines()
        .skip_while(|l| !l.starts_with(&format!("{name}:")))
        .take_while(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn verify_04_runs_every_sub_recipe_so_far() {
    let text = justfile();
    let line = text
        .lines()
        .find(|l| l.starts_with("verify-04:"))
        .expect("a verify-04 recipe");
    assert!(!text.contains("verify-04 not implemented"));
    for dep in [
        "engine-tools-check",
        "engine-lint",
        "engine-test",
        "engine-fuzz",
        "fuzz-smoke",
        "engine-bench",
    ] {
        assert!(line.contains(dep), "verify-04 does not run {dep}");
        assert!(
            text.contains(&format!("\n{dep}:")) || text.contains(&format!("\n{dep}: ")),
            "no recipe {dep}"
        );
    }
    // The opt-in real-snapshot run is not part of the gate.
    assert!(!line.contains("engine-real-snapshot"));
    // What the tests run: the library, the integration tests and the benches' code, with warnings as errors.
    assert!(recipe(&text, "engine-test").contains("cargo test -p engine --lib --tests"));
    assert!(recipe(&text, "engine-lint").contains("cargo clippy -p engine --all-targets -- -D warnings"));
}

#[test]
fn engine_fuzz_list_matches_the_fuzz_targets_directory() {
    let text = justfile();
    let list = text
        .lines()
        .find_map(|l| l.strip_prefix("engine_fuzz_targets := \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("engine_fuzz_targets");
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
    // The lockfile is checked before anything runs (S19), and a missing tool fails the recipe instead of skipping it.
    let body = recipe(&text, "engine-fuzz");
    assert!(body.contains("engine-fuzz: fuzz-tools-check"));
    assert!(body.contains("cargo deny --manifest-path crates/engine/fuzz/Cargo.toml check"));
    assert!(body.contains("cargo audit --file crates/engine/fuzz/Cargo.lock"));
    assert!(body.contains("--locked"));
}

/// The ids of the registered budgets whose benchmark lives in this crate, from `perf/budgets.toml`.
fn registered_engine_budgets() -> BTreeSet<String> {
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml")).unwrap();
    registry
        .split("[[budget]]")
        .skip(1)
        .filter(|block| block.contains("bench = \"engine/") && block.contains("registered = true"))
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
fn engine_bench_judges_exactly_the_registered_engine_budgets() {
    let text = justfile();
    let list = text
        .lines()
        .find_map(|l| l.strip_prefix("engine_budgets := \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("engine_budgets");
    let in_justfile: BTreeSet<String> = list.split_whitespace().map(str::to_owned).collect();
    assert_eq!(in_justfile, registered_engine_budgets());
    assert!(!in_justfile.is_empty());

    // The recipe runs the benchmarks and then judges the filtered registry, not the whole one (which needs the other
    // prompts' results) and not nothing.
    let body = recipe(&text, "engine-bench");
    // Earlier numbers are cleared first, so a benchmark that fails cannot pass the check on a previous run's data.
    let cleared = body.find("rm -rf target/criterion/engine");
    let benched = body.find("cargo bench -p engine");
    assert!(
        cleared.is_some() && cleared < benched,
        "clear stale criterion data before benching"
    );
    assert!(body.contains("cargo xtask bench-check --budgets"));
}

#[test]
fn every_registered_engine_budget_has_a_benchmark_function() {
    let benches = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches");
    let mut sources = String::new();
    for entry in fs::read_dir(benches).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            sources.push_str(&fs::read_to_string(path).unwrap());
        }
    }
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml")).unwrap();
    for block in registry.split("[[budget]]").skip(1) {
        let Some(bench) = block
            .lines()
            .find_map(|l| l.strip_prefix("bench = \"engine/"))
            .and_then(|l| l.strip_suffix('"'))
        else {
            continue;
        };
        if block.contains("registered = true") {
            assert!(
                sources.contains(&format!("\"{bench}\"")),
                "the budget for engine/{bench} has no benchmark function of that name in benches/"
            );
        }
    }
}

#[test]
fn engine_real_snapshot_is_opt_in_and_says_so_when_it_is_not_run() {
    let text = justfile();
    let body = recipe(&text, "engine-real-snapshot");
    assert!(!body.is_empty(), "no engine-real-snapshot recipe");
    // The gate never depends on repositories that this repository does not hold.
    let gate = text
        .lines()
        .find(|l| l.starts_with("verify-04:"))
        .expect("verify-04");
    assert!(!gate.contains("engine-real-snapshot"));
    // Without the two checkouts the recipe says that it did not run (and never prints a pass), and with them it runs the
    // ignored tests of `tests/real_snapshot.rs` with their output.
    assert!(body.contains("LK_REAL_BASE_DIR") && body.contains("LK_REAL_TENANT_DIR"));
    assert!(body.contains("real-snapshot: NOT RUN"));
    assert!(!body.contains("PASS"));
    assert!(body.contains("cargo test -p engine --test real_snapshot -- --ignored --nocapture"));
}
