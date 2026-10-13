//! Fuzz target `engine_regex` (S11, S13, S22): a flags byte, a pattern, a NUL and the text searched, through `search_text` (T7). The invariants are in
//! `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_regex(data) {
        panic!("{why}");
    }
});
