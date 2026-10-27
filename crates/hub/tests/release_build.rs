//! A release build of the hub cannot contain a test double: the `fakes` features are switched on by dev-dependencies only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

/// The lines of the `[dependencies]` table of a manifest (and of its `[build-dependencies]`), which are what a release
/// build uses.
fn production_dependency_lines(manifest: &str) -> Vec<String> {
    let mut in_production = false;
    let mut lines = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_production = trimmed == "[dependencies]" || trimmed == "[build-dependencies]";
            continue;
        }
        if in_production && !trimmed.is_empty() && !trimmed.starts_with('#') {
            lines.push(trimmed.to_owned());
        }
    }
    lines
}

#[test]
fn fakes_are_only_enabled_by_dev_dependencies() {
    for crate_dir in ["crates/hub", "crates/hub-platform"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(crate_dir)
            .join("Cargo.toml");
        let manifest = fs::read_to_string(&path).unwrap();
        for line in production_dependency_lines(&manifest) {
            assert!(
                !line.contains("\"fakes\"") && !line.contains("\"conformance\""),
                "{crate_dir} enables a test feature in a production dependency: {line}"
            );
        }
        // And the features themselves are not on by default.
        if crate_dir == "crates/hub-platform" {
            assert!(
                manifest.contains("default = []"),
                "hub-platform's default features must be empty"
            );
        }
    }
}

/// Every feature of the workspace that switches on a test double, a probe or a hook that a release build must not contain.
const TEST_FEATURES: [&str; 7] = [
    "fakes",
    "conformance",
    "test-doubles",
    "bench-probe",
    "fuzz-hooks",
    "route-table",
    // Plan 18, T4: the hub's own `SessionEdge::without_sessions`, which applies no authentication and no CSRF layer.
    "testing",
];

/// The test features that a manifest's production dependencies switch on, each with the line that does it.
fn test_features_in_production(manifest: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in production_dependency_lines(manifest) {
        for feature in TEST_FEATURES {
            if line.contains(&format!("\"{feature}\"")) {
                found.push((feature.to_owned(), line.clone()));
            }
        }
    }
    found
}

/// Plan 18, T3 (acceptance 3): no production dependency of the hub, or of the crates the composition links, enables any of the
/// features that make a test double constructible. The line that does is named, so that a person sees which edge to cut. The
/// registry and write path crates join the list when T5 links them.
#[test]
fn no_test_feature_in_any_production_dependency() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for crate_dir in [
        "crates/hub",
        "crates/hub-platform",
        "crates/hub-identity",
        "crates/hub-mcp",
    ] {
        let manifest = fs::read_to_string(root.join(crate_dir).join("Cargo.toml")).unwrap();
        let found = test_features_in_production(&manifest);
        assert!(
            found.is_empty(),
            "{crate_dir} enables a test feature in a production dependency: {found:?}"
        );
    }
}

#[test]
fn every_test_feature_is_caught_in_a_production_dependency_and_none_in_a_dev_one() {
    for feature in TEST_FEATURES {
        let bad = format!("[dependencies]\nx = {{ workspace = true, features = [\"{feature}\"] }}\n");
        assert_eq!(test_features_in_production(&bad).len(), 1, "{feature}");
        let dev = format!("[dev-dependencies]\nx = {{ workspace = true, features = [\"{feature}\"] }}\n");
        assert!(test_features_in_production(&dev).is_empty(), "{feature}");
        let build = format!("[build-dependencies]\nx = {{ workspace = true, features = [\"{feature}\"] }}\n");
        assert_eq!(test_features_in_production(&build).len(), 1, "{feature}");
    }
}

#[test]
fn the_planted_manifest_is_caught() {
    let bad = "[dependencies]\nhub-platform = { workspace = true, features = [\"fakes\"] }\n[dev-dependencies]\nx = 1\n";
    assert!(
        production_dependency_lines(bad)
            .iter()
            .any(|l| l.contains("\"fakes\""))
    );
    let good = "[dependencies]\nhub-platform.workspace = true\n[dev-dependencies]\nhub-platform = { workspace = true, features = [\"fakes\"] }\n";
    assert!(
        !production_dependency_lines(good)
            .iter()
            .any(|l| l.contains("\"fakes\""))
    );
}

/// Every source file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
    found
}

/// The code of a source file outside its test module and outside comments: from the start to the first `#[cfg(test)]` or
/// `#[cfg(feature = "testing")]` (the crate's own test-only items come after all the production code), without the lines that
/// are only comments.
fn production_code(text: &str) -> String {
    text.split("#[cfg(test)]")
        .next()
        .unwrap_or("")
        .split("#[cfg(feature = \"testing\")]")
        .next()
        .unwrap_or("")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Acceptance 3 (plan 18): a fake session store in production would let anyone in, so nothing in `src/` names one outside a test
/// module, and nothing in `src/` knows the dev example's knobs. (The manifest check above keeps the `fakes` features out of
/// the release graph; this keeps the names out of the code that is compiled into the binary.)
#[test]
fn no_fake_and_no_dev_knob_is_referenced_outside_cfg_test() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [
        "FakeSessionAuth",
        "FakeSecretSource",
        "ports::fakes",
        "hub_platform::fakes",
        "TEST_USER_HEADER",
        "test-doubles",
        "LK_DEV_",
        "DirSecrets",
        // T3: the software behind the edges (a KMS with keys the process holds, a verifier that accepts what it was told to, the
        // sinks that store what they are given). The real edges are Google's; these are for tests and the dev example only.
        "FakeTokenVerifier",
        "FakeKmsSigner",
        "FakeReportSink",
        "FakeSentinelSink",
        "FakeUsers",
        "SoftKms",
        "soft_kms",
        "MemorySecrets",
        "RemoteUrl::local",
        // T4: the identity module's in-memory bucket for checkpoints, and the hub's own session-less composition.
        "MemoryBucket",
        "without_sessions",
        "DenySessions",
    ];
    let files = rust_files(&src);
    assert!(files.len() > 15, "the scan found only {} files", files.len());
    for file in files {
        // `src/testing.rs` is the crate's test-only software; it is compiled only with the `testing` feature, which the next test
        // and the manifest check keep out of every release build.
        if file.file_name().is_some_and(|name| name == "testing.rs") {
            continue;
        }
        let code = production_code(&fs::read_to_string(&file).unwrap());
        for word in forbidden {
            assert!(
                !code.contains(word),
                "{} refers to {word} outside a test module",
                file.display()
            );
        }
    }
}

/// The crate's test-only module is declared behind the feature, and the feature is off by default: a release build has neither.
#[test]
fn the_test_only_module_and_feature_are_not_in_a_release_build() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(
        lib.contains("#[cfg(feature = \"testing\")]\npub mod testing;"),
        "src/testing.rs must be declared behind the `testing` feature"
    );
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("testing = []"));
    assert!(
        !manifest.contains("default = ["),
        "the crate has no default features, so `testing` is never on unless a dev-dependency asks"
    );
    // The only line that switches it on is the crate's own dev-dependency.
    let switching: Vec<&str> = manifest
        .lines()
        .filter(|line| line.contains("\"testing\"") && !line.trim_start().starts_with('#'))
        .collect();
    assert_eq!(switching.len(), 1, "{switching:?}");
    assert!(switching[0].starts_with("hub = { path = \".\""), "{switching:?}");
}

/// The process the tests start (`hub_dev`) has software for secrets. It is an example target: `cargo build --release -p hub`
/// builds the `hub` binary and no example, so it is not in the image (and the Dockerfile builds only that).
#[test]
fn the_dev_example_is_an_example_and_never_a_binary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(root.join("examples/hub_dev.rs").is_file());
    let bins = manifest.matches("[[bin]]").count();
    assert_eq!(bins, 1, "the crate has exactly one binary, `hub`");
    let dockerfile = fs::read_to_string(root.join("Dockerfile")).unwrap();
    assert!(dockerfile.contains("-p hub"));
    assert!(
        !dockerfile.contains("--examples")
            && !dockerfile.contains("hub_dev")
            && !dockerfile.contains("--all-targets")
    );
}

#[test]
fn the_planted_source_is_caught() {
    let bad = "use hub_platform::FakeSessionAuth;\nfn f() {}\n#[cfg(test)]\nmod tests {}\n";
    assert!(production_code(bad).contains("FakeSessionAuth"));
    let in_tests = "fn f() {}\n#[cfg(test)]\nmod tests { use hub_platform::FakeSessionAuth; }\n";
    assert!(!production_code(in_tests).contains("FakeSessionAuth"));
    let in_comment = "// the FakeSessionAuth stays out\nfn f() {}\n";
    assert!(!production_code(in_comment).contains("FakeSessionAuth"));
    // The crate's own `testing` items are the end of the production code, and a name before them is still caught.
    let testing_tail = "fn f() {}\n#[cfg(feature = \"testing\")]\nimpl X { fn without_sessions() {} }\n";
    assert!(!production_code(testing_tail).contains("without_sessions"));
    let before_it = "fn without_sessions() {}\n#[cfg(feature = \"testing\")]\nimpl X {}\n";
    assert!(production_code(before_it).contains("without_sessions"));
}
