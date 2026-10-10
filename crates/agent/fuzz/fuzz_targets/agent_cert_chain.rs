//! Fuzz target `agent_cert_chain` (S22): the certificate chain the hub issues, as the agent checks it before storing it (S5).
//! The invariants are in `crates/agent/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/agent/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::agent_cert_chain(data) {
        panic!("{why}");
    }
});
