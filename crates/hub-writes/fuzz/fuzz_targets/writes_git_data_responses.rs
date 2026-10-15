//! Fuzz target `writes_git_data_responses` (S22): bytes read as GitHub's answers to the Git Data calls of the pull-request path, and
//! as the text of a branch name, an object id and a repository path. The invariants are in `crates/hub-writes/fuzz/invariants.rs`,
//! shared with the stable seed replay in `crates/hub-writes/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::writes_git_data_responses(data) {
        panic!("{why}");
    }
});
