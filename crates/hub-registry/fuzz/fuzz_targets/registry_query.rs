//! Fuzz target `registry_query` (S11, S12, S22, rule 5): the typed, bounded parsing of every query string of the read API. No parser panics, a name that is not the operation's and a name twice are refused, and what is accepted is within its bounds and survives being written and read again. The invariants are in `crates/hub-registry/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-registry/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::registry_query(data) {
        panic!("{why}");
    }
});
