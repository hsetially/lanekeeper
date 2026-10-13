//! Fuzz target `engine_properties` (S22): `.properties` text through parse and flatten (T2).
//! The invariants are in `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_properties(data) {
        panic!("{why}");
    }
});
