//! Fuzz target `agent_id_token` (S22): the ID token text the metadata server returns (S5).
//! The invariants are in `crates/agent/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/agent/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::agent_id_token(data) {
        panic!("{why}");
    }
});
