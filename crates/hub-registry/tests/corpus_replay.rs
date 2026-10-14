//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never drift
//! apart. The fuzz crate is the registry's own (`crates/hub-registry/fuzz/`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use proptest::prelude::*;

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn dir_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn replay(target: &str, check: invariants::Check) {
    let dir = fuzz_dir().join("corpus").join(target);
    let names = dir_names(&dir);
    assert!(
        names.len() >= invariants::MIN_SEEDS,
        "corpus for {target} has {} seeds, need at least {}",
        names.len(),
        invariants::MIN_SEEDS
    );
    for name in names {
        let bytes = fs::read(dir.join(&name)).unwrap();
        if let Err(why) = check(&bytes) {
            panic!("seed {target}/{name} violates an invariant: {why}");
        }
    }
}

#[test]
fn corpus_replay_registry_delta() {
    replay("registry_delta", invariants::registry_delta);
}

#[test]
fn corpus_replay_registry_cursor() {
    replay("registry_cursor", invariants::registry_cursor);
}

#[test]
fn corpus_replay_registry_query() {
    replay("registry_query", invariants::registry_query);
}

fn seed(target: &str, name: &str) -> Vec<u8> {
    fs::read(fuzz_dir().join("corpus").join(target).join(name)).unwrap()
}

/// The cursor seeds reach what they are named for: keys that fit are made, keys that do not are refused, and text that is a cursor
/// is read as one.
#[test]
fn registry_cursor_seeds_reach_the_cases_they_are_named_for() {
    let probe = |name: &str| invariants::describe_cursor(&seed("registry_cursor", name));
    assert_eq!(probe("empty"), "program made");
    assert_eq!(probe("tree_child_name"), "program made");
    assert_eq!(probe("findings_id"), "program made");
    assert_eq!(probe("history_time_and_id"), "program made");
    assert_eq!(probe("key_at_the_byte_bound_255"), "program made");
    assert_eq!(probe("key_over_the_byte_bound_256"), "program refused");
    assert_eq!(probe("six_fields"), "program made");
    assert_eq!(probe("seven_fields"), "program refused");
    assert_eq!(
        probe("two_long_byte_fields_over_the_raw_bound"),
        "program refused"
    );
    assert_eq!(probe("negative_and_extreme_numbers"), "program made");
    assert_eq!(probe("binary_bytes_in_a_field"), "program made");
    assert_eq!(probe("a_real_cursor_text"), "text parses");
    for refused in [
        "a_cursor_text_with_padding",
        "a_cursor_text_cut_short",
        "a_cursor_of_another_version",
        "a_cursor_text_with_seven_fields",
        "a_cursor_text_with_a_field_over_the_bound",
        "a_cursor_text_with_trailing_bytes",
        "a_cursor_text_of_an_unknown_list",
        "a_cursor_text_with_an_unknown_field_type",
    ] {
        let text = String::from_utf8_lossy(&seed("registry_cursor", refused)).into_owned();
        assert!(
            hub_registry::read::cursor::parse(&text).is_err(),
            "{refused} must not parse"
        );
    }
}

/// The query seeds reach what they are named for.
#[test]
fn registry_query_seeds_reach_the_cases_they_are_named_for() {
    let probe = |name: &str| invariants::describe_query(&seed("registry_query", name));
    assert_eq!(probe("tree_with_prefix_and_limit"), "tree accepted");
    assert_eq!(probe("page_limit_501"), "page_only refused");
    assert_eq!(probe("page_limit_zero"), "page_only refused");
    assert_eq!(probe("page_limit_five"), "page_only accepted");
    assert_eq!(probe("grid_with_two_swimlanes"), "grid accepted");
    assert_eq!(probe("grid_without_swimlanes"), "grid refused");
    assert_eq!(probe("grid_duplicate_swimlane"), "grid refused");
    assert_eq!(probe("duplicate_param"), "page_only refused");
    assert_eq!(probe("unknown_param"), "page_only refused");
    assert_eq!(probe("none_with_a_param"), "none refused");
    assert_eq!(probe("percent_ff_in_a_path"), "tree refused");
    assert_eq!(probe("traversal_prefix"), "tree refused");
    assert_eq!(probe("encoded_traversal_prefix"), "tree refused");
    assert_eq!(probe("text_pattern_with_a_nul"), "search_text refused");
    assert_eq!(probe("text_search_literal"), "search_text accepted");
    assert_eq!(probe("text_caps_over"), "search_text refused");
    assert_eq!(probe("content_blob_without_hash"), "content refused");
    assert_eq!(probe("content_git_with_its_parameters"), "content accepted");
    assert_eq!(probe("content_hash_without_blob"), "content refused");
    assert_eq!(probe("served_ok"), "served accepted");
    assert_eq!(probe("compare_ok"), "compare accepted");
    assert_eq!(probe("findings_kinds_repeated"), "findings accepted");
    assert_eq!(probe("raw_fragment"), "page_only refused");
    assert_eq!(probe("raw_over_8k"), "page_only refused");
    assert_eq!(probe("raw_percent_encoded_name"), "page_only accepted");
    assert_eq!(probe("raw_empty_pairs"), "page_only accepted");
    assert_eq!(probe("raw_plus_and_equals"), "search_settings accepted");
    assert_eq!(probe("raw_non_utf8"), "tree refused");
}

/// The seeds are only worth having if each reaches the part of the checks it is named for. Each seed here is decoded and its
/// verdict read, so a change to the decoder or the rules that turns a seed into a different case is noticed.
#[test]
fn registry_delta_seeds_reach_the_cases_they_are_named_for() {
    // The seeds are programs; to read a verdict the first message is rebuilt by the same decoder through a probe.
    let probe = |name: &str| -> String {
        let bytes = fs::read(fuzz_dir().join("corpus/registry_delta").join(name)).unwrap();
        invariants::describe(&bytes)
    };
    assert_eq!(probe("one_good_file"), "accepted 1 entries, 4 bytes kept");
    assert_eq!(probe("denied_with_bytes"), "refused Invalid");
    assert_eq!(probe("hash_mismatch"), "refused Invalid");
    assert_eq!(probe("size_mismatch"), "refused Invalid");
    assert_eq!(probe("content_over_the_limit"), "refused TooLarge");
    assert_eq!(probe("negative_time"), "refused Invalid");
    assert_eq!(probe("beyond_year_9999"), "refused Invalid");
    assert_eq!(probe("a_year_ahead"), "accepted 1 entries, 4 bytes kept");
    assert_eq!(
        probe("pem_the_agent_did_not_deny"),
        "accepted 1 entries, 0 bytes kept"
    );
    assert_eq!(probe("private_in_the_name"), "accepted 1 entries, 0 bytes kept");
    assert_eq!(probe("denied_without_bytes"), "accepted 1 entries, 0 bytes kept");
    assert_eq!(
        probe("traversal_path_is_dropped"),
        "accepted 0 entries, 0 bytes kept"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    /// Whatever the bytes are, the checks hold.
    #[test]
    fn random_programs_hold_every_invariant(data in proptest::collection::vec(any::<u8>(), 0..400)) {
        if let Err(why) = invariants::registry_delta(&data) {
            panic!("{why}");
        }
    }

    /// Whatever the bytes are, a cursor is canonical and a key is bound to its list and its query.
    #[test]
    fn random_cursor_programs_hold_every_invariant(data in proptest::collection::vec(any::<u8>(), 0..700)) {
        if let Err(why) = invariants::registry_cursor(&data) {
            panic!("{why}");
        }
    }

    /// Programs of small numbers: kinds, few parts, fields of every length class.
    #[test]
    fn small_cursor_programs_hold_every_invariant(data in proptest::collection::vec(0u8..12, 0..400)) {
        if let Err(why) = invariants::registry_cursor(&data) {
            panic!("{why}");
        }
    }

    /// Whatever the bytes are, no query parser panics, and what it accepts is within its bounds.
    #[test]
    fn random_query_programs_hold_every_invariant(data in proptest::collection::vec(any::<u8>(), 0..300)) {
        if let Err(why) = invariants::registry_query(&data) {
            panic!("{why}");
        }
    }

    /// Query programs of small numbers: every operation, every name of it and the awkward values of the pool.
    #[test]
    fn small_query_programs_hold_every_invariant(data in proptest::collection::vec(0u8..48, 0..200)) {
        if let Err(why) = invariants::registry_query(&data) {
            panic!("{why}");
        }
    }

    /// Raw query text, from the characters a query is made of.
    #[test]
    fn raw_query_text_holds_every_invariant(op in 0u8..14, text in "[a-z0-9=&%,./:+ _#-]{0,200}") {
        let mut data = vec![0x80 | op];
        data.extend_from_slice(text.as_bytes());
        if let Err(why) = invariants::registry_query(&data) {
            panic!("{why}");
        }
    }

    /// Programs made of the interesting bytes: flags with every combination, short paths, small contents.
    #[test]
    fn programs_of_small_numbers_hold_every_invariant(data in proptest::collection::vec(0u8..24, 0..400)) {
        if let Err(why) = invariants::registry_delta(&data) {
            panic!("{why}");
        }
    }
}
