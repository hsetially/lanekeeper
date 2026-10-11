//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never
//! drift apart. The fuzz crate is the identity crate's own (`crates/hub-identity/fuzz/`).
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
fn corpus_replay_identity_jwt() {
    replay("identity_jwt", invariants::identity_jwt);
}

#[test]
fn corpus_replay_identity_cookie() {
    replay("identity_cookie", invariants::identity_cookie);
}

/// Every target in `fuzz_targets/` has a corpus and a replay test, and the other way round.
#[test]
fn every_fuzz_target_has_a_corpus_and_a_replay() {
    let targets: BTreeSet<String> = dir_names(&fuzz_dir().join("fuzz_targets"))
        .into_iter()
        .map(|f| f.trim_end_matches(".rs").to_owned())
        .collect();
    let corpora = dir_names(&fuzz_dir().join("corpus"));
    assert_eq!(targets, corpora);
    let this =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus_replay.rs")).unwrap();
    for t in &targets {
        assert!(
            this.contains(&format!("fn corpus_replay_{t}")),
            "no replay test for {t}"
        );
    }
}

mod random_inputs {
    use proptest::prelude::*;

    use super::invariants;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn any_bytes_satisfy_the_jwt_invariants(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            prop_assert_eq!(invariants::identity_jwt(&data), Ok(()));
        }

        #[test]
        fn any_bytes_satisfy_the_cookie_invariants(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            prop_assert_eq!(invariants::identity_cookie(&data), Ok(()));
        }

        #[test]
        fn path_like_text_is_judged_by_the_return_to_rule(s in "[/\\\\a-z?=&%.-]{0,40}") {
            prop_assert_eq!(invariants::identity_cookie(s.as_bytes()), Ok(()));
        }
    }
}
