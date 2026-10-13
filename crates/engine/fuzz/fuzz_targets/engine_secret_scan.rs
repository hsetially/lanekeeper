//! Fuzz target `engine_secret_scan` (S11, S13, S22): a flags byte and a text, through `secret_scan` (T7). The invariants are in
//! `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_secret_scan(data) {
        panic!("{why}");
    }
});
