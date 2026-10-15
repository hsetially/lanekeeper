//! The `verify-07` gate in the Justfile is wired the way the plan says (plan 07, "Recipes"). Later tasks append their
//! sub-recipes (fuzz, bench, the Postgres-backed tests); this test keeps the ones that exist from being dropped or skipped.
#![allow(clippy::unwrap_used, clippy::expect_used)]

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
    for dep in ["mcp-tools-check", "mcp-lint", "mcp-test", "mcp-t0-status"] {
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
    let unit_tests = recipe(&text, "mcp-test");
    assert!(unit_tests.contains("cargo test -p hub-mcp"), "{unit_tests}");
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
}
