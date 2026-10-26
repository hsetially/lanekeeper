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

/// The code of a source file outside its test module and outside comments: from the start to the first `#[cfg(test)]`, without
/// the lines that are only comments.
fn production_code(text: &str) -> String {
    text.split("#[cfg(test)]")
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
    ];
    let files = rust_files(&src);
    assert!(files.len() > 15, "the scan found only {} files", files.len());
    for file in files {
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
}
