//! Fuzz target `hub_pem` (plan 18, T1; rule 13): a certificate chain and a key as the web listener's secrets hold them, as `web_tls::decode` reads them.
//! The invariants are in `crates/hub/fuzz/invariants.rs`, shared with the stable seed replay in
//! `crates/hub/tests/corpus_replay.rs`.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = invariants::hub_pem(data) {
        panic!("{why}");
    }
});
