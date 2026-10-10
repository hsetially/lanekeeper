//! Fuzz target `agent_hub_message` (S22): every message the hub can send, decoded, validated and carried out (S11).
//! The invariants are in `crates/agent/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/agent/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::agent_hub_message(data) {
        panic!("{why}");
    }
});
