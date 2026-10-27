//! The gates (`just verify-03b` and, since plan 18, `just verify-18`) are what they say they are: they run every sub-recipe, and nothing
//! in them can pass by skipping.
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

#[test]
fn hub_fuzz_runs_every_target_the_hub_has_and_checks_its_lockfile() {
    let source = justfile();
    let (deps, body) = recipe(&source, "hub-fuzz").expect("a hub-fuzz recipe");
    assert!(
        deps.iter().any(|d| d == "fuzz-tools-check"),
        "it must fail, not skip, without the tools"
    );
    assert!(body.contains("--fuzz-dir crates/hub/fuzz"));
    assert!(body.contains("cargo deny --manifest-path crates/hub/fuzz/Cargo.toml"));
    assert!(body.contains("cargo audit --file crates/hub/fuzz/Cargo.lock"));
    assert!(body.contains("--locked"), "the lockfile must be complete");

    let listed: BTreeSet<String> = source
        .lines()
        .find_map(|l| l.strip_prefix("hub_fuzz_targets := "))
        .expect("hub_fuzz_targets")
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
        "hub-fuzz must run every target in crates/hub/fuzz/fuzz_targets"
    );
    // The plan names these four (rule 13): the settings, the command line, the PEM at the web certificate, and the seed file.
    for target in ["hub_settings", "hub_cli_args", "hub_pem", "hub_seed_file"] {
        assert!(listed.contains(target), "{target} is not fuzzed");
    }
    for target in &listed {
        assert!(
            dir.join("corpus").join(target).is_dir(),
            "no seed corpus for {target}"
        );
    }
    // The fuzz crate is outside the workspace and has its own lockfile, like the other crates'.
    assert!(dir.join("Cargo.lock").is_file());
    let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("\n[workspace]\n"),
        "the fuzz crate must not join the root workspace"
    );
}

// ------------------------------------------------------------------------------------------------------ plan 18, T8: verify-18

/// The sub-recipes of `just verify-18`, in the order the gate runs them.
const VERIFY_18: [&str; 5] = ["hub-tools-check", "hub-lint", "hub-test", "hub-fuzz", "hub-bench"];

/// Every feature of the workspace that makes a test double constructible (the list of `tests/release_build.rs`, which reads manifests;
/// the gate reads the resolved graph).
const TEST_FEATURES: [&str; 7] = [
    "fakes",
    "conformance",
    "test-doubles",
    "bench-probe",
    "fuzz-hooks",
    "route-table",
    "testing",
];

#[test]
fn verify_18_runs_every_sub_recipe() {
    let source = justfile();
    assert!(
        !source.contains("verify-18 not implemented"),
        "verify-18 is still the placeholder"
    );
    let (deps, _) = recipe(&source, "verify-18").expect("a verify-18 recipe");
    assert_eq!(
        deps,
        VERIFY_18.map(str::to_owned),
        "verify-18 runs these, in this order, and stops at the first that fails"
    );
    for needed in VERIFY_18 {
        assert!(recipe(&source, needed).is_some(), "{needed} is not defined");
    }

    // The tools: a missing Docker daemon, nightly toolchain, cargo-fuzz, cargo-deny or cargo-audit fails the gate before the slow steps.
    let (tool_deps, tools) = recipe(&source, "hub-tools-check").unwrap();
    for needed in ["docker-check", "fuzz-tools-check"] {
        assert!(
            tool_deps.iter().any(|d| d == needed),
            "hub-tools-check does not depend on {needed}: a missing tool must fail, never skip"
        );
    }
    assert!(tools.contains("cargo deny --version") && tools.contains("cargo audit --version"));

    // Lint: formatting, clippy on every target with warnings denied, the resolved release graph, and the supply chain.
    let (_, lint) = recipe(&source, "hub-lint").unwrap();
    assert!(lint.contains("cargo fmt --all -- --check"));
    assert!(
        lint.contains("cargo clippy -p hub --all-targets --all-features") && lint.contains("-D warnings"),
        "hub-lint must run clippy on the hub, every target, with warnings denied"
    );
    assert!(lint.contains("cargo deny check") && lint.contains("cargo audit"));

    // Tests: the database tests fail without Docker, and they are the crate's own (the route-table and CSRF tests, the settings and
    // hardening tests, the migration and seed tests are all targets of `crates/hub/tests`), plus the three files of hub-platform
    // that plan 18 changed (the per-session stream end, G2) and the registered budgets' own check.
    let (_, test) = recipe(&source, "hub-test").unwrap();
    let line = |needle: &str| {
        test.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("hub-test does not run {needle}"))
    };
    for needle in [
        "cargo test -p hub --all-features",
        "cargo test -p hub-platform --test events_sse",
    ] {
        assert!(
            line(needle).contains("LK_REQUIRE_DOCKER=1"),
            "{needle}: the database tests must fail, not skip, without Docker"
        );
    }
    assert!(line("cargo test -p xtask").contains("budgets_registry"));

    // The benchmarks run, then they are judged against perf/budgets.toml: P11 and the two composed-authentication budgets.
    let (_, bench) = recipe(&source, "hub-bench").unwrap();
    assert!(bench.contains("cargo bench -p hub"));
    assert!(bench.contains("cargo xtask bench-check --budgets"));
    assert!(
        bench.contains("-ne \"$wanted\"") && bench.contains("exit 1"),
        "a budget that is not registered must fail the recipe, not pass it"
    );
    assert!(bench.contains("agent-fixtures") || source.contains("hub-bench: agent-fixtures"));

    // Nothing in the gate can pass by skipping.
    for name in VERIFY_18 {
        let (_, body) = recipe(&source, name).unwrap();
        for forbidden in ["exit 0", "SKIPPED", "|| echo", "|| :", "continue-on-error"] {
            assert!(!body.contains(forbidden), "{name} contains {forbidden:?}");
        }
    }
}

/// The budgets `hub-bench` judges are the registered budgets whose benchmark is in `crates/hub/benches`, and no other.
#[test]
fn hub_bench_judges_exactly_the_hubs_registered_budgets() {
    let source = justfile();
    let listed: BTreeSet<String> = source
        .lines()
        .find_map(|l| l.strip_prefix("hub_budgets := "))
        .expect("hub_budgets")
        .trim_matches('"')
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([
            "P11".to_owned(),
            "P7.auth_composed".to_owned(),
            "P7.auth_composed_cold".to_owned()
        ]),
        "P11 (start-up, benches/startup.rs) and the two composed-authentication budgets (benches/auth_adapter.rs, plan 18 CC1)"
    );
    // Each is registered, and its benchmark is one this crate's `benches/` writes.
    let budgets =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../perf/budgets.toml")).unwrap();
    let benches_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches");
    let bench_sources: String = fs::read_dir(&benches_dir)
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    for id in &listed {
        let block = budgets
            .split("[[budget]]")
            .find(|b| b.contains(&format!("id = \"{id}\"\n")))
            .unwrap_or_else(|| panic!("{id} is not in perf/budgets.toml"));
        assert!(block.contains("registered = true"), "{id} is not registered");
        let bench = block
            .lines()
            .find_map(|l| l.strip_prefix("bench = \""))
            .and_then(|l| l.strip_suffix('"'))
            .unwrap();
        assert!(bench.starts_with("hub/"), "{id}: {bench}");
        // A harness names its result file (`write_result("hub/startup", ..)`); a criterion benchmark is a group and a function.
        let function = bench.strip_prefix("hub/").unwrap();
        assert!(
            bench_sources.contains(&format!("\"{bench}\""))
                || (bench_sources.contains("benchmark_group(\"hub\")")
                    && bench_sources.contains(&format!("bench_function(\"{function}\""))),
            "{id}: no file in crates/hub/benches writes {bench}"
        );
    }
    // And the other way: every `hub/` budget that is registered is judged here (the platform's and identity's have other prefixes).
    for block in budgets.split("[[budget]]").skip(1) {
        let bench = block
            .lines()
            .find_map(|l| l.strip_prefix("bench = \""))
            .and_then(|l| l.strip_suffix('"'));
        if let Some(bench) = bench.filter(|b| b.starts_with("hub/") && !b.starts_with("hub/memory_")) {
            let id = block
                .lines()
                .find_map(|l| l.strip_prefix("id = \""))
                .and_then(|l| l.strip_suffix('"'))
                .unwrap();
            assert!(
                listed.contains(id),
                "{id} ({bench}) is registered and hub-bench does not judge it"
            );
        }
    }
}

/// S16, S21: a release build of the hub holds no test double. The gate reads the resolved dependency graph of the production build
/// (`cargo tree -e no-dev,features`), which `tests/release_build.rs` approximates from the manifests: a feature switched on by a
/// transitive dependency shows here and not there. The check is shown to be able to fail: the same command with the dev edges lists them.
#[test]
fn hub_lint_forbids_test_features_in_the_release_graph() {
    let source = justfile();
    let (_, lint) = recipe(&source, "hub-lint").expect("a hub-lint recipe");
    assert!(
        lint.contains("cargo tree -p hub -e no-dev,features"),
        "hub-lint does not read the release graph"
    );
    for feature in TEST_FEATURES {
        assert!(
            source.lines().any(|l| l.starts_with("hub_test_features := ")
                && l.split_whitespace().any(|w| w.trim_matches('"') == feature)),
            "hub_test_features does not name {feature}"
        );
    }
    assert!(
        lint.contains("{{hub_test_features}}"),
        "hub-lint does not use the list"
    );

    let tree = |edges: &str| -> String {
        let output =
            std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
                .args(["tree", "-p", "hub", "-e", edges, "--locked", "--offline"])
                .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .output()
                .unwrap();
        assert!(
            output.status.success(),
            "cargo tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let production = tree("no-dev,features");
    for feature in TEST_FEATURES {
        assert!(
            !production.contains(&format!("feature \"{feature}\"")),
            "the release graph of hub switches on {feature}"
        );
    }
    let with_dev = tree("features");
    let seen = TEST_FEATURES
        .iter()
        .filter(|f| with_dev.contains(&format!("feature \"{f}\"")))
        .count();
    assert!(
        seen >= 4,
        "the check is blind: with the dev edges it finds only {seen} of the test features"
    );
}

/// `just dev` (G23): Postgres with TLS, the migrations, the seed of synthetic swimlanes, the web app, and the hub with software edges.
#[test]
fn the_dev_recipes_run_the_real_commands_on_the_synthetic_files() {
    let source = justfile();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (_, dev) = recipe(&source, "dev").expect("a dev recipe");
    assert!(
        !dev.contains("is added by prompt"),
        "dev is still the placeholder"
    );
    // In this order: the database, then the schema, then the rows, then the app, then the hub.
    let at = |needle: &str| {
        dev.find(needle)
            .unwrap_or_else(|| panic!("dev does not run {needle}"))
    };
    assert!(at("compose.yaml") < at("-- migrate"));
    assert!(at("-- migrate") < at("-- seed"));
    assert!(at("-- seed") < at("pnpm build"));
    assert!(at("pnpm build") < at("--example hub_dev"));
    assert!(dev.contains("crates/hub/examples/seed.dev.yaml"));
    // It never gives the hub the migrator's credential: that is the migration command's alone.
    assert!(dev.contains("LK_MIGRATOR_URL_FILE"));
    let (_, down) = recipe(&source, "dev-down").expect("a dev-down recipe");
    assert!(down.contains("compose.yaml") && down.contains("down"));
    for file in [
        "examples/seed.dev.yaml",
        "examples/hub_dev.rs",
        "examples/dev/compose.yaml",
        "examples/dev/dev.env",
    ] {
        assert!(root.join(file).is_file(), "{file} is missing");
    }
    // The dev database is a TLS one: the hub's pool verifies the server and has no mode that does not (S6).
    let compose = fs::read_to_string(root.join("examples/dev/compose.yaml")).unwrap();
    assert!(compose.contains("ssl=on") && compose.contains("TLSv1.3"));
    assert!(
        compose.contains("pgvector/pgvector:pg16@sha256:"),
        "the dev image is pinned by digest (S19)"
    );
    assert!(
        compose.contains("127.0.0.1:"),
        "the dev database listens on loopback only"
    );
}
