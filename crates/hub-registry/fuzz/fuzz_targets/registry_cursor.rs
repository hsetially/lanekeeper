//! Fuzz target `registry_cursor` (S11, S22, rule 5, A31): the keyset cursors of the read API. Any text is parsed without a panic, a text that parses has one spelling, a key is made exactly when it fits, and it is bound to its list and its query. The invariants are in `crates/hub-registry/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-registry/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::registry_cursor(data) {
        panic!("{why}");
    }
});
