//! Fuzz target `engine_path_mapping` (S11, S22): an NFS path, tenant ids, a repository path and application names, separated
//! by NUL bytes, through `reverse_map`, `classify_path`, `map_path`, `file_role` and `logical_file_role` (T4, T10). The
//! invariants are in `crates/engine/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/engine/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::engine_path_mapping(data) {
        panic!("{why}");
    }
});
