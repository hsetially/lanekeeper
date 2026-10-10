//! Fuzz target `agent_spool_record` (S22, D74): the bytes of a spool segment, opened and replayed. The invariants are in
//! `crates/agent/fuzz/invariants.rs`, shared with the stable seed replay in `crates/agent/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::agent_spool_record(data) {
        panic!("{why}");
    }
});
