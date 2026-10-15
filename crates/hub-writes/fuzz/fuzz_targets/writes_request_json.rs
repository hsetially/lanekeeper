//! Fuzz target `writes_request_json` (S22): the JSON bodies of an edit, a revert and a restart, and the lines of a delete. The invariants are in
//! `crates/hub-writes/fuzz/invariants.rs`, shared with the stable seed replay in `crates/hub-writes/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::writes_request_json(data) {
        panic!("{why}");
    }
});
