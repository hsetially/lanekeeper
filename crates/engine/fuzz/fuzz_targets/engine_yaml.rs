//! Fuzz target `engine_yaml` (S22): YAML text through parse and flatten, with the limits small (T2).
//! The invariants are in `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_yaml(data) {
        panic!("{why}");
    }
});
