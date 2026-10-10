//! Fuzz target `agent_path` (S22): every file operation on an arbitrary path, against a root with planted symlinks (S17).
//! The invariants are in `crates/agent/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/agent/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::agent_path(data) {
        panic!("{why}");
    }
});
