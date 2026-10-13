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
use domain::{Expected, HubCommand, NfsPath, RequestId, ServiceRef, SwimlaneId};
use hub_platform::gateway::local::Epoch;
use hub_platform::gateway::wire::{ForwardCall, pb::ForwardRequest};
use prost::Message;
use proto::convert::{FromAgent, FromSentinel, ToAgent};
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
fn corpus_replay_hub_forward_request() {
    replay("hub_forward_request", invariants::hub_forward_request);
}

#[test]
fn corpus_replay_hub_webhook_payload() {
    replay("hub_webhook_payload", invariants::hub_webhook_payload);
}

#[test]
fn corpus_replay_hub_sentinel_message() {
    replay("hub_sentinel_message", invariants::hub_sentinel_message);
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

/// The same for what the sentinel on the NFS VM sends.
#[test]
fn sentinel_message_seeds_keep_their_meaning() {
    let dir = corpus("hub_sentinel_message");
    let (mut accepted, mut batches) = (0, 0);
    for name in names(&dir) {
        let bytes = fs::read(dir.join(&name)).unwrap();
        let outcome = pb::SentinelMessage::decode(bytes.as_slice())
            .map_err(|_| ())
            .and_then(|m| FromSentinel::from_proto(m).map_err(|_| ()));
        if name.starts_with("accepted_") {
            assert!(
                matches!(outcome, Ok(Some(_))),
                "{name} is no longer accepted: {outcome:?}"
            );
            accepted += 1;
            batches += usize::from(matches!(outcome, Ok(Some(FromSentinel::Batch(_)))));
        } else if name.starts_with("rejected_") {
            assert!(outcome.is_err(), "{name} is no longer refused: {outcome:?}");
        } else if name.starts_with("skipped_") {
            assert!(
                matches!(outcome, Ok(None)),
                "{name} is no longer skipped: {outcome:?}"
            );
        }
    }
    assert!(
        accepted >= 4 && batches >= 2,
        "the corpus keeps accepted messages and batches, or the fuzzer starts from nothing"
    );
}

/// The same for the forward requests a peer replica sends.
#[test]
fn forward_request_seeds_keep_their_meaning() {
    let dir = corpus("hub_forward_request");
    let mut accepted = 0;
    for name in names(&dir) {
        let bytes = fs::read(dir.join(&name)).unwrap();
        let outcome = ForwardRequest::decode(bytes.as_slice())
            .map_err(|_| ())
            .and_then(|r| ForwardCall::try_from(r).map_err(|_| ()));
        if name.starts_with("accepted_") {
            assert!(outcome.is_ok(), "{name} is no longer accepted: {outcome:?}");
            accepted += 1;
        } else if name.starts_with("rejected_") {
            assert!(outcome.is_err(), "{name} is no longer refused: {outcome:?}");
        }
    }
    assert!(
        accepted >= 6,
        "the corpus keeps accepted requests, or the fuzzer starts from nothing"
    );
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

/// The webhook payload seeds: `push_*` are accepted as a push, `pr_*` as a pull request, `rejected_*` by neither.
#[test]
fn webhook_payload_seeds_keep_their_meaning() {
    use hub_platform::git::payload::{PullRequestEvent, PushEvent};
    let dir = corpus("hub_webhook_payload");
    let (mut pushes, mut prs) = (0, 0);
    for name in names(&dir) {
        let bytes = fs::read(dir.join(&name)).unwrap();
        let push = PushEvent::parse(&bytes);
        let pr = PullRequestEvent::parse(&bytes);
        if name.starts_with("push_") {
            assert!(push.is_ok(), "{name} is no longer accepted as a push: {push:?}");
            pushes += 1;
        } else if name.starts_with("pr_") {
            assert!(
                pr.is_ok(),
                "{name} is no longer accepted as a pull request: {pr:?}"
            );
            prs += 1;
        } else if name.starts_with("rejected_") {
            assert!(push.is_err() && pr.is_err(), "{name} is no longer refused");
        }
    }
    assert!(
        pushes >= 4 && prs >= 3,
        "the corpus keeps accepted payloads, or the fuzzer starts from nothing"
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

fn sentinel_record(path: &str, operation: i32) -> pb::AuditRecord {
    pb::AuditRecord {
        time_ms: 1_800_000_000_000,
        path: path.into(),
        operation,
        success: true,
        login_user: "alice".into(),
        effective_user: "app".into(),
        exe: "/usr/bin/vim".into(),
        comm: "vim".into(),
    }
}

fn sentinel_batch(seq: u64, records: Vec<pb::AuditRecord>) -> Vec<u8> {
    use pb::sentinel_message::Kind;
    pb::SentinelMessage {
        kind: Some(Kind::Batch(pb::AuditRecordBatch { seq, records })),
    }
    .encode_to_vec()
}

fn sentinel_hello(vm: &str) -> Vec<u8> {
    use pb::sentinel_message::Kind;
    pb::SentinelMessage {
        kind: Some(Kind::Hello(pb::SentinelHello {
            vm: vm.into(),
            export_root: "/export".into(),
            version: "0.1.0".into(),
        })),
    }
    .encode_to_vec()
}

#[allow(clippy::too_many_lines)]
fn sentinel_message_seeds() -> Vec<(&'static str, Vec<u8>)> {
    use pb::AuditOperation as Op;
    use pb::sentinel_message::Kind;
    let valid_hello = sentinel_hello("nfs-vm-1");
    let mut duplicated = valid_hello.clone();
    duplicated.extend_from_slice(&sentinel_hello("nfs-vm-2"));
    let valid_batch = sentinel_batch(
        5,
        vec![
            sentinel_record("svc/app.yml", Op::Write as i32),
            sentinel_record("svc/new.yml", Op::Create as i32),
        ],
    );
    let all_operations = [Op::Create, Op::Write, Op::Delete, Op::Rename, Op::Attribute]
        .into_iter()
        .map(|op| sentinel_record("svc/app.yml", op as i32))
        .collect();
    vec![
        ("accepted_hello", valid_hello.clone()),
        (
            "accepted_heartbeat",
            pb::SentinelMessage {
                kind: Some(Kind::Heartbeat(pb::SentinelHeartbeat {
                    sent_at_ms: 1_800_000_000_000,
                })),
            }
            .encode_to_vec(),
        ),
        ("accepted_batch_two_records", valid_batch.clone()),
        ("accepted_batch_empty", sentinel_batch(1, Vec::new())),
        (
            "accepted_batch_every_operation",
            sentinel_batch(9, all_operations),
        ),
        (
            "accepted_batch_a_failed_operation",
            sentinel_batch(
                3,
                vec![pb::AuditRecord {
                    success: false,
                    ..sentinel_record("svc/denied.yml", Op::Delete as i32)
                }],
            ),
        ),
        (
            "accepted_batch_nested_path",
            sentinel_batch(4, vec![sentinel_record("a/b/c/d/e/f.yml", Op::Rename as i32)]),
        ),
        (
            "rejected_batch_traversal_path",
            sentinel_batch(6, vec![sentinel_record("../../etc/passwd", Op::Write as i32)]),
        ),
        (
            "rejected_batch_absolute_path",
            sentinel_batch(6, vec![sentinel_record("/etc/passwd", Op::Write as i32)]),
        ),
        (
            "rejected_batch_unspecified_operation",
            sentinel_batch(7, vec![sentinel_record("svc/app.yml", 0)]),
        ),
        (
            "rejected_batch_unknown_operation",
            sentinel_batch(7, vec![sentinel_record("svc/app.yml", 99)]),
        ),
        (
            "rejected_hello_control_character_in_vm",
            sentinel_hello("nfs\u{1}vm"),
        ),
        ("skipped_empty_message", Vec::new()),
        // Field 16 is reserved: a newer peer's message kind this build does not know. Tag 16, length-delimited, empty.
        ("skipped_unknown_variant_16", vec![0x82, 0x01, 0x00]),
        ("truncated_batch", valid_batch[..valid_batch.len() / 2].to_vec()),
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

fn forward(swimlane: &str, epoch: i64, command: HubCommand) -> ForwardRequest {
    ForwardCall {
        swimlane: SwimlaneId::parse(swimlane).unwrap(),
        epoch: Epoch::new(epoch),
        timeout: std::time::Duration::from_millis(2500),
        command,
    }
    .into_proto()
}

fn read_file(id: &str, path: &str) -> HubCommand {
    HubCommand::ReadFile {
        request_id: RequestId::parse(id).unwrap(),
        path: NfsPath::parse(path).unwrap(),
    }
}

fn hash(bytes: &[u8]) -> domain::ContentHash {
    ports::content_hash(bytes)
}

#[allow(clippy::too_many_lines)]
fn forward_request_seeds() -> Vec<(&'static str, Vec<u8>)> {
    use proto::pb::hub_message::Kind;
    let enc = |request: ForwardRequest| request.encode_to_vec();
    let with_message = |kind: Kind| ForwardRequest {
        command: Some(proto::pb::HubMessage { kind: Some(kind) }),
        ..forward("sit1", 3, read_file("r-1", "a.yml"))
    };
    let valid = enc(forward("sit1", 3, read_file("req-1", "svc/app.yml")));
    let mut duplicated = valid.clone();
    duplicated.extend_from_slice(&enc(forward("sit2", 4, read_file("req-2", "svc/b.yml"))));
    vec![
        ("accepted_read_file", valid.clone()),
        (
            "accepted_write_file_expected_hash",
            enc(forward(
                "sit1",
                2,
                HubCommand::WriteFile {
                    request_id: RequestId::parse("req-3").unwrap(),
                    path: NfsPath::parse("svc/app.yml").unwrap(),
                    expected: Expected::Hash { hash: hash(b"old") },
                    bytes: Bytes::from_static(b"a: 2\n"),
                },
            )),
        ),
        (
            "accepted_write_file_absent",
            enc(forward(
                "sit1",
                2,
                HubCommand::WriteFile {
                    request_id: RequestId::parse("req-4").unwrap(),
                    path: NfsPath::parse("svc/new.yml").unwrap(),
                    expected: Expected::Absent,
                    bytes: Bytes::from_static(b"b: 1\n"),
                },
            )),
        ),
        (
            "accepted_delete_file",
            enc(forward(
                "sit1",
                5,
                HubCommand::DeleteFile {
                    request_id: RequestId::parse("req-5").unwrap(),
                    path: NfsPath::parse("svc/old.yml").unwrap(),
                    expected: hash(b"gone"),
                },
            )),
        ),
        (
            "accepted_restart_deployment",
            enc(forward(
                "sit1",
                1,
                HubCommand::RestartDeployment {
                    request_id: RequestId::parse("req-6").unwrap(),
                    service: ServiceRef::new("ns", "svc").unwrap(),
                },
            )),
        ),
        (
            "accepted_notify_config_server",
            enc(forward(
                "sit1",
                1,
                HubCommand::NotifyConfigServer {
                    request_id: RequestId::parse("req-7").unwrap(),
                    paths: vec![NfsPath::parse("svc/app.yml").unwrap()],
                },
            )),
        ),
        (
            "accepted_request_full_scan_without_a_request_id",
            enc(forward("sit1", 1, HubCommand::RequestFullScan)),
        ),
        (
            "accepted_timeout_over_the_cap_is_capped",
            enc(ForwardRequest {
                timeout_ms: u32::MAX,
                ..forward("sit1", 1, read_file("req-8", "a.yml"))
            }),
        ),
        (
            "rejected_zero_epoch",
            enc(ForwardRequest {
                epoch: 0,
                ..forward("sit1", 1, read_file("r", "a.yml"))
            }),
        ),
        (
            "rejected_zero_timeout",
            enc(ForwardRequest {
                timeout_ms: 0,
                ..forward("sit1", 1, read_file("r", "a.yml"))
            }),
        ),
        (
            "rejected_swimlane_with_a_slash",
            enc(ForwardRequest {
                swimlane_id: "sit1/../sit2".into(),
                ..forward("sit1", 1, read_file("r", "a.yml"))
            }),
        ),
        (
            "rejected_no_command",
            enc(ForwardRequest {
                command: None,
                ..forward("sit1", 1, read_file("r", "a.yml"))
            }),
        ),
        (
            "rejected_traversal_path",
            enc(with_message(Kind::ReadFile(proto::pb::ReadFile {
                request_id: "r-1".into(),
                path: "../../etc/passwd".into(),
            }))),
        ),
        (
            "rejected_an_acknowledgement_is_not_a_command",
            enc(with_message(Kind::Ack(proto::pb::Ack { seq: 9 }))),
        ),
        (
            "rejected_a_configuration_is_not_a_command",
            enc(ForwardRequest {
                command: Some(
                    ToAgent::Config(domain::AgentConfig {
                        scan_interval_secs: 10,
                        heartbeat_interval_secs: 5,
                        max_file_bytes: 1000,
                        deny_globs: Vec::new(),
                        env_allowlist: Vec::new(),
                        tenants: Vec::new(),
                    })
                    .into_proto(),
                ),
                ..forward("sit1", 1, read_file("r", "a.yml"))
            }),
        ),
        ("truncated_forward_request", valid[..valid.len() / 2].to_vec()),
        ("duplicated_forward_request_fields", duplicated),
        ("empty_forward_request", Vec::new()),
        ("wrong_wire_type_for_the_swimlane", vec![0x08, 0x01]),
        (
            "garbage_bytes",
            vec![
                0xff, 0xfe, 0x01, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02,
            ],
        ),
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

const AFTER: &str = "0123456789abcdef0123456789abcdef01234567";
const BEFORE: &str = "89abcdef0123456789abcdef0123456789abcdef";
const ZERO: &str = "0000000000000000000000000000000000000000";

fn push_json(git_ref: &str, before: &str, after: &str, deleted: bool, repo: &str) -> Vec<u8> {
    serde_json::json!({
        "ref": git_ref,
        "before": before,
        "after": after,
        "deleted": deleted,
        "created": false,
        "forced": false,
        "repository": {"id": 1, "full_name": repo, "private": true, "owner": {"login": "acme"}},
        "pusher": {"name": "someone"},
        "commits": [{"id": after, "message": "text", "added": [], "removed": [], "modified": ["a.yml"]}],
        "sender": {"login": "someone"},
    })
    .to_string()
    .into_bytes()
}

fn pr_json(action: &str, number: i64, merged: bool) -> Vec<u8> {
    serde_json::json!({
        "action": action,
        "number": number,
        "pull_request": {"number": number, "merged": merged, "state": "closed", "title": "text", "user": {"login": "u"}},
        "repository": {"full_name": "acme/csp-tenant-data"},
        "sender": {"login": "u"},
    })
    .to_string()
    .into_bytes()
}

fn webhook_payload_seeds() -> Vec<(&'static str, Vec<u8>)> {
    let deep = format!(
        r#"{{"ref":"refs/heads/x","extra":{}{}}}"#,
        "[".repeat(2000),
        "]".repeat(2000)
    );
    vec![
        ("push_branch", push_json("refs/heads/sit1", BEFORE, AFTER, false, "acme/csp-tenant-data")),
        ("push_branch_with_slash", push_json("refs/heads/templates/common-docs", BEFORE, AFTER, false, "acme/csp-tenant-data")),
        ("push_tag", push_json("refs/tags/v2.3.1", ZERO, AFTER, false, "acme/configuration-base-saas")),
        ("push_new_branch_zero_before", push_json("refs/heads/sit9", ZERO, AFTER, false, "acme/csp-tenant-data")),
        ("push_deleted_by_flag", push_json("refs/heads/old", BEFORE, AFTER, true, "acme/csp-tenant-data")),
        ("push_deleted_by_zero_after", push_json("refs/heads/old", BEFORE, ZERO, false, "acme/csp-tenant-data")),
        ("pr_opened", pr_json("opened", 17, false)),
        ("pr_closed_merged", pr_json("closed", 17, true)),
        ("pr_closed_not_merged", pr_json("closed", 18, false)),
        ("pr_reopened", pr_json("reopened", 18, false)),
        ("pr_edited_other_action", pr_json("edited", 19, false)),
        ("rejected_ref_outside_heads_and_tags", push_json("refs/pull/1/head", BEFORE, AFTER, false, "a/b")),
        ("rejected_ref_with_dotdot", push_json("refs/heads/a..b", BEFORE, AFTER, false, "a/b")),
        ("rejected_short_commit_id", push_json("refs/heads/x", BEFORE, "abc123", false, "a/b")),
        ("rejected_repository_with_two_slashes", push_json("refs/heads/x", BEFORE, AFTER, false, "a/b/c")),
        ("rejected_pr_number_zero", pr_json("opened", 0, false)),
        ("rejected_pr_number_over_the_column", pr_json("opened", i64::from(i32::MAX) + 1, false)),
        ("rejected_wrong_type_for_ref", br#"{"ref":1,"before":"a","after":"b","repository":{"full_name":"a/b"}}"#.to_vec()),
        (
            "rejected_duplicated_keys",
            br#"{"ref":"refs/heads/x","ref":"refs/heads/y","before":"89abcdef0123456789abcdef0123456789abcdef","after":"0123456789abcdef0123456789abcdef01234567","repository":{"full_name":"a/b"}}"#.to_vec(),
        ),
        ("rejected_deeply_nested", deep.into_bytes()),
        ("rejected_truncated_push", push_json("refs/heads/sit1", BEFORE, AFTER, false, "a/b")[..60].to_vec()),
        ("rejected_garbage_bytes", vec![0xff, 0xfe, 0x00, 0x7b, 0x22]),
        ("rejected_empty", Vec::new()),
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
        ("hub_forward_request", forward_request_seeds()),
        ("hub_webhook_payload", webhook_payload_seeds()),
        ("hub_sentinel_message", sentinel_message_seeds()),
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

#[test]
fn the_forward_request_seeds_are_what_the_generator_makes() {
    let dir = corpus("hub_forward_request");
    let wanted = forward_request_seeds();
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

#[test]
fn the_webhook_payload_seeds_are_what_the_generator_makes() {
    let dir = corpus("hub_webhook_payload");
    let wanted = webhook_payload_seeds();
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

#[test]
fn the_sentinel_message_seeds_are_what_the_generator_makes() {
    let dir = corpus("hub_sentinel_message");
    let wanted = sentinel_message_seeds();
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
