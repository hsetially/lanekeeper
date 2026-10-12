//! Fuzz target `identity_jwt` (S22): the bearer token and key-set text the hub parses (S1).
//! The invariants are in `crates/hub-identity/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-identity/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::identity_jwt(data) {
        panic!("{why}");
    }
});
