//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the
//! same invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can
//! never drift apart.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz")
}

fn dir_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn replays_all_seed_corpora() {
    let corpus = fuzz_dir().join("corpus");
    for (target, check) in invariants::TARGETS {
        let dir = corpus.join(target);
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
}

#[test]
fn every_target_has_a_fuzz_binary_and_a_corpus() {
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
}
