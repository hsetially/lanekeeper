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
        "accepted_token_at_the_edge_of_ten_minutes",
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
