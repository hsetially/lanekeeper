//! The committed fuzz seeds, replayed on stable (plan 18, T1; rule 13; S7, S10).
//!
//! `crates/hub/fuzz/` holds the fuzz targets (nightly, `just hub-fuzz`) and `invariants.rs`, which this test includes: every
//! seed in `fuzz/corpus/<target>/` is replayed with the checks the fuzzer runs, so `cargo test -p hub` (and so `just verify`)
//! catches a regression in a parser at a trust boundary without a nightly toolchain. A seed's name says what it must still do:
//! `accepted_*` is accepted, `rejected_*` refused; a test fails if a seed stops meaning that.
//!
//! The seeds hold no secret: the key bodies are garbage between real PEM framing, and the certificate is a public throwaway.
//! `UPDATE_HUB_SEEDS=1 cargo test -p hub --test corpus_replay regenerate_the_seeds` rewrites them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::too_many_lines
)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use invariants::{Check, Verdict};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn corpus(target: &str) -> PathBuf {
    fuzz_dir().join("corpus").join(target)
}

fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = corpus(target);
    let mut out: Vec<(String, Vec<u8>)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

fn replay(target: &str, check: Check) {
    let all = seeds(target);
    assert!(all.len() >= 5, "{target} has only {} seeds", all.len());
    for (name, data) in &all {
        let verdict = check(data).unwrap_or_else(|why| panic!("{target}/{name}: {why}"));
        let want = if name.starts_with("accepted_") {
            Verdict::Accepted
        } else if name.starts_with("rejected_") {
            Verdict::Rejected
        } else {
            panic!("{target}/{name}: a seed's name must start with accepted_ or rejected_");
        };
        assert_eq!(
            verdict, want,
            "{target}/{name} no longer means what its name says"
        );
    }
}

#[test]
fn every_settings_seed_keeps_its_verdict_and_its_invariants() {
    replay("hub_settings", invariants::hub_settings);
}

#[test]
fn every_command_line_seed_keeps_its_verdict_and_its_invariants() {
    replay("hub_cli_args", invariants::hub_cli_args);
}

#[test]
fn every_pem_seed_keeps_its_verdict_and_its_invariants() {
    replay("hub_pem", invariants::hub_pem);
}

#[test]
fn the_corpus_has_a_directory_for_every_target_and_nothing_else() {
    let targets: BTreeSet<String> = fs::read_dir(fuzz_dir().join("fuzz_targets"))
        .unwrap()
        .filter_map(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .strip_suffix(".rs")
                .map(str::to_owned)
        })
        .collect();
    let dirs: BTreeSet<String> = fs::read_dir(fuzz_dir().join("corpus"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(targets, dirs);
    assert_eq!(
        targets,
        ["hub_cli_args", "hub_pem", "hub_settings"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    );
}

/// The accepted path of the PEM parser has no committed seed (that would be a private key in the repository), so a pair made
/// here stands in: a chain and its key are accepted, and the same chain with another key is refused.
#[test]
fn a_fresh_valid_pair_is_accepted_and_a_mismatched_one_is_not() {
    let make = || {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["hub.example".to_owned()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        (cert.pem(), key.serialize_pem())
    };
    let ((chain, key), (_, other_key)) = (make(), make());
    let join = |chain: &str, key: &str| [chain.as_bytes(), &[0], key.as_bytes()].concat();
    assert_eq!(invariants::hub_pem(&join(&chain, &key)), Ok(Verdict::Accepted));
    assert_eq!(
        invariants::hub_pem(&join(&chain, &other_key)),
        Ok(Verdict::Rejected)
    );
}

#[test]
fn the_invariants_notice_a_settings_error_that_repeats_a_value() {
    // The check that guards the error text is itself checked against messages that break it.
    for fine in [
        "LK_DB_MAX_CONNECTIONS is not valid",
        "LK_WEB_CERT_REF is required",
        "LK_A1 is required",
    ] {
        assert!(invariants::is_fixed_settings_error(fine), "{fine}");
    }
    for bad in [
        "LK_DB_MAX_CONNECTIONS is not valid: lots",
        "LK_DB_MAX_CONNECTIONS is not valid (lots)",
        "lots is not valid",
        "LK_x is required",
        "LK_WEB_ADDR=127.0.0.1:1 is not valid",
        "",
    ] {
        assert!(!invariants::is_fixed_settings_error(bad), "{bad}");
    }
}

fn write_seeds(target: &str, seeds: &[(&str, Vec<u8>)]) {
    let dir = corpus(target);
    fs::create_dir_all(&dir).unwrap();
    for entry in fs::read_dir(&dir).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for (name, data) in seeds {
        fs::write(dir.join(name), data).unwrap();
    }
}

/// Rewrites the committed seeds. Not a check: it runs only when asked.
#[test]
fn regenerate_the_seeds() {
    if std::env::var_os("UPDATE_HUB_SEEDS").is_none() {
        return;
    }
    let text = |s: &str| s.as_bytes().to_vec();
    write_seeds(
        "hub_settings",
        &[
            ("accepted_empty", text("")),
            (
                "accepted_serving",
                text(
                    "LK_DATABASE_URL=postgres://u:p@db/x\nLK_DB_CA_FILE=/ca.pem\nLK_WEB_CERT_REF=web-cert\nLK_WEB_KEY_REF=web-key\nLK_GCP_PROJECT=lanekeeper-prod\n",
                ),
            ),
            (
                "accepted_database_url_from_a_file",
                text(
                    "LK_DATABASE_URL_FILE=/run/db/uri\nLK_DB_CA_FILE=/run/ca/ca.pem\nFILE_CONTENT=postgres://u:hunter2@db.internal/x\n",
                ),
            ),
            (
                "accepted_git_remotes",
                text(
                    "LK_GIT_BASE_URL=https://github.com/acme/configuration-base-saas.git\nLK_GIT_TENANT_URL=https://github.com/acme/csp-tenant-data.git\nLK_GIT_BASE_REPO=acme/configuration-base-saas\nLK_GIT_TENANT_REPO=acme/csp-tenant-data\nLK_GIT_READ_REF=my-read-token\nLK_GIT_WEBHOOK_REF=my-hook\n",
                ),
            ),
            (
                "accepted_every_number_at_its_edge",
                text(
                    "LK_START_TIMEOUT_SECS=120\nLK_WEB_CERT_RELOAD_SECS=86400\nLK_SHUTDOWN_GRACE_SECS=0\nLK_BODY_MAX_BYTES=67108864\nLK_TRUSTED_PROXY_HOPS=8\n",
                ),
            ),
            ("rejected_half_a_certificate", text("LK_WEB_CERT_REF=web-cert\n")),
            (
                "rejected_project_with_a_space",
                text("LK_GCP_PROJECT=Has Space\n"),
            ),
            ("rejected_timeout_zero", text("LK_START_TIMEOUT_SECS=0\n")),
            (
                "rejected_control_character",
                text("LK_WEB_ADDR=127.0.0.1:1\u{7}\n"),
            ),
            (
                "rejected_a_bad_number_beside_a_password",
                text(
                    "LK_DATABASE_URL=postgres://u:hunter2@db/x\nLK_DB_CA_FILE=/ca\nLK_DB_MAX_CONNECTIONS=lots\n",
                ),
            ),
            (
                "rejected_both_database_forms",
                text(
                    "LK_DATABASE_URL_FILE=/u\nLK_DATABASE_URL=postgres://x/y\nLK_DB_CA_FILE=/ca\nFILE_CONTENT=postgres://a/b\n",
                ),
            ),
            (
                "rejected_a_secret_name_with_a_slash",
                text("LK_WEB_CERT_REF=a/b\nLK_WEB_KEY_REF=k\n"),
            ),
            (
                "rejected_csp_injection",
                text("LK_TRUSTED_TYPES=a; script-src *\n"),
            ),
        ],
    );
    write_seeds(
        "hub_cli_args",
        &[
            ("accepted_no_arguments", text("")),
            ("accepted_serve", text("serve")),
            ("accepted_help", text("help")),
            ("accepted_long_help", text("--help")),
            ("accepted_short_help", text("-h")),
            ("rejected_unknown_command", text("migrate")),
            ("rejected_an_empty_word", vec![0]),
            ("rejected_extra_argument", text("serve\0now")),
            ("rejected_a_pasted_secret", text("hunter2-planted-secret")),
            ("rejected_help_then_serve", text("help\0serve")),
        ],
    );
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["hub.example".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let chain = cert.pem();
    let framing =
        |label: &str, body: &str| format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n");
    let with_key = |chain: &str, key: &str| [chain.as_bytes(), &[0], key.as_bytes()].concat();
    write_seeds(
        "hub_pem",
        &[
            ("rejected_empty", Vec::new()),
            ("rejected_not_pem", text("not pem at all\0not pem at all")),
            ("rejected_certificate_without_a_key", with_key(&chain, "")),
            (
                "rejected_key_with_a_garbage_body",
                with_key(&chain, &framing("PRIVATE KEY", "AAAAAAAAAAAA")),
            ),
            (
                "rejected_key_with_the_wrong_label",
                with_key(&chain, &framing("CERTIFICATE", "AAAA")),
            ),
            (
                "rejected_chain_with_a_garbage_certificate",
                with_key(&framing("CERTIFICATE", "AAAA"), &framing("PRIVATE KEY", "AAAA")),
            ),
            (
                "rejected_nine_certificates",
                with_key(&chain.repeat(9), &framing("PRIVATE KEY", "AAAA")),
            ),
            (
                "rejected_a_key_block_in_the_chain",
                with_key(&framing("PRIVATE KEY", "AAAA"), &framing("PRIVATE KEY", "AAAA")),
            ),
            (
                "rejected_bad_base64_in_the_chain",
                with_key(&framing("CERTIFICATE", "!!!not base64!!!"), ""),
            ),
        ],
    );
    println!("seeds rewritten");
}
