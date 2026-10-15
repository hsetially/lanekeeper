//! Fuzz target `mcp_http_body` (S4, S11, S22): arbitrary bytes as the body of a `POST /mcp` with valid credentials and a live
//! session, through the whole service on fakes. The invariants are in `crates/hub-mcp/fuzz/invariants_http.rs`, shared with the
//! stable seed replay in `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs`. This is the one target that depends on the `hub-mcp`
//! crate (the others include the one source file they test).
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants_http.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::mcp_http_body(data) {
        panic!("{why}");
    }
});
