//! Fuzz target `engine_index_settings` (S22): bytes through `index_settings` and `locate`, as YAML, JSON or `.properties`
//! (the first byte chooses), with the limits small (T3). The invariants are in `crates/engine/fuzz/invariants.rs`, shared
//! with the stable seed replay in `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_index_settings(data) {
        panic!("{why}");
    }
});
