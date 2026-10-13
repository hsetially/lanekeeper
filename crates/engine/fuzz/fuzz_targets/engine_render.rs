//! Fuzz target `engine_render` (S11, S22): a property file, a NUL and a text to render, through `property_view` and `render` (T10). The invariants are in
//! `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_render(data) {
        panic!("{why}");
    }
});
