//! Fuzz target `mcp_elicitation_result` (S14, S22): the client's answer to the confirmation question, read as a decision. Only an
//! exact accept with `{"confirm": true}` proceeds. The invariants are in `crates/hub-mcp/fuzz/invariants_elicitation.rs`, shared
//! with the stable seed replay in `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/confirm/decision.rs"]
mod decision;
#[path = "../invariants_elicitation.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::mcp_elicitation_result(data) {
        panic!("{why}");
    }
});
