//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never
//! drift apart. The fuzz crate is the engine's own (`crates/engine/fuzz/`, plan decision A2): the root `fuzz/` crate holds
//! the domain's targets and is replayed by `crates/domain/tests/corpus_replay.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use engine::{LimitKind, ParseError, Syntax, flatten, parse};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn dir_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn seed(target: &str, name: &str) -> Vec<u8> {
    fs::read(fuzz_dir().join("corpus").join(target).join(name))
        .unwrap_or_else(|e| panic!("{target}/{name}: {e}"))
}

fn replay(target: &str, check: invariants::Check) {
    let dir = fuzz_dir().join("corpus").join(target);
    let names = dir_names(&dir);
    assert!(
        names.len() >= invariants::MIN_SEEDS,
        "corpus for {target} has {} seeds, need at least {}",
        names.len(),
        invariants::MIN_SEEDS
    );
    for name in names {
        let bytes = fs::read(dir.join(&name)).unwrap();
        if let Err(why) = check(&bytes) {
            panic!("seed {target}/{name} violates an invariant: {why}");
        }
    }
}

#[test]
fn corpus_replay_all_engine_targets() {
    replay("engine_bytes", invariants::engine_bytes);
    replay("engine_yaml", invariants::engine_yaml);
    replay("engine_json", invariants::engine_json);
    replay("engine_properties", invariants::engine_properties);
    replay("engine_flatten", invariants::engine_flatten);
}

#[test]
fn every_target_has_a_fuzz_binary_a_corpus_and_a_manifest_entry() {
    let targets: BTreeSet<String> = invariants::TARGETS.iter().map(|(t, _)| (*t).to_owned()).collect();
    let binaries: BTreeSet<String> = dir_names(&fuzz_dir().join("fuzz_targets"))
        .into_iter()
        .map(|f| f.trim_end_matches(".rs").to_owned())
        .collect();
    let corpora = dir_names(&fuzz_dir().join("corpus"));
    assert_eq!(
        targets, binaries,
        "fuzz/fuzz_targets must hold exactly the targets in fuzz/invariants.rs"
    );
    assert_eq!(
        targets, corpora,
        "fuzz/corpus must hold exactly the targets in fuzz/invariants.rs"
    );
    let manifest = fs::read_to_string(fuzz_dir().join("Cargo.toml")).unwrap();
    for target in &targets {
        assert!(
            manifest.contains(&format!("path = \"fuzz_targets/{target}.rs\"")),
            "fuzz/Cargo.toml has no [[bin]] for {target}"
        );
        // Every fuzz binary calls the check of the same name, so a target cannot assert nothing.
        let source =
            fs::read_to_string(fuzz_dir().join("fuzz_targets").join(format!("{target}.rs"))).unwrap();
        assert!(
            source.contains(&format!("invariants::{target}(data)")),
            "{target} does not run its check"
        );
    }
}

/// The replay is not blind: the checks do notice the harm they exist to find.
#[test]
fn the_checks_catch_a_planted_violation() {
    invariants::planted_harm_is_detected().unwrap();
}

/// The hostile seeds mean something only while they do what they are named for under the limits the fuzzer uses.
#[test]
fn the_hostile_seeds_hit_the_limits_they_are_named_for() {
    let limits = invariants::fuzz_limits();
    let text = |target: &str, name: &str| String::from_utf8(seed(target, name)).unwrap();
    let limit = |target: &str, name: &str, syntax: Syntax| match parse(&text(target, name), syntax, &limits) {
        Err(ParseError::Limit(kind)) => kind,
        other => panic!("{target}/{name}: expected a limit, got {:?}", other.map(|_| ())),
    };
    assert_eq!(limit("engine_yaml", "alias_bomb", Syntax::Yaml), LimitKind::Nodes);
    assert_eq!(limit("engine_yaml", "merge_bomb", Syntax::Yaml), LimitKind::Nodes);
    assert_eq!(
        limit("engine_yaml", "oversized", Syntax::Yaml),
        LimitKind::InputBytes
    );
    assert_eq!(limit("engine_yaml", "deep_flow", Syntax::Yaml), LimitKind::Depth);
    assert_eq!(limit("engine_yaml", "deep_block", Syntax::Yaml), LimitKind::Depth);
    assert_eq!(
        limit("engine_json", "oversized", Syntax::Json),
        LimitKind::InputBytes
    );
    assert_eq!(
        limit("engine_json", "deep_arrays", Syntax::Json),
        LimitKind::Depth
    );
    assert_eq!(
        limit("engine_json", "deep_objects", Syntax::Json),
        LimitKind::Depth
    );
    assert_eq!(
        limit("engine_properties", "oversized", Syntax::Properties),
        LimitKind::InputBytes
    );
    // The key longer than the path limit.
    assert_eq!(
        limit("engine_properties", "hostile_keys", Syntax::Properties),
        LimitKind::SettingPathBytes
    );
    assert!(matches!(
        parse(&text("engine_yaml", "self_reference"), Syntax::Yaml, &limits),
        Err(ParseError::UnknownAlias { .. })
    ));
    // Seeds that are meant to be good parse and give settings.
    for (target, name, syntax) in [
        ("engine_yaml", "valid_nested", Syntax::Yaml),
        ("engine_yaml", "valid_anchor_merge", Syntax::Yaml),
        ("engine_yaml", "valid_multidoc", Syntax::Yaml),
        ("engine_json", "valid_object", Syntax::Json),
        ("engine_json", "valid_multiline", Syntax::Json),
        ("engine_properties", "valid", Syntax::Properties),
        ("engine_properties", "continuation", Syntax::Properties),
    ] {
        let parsed = parse(&text(target, name), syntax, &limits).unwrap();
        assert!(
            !flatten(&parsed, &limits).unwrap().settings.is_empty(),
            "{target}/{name}"
        );
    }
}

#[test]
fn the_huge_scalar_seeds_hit_the_scalar_limit() {
    let limits = invariants::fuzz_limits();
    for (target, name, syntax) in [
        ("engine_yaml", "huge_scalar", Syntax::Yaml),
        ("engine_yaml", "huge_quoted_scalar", Syntax::Yaml),
        ("engine_json", "huge_scalar", Syntax::Json),
        ("engine_properties", "huge_scalar", Syntax::Properties),
    ] {
        let text = String::from_utf8(seed(target, name)).unwrap();
        assert_eq!(
            parse(&text, syntax, &limits).map(|_| ()),
            Err(ParseError::Limit(LimitKind::ScalarBytes)),
            "{target}/{name}"
        );
    }
    let key = String::from_utf8(seed("engine_yaml", "huge_key")).unwrap();
    assert_eq!(
        parse(&key, Syntax::Yaml, &limits).map(|_| ()),
        Err(ParseError::Limit(LimitKind::SettingPathBytes))
    );
}
