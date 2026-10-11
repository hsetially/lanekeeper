//! The gate (`just verify-03b`) is what it says it is: it runs every sub-recipe, and nothing in it can pass by skipping.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

use std::fs;
use std::path::Path;

fn justfile() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Justfile")).unwrap()
}

/// The recipe `name`: its dependencies, and its body (the indented lines that follow).
fn recipe(source: &str, name: &str) -> Option<(Vec<String>, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let at = lines.iter().position(|l| {
        l.strip_prefix(name)
            .is_some_and(|rest| rest.starts_with(':') && !rest.starts_with(":="))
    })?;
    let deps = lines[at][name.len() + 1..]
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let body = lines[at + 1..]
        .iter()
        .take_while(|l| l.is_empty() || l.starts_with(' ') || l.starts_with('\t'))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Some((deps, body))
}

#[test]
fn verify_03b_runs_every_sub_recipe() {
    let source = justfile();
    assert!(
        !source.contains("verify-03b not implemented"),
        "verify-03b is still the placeholder"
    );
    let (deps, _) = recipe(&source, "verify-03b").expect("a verify-03b recipe");
    for needed in ["platform-tools-check", "platform-lint", "platform-test"] {
        assert!(
            deps.iter().any(|d| d == needed),
            "verify-03b does not run {needed}: {deps:?}"
        );
        assert!(recipe(&source, needed).is_some(), "{needed} is not defined");
    }
    // The checks that make a pass mean something.
    let (tool_deps, tools) = recipe(&source, "platform-tools-check").unwrap();
    assert!(
        tool_deps.iter().any(|d| d == "docker-check"),
        "a missing Docker daemon must fail the gate"
    );
    assert!(tools.contains("cargo deny --version") && tools.contains("cargo audit --version"));
    let (_, lint) = recipe(&source, "platform-lint").unwrap();
    assert!(
        lint.contains("cargo fmt")
            && lint.contains("cargo clippy -p hub-platform -p hub --all-targets")
            && lint.contains("-D warnings")
    );
    let (_, test) = recipe(&source, "platform-test").unwrap();
    assert!(
        test.contains("LK_REQUIRE_DOCKER=1"),
        "the database tests must fail, not skip, without Docker"
    );
    assert!(test.contains("cargo test -p hub-platform -p hub"));
}
