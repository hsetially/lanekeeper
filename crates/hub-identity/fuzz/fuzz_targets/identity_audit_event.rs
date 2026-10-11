//! Fuzz target `identity_audit_event` (S22): the audit event, the stored row, the checkpoint object and the public key (S9).
//! The invariants are in `crates/hub-identity/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub-identity/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::identity_audit_event(data) {
        panic!("{why}");
    }
});
