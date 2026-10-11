//! Fuzz target `hub_webhook_payload` (S22): any bytes as the body of a GitHub `push` or `pull_request` delivery, and as the
//! inputs of the signature check. The invariants are in `crates/hub-platform/fuzz/invariants.rs`, shared with the stable seed
//! replay in `crates/hub-platform/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::hub_webhook_payload(data) {
        panic!("{why}");
    }
});
