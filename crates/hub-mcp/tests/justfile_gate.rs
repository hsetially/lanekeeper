//! The `verify-07` gate in the Justfile is wired the way the plan says (plan 07, "Recipes"). Later tasks append their
//! sub-recipes (fuzz, bench, the Postgres-backed tests); this test keeps the ones that exist from being dropped or skipped.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn justfile() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Justfile")).unwrap()
}

/// The text of one recipe: its header line and the indented lines under it.
fn recipe(text: &str, name: &str) -> String {
    let mut lines = text.lines();
    let header = lines
        .by_ref()
        .find(|l| l.starts_with(&format!("{name}:")) || l.starts_with(&format!("{name} ")));
    assert!(header.is_some(), "no recipe {name}");
    let header = header.unwrap();
    let mut out = String::from(header);
    for line in lines.take_while(|l| l.starts_with(' ') || l.starts_with('\t')) {
        out.push('\n');
        out.push_str(line);
    }
    out
}

#[test]
fn verify_07_runs_every_sub_recipe_and_is_no_longer_the_stub() {
    let text = justfile();
    assert!(
        !text.contains("verify-07 not implemented"),
        "verify-07 is still the stub"
    );
    let gate = recipe(&text, "verify-07");
    for dep in [
        "mcp-tools-check",
        "mcp-lint",
        "mcp-test",
        "mcp-fuzz",
        "mcp-bench",
        "mcp-t0-status",
    ] {
        assert!(gate.contains(dep), "verify-07 does not run {dep}");
        recipe(&text, dep);
    }
}

#[test]
fn the_gate_never_skips_a_test_and_lints_with_warnings_as_errors() {
    let text = justfile();
    let lint = recipe(&text, "mcp-lint");
    assert!(lint.contains("cargo fmt --all -- --check"), "{lint}");
    assert!(
        lint.contains("cargo clippy -p hub-mcp --all-targets -- -D warnings"),
        "{lint}"
    );
    assert!(
        lint.contains("cargo clippy -p hub --all-targets -- -D warnings"),
        "the hub's module is part of this prompt: {lint}"
    );
    let unit_tests = recipe(&text, "mcp-test");
    assert!(unit_tests.contains("cargo test -p hub-mcp"), "{unit_tests}");
    assert!(
        unit_tests.contains("LK_REQUIRE_DOCKER=1 cargo test -p hub-mcp"),
        "a missing database is a failure, not a skip: {unit_tests}"
    );
    assert!(
        unit_tests.contains("cargo test -p hub --test mcp_module"),
        "{unit_tests}"
    );
    for skipped in ["--skip", "|| true", "--ignored", "exit 0"] {
        assert!(
            !unit_tests.contains(skipped),
            "mcp-test must not skip: {unit_tests}"
        );
    }
}

#[test]
fn the_status_recipe_runs_the_status_tool_and_passes_its_arguments_on() {
    let text = justfile();
    let status = recipe(&text, "mcp-t0-status");
    assert!(
        status.starts_with("mcp-t0-status *args:"),
        "so that `just mcp-t0-status --strict` reaches the tool: {status}"
    );
    assert!(status.contains("--example t0_status -- {{args}}"), "{status}");
    // It must not swallow the exit code: `--strict` exits 1 until a human has recorded every cell.
    assert!(
        !status.contains("|| true") && !status.contains("exit 0"),
        "{status}"
    );
}

#[test]
fn the_tools_check_is_the_one_every_other_gate_uses() {
    let text = justfile();
    let check = recipe(&text, "mcp-tools-check");
    assert!(check.starts_with("mcp-tools-check: agent-tools-check"), "{check}");
    assert!(
        check.contains("docker-check"),
        "the token store tests need Docker: {check}"
    );
}

/// The names in `mcp_fuzz_targets` and the files of `crates/hub-mcp/fuzz/fuzz_targets/`, which are also the corpus directories.
#[test]
fn fuzz_target_list_matches_directory() {
    let text = justfile();
    let line = text
        .lines()
        .find(|l| l.starts_with("mcp_fuzz_targets :="))
        .expect("the recipe lists its targets");
    let listed: BTreeSet<String> = line
        .split('"')
        .nth(1)
        .expect("the list is quoted")
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let fuzz = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz");
    let on_disk = |dir: &str, strip: &str| -> BTreeSet<String> {
        fs::read_dir(fuzz.join(dir))
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .trim_end_matches(strip)
                    .to_owned()
            })
            .collect()
    };
    assert_eq!(listed, on_disk("fuzz_targets", ".rs"));
    assert_eq!(listed, on_disk("corpus", ""));
    let fuzz_recipe = recipe(&text, "mcp-fuzz");
    for needed in [
        "cargo +{{fuzz_nightly}} metadata --locked",
        "cargo deny --manifest-path crates/hub-mcp/fuzz/Cargo.toml",
        "cargo audit --file crates/hub-mcp/fuzz/Cargo.lock",
        "--fuzz-dir crates/hub-mcp/fuzz",
    ] {
        assert!(
            fuzz_recipe.contains(needed),
            "mcp-fuzz lacks `{needed}`: {fuzz_recipe}"
        );
    }
}

/// The ids in `mcp_budgets` (what `mcp-bench` judges) and the registered `hub-mcp/` budgets of `perf/budgets.toml`.
#[test]
fn p9_budgets_match_the_registry() {
    let text = justfile();
    let line = text
        .lines()
        .find(|l| l.starts_with("mcp_budgets :="))
        .expect("the recipe lists its budgets");
    let listed: BTreeSet<String> = line
        .split('"')
        .nth(1)
        .expect("the list is quoted")
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml")).unwrap();
    // Every `[[budget]]` of the file, as (id, bench, registered).
    let mut registered_mcp = BTreeSet::new();
    for block in registry.split("[[budget]]").skip(1) {
        let field = |name: &str| {
            block
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{name} = ")))
                .map(|v| v.trim().trim_matches('"').to_owned())
        };
        let (id, bench, registered) = (
            field("id").unwrap(),
            field("bench").unwrap(),
            field("registered").unwrap(),
        );
        if bench.starts_with("hub-mcp/") && registered == "true" {
            registered_mcp.insert(id);
        }
    }
    assert_eq!(listed, registered_mcp, "mcp-bench and perf/budgets.toml disagree");
    assert!(listed.contains("P9") && listed.contains("P9.read_file"));
    assert_eq!(listed.len(), 21, "the aggregate and the 20 read tools");

    let bench = recipe(&text, "mcp-bench");
    for needed in [
        "rm -rf target/criterion/hub-mcp",
        "cargo bench -p hub-mcp",
        "cargo xtask bench-check --budgets",
        "found $found of $wanted",
    ] {
        assert!(bench.contains(needed), "mcp-bench lacks `{needed}`: {bench}");
    }
    for skipped in ["|| true\n", "exit 0"] {
        assert!(!bench.contains(skipped), "mcp-bench must not skip: {bench}");
    }
}

#[test]
fn the_schemas_recipe_regenerates_the_file_from_the_document() {
    let text = justfile();
    let schemas = recipe(&text, "mcp-schemas");
    assert!(
        schemas.contains("UPDATE_SCHEMAS=1 cargo test -p hub-mcp --test mcp_auth contract_schemas::embedded_schemas_equal_the_contract_document"),
        "{schemas}"
    );
}
