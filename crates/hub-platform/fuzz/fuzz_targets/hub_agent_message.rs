//! Fuzz target `hub_agent_message` (S22): any bytes decoded as an `AgentMessage` and validated, as on a `Connect` stream.
//! The invariants are in `crates/hub-platform/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-platform/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::hub_agent_message(data) {
        panic!("{why}");
    }
});
