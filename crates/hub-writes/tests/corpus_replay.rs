//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never drift
//! apart. The fuzz crate is the write path's own (`crates/hub-writes/fuzz/`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines
)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use domain::ContentHash;
use serde_json::json;

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn corpus(target: &str) -> PathBuf {
    fuzz_dir().join("corpus").join(target)
}

fn names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// Replay every seed of a target. A seed's name says what it is for: `accepted_*` must still be accepted by the reader and
/// `rejected_*` refused, so a change that quietly flips a seed fails here.
fn replay(target: &str) {
    let (_, check, accepts) = invariants::TARGETS.iter().find(|(n, _, _)| *n == target).unwrap();
    let dir = corpus(target);
    let names = names(&dir);
    assert!(
        names.len() >= invariants::MIN_SEEDS,
        "corpus for {target} has {} seeds, need at least {}",
        names.len(),
        invariants::MIN_SEEDS
    );
    let mut accepted = 0;
    let mut rejected = 0;
    for name in names {
        let bytes = fs::read(dir.join(&name)).unwrap();
        if let Err(why) = check(&bytes) {
            panic!("seed {target}/{name} violates an invariant: {why}");
        }
        if name.starts_with("accepted_") {
            assert!(accepts(&bytes), "seed {target}/{name} is no longer accepted");
            accepted += 1;
        } else if name.starts_with("rejected_") {
            assert!(!accepts(&bytes), "seed {target}/{name} is no longer refused");
            rejected += 1;
        } else {
            panic!("seed {target}/{name}: a seed is named accepted_* or rejected_*");
        }
    }
    assert!(
        accepted >= 2 && rejected >= 2,
        "{target}: both kinds of seed are needed"
    );
}

#[test]
fn corpus_replay_writes_request_json() {
    replay("writes_request_json");
}

#[test]
fn corpus_replay_writes_upload_multipart() {
    replay("writes_upload_multipart");
}

#[test]
fn corpus_replay_writes_preview_id() {
    replay("writes_preview_id");
}

#[test]
fn corpus_replay_writes_github_token() {
    replay("writes_github_token");
}

#[test]
fn corpus_replay_writes_github_responses() {
    replay("writes_github_responses");
}

#[test]
fn corpus_replay_writes_git_data_responses() {
    replay("writes_git_data_responses");
}

/// A stand-in for the fuzzer on stable: every seed of the Git Data target, cut and changed byte by byte in a fixed pseudo-random
/// way, still keeps the invariants. The fuzzer goes much further; this keeps a regression in the readers from waiting for it.
#[test]
fn mutated_git_data_seeds_keep_the_invariants() {
    let dir = corpus("writes_git_data_responses");
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for name in names(&dir) {
        let seed = fs::read(dir.join(&name)).unwrap();
        for round in 0..400 {
            let mut bytes = seed.clone();
            for _ in 0..=(next() % 4) {
                if bytes.is_empty() {
                    break;
                }
                let at = usize::try_from(next()).unwrap() % bytes.len();
                match next() % 4 {
                    0 => bytes[at] = u8::try_from(next() % 256).unwrap(),
                    1 => bytes.truncate(at),
                    2 => {
                        bytes.remove(at);
                    }
                    _ => bytes.insert(
                        at,
                        b"\"{}[]:,\\ 0aF"
                            .get(usize::try_from(next()).unwrap() % 13)
                            .copied()
                            .unwrap_or(b'x'),
                    ),
                }
            }
            if let Err(why) = invariants::writes_git_data_responses(&bytes) {
                panic!(
                    "a change of seed {name} (round {round}) violates an invariant: {why}\n{:?}",
                    String::from_utf8_lossy(&bytes)
                );
            }
        }
    }
}

#[test]
fn every_target_has_a_fuzz_binary_and_a_corpus() {
    let manifest = fs::read_to_string(fuzz_dir().join("Cargo.toml")).unwrap();
    for (name, _, _) in invariants::TARGETS {
        assert!(
            manifest.contains(&format!("name = \"{name}\"")),
            "{name} is not a [[bin]] of the fuzz crate"
        );
        assert!(
            fuzz_dir()
                .join("fuzz_targets")
                .join(format!("{name}.rs"))
                .is_file(),
            "{name} has no target file"
        );
        assert!(corpus(name).is_dir(), "{name} has no corpus");
    }
    let bins = names(&fuzz_dir().join("fuzz_targets"));
    assert_eq!(
        bins.len(),
        invariants::TARGETS.len(),
        "a fuzz target file without a check: {bins:?}"
    );
}

// ------------------------------------------------------------------------------------------------------------- the seeds

fn h(text: &str) -> String {
    ports::content_hash(text.as_bytes()).to_string()
}

fn form(content_type: &str, fields: &[(&str, &[u8])]) -> Vec<u8> {
    const B: &str = "----seed";
    let mut out = format!("{content_type}\n").into_bytes();
    for (name, value) in fields {
        out.extend_from_slice(format!("--{B}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes());
        if *name == "file" {
            out.extend_from_slice(b"; filename=\"f\"\r\nContent-Type: application/octet-stream");
        }
        out.extend_from_slice(b"\r\n\r\n");
        out.extend_from_slice(value);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{B}--\r\n").as_bytes());
    out
}

const CT: &str = "multipart/form-data; boundary=----seed";

fn seeds() -> Vec<(&'static str, String, Vec<u8>)> {
    let mut out: Vec<(&'static str, String, Vec<u8>)> = Vec::new();
    let mut add =
        |target: &'static str, name: &str, bytes: Vec<u8>| out.push((target, name.to_owned(), bytes));
    let json = |v: serde_json::Value| serde_json::to_vec(&v).unwrap();

    // ---- writes_request_json
    let t = "writes_request_json";
    add(
        t,
        "accepted_edit_hash",
        json(
            json!({ "path": "tx-api/a.yml", "expected": { "state": "hash", "hash": h("x: 1\n") }, "content": "x: 2\n" }),
        ),
    );
    add(
        t,
        "accepted_edit_absent_with_crlf",
        json(
            json!({ "path": "new.yml", "expected": { "state": "absent" }, "content": "a: 1\r\nb: 2\r\n", "change_eol": "crlf" }),
        ),
    );
    add(t, "accepted_edit_escapes", b"{\"path\":\"a.yml\",\"expected\":{\"state\":\"absent\"},\"content\":\"\\u0000\\u0001\\ud83d\\ude00\\n\"}".to_vec());
    add(
        t,
        "accepted_revert",
        json(
            json!({ "path": "a.yml", "expected": { "state": "hash", "hash": h("x: 2\n") }, "to": h("x: 1\n"), "change_eol": "lf" }),
        ),
    );
    add(
        t,
        "accepted_restart",
        json(json!({ "namespace": "sit1", "name": "payments" })),
    );
    add(
        t,
        "accepted_notify",
        json(json!({ "paths": ["tx-api/tx-api.yml", "application.yml", "tx-api/receipt.bmp"] })),
    );
    add(t, "accepted_notify_one_path", json(json!({ "paths": ["a.yml"] })));
    add(t, "rejected_notify_no_paths", json(json!({ "paths": [] })));
    add(
        t,
        "rejected_notify_with_a_single_path",
        json(json!({ "path": "a.yml" })),
    );
    add(
        t,
        "rejected_notify_path_twice",
        json(json!({ "paths": ["a.yml", "a.yml"] })),
    );
    add(t, "rejected_notify_as_array", json(json!([["a.yml"]])));
    add(
        t,
        "rejected_notify_with_a_host",
        json(json!({ "paths": ["a.yml"], "host": "http://config-server.evil.example" })),
    );
    add(
        t,
        "rejected_notify_with_a_traversing_path",
        json(json!({ "paths": ["../../etc/passwd"] })),
    );
    add(
        t,
        "rejected_notify_with_101_paths",
        json(json!({ "paths": (0..=100).map(|n| format!("d/f{n}.yml")).collect::<Vec<_>>() })),
    );
    add(
        t,
        "accepted_reveal",
        json(json!({ "path": "tx-api/creds-sit1.yml", "setting": "creds.aws" })),
    );
    add(
        t,
        "accepted_reveal_with_reason",
        json(
            json!({ "path": "tx-api/creds.yml", "setting": "#1/spring.datasource.password", "reason": "rotating the staging key, ticket 4411" }),
        ),
    );
    add(
        t,
        "rejected_reveal_as_array",
        json(json!(["tx-api/creds.yml", "creds.aws"])),
    );
    add(
        t,
        "rejected_reveal_with_a_line",
        json(json!({ "path": "tx-api/creds.yml", "setting": "creds.aws", "line": 3 })),
    );
    add(
        t,
        "rejected_reveal_without_a_setting",
        json(json!({ "path": "tx-api/creds.yml" })),
    );
    add(
        t,
        "rejected_reveal_with_a_traversing_path",
        json(json!({ "path": "../creds.yml", "setting": "creds.aws" })),
    );
    add(
        t,
        "accepted_delete_lines",
        format!("tx-api/a.yml\n{}\npreview-token-0123456789", h("x: 1\n")).into_bytes(),
    );
    add(
        t,
        "accepted_delete_without_token",
        format!("a.yml\n{}", h("x: 1\n")).into_bytes(),
    );
    add(
        t,
        "accepted_draft",
        json(
            json!({ "swimlane": "sit1", "path": "tx-api/a.yml", "expected": { "state": "hash", "hash": h("x: 1\n") }, "content": "x: 2\n" }),
        ),
    );
    add(
        t,
        "accepted_pr_tenant_target",
        json(
            json!({ "swimlane": "sit1", "paths": ["tx-api/a-sit1.yml", "tx-api/b.yml"], "title": "Sync the NFS changes", "body": "Please review.\n", "target": { "kind": "tenant", "branch": "sit1" } }),
        ),
    );
    add(
        t,
        "accepted_pr_minimal",
        json(json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t" })),
    );
    add(
        t,
        "accepted_pr_base_target_null_text",
        json(
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "body": null, "target": { "kind": "base" } }),
        ),
    );
    add(
        t,
        "rejected_pr_no_paths",
        json(json!({ "swimlane": "sit1", "paths": [], "title": "t" })),
    );
    add(
        t,
        "rejected_pr_path_twice",
        json(json!({ "swimlane": "sit1", "paths": ["a.yml", "a.yml"], "title": "t" })),
    );
    add(
        t,
        "rejected_pr_blank_title",
        json(json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "  " })),
    );
    add(
        t,
        "rejected_pr_text_with_nul",
        b"{\"swimlane\":\"sit1\",\"paths\":[\"a.yml\"],\"title\":\"t\",\"body\":\"a\\u0000b\"}".to_vec(),
    );
    add(
        t,
        "rejected_pr_base_target_with_a_branch",
        json(
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "target": { "kind": "base", "branch": "sit1" } }),
        ),
    );
    add(
        t,
        "rejected_pr_as_array",
        json(json!(["sit1", ["a.yml"], "t", null, null])),
    );
    add(
        t,
        "accepted_approve_with_comment",
        json(json!({ "comment": "looks right" })),
    );
    add(
        t,
        "accepted_approve_with_null_comment",
        json(json!({ "comment": null })),
    );
    add(
        t,
        "accepted_reject_with_reason",
        json(json!({ "reason": "not now" })),
    );
    add(
        t,
        "rejected_reject_with_blank_reason",
        json(json!({ "reason": "   " })),
    );
    add(
        t,
        "rejected_approve_with_control_character",
        b"{\"comment\":\"a\\u0007b\"}".to_vec(),
    );
    // serde would read an array as the struct, field by field; the contract says an object (S11).
    add(
        t,
        "rejected_edit_as_array",
        json(json!(["a.yml", { "state": "absent" }, "x: 1\n", null])),
    );
    add(t, "rejected_restart_as_array", json(json!(["sit1", "payments"])));
    add(t, "rejected_approve_as_array", json(json!(["looks right"])));
    add(t, "rejected_reject_as_array", json(json!(["not now"])));
    add(
        t,
        "rejected_edit_unknown_field",
        json(json!({ "path": "a.yml", "expected": { "state": "absent" }, "content": "", "sneaky": true })),
    );
    add(
        t,
        "rejected_edit_traversal",
        json(json!({ "path": "../../etc/passwd", "expected": { "state": "absent" }, "content": "x" })),
    );
    add(
        t,
        "rejected_edit_bad_hash",
        json(json!({ "path": "a.yml", "expected": { "state": "hash", "hash": "abc" }, "content": "x" })),
    );
    add(t, "rejected_not_json", b"path=a.yml&content=x".to_vec());
    add(
        t,
        "rejected_restart_bad_name",
        json(json!({ "namespace": "Sit 1", "name": "payments" })),
    );
    add(t, "rejected_empty", Vec::new());
    add(
        t,
        "rejected_truncated",
        b"{\"path\":\"a.yml\",\"expected\":{\"sta".to_vec(),
    );

    // ---- writes_upload_multipart
    let t = "writes_upload_multipart";
    let png: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', b'\r', b'\n', 0, 255, b'\n', b'\r', b'\n', b'-', b'-',
    ];
    add(
        t,
        "accepted_upload_absent",
        form(
            CT,
            &[
                ("path", b"new.yml"),
                ("expected_state", b"absent"),
                ("file", b"n: 1\n"),
            ],
        ),
    );
    add(
        t,
        "accepted_upload_hash_with_token",
        form(
            CT,
            &[
                ("path", b"tx-api/a.yml"),
                ("expected_state", b"hash"),
                ("expected_hash", h("x: 1\n").as_bytes()),
                ("preview_id", b"preview-token-0123456789"),
                ("file", b"x: 2\n"),
            ],
        ),
    );
    add(
        t,
        "accepted_upload_binary_file",
        form(
            CT,
            &[
                ("path", b"img/logo.png"),
                ("expected_state", b"absent"),
                ("file", &png),
            ],
        ),
    );
    add(
        t,
        "accepted_preview_upload",
        form(
            CT,
            &[
                ("action", b"upload"),
                ("path", b"new.yml"),
                ("expected_state", b"absent"),
                ("file", b"n: 1\n"),
            ],
        ),
    );
    add(
        t,
        "accepted_preview_delete",
        form(
            CT,
            &[
                ("action", b"delete"),
                ("path", b"a.yml"),
                ("expected_state", b"hash"),
                ("expected_hash", h("x: 1\n").as_bytes()),
            ],
        ),
    );
    add(
        t,
        "accepted_empty_file",
        form(
            CT,
            &[
                ("path", b"empty.txt"),
                ("expected_state", b"absent"),
                ("file", b""),
            ],
        ),
    );
    add(
        t,
        "rejected_upload_without_a_file",
        form(CT, &[("path", b"new.yml"), ("expected_state", b"absent")]),
    );
    add(
        t,
        "rejected_unknown_field",
        form(
            CT,
            &[
                ("path", b"new.yml"),
                ("expected_state", b"absent"),
                ("file", b"x"),
                ("sneaky", b"1"),
            ],
        ),
    );
    add(
        t,
        "rejected_duplicate_field",
        form(
            CT,
            &[
                ("path", b"a.yml"),
                ("path", b"b.yml"),
                ("expected_state", b"absent"),
                ("file", b"x"),
            ],
        ),
    );
    add(
        t,
        "rejected_traversal",
        form(
            CT,
            &[
                ("path", b"../x.yml"),
                ("expected_state", b"absent"),
                ("file", b"x"),
            ],
        ),
    );
    add(
        t,
        "rejected_hash_state_without_a_hash",
        form(
            CT,
            &[("path", b"a.yml"), ("expected_state", b"hash"), ("file", b"x")],
        ),
    );
    add(t, "rejected_not_multipart", b"application/json\n{}".to_vec());
    add(t, "rejected_truncated_body", b"multipart/form-data; boundary=----seed\n------seed\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\nabc".to_vec());
    add(
        t,
        "rejected_no_boundary",
        b"multipart/form-data\n--x\r\n".to_vec(),
    );

    // ---- writes_preview_id
    let t = "writes_preview_id";
    let at = |ms: i64| invariants::fuzz_hooks_issue(ms);
    add(t, "accepted_fresh_token", at(invariants::NOW_MS).into_bytes());
    add(
        t,
        "accepted_token_issued_nine_minutes_ago",
        at(invariants::NOW_MS - 540_000).into_bytes(),
    );
    add(
        t,
        "accepted_token_a_second_before_ten_minutes",
        at(invariants::NOW_MS - 599_000).into_bytes(),
    );
    add(
        t,
        "rejected_token_at_exactly_ten_minutes",
        at(invariants::NOW_MS - 600_000).into_bytes(),
    );
    add(
        t,
        "accepted_token_from_a_clock_a_little_ahead",
        at(invariants::NOW_MS + 20_000).into_bytes(),
    );
    add(
        t,
        "rejected_token_older_than_ten_minutes",
        at(invariants::NOW_MS - 601_000).into_bytes(),
    );
    add(
        t,
        "rejected_token_from_the_future",
        at(invariants::NOW_MS + 120_000).into_bytes(),
    );
    let mut tampered = at(invariants::NOW_MS).into_bytes();
    let last = tampered.len() - 2;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    add(t, "rejected_tampered_mac", tampered);
    let mut padded = at(invariants::NOW_MS);
    padded.push('=');
    add(t, "rejected_padded_token", padded.into_bytes());
    add(t, "rejected_empty", Vec::new());
    add(t, "rejected_not_base64", b"not-a-token!!".to_vec());
    add(t, "rejected_short", b"AQAAAABq".to_vec());
    add(t, "rejected_non_utf8", vec![0xff, 0xfe, 0xfd]);

    // ---- writes_github_token. Made-up tokens: the right shape and no account behind them.
    let t = "writes_github_token";
    let fine = format!("github_pat_11SEED{}", "Ab1_".repeat(15));
    let body = |token: &str| json(json!({ "token": token }));
    add(t, "accepted_fine_grained_token", body(&fine));
    add(
        t,
        "accepted_token_at_512_bytes",
        body(&format!("github_pat_{}", "a".repeat(512 - 11))),
    );
    add(
        t,
        "accepted_classic_token_body_is_read_and_judged_later",
        body("ghp_0123456789abcdefghijklmnopqrstuvwxyz"),
    );
    add(
        t,
        "accepted_token_with_escapes",
        b"{\"token\":\"github_pat_\\u0031\\u0031abcdefghij0123456789ABCDEF\"}".to_vec(),
    );
    add(t, "rejected_empty_body", Vec::new());
    add(t, "rejected_empty_token", body(""));
    add(
        t,
        "rejected_token_over_512_bytes",
        body(&format!("github_pat_{}", "a".repeat(512))),
    );
    add(
        t,
        "rejected_unknown_field",
        json(json!({ "token": fine, "extra": 1 })),
    );
    add(t, "rejected_missing_token", json(json!({})));
    add(t, "rejected_token_is_a_number", json(json!({ "token": 1 })));
    add(t, "rejected_array", b"[\"github_pat_x\"]".to_vec());
    add(
        t,
        "rejected_broken_json",
        format!("{{\"token\": \"{fine}").into_bytes(),
    );
    add(
        t,
        "rejected_raw_fine_grained_token_text",
        fine.clone().into_bytes(),
    );
    add(
        t,
        "rejected_raw_classic_token_text",
        b"ghp_0123456789abcdefghijklmnopqrstuvwxyz".to_vec(),
    );
    add(
        t,
        "rejected_raw_forty_hex_classic_token",
        b"0123456789abcdef0123456789abcdef01234567".to_vec(),
    );
    add(t, "rejected_non_utf8", vec![0xff, 0xfe, b'{', 0x80]);

    // ---- writes_github_responses
    let t = "writes_github_responses";
    add(
        t,
        "accepted_user",
        json(json!({ "login": "octocat", "id": 1, "plan": { "name": "free" } })),
    );
    add(
        t,
        "accepted_user_with_capitals_and_hyphens",
        json(json!({ "login": "Mona-Lisa-2" })),
    );
    add(
        t,
        "accepted_repository_with_push",
        json(json!({ "name": "r", "permissions": { "admin": false, "push": true, "pull": true } })),
    );
    add(
        t,
        "accepted_repository_without_push",
        json(json!({ "permissions": { "push": false } })),
    );
    add(
        t,
        "accepted_repository_without_permissions",
        json(json!({ "name": "r" })),
    );
    add(t, "accepted_expiry_header", b"2026-11-20 12:00:00 UTC".to_vec());
    add(
        t,
        "accepted_expiry_header_with_spaces_around",
        b" 2028-02-29 23:59:59 UTC ".to_vec(),
    );
    add(t, "rejected_empty", Vec::new());
    add(t, "rejected_not_json", b"<html>Bad gateway</html>".to_vec());
    add(t, "rejected_array", b"[]".to_vec());
    // A bad login, and a bad push flag beside it, so that neither reader takes the answer.
    let bad_flag = json!({ "push": 1 });
    add(
        t,
        "rejected_login_is_a_number",
        json(json!({ "login": 1, "permissions": bad_flag })),
    );
    add(
        t,
        "rejected_login_with_a_space",
        json(json!({ "login": "octo cat", "permissions": bad_flag })),
    );
    add(
        t,
        "rejected_login_with_a_leading_hyphen",
        json(json!({ "login": "-octocat", "permissions": bad_flag })),
    );
    add(
        t,
        "rejected_login_too_long",
        json(json!({ "login": "a".repeat(40), "permissions": bad_flag })),
    );
    add(t, "rejected_login_in_an_array", b"[\"octocat\"]".to_vec());
    add(
        t,
        "rejected_push_flag_is_text",
        json(json!({ "permissions": { "push": "true" } })),
    );
    add(
        t,
        "rejected_expiry_in_another_zone",
        b"2026-11-20 12:00:00 PST".to_vec(),
    );
    add(
        t,
        "rejected_expiry_on_a_day_that_is_not",
        b"2027-02-29 00:00:00 UTC".to_vec(),
    );
    add(
        t,
        "rejected_expiry_in_the_year_1999",
        b"1999-12-31 23:59:59 UTC".to_vec(),
    );
    add(t, "rejected_non_utf8", vec![0xff, 0xfe, 0x00]);
    // Found by the fuzzer: a good login, and bytes that are not text in a field nobody reads.
    add(
        t,
        "rejected_login_with_invalid_utf8_in_a_skipped_field",
        b"{\"id\":1,\"login\":\"octocat\",\"plan\":{\"name\":\"fr\x9b\xa2\"}}".to_vec(),
    );

    // ---- writes_git_data_responses
    let t = "writes_git_data_responses";
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let pull =
        |number: serde_json::Value, url: &str| json!({ "number": number, "html_url": url, "state": "open" });
    add(
        t,
        "accepted_branch_head",
        json(json!({ "ref": "refs/heads/sit1", "object": { "sha": a, "type": "commit", "url": "u" } })),
    );
    add(
        t,
        "accepted_commit_with_its_tree",
        json(json!({ "sha": a, "tree": { "sha": b }, "parents": [] })),
    );
    add(
        t,
        "accepted_created_object",
        json(json!({ "sha": b, "url": "u" })),
    );
    add(
        t,
        "accepted_blob_with_line_breaks",
        json(
            json!({ "sha": a, "size": 10, "encoding": "base64", "content": format!("{}\n", b64(b"a: 1\r\nb: 2")) }),
        ),
    );
    add(
        t,
        "accepted_file_with_its_bytes",
        json(
            json!({ "type": "file", "encoding": "base64", "size": 6, "sha": a, "content": b64(b"k: v\r\n") }),
        ),
    );
    add(
        t,
        "accepted_file_without_its_bytes",
        json(json!({ "type": "file", "encoding": "none", "size": 2_000_000, "sha": a, "content": "" })),
    );
    add(
        t,
        "accepted_pull_request",
        json(pull(json!(17), "https://github.com/acme/r/pull/17")),
    );
    add(
        t,
        "accepted_pull_list_with_the_branch",
        json(json!([
            { "number": 3, "html_url": "https://github.com/a/r/pull/3", "head": { "ref": "other" } },
            { "number": 4, "html_url": "https://github.com/a/r/pull/4", "head": { "ref": "lanekeeper/sit1/0a1b2c3d4e" } },
        ])),
    );
    add(t, "rejected_empty", Vec::new());
    add(t, "rejected_not_json", b"<html>Bad gateway</html>".to_vec());
    add(t, "rejected_non_utf8", vec![0xff, 0xfe, b'{', 0x80]);
    add(
        t,
        "rejected_head_is_a_tag",
        json(json!({ "object": { "sha": a, "type": "tag" } })),
    );
    add(
        t,
        "rejected_object_id_in_capitals",
        json(json!({ "sha": a.to_uppercase(), "tree": { "sha": b.to_uppercase() } })),
    );
    add(
        t,
        "rejected_blob_with_a_wrong_size",
        json(json!({ "size": 4, "encoding": "base64", "content": b64(b"abc") })),
    );
    add(
        t,
        "rejected_blob_that_is_not_base64",
        json(json!({ "size": 3, "encoding": "base64", "content": "not base64 !!" })),
    );
    add(
        t,
        "rejected_directory_listing",
        json(json!([{ "type": "file", "name": "x.yml", "sha": a }])),
    );
    add(
        t,
        "rejected_file_with_a_blob_id_that_is_not_one",
        json(json!({ "type": "file", "sha": "../../etc/passwd", "content": "" })),
    );
    add(
        t,
        "rejected_pull_request_with_an_http_address",
        json(pull(json!(1), "http://github.com/acme/r/pull/1")),
    );
    add(
        t,
        "rejected_pull_request_number_zero",
        json(pull(json!(0), "https://github.com/acme/r/pull/0")),
    );
    add(t, "rejected_pull_list_without_the_branch", b"[]".to_vec());
    let _ = ContentHash::from_bytes([0; 32]);
    out
}

/// `UPDATE_WRITES_SEEDS=1 cargo test -p hub-writes --test corpus_replay regenerate_the_seeds` rewrites the seeds.
#[test]
fn regenerate_the_seeds() {
    if std::env::var_os("UPDATE_WRITES_SEEDS").is_none() {
        // Not asked to: only check that the committed seeds are the ones the code makes, so they cannot drift unnoticed.
        for (target, name, bytes) in seeds() {
            let on_disk = fs::read(corpus(target).join(&name))
                .unwrap_or_else(|_| panic!("seed {target}/{name} is missing"));
            assert_eq!(
                on_disk, bytes,
                "seed {target}/{name} is not what the generator makes (UPDATE_WRITES_SEEDS=1 rewrites it)"
            );
        }
        return;
    }
    for (target, name, bytes) in seeds() {
        let dir = corpus(target);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), bytes).unwrap();
    }
}
