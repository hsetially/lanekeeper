//! Fuzz target `writes_github_token` (S22): bytes read as the body of a token request, and as the text of a token. The invariants are in
//! `crates/hub-writes/fuzz/invariants.rs`, shared with the stable seed replay in `crates/hub-writes/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::writes_github_token(data) {
        panic!("{why}");
    }
});
