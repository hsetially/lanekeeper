//! Fuzz target `identity_cookie` (S22): the cookie, query and redirect values the hub parses (S2, S3).
//! The invariants are in `crates/hub-identity/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-identity/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::identity_cookie(data) {
        panic!("{why}");
    }
});
