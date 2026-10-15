//! Fuzz target `writes_upload_multipart` (S22): a multipart body, read as an upload form and as a preview form. The invariants are in
//! `crates/hub-writes/fuzz/invariants.rs`, shared with the stable seed replay in `crates/hub-writes/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::writes_upload_multipart(data) {
        panic!("{why}");
    }
});
