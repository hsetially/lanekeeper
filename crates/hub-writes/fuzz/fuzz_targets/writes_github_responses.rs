//! Fuzz target `writes_github_responses` (S22): bytes read as GitHub's answers about a token. The invariants are in
//! `crates/hub-writes/fuzz/invariants.rs`, shared with the stable seed replay in `crates/hub-writes/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::writes_github_responses(data) {
        panic!("{why}");
    }
});
