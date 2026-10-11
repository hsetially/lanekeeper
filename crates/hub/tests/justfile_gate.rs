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
fn verify_03b_runs_every_sub_recipe() {
    let source = justfile();
    assert!(
        !source.contains("verify-03b not implemented"),
        "verify-03b is still the placeholder"
    );
    let (deps, _) = recipe(&source, "verify-03b").expect("a verify-03b recipe");
    for needed in [
        "platform-tools-check",
        "platform-lint",
        "platform-test",
        "platform-slow-test",
        "platform-fuzz",
        "platform-bench",
    ] {
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

#[test]
fn the_slow_and_bench_recipes_are_what_their_names_say() {
    let source = justfile();
    let (_, slow) = recipe(&source, "platform-slow-test").unwrap();
    assert!(
        slow.contains("LK_REQUIRE_DOCKER=1"),
        "the three-replica tests must fail, not skip, without Docker"
    );
    assert!(slow.contains("--release") && slow.contains("--test-threads=1"));
    // The tests whose time bounds are the point: events within 1 s, a push within 10 s, the routing, lease and outbox races.
    for test in [
        "events_bus",
        "gateway_cluster",
        "git_sync",
        "lease_race",
        "outbox",
    ] {
        assert!(
            slow.contains(&format!("--test {test}")),
            "{test} is not in the slow run"
        );
        assert!(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../hub-platform/tests")
                .join(format!("{test}.rs"))
                .is_file(),
            "{test} is not a test of hub-platform"
        );
    }
    let (_, bench) = recipe(&source, "platform-bench").unwrap();
    assert!(
        bench.contains("cargo bench -p hub-platform -p hub") && bench.contains("cargo xtask bench-check"),
        "the benchmarks run, then they are checked against perf/budgets.toml"
    );
}

#[test]
fn platform_fuzz_runs_every_target_the_platform_has_and_checks_its_lockfile() {
    let source = justfile();
    let (deps, body) = recipe(&source, "platform-fuzz").expect("a platform-fuzz recipe");
    assert!(
        deps.iter().any(|d| d == "fuzz-tools-check"),
        "it must fail, not skip, without the tools"
    );
    assert!(body.contains("--fuzz-dir crates/hub-platform/fuzz"));
    assert!(body.contains("cargo deny --manifest-path crates/hub-platform/fuzz/Cargo.toml"));
    assert!(body.contains("cargo audit --file crates/hub-platform/fuzz/Cargo.lock"));
    assert!(body.contains("--locked"), "the lockfile must be complete");

    let listed: BTreeSet<String> = source
        .lines()
        .find_map(|l| l.strip_prefix("platform_fuzz_targets := "))
        .expect("platform_fuzz_targets")
        .trim_matches('"')
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../hub-platform/fuzz");
    let on_disk: BTreeSet<String> = fs::read_dir(dir.join("fuzz_targets"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            name.strip_suffix(".rs").map(str::to_owned)
        })
        .collect();
    assert_eq!(
        listed, on_disk,
        "platform-fuzz must run every target in crates/hub-platform/fuzz/fuzz_targets"
    );
    // The plan names these five (S22).
    for target in [
        "hub_agent_message",
        "hub_join_csr",
        "hub_forward_request",
        "hub_webhook_payload",
        "hub_sentinel_message",
    ] {
        assert!(listed.contains(target), "{target} is not fuzzed");
    }
    for target in &listed {
        assert!(
            dir.join("corpus").join(target).is_dir(),
            "no seed corpus for {target}"
        );
    }
}
