//! Fuzz target `engine_bytes` (S22): classification, encodings and line endings of any bytes (T1).
//! The invariants are in `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_bytes(data) {
        panic!("{why}");
    }
});
