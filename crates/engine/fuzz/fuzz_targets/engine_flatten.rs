//! Fuzz target `engine_flatten` (S22): the setting path syntax: keys and indexes written and read back (T2).
//! The invariants are in `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_flatten(data) {
        panic!("{why}");
    }
});
