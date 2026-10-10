//! The gate (`just verify-02`, T7) is what it says it is: it runs every sub-recipe, the fuzz recipe covers every fuzz
//! target the agent has, and nothing in it can pass by skipping.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
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
fn verify_02_runs_every_sub_recipe() {
    let source = justfile();
    let (deps, _) = recipe(&source, "verify-02").expect("a verify-02 recipe");
    assert!(
        !source.contains("verify-02 not implemented"),
        "verify-02 is still the placeholder"
    );
    for needed in [
        "agent-tools-check",
        "agent-fixtures",
        "agent-lint",
        "agent-test",
        "agent-slow-test",
        "agent-fuzz",
        "agent-bench",
    ] {
        assert!(
            deps.iter().any(|d| d == needed),
            "verify-02 does not run {needed}: {deps:?}"
        );
        assert!(recipe(&source, needed).is_some(), "{needed} is not defined");
    }
    // The checks that make a pass mean something.
    let (_, tests) = recipe(&source, "agent-test").unwrap();
    assert!(tests.contains("cargo test -p agent --lib --tests"));
    let (_, slow) = recipe(&source, "agent-slow-test").unwrap();
    assert!(slow.contains("--release") && slow.contains("--ignored"));
    let (_, lint) = recipe(&source, "agent-lint").unwrap();
    assert!(
        lint.contains("cargo clippy -p agent --all-targets -- -D warnings") && lint.contains("cargo fmt")
    );
    let (_, bench) = recipe(&source, "agent-bench").unwrap();
    assert!(bench.contains("cargo bench -p agent") && bench.contains("cargo xtask bench-check"));
}

#[test]
fn agent_fuzz_runs_every_target_the_agent_has_and_checks_its_lockfile() {
    let source = justfile();
    let (deps, body) = recipe(&source, "agent-fuzz").expect("an agent-fuzz recipe");
    assert!(
        deps.iter().any(|d| d == "fuzz-tools-check"),
        "it must fail, not skip, without the tools"
    );
    assert!(body.contains("--fuzz-dir crates/agent/fuzz"));
    assert!(body.contains("cargo deny --manifest-path crates/agent/fuzz/Cargo.toml"));
    assert!(body.contains("cargo audit --file crates/agent/fuzz/Cargo.lock"));
    assert!(body.contains("--locked"), "the lockfile must be complete");

    let listed: BTreeSet<String> = source
        .lines()
        .find_map(|l| l.strip_prefix("agent_fuzz_targets := "))
        .expect("agent_fuzz_targets")
        .trim_matches('"')
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz");
    let on_disk: BTreeSet<String> = fs::read_dir(dir.join("fuzz_targets"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            name.strip_suffix(".rs").map(str::to_owned)
        })
        .collect();
    assert_eq!(
        listed, on_disk,
        "agent-fuzz must run every target in crates/agent/fuzz/fuzz_targets"
    );
    for target in &listed {
        assert!(
            dir.join("corpus").join(target).is_dir(),
            "no seed corpus for {target}"
        );
    }
}

#[test]
fn the_gate_tools_are_pinned_like_the_other_gates() {
    let source = justfile();
    let (deps, body) = recipe(&source, "agent-tools-check").unwrap();
    assert!(deps.iter().any(|d| d == "fuzz-tools-check"));
    assert!(
        body.contains("cargo-deny {{cargo_deny_version}}")
            && body.contains("cargo-audit {{cargo_audit_version}}")
    );
}
