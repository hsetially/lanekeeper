//! Fuzz target `registry_delta` (S22, S11, S17): the checks a scan delta passes before the hub stores anything, the bounds on a
//! logical delta, and what becomes of the versions of a path. The invariants are in `crates/hub-registry/fuzz/invariants.rs`,
//! shared with the stable seed replay in `crates/hub-registry/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::registry_delta(data) {
        panic!("{why}");
    }
});
