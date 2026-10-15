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
mod args {
    pub use hub_mcp::tools::args::*;
}
mod cursor {
    pub use hub_mcp::output::cursor::*;
}
#[path = "../../fuzz/invariants.rs"]
mod invariants;
#[path = "../../fuzz/invariants_cursor.rs"]
mod invariants_cursor;
#[path = "../../fuzz/invariants_http.rs"]
mod invariants_http;
#[path = "../../fuzz/invariants_tool_args.rs"]
mod invariants_tool_args;

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
    replay_at_least(target, check, invariants::MIN_SEEDS);
}

fn replay_at_least(target: &str, check: invariants::Check, min_seeds: usize) {
    let dir = fuzz_dir().join("corpus").join(target);
    let names = dir_names(&dir);
    assert!(
        names.len() >= min_seeds,
        "corpus for {target} has {} seeds, need at least {min_seeds}",
        names.len(),
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
    replay_at_least(
        "mcp_tool_args",
        invariants_tool_args::mcp_tool_args,
        invariants_tool_args::MIN_SEEDS,
    );
    replay_at_least(
        "mcp_cursor",
        invariants_cursor::mcp_cursor,
        invariants_cursor::MIN_SEEDS,
    );
    replay_at_least(
        "mcp_http_body",
        invariants_http::mcp_http_body,
        invariants_http::MIN_SEEDS,
    );
}

#[test]
fn every_fuzz_target_has_a_corpus_and_a_source_file_and_no_other_exists() {
    let targets = dir_names(&fuzz_dir().join("fuzz_targets"));
    let corpora = dir_names(&fuzz_dir().join("corpus"));
    let expected: BTreeSet<String> = [
        "mcp_bearer_header",
        "mcp_pat_token",
        "mcp_tool_args",
        "mcp_cursor",
        "mcp_http_body",
    ]
    .map(str::to_owned)
    .into();
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

#[test]
fn the_files_the_tool_args_and_cursor_targets_include_need_nothing_more_of_the_crate() {
    // `args.rs` may name `domain`, `serde`, `serde_json` and `thiserror`; `cursor.rs` only `base64` and `thiserror`.
    let rest = [
        "crate::",
        "super::",
        "ports::",
        "hub_platform",
        "hub_mcp",
        "tokio",
        "axum",
        "rmcp",
        "use engine",
    ];
    for (file, extra) in [
        ("src/tools/args.rs", &[][..]),
        ("src/output/cursor.rs", &["domain::", "serde", "http"][..]),
    ] {
        let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for forbidden in rest.iter().chain(extra) {
                assert!(!line.contains(forbidden), "{file} uses `{forbidden}`: {line}");
            }
        }
    }
}

#[test]
fn the_new_seeds_hold_what_they_are_named_for() {
    let read =
        |target: &str, name: &str| fs::read(fuzz_dir().join("corpus").join(target).join(name)).unwrap();
    let parsed = |name: &str| {
        let bytes = read("mcp_tool_args", name);
        let tool = hub_mcp::tools::ToolName::ALL[usize::from(bytes[0]) % hub_mcp::tools::ToolName::ALL.len()];
        let value: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
        hub_mcp::tools::parse_args(tool, value.as_object().cloned())
    };
    use hub_mcp::tools::ArgError;
    for valid in [
        "valid_get_swimlane",
        "valid_read_file_lines",
        "valid_read_file_setting_path",
        "valid_compare_refs",
        "valid_compare_grid",
        "valid_search_text",
        "valid_propose_change",
        "valid_raise_pr_null_body",
        "valid_restart_service",
        "valid_read_doc",
    ] {
        assert!(parsed(valid).is_ok(), "{valid}: {:?}", parsed(valid));
    }
    assert_eq!(parsed("unknown_field"), Err(ArgError::Invalid));
    assert_eq!(parsed("missing_required"), Err(ArgError::Missing("path")));
    assert_eq!(parsed("wrong_type"), Err(ArgError::Invalid));
    assert_eq!(parsed("null_optional"), Err(ArgError::Invalid));
    assert_eq!(parsed("path_traversal"), Err(ArgError::Invalid));
    assert_eq!(parsed("docs_path_traversal"), Err(ArgError::Invalid));
    assert_eq!(parsed("duplicate_swimlanes"), Err(ArgError::Invalid));
    assert_eq!(parsed("uppercase_hash"), Err(ArgError::Invalid));
    assert_eq!(parsed("pattern_over_512_bytes"), Err(ArgError::Invalid));

    // Cursors: the valid ones are read for the tool they name, and the others are refused.
    let cursor = |name: &str| {
        let bytes = read("mcp_cursor", name);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let (tag, rest) = text.split_once('\n').unwrap_or((text.as_str(), ""));
        hub_mcp::output::cursor::decode(rest, tag).is_ok()
    };
    for valid in [
        "valid_start",
        "valid_with_inner_key",
        "valid_skip_at_the_max",
        "valid_service_key",
    ] {
        assert!(cursor(valid), "{valid}");
    }
    for refused in [
        "skip_over_the_max",
        "made_for_another_tool",
        "padded",
        "upper_case_prefix",
        "leading_zero_skip",
        "not_base64",
        "empty_cursor",
        "tag_only",
        "oversize_inner",
        "space_in_inner",
        "four_lines",
        "upper_case_tag",
        "hostile_unicode",
        "standard_alphabet",
    ] {
        assert!(!cursor(refused), "{refused}");
    }
}

proptest! {
    #[test]
    fn arbitrary_arguments_satisfy_the_tool_args_invariants(
        selector in any::<u8>(),
        text in "[ -~]{0,200}",
    ) {
        let mut data = vec![selector];
        data.extend_from_slice(text.as_bytes());
        prop_assert_eq!(invariants_tool_args::mcp_tool_args(&data), Ok(()));
    }

    #[test]
    fn objects_of_declared_names_satisfy_the_tool_args_invariants(
        selector in any::<u8>(),
        values in proptest::collection::vec(
            prop_oneof![
                Just("null".to_owned()),
                Just("true".to_owned()),
                any::<u32>().prop_map(|n| n.to_string()),
                "[a-z0-9_./:@-]{0,40}".prop_map(|s| format!("\"{s}\"")),
                "[a-z0-9]{1,8}".prop_map(|s| format!("[\"{s}\",\"{s}\"]")),
                Just("{\"start\":1,\"end\":9}".to_owned()),
                Just("{\"state\":\"absent\"}".to_owned()),
            ],
            0..8,
        ),
    ) {
        let tool = hub_mcp::tools::ToolName::ALL[usize::from(selector) % hub_mcp::tools::ToolName::ALL.len()];
        let (known, _) = hub_mcp::tools::args::declared(tool);
        let body: Vec<String> = known.iter().zip(&values).map(|(k, v)| format!("\"{k}\":{v}")).collect();
        let mut data = vec![selector];
        data.extend_from_slice(format!("{{{}}}", body.join(",")).as_bytes());
        prop_assert_eq!(invariants_tool_args::mcp_tool_args(&data), Ok(()));
    }

    #[test]
    fn arbitrary_cursors_satisfy_the_cursor_invariants(bytes in proptest::collection::vec(any::<u8>(), 0..700)) {
        prop_assert_eq!(invariants_cursor::mcp_cursor(&bytes), Ok(()));
    }

    #[test]
    fn made_cursors_round_trip(
        tag in "[a-z_]{1,32}",
        skip in 0u32..=200,
        inner in "[!-~]{0,300}",
    ) {
        let cursor = hub_mcp::output::cursor::encode(&tag, Some(&inner).filter(|i| !i.is_empty()).map(String::as_str), skip).unwrap();
        let mut data = tag.clone().into_bytes();
        data.push(b'\n');
        data.extend_from_slice(cursor.as_bytes());
        prop_assert_eq!(invariants_cursor::mcp_cursor(&data), Ok(()));
        let wrapped = hub_mcp::output::cursor::decode(&cursor, &tag).unwrap();
        prop_assert_eq!(wrapped.skip, skip);
        prop_assert_eq!(wrapped.inner.unwrap_or_default(), inner);
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
