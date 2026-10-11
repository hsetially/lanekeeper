//! Fuzz target `hub_forward_request` (S22): any bytes decoded as a `ForwardRequest` from a peer replica and validated. The
//! invariants are in `crates/hub-platform/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-platform/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::hub_forward_request(data) {
        panic!("{why}");
    }
});
