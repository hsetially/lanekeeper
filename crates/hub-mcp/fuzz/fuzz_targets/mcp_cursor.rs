//! Fuzz target `mcp_cursor` (S11, S22): the cursor a paging tool hands out and takes back. The invariants are in
//! `crates/hub-mcp/fuzz/invariants_cursor.rs`, shared with the stable seed replay in
//! `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/output/cursor.rs"]
mod cursor;
#[path = "../invariants_cursor.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::mcp_cursor(data) {
        panic!("{why}");
    }
});
