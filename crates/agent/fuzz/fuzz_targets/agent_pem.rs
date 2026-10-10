//! Fuzz target `agent_pem` (S22): the PEM text of the private key and of the hub CA file (S5, S6).
//! The invariants are in `crates/agent/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/agent/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::agent_pem(data) {
        panic!("{why}");
    }
});
