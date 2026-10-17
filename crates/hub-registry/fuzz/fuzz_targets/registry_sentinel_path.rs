//! Fuzz target `registry_sentinel_path` (S11, S22, rule 10, A17, S25): the paths of a sentinel's records, mapped to the paths of its swimlane. Any
//! roots and any path are mapped without a panic, to exactly what an oracle written from the rules says, and what comes out is a clean
//! `NfsPath` that no traversal survives. The invariants are in `crates/hub-registry/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-registry/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::registry_sentinel_path(data) {
        panic!("{why}");
    }
});
