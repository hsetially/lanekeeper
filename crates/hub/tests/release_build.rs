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
