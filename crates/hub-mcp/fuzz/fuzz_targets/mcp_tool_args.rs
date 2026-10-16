//! Fuzz target `mcp_tool_args` (S11, S22): arbitrary JSON as the arguments of any of the 27 tools. The invariants are in
//! `crates/hub-mcp/fuzz/invariants_tool_args.rs`, shared with the stable seed replay in
//! `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/tools/args.rs"]
mod args;
#[path = "../invariants_tool_args.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::mcp_tool_args(data) {
        panic!("{why}");
    }
});
