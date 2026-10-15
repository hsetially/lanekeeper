//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify-07`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never drift
//! apart. The fuzz crate is this crate's own (`crates/hub-mcp/fuzz/`, decision A10 of plan 07). The targets include
//! `src/auth/header.rs` and `src/auth/token.rs` as they are; here the same two modules are the library's.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use hub_mcp::auth::header::HeaderError as E;
use proptest::prelude::*;

mod header {
    pub use hub_mcp::auth::header::*;
}
mod token {
    pub use hub_mcp::auth::token::*;
}
#[path = "../../fuzz/invariants.rs"]
mod invariants;

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
fn replay_all_mcp_targets() {
    replay("mcp_bearer_header", invariants::mcp_bearer_header);
    replay("mcp_pat_token", invariants::mcp_pat_token);
}

#[test]
fn every_fuzz_target_has_a_corpus_and_a_source_file_and_no_other_exists() {
    let targets = dir_names(&fuzz_dir().join("fuzz_targets"));
    let corpora = dir_names(&fuzz_dir().join("corpus"));
    let expected: BTreeSet<String> = ["mcp_bearer_header", "mcp_pat_token"].map(str::to_owned).into();
    assert_eq!(corpora, expected);
    assert_eq!(
        targets,
        expected
            .iter()
            .map(|t| format!("{t}.rs"))
            .collect::<BTreeSet<_>>()
    );
    let manifest = fs::read_to_string(fuzz_dir().join("Cargo.toml")).unwrap();
    for target in &expected {
        assert!(
            manifest.contains(&format!("name = \"{target}\"")),
            "{target} is not a [[bin]]"
        );
    }
}

#[test]
fn the_seeds_hold_what_they_are_named_for() {
    // The seeds that matter reach the code they are for: an accepted header, a refused one for each class, an accepted token.
    let read =
        |target: &str, name: &str| fs::read(fuzz_dir().join("corpus").join(target).join(name)).unwrap();
    let parse = |bytes: &[u8]| {
        let mut map = http::HeaderMap::new();
        for line in bytes.split(|&b| b == b'\n') {
            if let Ok(v) = http::HeaderValue::from_bytes(line) {
                map.append(http::header::AUTHORIZATION, v);
            }
        }
        hub_mcp::auth::header::bearer(&map).map(str::to_owned)
    };
    assert!(parse(&read("mcp_bearer_header", "bearer_plain")).is_ok());
    assert_eq!(
        parse(&read("mcp_bearer_header", "two_headers")),
        Err(E::Duplicate)
    );
    assert_eq!(
        parse(&read("mcp_bearer_header", "basic_scheme")),
        Err(E::NotBearer)
    );
    assert_eq!(
        parse(&read("mcp_bearer_header", "oversize_token")),
        Err(E::TooLong)
    );
    assert_eq!(
        parse(&read("mcp_bearer_header", "scheme_only")),
        Err(E::Malformed)
    );
    assert!(parse(&read("mcp_bearer_header", "at_the_limit")).is_ok());
    let valid = read("mcp_pat_token", "valid_token");
    assert!(hub_mcp::auth::token::PersonalToken::parse(std::str::from_utf8(&valid).unwrap()).is_some());
    let second = read("mcp_pat_token", "second_spelling_of_all_a");
    assert!(hub_mcp::auth::token::PersonalToken::parse(std::str::from_utf8(&second).unwrap()).is_none());
    let key = read("mcp_pat_token", "key_text_valid");
    assert!(hub_mcp::auth::token::HmacKey::from_secret_text(std::str::from_utf8(&key).unwrap()).is_ok());
}

#[test]
fn the_two_files_the_fuzz_targets_include_need_nothing_of_the_crate() {
    // The fuzz crate builds only these two files, so they may name the standard library and the crates in its manifest, and
    // nothing else. A line that reaches for the rest of this crate would break the fuzz build, not this test.
    for file in ["src/auth/header.rs", "src/auth/token.rs"] {
        let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for forbidden in [
                "crate::",
                "super::",
                "domain::",
                "ports::",
                "hub_platform",
                "hub_mcp",
                "tokio",
                "axum",
                "rmcp",
            ] {
                assert!(!line.contains(forbidden), "{file} uses `{forbidden}`: {line}");
            }
        }
    }
}

proptest! {
    #[test]
    fn arbitrary_bytes_satisfy_every_invariant(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        prop_assert_eq!(invariants::mcp_bearer_header(&bytes), Ok(()));
        prop_assert_eq!(invariants::mcp_pat_token(&bytes), Ok(()));
    }

    #[test]
    fn bearer_values_built_from_the_alphabet_satisfy_the_invariants(
        scheme in "(?i)bearer",
        token in "[A-Za-z0-9._~+/-]{1,80}={0,3}",
        extra in prop::option::of("[ -~]{0,20}"),
    ) {
        let mut line = format!("{scheme} {token}").into_bytes();
        if let Some(extra) = extra {
            line.push(b'\n');
            line.extend_from_slice(extra.as_bytes());
        }
        prop_assert_eq!(invariants::mcp_bearer_header(&line), Ok(()));
    }

    #[test]
    fn tokens_of_the_right_shape_satisfy_the_invariants(body in "[A-Za-z0-9_-]{43}") {
        let text = format!("lkpat_{body}");
        prop_assert_eq!(invariants::mcp_pat_token(text.as_bytes()), Ok(()));
    }
}
