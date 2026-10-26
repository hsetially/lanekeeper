//! Fuzz target `hub_settings` (plan 18, T1; rule 13): the settings read from `LK_*` variables and a file, as `HubSettings::from_parts` does.
//! The invariants are in `crates/hub/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::hub_settings(data) {
        panic!("{why}");
    }
});
