//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the
//! same invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never
//! drift apart. The fuzz crate is the agent's own (`crates/agent/fuzz/`, decision A16); the root `fuzz/` crate holds the
//! domain's targets and is replayed by `crates/domain/tests/corpus_replay.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn dir_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
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
fn corpus_replay_agent_path() {
    replay("agent_path", invariants::agent_path);
}

#[test]
fn corpus_replay_agent_hub_message() {
    replay("agent_hub_message", invariants::agent_hub_message);
}

#[test]
fn corpus_replay_agent_cert_chain() {
    replay("agent_cert_chain", invariants::agent_cert_chain);
}

#[test]
fn corpus_replay_agent_pem() {
    replay("agent_pem", invariants::agent_pem);
}

#[test]
fn corpus_replay_agent_id_token() {
    replay("agent_id_token", invariants::agent_id_token);
}

#[test]
fn corpus_replay_agent_spool_record() {
    replay("agent_spool_record", invariants::agent_spool_record);
}

/// The spool seeds are valid segments and damaged copies of them, which only mean something while they match the record
/// format. They are made by `invariants::spool_seeds`, and this test fails if the files drift from it; run with
/// `UPDATE_SPOOL_SEEDS=1` to rewrite them after a deliberate change to the format.
#[test]
fn the_spool_seeds_are_what_the_generator_makes() {
    let dir = fuzz_dir().join("corpus").join("agent_spool_record");
    let seeds = invariants::spool_seeds();
    if std::env::var_os("UPDATE_SPOOL_SEEDS").is_some() {
        fs::create_dir_all(&dir).unwrap();
        for stale in dir_names(&dir) {
            fs::remove_file(dir.join(stale)).unwrap();
        }
        for (name, bytes) in &seeds {
            fs::write(dir.join(name), bytes).unwrap();
        }
    }
    let expected: BTreeSet<String> = seeds.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(
        dir_names(&dir),
        expected,
        "the files in corpus/agent_spool_record differ from the generator (UPDATE_SPOOL_SEEDS=1 rewrites them)"
    );
    for (name, bytes) in &seeds {
        assert_eq!(
            &fs::read(dir.join(name)).unwrap(),
            bytes,
            "seed {name} differs from the generator (UPDATE_SPOOL_SEEDS=1 rewrites it)"
        );
    }
    assert!(seeds.len() >= invariants::MIN_SEEDS);
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
    }
    // Every fuzz binary calls the check of the same name, so a target cannot assert nothing.
    for target in &targets {
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
    invariants::planted_spool_harm_is_detected().unwrap();
}
