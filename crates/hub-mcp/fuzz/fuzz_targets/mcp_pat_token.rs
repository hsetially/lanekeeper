//! Fuzz target `mcp_pat_token` (S7, S11, S22): the syntax of a personal token and the key that hashes it. The invariants are in
//! `crates/hub-mcp/fuzz/invariants.rs`, shared with the stable seed replay in `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/auth/header.rs"]
mod header;
#[path = "../../src/auth/token.rs"]
mod token;
#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::mcp_pat_token(data) {
        panic!("{why}");
    }
});
