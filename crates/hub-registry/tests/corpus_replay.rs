//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never drift
//! apart. The fuzz crate is the registry's own (`crates/hub-registry/fuzz/`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use proptest::prelude::*;

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
fn corpus_replay_registry_delta() {
    replay("registry_delta", invariants::registry_delta);
}

/// The seeds are only worth having if each reaches the part of the checks it is named for. Each seed here is decoded and its
/// verdict read, so a change to the decoder or the rules that turns a seed into a different case is noticed.
#[test]
fn registry_delta_seeds_reach_the_cases_they_are_named_for() {
    // The seeds are programs; to read a verdict the first message is rebuilt by the same decoder through a probe.
    let probe = |name: &str| -> String {
        let bytes = fs::read(fuzz_dir().join("corpus/registry_delta").join(name)).unwrap();
        invariants::describe(&bytes)
    };
    assert_eq!(probe("one_good_file"), "accepted 1 entries, 4 bytes kept");
    assert_eq!(probe("denied_with_bytes"), "refused Invalid");
    assert_eq!(probe("hash_mismatch"), "refused Invalid");
    assert_eq!(probe("size_mismatch"), "refused Invalid");
    assert_eq!(probe("content_over_the_limit"), "refused TooLarge");
    assert_eq!(probe("negative_time"), "refused Invalid");
    assert_eq!(probe("beyond_year_9999"), "refused Invalid");
    assert_eq!(probe("a_year_ahead"), "accepted 1 entries, 4 bytes kept");
    assert_eq!(
        probe("pem_the_agent_did_not_deny"),
        "accepted 1 entries, 0 bytes kept"
    );
    assert_eq!(probe("private_in_the_name"), "accepted 1 entries, 0 bytes kept");
    assert_eq!(probe("denied_without_bytes"), "accepted 1 entries, 0 bytes kept");
    assert_eq!(
        probe("traversal_path_is_dropped"),
        "accepted 0 entries, 0 bytes kept"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    /// Whatever the bytes are, the checks hold.
    #[test]
    fn random_programs_hold_every_invariant(data in proptest::collection::vec(any::<u8>(), 0..400)) {
        if let Err(why) = invariants::registry_delta(&data) {
            panic!("{why}");
        }
    }

    /// Programs made of the interesting bytes: flags with every combination, short paths, small contents.
    #[test]
    fn programs_of_small_numbers_hold_every_invariant(data in proptest::collection::vec(0u8..24, 0..400)) {
        if let Err(why) = invariants::registry_delta(&data) {
            panic!("{why}");
        }
    }
}
