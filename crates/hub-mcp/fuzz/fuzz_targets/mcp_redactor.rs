//! Fuzz target `mcp_redactor` (S14b, S22): arbitrary text through the secret redactor, the window of a file, and the final sweep of a
//! result. The invariants are in `crates/hub-mcp/fuzz/invariants_redactor.rs`, shared with the stable seed replay in
//! `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs`. Like `mcp_http_body`, this target depends on the `hub-mcp` crate: the redactor
//! and the sweep are its public API, and the secrets the input asks for are built at run time, so no corpus file holds a key.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants_redactor.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::mcp_redactor(data) {
        panic!("{why}");
    }
});
