//! Fuzz target `nfs_path` (S22). The invariants are in `fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/domain/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::nfs_path(data) {
        panic!("{why}");
    }
});
