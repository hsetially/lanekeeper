//! Fuzz target `hub_seed_file` (plan 18, T7; rule 13): the seed file, as `hub::seed::file::parse` reads it, and what is planned from it.
//! The invariants are in `crates/hub/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::hub_seed_file(data) {
        panic!("{why}");
    }
});
