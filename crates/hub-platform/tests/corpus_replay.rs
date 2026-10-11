//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never
//! drift apart. The fuzz crate is the hub platform's own (`crates/hub-platform/fuzz/`, plan A17).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stderr)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use prost::Message;
use proto::convert::FromAgent;
use proto::pb;

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

fn replay(target: &str, check: invariants::Check) {
    let dir = corpus(target);
    let names = names(&dir);
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
fn corpus_replay_hub_agent_message() {
    replay("hub_agent_message", invariants::hub_agent_message);
}

#[test]
fn corpus_replay_hub_join_csr() {
    replay("hub_join_csr", invariants::hub_join_csr);
}

#[test]
fn every_target_has_a_corpus_a_fuzz_target_file_and_a_manifest_entry() {
    let manifest = fs::read_to_string(fuzz_dir().join("Cargo.toml")).unwrap();
    for (target, _) in invariants::TARGETS {
        assert!(corpus(target).is_dir(), "{target} has no corpus directory");
        assert!(
            fuzz_dir()
                .join("fuzz_targets")
                .join(format!("{target}.rs"))
                .is_file(),
            "{target} has no fuzz target file"
        );
        assert!(
            manifest.contains(&format!("name = \"{target}\"")),
            "{target} is not a [[bin]] of the fuzz crate"
        );
    }
}

// ------------------------------------------------------------------------------------------ what the seeds mean

/// A seed's name says what it is for, so that a seed cannot silently stop testing what it was made to test: `accepted_*` must
/// still be accepted, `rejected_*` must still be refused.
#[test]
fn agent_message_seeds_keep_their_meaning() {
    let dir = corpus("hub_agent_message");
    for name in names(&dir) {
        let bytes = fs::read(dir.join(&name)).unwrap();
        let outcome = pb::AgentMessage::decode(bytes.as_slice())
            .map_err(|_| ())
            .and_then(|m| FromAgent::from_proto(m).map_err(|_| ()));
        if name.starts_with("accepted_") {
            assert!(
                matches!(outcome, Ok(Some(_))),
                "{name} is no longer accepted: {outcome:?}"
            );
        } else if name.starts_with("rejected_") {
            assert!(outcome.is_err(), "{name} is no longer refused: {outcome:?}");
        } else if name.starts_with("skipped_") {
            assert!(
                matches!(outcome, Ok(None)),
                "{name} is no longer skipped: {outcome:?}"
            );
        }
    }
}

#[test]
fn csr_seeds_keep_their_meaning() {
    let dir = corpus("hub_join_csr");
    let mut valid = 0;
    for name in names(&dir) {
        let bytes = fs::read(dir.join(&name)).unwrap();
        let outcome = hub_platform::ca::csr::parse_csr(&bytes);
        if name.starts_with("valid_") {
            assert!(outcome.is_ok(), "{name} is no longer accepted: {outcome:?}");
            valid += 1;
        } else if name.starts_with("bad_") {
            assert!(outcome.is_err(), "{name} is no longer refused");
        }
    }
    assert!(
        valid >= 3,
        "the corpus keeps valid requests, or the fuzzer starts from nothing"
    );
}

// ------------------------------------------------------------------------------------------- making the seeds

fn encode(kind: pb::agent_message::Kind) -> Vec<u8> {
    pb::AgentMessage { kind: Some(kind) }.encode_to_vec()
}

fn hello(swimlane: &str) -> pb::Hello {
    pb::Hello {
        agent_version: "0.1.0".into(),
        swimlane: swimlane.into(),
        cluster: "cluster-a".into(),
        project: "proj".into(),
        nfs_server: "nfs.internal".into(),
        export: "/export".into(),
        mount_root: "/mnt/config".into(),
    }
}

fn entry(path: &str, hash_len: usize) -> pb::ScanEntry {
    pb::ScanEntry {
        path: path.into(),
        hash: Bytes::from(vec![2u8; hash_len]),
        size: 3,
        mtime_ms: 1,
        observed_at_ms: 2,
        denied: false,
        content: Some(Bytes::from_static(b"abc")),
    }
}

fn delta(entries: Vec<pb::ScanEntry>) -> pb::ScanDelta {
    pb::ScanDelta {
        seq: 7,
        new_root: Bytes::from(vec![1u8; 32]),
        entries,
        ..pb::ScanDelta::default()
    }
}

#[allow(clippy::too_many_lines)]
fn agent_message_seeds() -> Vec<(&'static str, Vec<u8>)> {
    use pb::agent_message::Kind;
    let valid_hello = encode(Kind::Hello(hello("sit1")));
    let mut duplicated = valid_hello.clone();
    duplicated.extend_from_slice(&encode(Kind::Hello(hello("sit2"))));
    vec![
        ("accepted_hello", valid_hello.clone()),
        (
            "accepted_heartbeat",
            encode(Kind::Heartbeat(pb::Heartbeat {
                scan_seq: 3,
                merkle_root: Bytes::from(vec![7u8; 32]),
                file_count: 12,
            })),
        ),
        (
            "accepted_scan_delta_small",
            encode(Kind::ScanDelta(delta(vec![entry("svc/app.yml", 32)]))),
        ),
        (
            "accepted_scan_delta_more_and_gap",
            encode(Kind::ScanDelta(pb::ScanDelta {
                more: true,
                part: 1,
                removed: vec!["old/gone.yml".into()],
                gap: Some(pb::SpoolGap {
                    from_ms: 10,
                    to_ms: 20,
                    lost_entries: 4,
                }),
                ..delta(vec![entry("svc/b.yml", 32)])
            })),
        ),
        (
            "accepted_cluster_report",
            encode(Kind::ClusterReport(pb::ClusterReport {
                full: true,
                deployments: vec![pb::DeploymentInfo {
                    service: Some(pb::ServiceRef {
                        namespace: "ns".into(),
                        name: "svc".into(),
                    }),
                    env_names: vec!["A".into(), "B".into()],
                    ..pb::DeploymentInfo::default()
                }],
                ..pb::ClusterReport::default()
            })),
        ),
        (
            "accepted_cluster_report_answering_a_request",
            encode(Kind::ClusterReport(pb::ClusterReport {
                request_id: "req-1".into(),
                ..pb::ClusterReport::default()
            })),
        ),
        (
            "accepted_op_result",
            encode(Kind::OpResult(pb::OpResult {
                request_id: "req-2".into(),
                ok: true,
                ..pb::OpResult::default()
            })),
        ),
        (
            "accepted_file_content",
            encode(Kind::FileContent(pb::FileContent {
                request_id: "req-3".into(),
                path: "svc/app.yml".into(),
                hash: Bytes::from(vec![9u8; 32]),
                content: Bytes::from_static(b"a: 1\n"),
            })),
        ),
        (
            "accepted_cert_renewal_request",
            encode(Kind::CertRenewalRequest(pb::CertRenewalRequest {
                csr_der: Bytes::from(vec![0x30; 40]),
            })),
        ),
        (
            "rejected_hello_bad_swimlane",
            encode(Kind::Hello(hello("SIT 1/../x"))),
        ),
        (
            "rejected_delta_traversal_path",
            encode(Kind::ScanDelta(delta(vec![entry("../../etc/passwd", 32)]))),
        ),
        (
            "rejected_delta_short_hash",
            encode(Kind::ScanDelta(delta(vec![entry("svc/app.yml", 31)]))),
        ),
        (
            "rejected_served_response_status_zero",
            encode(Kind::ServedResponse(pb::ServedResponse {
                request_id: "req-4".into(),
                status: 0,
                body: Bytes::new(),
            })),
        ),
        (
            "rejected_cert_renewal_empty_csr",
            encode(Kind::CertRenewalRequest(pb::CertRenewalRequest {
                csr_der: Bytes::new(),
            })),
        ),
        (
            "rejected_op_result_unknown_error_code",
            encode(Kind::OpResult(pb::OpResult {
                request_id: "req-5".into(),
                ok: false,
                error_code: 99,
                current_hash: None,
            })),
        ),
        ("skipped_empty_message", Vec::new()),
        // Field 16 is reserved: a newer peer's message kind this build does not know. Tag 16, length-delimited, empty.
        ("skipped_unknown_variant_16", vec![0x82, 0x01, 0x00]),
        ("truncated_hello", valid_hello[..valid_hello.len() / 2].to_vec()),
        ("duplicated_hello_fields", duplicated),
        (
            "garbage_bytes",
            vec![
                0xff, 0xfe, 0x01, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02,
            ],
        ),
        ("wrong_wire_type_for_hello", vec![0x08, 0x01]),
    ]
}

fn csr_seeds() -> Vec<(&'static str, Vec<u8>)> {
    use rcgen::{
        CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, PKCS_ECDSA_P384_SHA384, PKCS_ED25519,
        SanType,
    };
    let der =
        |params: &CertificateParams, key: &KeyPair| params.serialize_request(key).unwrap().der().to_vec();
    let plain = der(&CertificateParams::default(), &KeyPair::generate().unwrap());
    let mut named = CertificateParams::default();
    named.distinguished_name.push(DnType::CommonName, "agent");
    let with_cn = der(&named, &KeyPair::generate().unwrap());
    let mut asks = CertificateParams::default();
    asks.subject_alt_names = vec![SanType::URI(
        "spiffe://lanekeeper/swimlane/other".try_into().unwrap(),
    )];
    asks.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let with_san = der(&asks, &KeyPair::generate().unwrap());
    let mut flipped = plain.clone();
    let at = flipped.len() - 8;
    flipped[at] ^= 1;
    let mut trailing = plain.clone();
    trailing.extend_from_slice(&[0, 0]);
    vec![
        ("valid_plain", plain.clone()),
        ("valid_with_common_name", with_cn),
        ("valid_asking_for_a_san_and_server_auth", with_san),
        ("bad_signature_bit_flipped", flipped),
        (
            "bad_p384_key",
            der(
                &CertificateParams::default(),
                &KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap(),
            ),
        ),
        (
            "bad_ed25519_key",
            der(
                &CertificateParams::default(),
                &KeyPair::generate_for(&PKCS_ED25519).unwrap(),
            ),
        ),
        ("bad_trailing_bytes", trailing),
        ("bad_truncated", plain[..plain.len() - 20].to_vec()),
        ("bad_one_byte_sequence_tag", vec![0x30]),
        ("bad_garbage", vec![0x30, 0x82, 0xff, 0xff, 0x01, 0x02]),
    ]
}

/// Write the seeds: `UPDATE_HUB_SEEDS=1 cargo test -p hub-platform --test corpus_replay regenerate_the_seeds`. The CSR seeds hold
/// freshly made throwaway keys' public halves, so regenerating changes their bytes; the message seeds are deterministic. Does
/// nothing without the variable.
#[test]
fn regenerate_the_seeds() {
    if std::env::var("UPDATE_HUB_SEEDS").is_err() {
        return;
    }
    for (target, seeds) in [
        ("hub_agent_message", agent_message_seeds()),
        ("hub_join_csr", csr_seeds()),
    ] {
        let dir = corpus(target);
        fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in seeds {
            fs::write(dir.join(name), bytes).unwrap();
        }
        eprintln!("wrote the {target} seeds");
    }
}

/// The message seeds are deterministic, so the files must be exactly what the generator makes. (The CSR seeds are not: they
/// carry random keys.)
#[test]
fn the_message_seeds_are_what_the_generator_makes() {
    let dir = corpus("hub_agent_message");
    let wanted = agent_message_seeds();
    assert_eq!(
        names(&dir),
        wanted
            .iter()
            .map(|(n, _)| (*n).to_owned())
            .collect::<BTreeSet<_>>(),
        "run with UPDATE_HUB_SEEDS=1 to rewrite the seeds"
    );
    for (name, bytes) in wanted {
        assert_eq!(
            fs::read(dir.join(name)).unwrap(),
            bytes,
            "{name} drifted; run with UPDATE_HUB_SEEDS=1"
        );
    }
}
