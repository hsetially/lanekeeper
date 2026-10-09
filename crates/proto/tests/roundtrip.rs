//! Round trips, limits and transport behaviour of the agent contract (task T4; S5, S11, S21).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use domain::{
    AgentConfig, AgentReply, AppName, AuditOperation, AuditRecord, AuditRecordBatch, ChannelName,
    ClusterReport, ContentHash, DeploymentInfo, EnvValue, Expected, Heartbeat, Hello, HubCommand, JobRef,
    NfsPath, OpError, OpResult, PodInfo, ReleaseHint, RequestId, ScanDelta, ScanEntry, SentinelConfig,
    SentinelHello, ServeRequest, ServiceRef, ShortText, SkippedEntry, SwimlaneId, SyncWindowEvent,
    SyncWindowKind, TenantId, Timestamp,
};
use prost::Message;
use proto::convert::{
    ConvertError, FromAgent, FromSentinel, IssuedCert, JoinCredential, JoinParams, JoinSubject, ToAgent,
    ToSentinel,
};
use proto::{limits, pb};

// ------------------------------------------------------------------ builders

fn hash(n: u8) -> ContentHash {
    ContentHash::from_bytes([n; 32])
}

fn path(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn text(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

fn rid(s: &str) -> RequestId {
    RequestId::parse(s).unwrap()
}

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(ms)
}

fn service() -> ServiceRef {
    ServiceRef::new("csp", "tx-infinity-api").unwrap()
}

fn job() -> JobRef {
    JobRef::new("argocd-sync-1", "7f3c-11ee").unwrap()
}

fn entry(name: &str, n: u8, content: Option<&'static [u8]>) -> ScanEntry {
    ScanEntry {
        path: path(name),
        hash: hash(n),
        size: content.map_or(10, |c| c.len() as u64),
        mtime: ts(1_700_000_000_000),
        observed_at: ts(1_700_000_001_000),
        denied: false,
        bytes: content.map(Bytes::from_static),
    }
}

fn cluster_report() -> ClusterReport {
    ClusterReport {
        full: true,
        deployments: vec![
            DeploymentInfo {
                service: service(),
                pods: vec![PodInfo {
                    name: text("tx-infinity-api-abc"),
                    started_at: ts(1_700_000_000_000),
                }],
                env_values: vec![EnvValue {
                    name: text("CONFIG_CLIENT_CACHE_TTL"),
                    value: text("20m"),
                }],
                env_names: vec![text("DB_PASSWORD"), text("JAVA_OPTS")],
                removed: false,
            },
            DeploymentInfo {
                service: ServiceRef::new("csp", "old-service").unwrap(),
                pods: vec![],
                env_values: vec![],
                env_names: vec![],
                removed: true,
            },
        ],
        release_hints: vec![ReleaseHint {
            service: service(),
            key: text("image"),
            value: text("registry/tx:1.2.3"),
        }],
        config_server_started_at: Some(ts(1_699_999_000_000)),
        sync_windows: vec![
            SyncWindowEvent {
                kind: SyncWindowKind::Opened,
                job: job(),
                at: ts(1_700_000_000_000),
            },
            SyncWindowEvent {
                kind: SyncWindowKind::Closed,
                job: job(),
                at: ts(1_700_000_060_000),
            },
        ],
    }
}

fn from_agent_samples() -> Vec<FromAgent> {
    let mut denied = entry("secrets/key.pem", 9, None);
    denied.denied = true;
    vec![
        FromAgent::Hello(Hello {
            agent_version: text("0.1.0"),
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            cluster: text("gke-eu"),
            project: text("proj-1"),
            nfs_server: text("10.0.0.5"),
            export: text("/exports/config"),
            mount_root: text("/mnt/config"),
        }),
        FromAgent::Heartbeat(Heartbeat {
            scan_seq: 42,
            merkle_root: hash(1),
            file_count: 2000,
        }),
        FromAgent::Delta(ScanDelta {
            seq: 7,
            base_root: Some(hash(1)),
            new_root: hash(2),
            entries: vec![
                entry("app/a.yml", 3, Some(b"a: 1\r\n")),
                denied,
                entry("big.bin", 4, None),
            ],
            removed: vec![path("app/gone.yml")],
            skipped: vec![SkippedEntry {
                path: path("app/huge.bin"),
                reason: text("too large"),
            }],
            during_job: Some(job()),
            more: true,
            part: 2,
        }),
        FromAgent::Delta(ScanDelta {
            seq: 8,
            base_root: None,
            new_root: hash(5),
            entries: vec![],
            removed: vec![],
            skipped: vec![],
            during_job: None,
            more: false,
            part: 0,
        }),
        FromAgent::Cluster(cluster_report()),
        FromAgent::Reply(AgentReply::Cluster {
            request_id: rid("req-9"),
            report: cluster_report(),
        }),
        FromAgent::Reply(AgentReply::Op(OpResult {
            request_id: rid("req-1"),
            ok: true,
            error: None,
            current_hash: Some(hash(6)),
        })),
        FromAgent::Reply(AgentReply::Op(OpResult {
            request_id: rid("req-2"),
            ok: false,
            error: Some(OpError::Conflict),
            current_hash: Some(hash(7)),
        })),
        FromAgent::Reply(AgentReply::Op(OpResult {
            request_id: rid("req-3"),
            ok: false,
            error: Some(OpError::NotFound),
            current_hash: None,
        })),
        FromAgent::Reply(AgentReply::File {
            request_id: rid("req-4"),
            path: path("app/a.yml"),
            hash: hash(8),
            bytes: Bytes::from_static(b"file bytes"),
        }),
        FromAgent::Reply(AgentReply::Served {
            request_id: rid("req-5"),
            status: 200,
            bytes: Bytes::from_static(b"served"),
        }),
        FromAgent::CertRenewal {
            csr_der: Bytes::from_static(&[0x30, 0x82, 0x01]),
        },
    ]
}

fn to_agent_samples() -> Vec<ToAgent> {
    vec![
        ToAgent::Config(AgentConfig {
            scan_interval_secs: 300,
            heartbeat_interval_secs: 30,
            max_file_bytes: 2 * 1024 * 1024,
            deny_globs: vec![text("**/*.pem"), text("**/secrets/**")],
            env_allowlist: vec![text("CONFIG_CLIENT_CACHE_TTL")],
            tenants: vec![TenantId::parse("sit1").unwrap(), TenantId::parse("sit2").unwrap()],
        }),
        ToAgent::Command(HubCommand::RequestDelta { since_root: hash(1) }),
        ToAgent::Command(HubCommand::RequestFullScan),
        ToAgent::Command(HubCommand::ReadFile {
            request_id: rid("r1"),
            path: path("app/a.yml"),
        }),
        ToAgent::Command(HubCommand::WriteFile {
            request_id: rid("r2"),
            path: path("app/a.yml"),
            expected: Expected::Hash { hash: hash(3) },
            bytes: Bytes::from_static(b"new: 1\r\n"),
        }),
        ToAgent::Command(HubCommand::WriteFile {
            request_id: rid("r3"),
            path: path("app/new.yml"),
            expected: Expected::Absent,
            bytes: Bytes::from_static(b"x"),
        }),
        ToAgent::Command(HubCommand::DeleteFile {
            request_id: rid("r4"),
            path: path("app/old.yml"),
            expected: hash(4),
        }),
        ToAgent::Command(HubCommand::RestartDeployment {
            request_id: rid("r5"),
            service: service(),
        }),
        ToAgent::Command(HubCommand::RequestClusterReport {
            request_id: rid("r6"),
        }),
        ToAgent::Command(HubCommand::NotifyConfigServer {
            request_id: rid("r7"),
            paths: vec![path("app/a.yml"), path("app/b.yml")],
        }),
        ToAgent::Command(HubCommand::FetchServed {
            request_id: rid("r8"),
            request: ServeRequest {
                application: AppName::parse("tx-infinity-api").unwrap(),
                tenant: TenantId::parse("sit1").unwrap(),
                channel: Some(ChannelName::parse("web").unwrap()),
                file: path("tx-infinity-api/receipt.xsl"),
            },
        }),
        ToAgent::Command(HubCommand::FetchServed {
            request_id: rid("r9"),
            request: ServeRequest {
                application: AppName::parse("application").unwrap(),
                tenant: TenantId::parse("sit2").unwrap(),
                channel: None,
                file: path("application.yml"),
            },
        }),
        ToAgent::Ack(77),
        ToAgent::CertRenewal(IssuedCert {
            cert_chain_der: vec![Bytes::from_static(b"leaf"), Bytes::from_static(b"intermediate")],
            not_after: ts(1_800_000_000_000),
        }),
    ]
}

fn from_sentinel_samples() -> Vec<FromSentinel> {
    vec![
        FromSentinel::Hello(SentinelHello {
            vm: text("nfs-vm-1"),
            export_root: text("/exports/config"),
            version: text("0.1.0"),
        }),
        FromSentinel::Batch(AuditRecordBatch {
            seq: 11,
            records: vec![
                AuditRecord {
                    time: ts(1_700_000_000_000),
                    path: path("app/a.yml"),
                    operation: AuditOperation::Write,
                    success: true,
                    login_user: text("alice"),
                    effective_user: text("root"),
                    exe: text("/usr/bin/vim"),
                    comm: text("vim"),
                },
                AuditRecord {
                    time: ts(1_700_000_001_000),
                    path: path("app/b.yml"),
                    operation: AuditOperation::Rename,
                    success: false,
                    login_user: text("bob"),
                    effective_user: text("bob"),
                    exe: text("/bin/mv"),
                    comm: text("mv"),
                },
            ],
        }),
        FromSentinel::Heartbeat {
            sent_at: ts(1_700_000_002_000),
        },
    ]
}

fn to_sentinel_samples() -> Vec<ToSentinel> {
    vec![
        ToSentinel::Settings(SentinelConfig {
            max_batch_records: 500,
            flush_interval_secs: 5,
        }),
        ToSentinel::Ack(11),
    ]
}

// ------------------------------------------------------------------ AC3a: round trips

#[test]
fn every_message_roundtrips() {
    for sample in from_agent_samples() {
        let wire = sample.clone().into_proto().encode_to_vec();
        let decoded = pb::AgentMessage::decode(wire.as_slice()).unwrap();
        assert_eq!(FromAgent::from_proto(decoded).unwrap(), Some(sample));
    }
    for sample in to_agent_samples() {
        let wire = sample.clone().into_proto().encode_to_vec();
        let decoded = pb::HubMessage::decode(wire.as_slice()).unwrap();
        assert_eq!(ToAgent::from_proto(decoded).unwrap(), Some(sample));
    }
    for sample in from_sentinel_samples() {
        let wire = sample.clone().into_proto().encode_to_vec();
        let decoded = pb::SentinelMessage::decode(wire.as_slice()).unwrap();
        assert_eq!(FromSentinel::from_proto(decoded).unwrap(), Some(sample));
    }
    for sample in to_sentinel_samples() {
        let wire = sample.clone().into_proto().encode_to_vec();
        let decoded = pb::SentinelAck::decode(wire.as_slice()).unwrap();
        assert_eq!(ToSentinel::from_proto(decoded).unwrap(), Some(sample));
    }

    // Join, for an agent (Google ID token) and for a sentinel (join token), and the issued certificate.
    for params in [
        JoinParams {
            subject: JoinSubject::Agent(SwimlaneId::parse("sit1").unwrap()),
            csr_der: Bytes::from_static(&[1, 2, 3]),
            credential: JoinCredential::GoogleIdToken("eyJ.google.token".to_owned().into()),
        },
        JoinParams {
            subject: JoinSubject::Sentinel(text("nfs-vm-1")),
            csr_der: Bytes::from_static(&[4, 5]),
            credential: JoinCredential::JoinToken("one-time-token".to_owned().into()),
        },
    ] {
        let wire = pb::JoinRequest::from(params).encode_to_vec();
        let decoded = JoinParams::try_from(pb::JoinRequest::decode(wire.as_slice()).unwrap()).unwrap();
        let again = pb::JoinRequest::from(decoded).encode_to_vec();
        assert_eq!(wire, again);
    }
    let issued = IssuedCert {
        cert_chain_der: vec![Bytes::from_static(b"leaf"), Bytes::from_static(b"ca")],
        not_after: ts(1_800_000_000_000),
    };
    let wire = pb::JoinResponse::from(issued.clone()).encode_to_vec();
    let decoded = IssuedCert::try_from(pb::JoinResponse::decode(wire.as_slice()).unwrap()).unwrap();
    assert_eq!(decoded, issued);
}

proptest::proptest! {
    #[test]
    fn scan_delta_roundtrips_for_any_entries(
        seq in 0_u64..u64::MAX,
        more in proptest::bool::ANY,
        part in 0_u32..1000,
        entries in proptest::collection::vec(
            (0_u8..=255, 0_u64..1_000_000, proptest::bool::ANY, proptest::collection::vec(0_u8..=255, 0..64)),
            0..20,
        ),
    ) {
        let entries = entries
            .into_iter()
            .enumerate()
            .map(|(i, (h, size, denied, content))| ScanEntry {
                path: path(&format!("dir/file-{i}.yml")),
                hash: hash(h),
                size,
                mtime: ts(i64::try_from(size).unwrap()),
                observed_at: ts(5),
                denied,
                bytes: if denied { None } else { Some(Bytes::from(content)) },
            })
            .collect();
        let delta = ScanDelta {
            seq,
            base_root: None,
            new_root: hash(1),
            entries,
            removed: vec![],
            skipped: vec![],
            during_job: None,
            more,
            part,
        };
        let wire = FromAgent::Delta(delta.clone()).into_proto().encode_to_vec();
        let decoded = pb::AgentMessage::decode(wire.as_slice()).unwrap();
        proptest::prop_assert_eq!(FromAgent::from_proto(decoded).unwrap(), Some(FromAgent::Delta(delta)));
    }
}

// ------------------------------------------------------------------ AC3a: limits

fn delta_with_content(entries: &[usize]) -> pb::ScanDelta {
    pb::ScanDelta {
        seq: 1,
        new_root: Bytes::from(vec![2; 32]),
        entries: entries
            .iter()
            .enumerate()
            .map(|(i, len)| pb::ScanEntry {
                path: format!("f{i}.bin"),
                hash: Bytes::from(vec![3; 32]),
                size: *len as u64,
                content: Some(Bytes::from(vec![0; *len])),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

fn agent_message(delta: pb::ScanDelta) -> pb::AgentMessage {
    pb::AgentMessage {
        kind: Some(pb::agent_message::Kind::ScanDelta(delta)),
    }
}

#[test]
fn scan_delta_over_3mib_is_rejected() {
    const MIB3: usize = 3 * 1024 * 1024;
    assert_eq!(limits::MAX_SCAN_DELTA_BYTES, MIB3);
    assert_eq!(limits::MAX_SCAN_DELTA_BYTES, ScanDelta::MAX_BYTES);

    // Exactly 3 MiB, split over two entries, is accepted.
    let ok = FromAgent::from_proto(agent_message(delta_with_content(&[MIB3 - 10, 10]))).unwrap();
    assert!(matches!(ok, Some(FromAgent::Delta(_))));

    // One byte more is not, whether it is one entry or many.
    let err = FromAgent::from_proto(agent_message(delta_with_content(&[MIB3 + 1]))).unwrap_err();
    assert_eq!(err, ConvertError::TooLarge("scan_delta.entries.content"));
    let err = FromAgent::from_proto(agent_message(delta_with_content(&[MIB3 - 10, 11]))).unwrap_err();
    assert_eq!(err, ConvertError::TooLarge("scan_delta.entries.content"));
}

#[test]
fn counts_are_bounded() {
    let mut delta = delta_with_content(&[]);
    delta.removed = vec!["gone.yml".to_owned(); ScanDelta::MAX_ENTRIES + 1];
    let err = FromAgent::from_proto(agent_message(delta)).unwrap_err();
    assert_eq!(err, ConvertError::TooLarge("scan_delta.removed"));

    let mut delta = delta_with_content(&[]);
    delta.entries = (0..=ScanDelta::MAX_ENTRIES)
        .map(|i| pb::ScanEntry {
            path: format!("f{i}"),
            hash: Bytes::from(vec![1; 32]),
            ..Default::default()
        })
        .collect();
    let err = FromAgent::from_proto(agent_message(delta)).unwrap_err();
    assert_eq!(err, ConvertError::TooLarge("scan_delta.entries"));
}

#[test]
fn write_file_over_3mib_is_rejected() {
    let msg = pb::HubMessage {
        kind: Some(pb::hub_message::Kind::WriteFile(pb::WriteFile {
            request_id: "r1".to_owned(),
            path: "a.yml".to_owned(),
            expected: Some(pb::Expected {
                state: Some(pb::expected::State::Absent(pb::expected::Absent {})),
            }),
            content: Bytes::from(vec![0; limits::MAX_SCAN_DELTA_BYTES + 1]),
        })),
    };
    assert_eq!(
        ToAgent::from_proto(msg).unwrap_err(),
        ConvertError::TooLarge("write_file.content")
    );
}

// ------------------------------------------------------------------ S11: validation at the edge

#[test]
fn hostile_paths_never_become_nfs_paths() {
    for bad in ["../etc/passwd", "/etc/passwd", "a/../../b", "a\\b", "a\0b", ""] {
        let mut delta = delta_with_content(&[1]);
        delta.entries[0].path = bad.to_owned();
        let err = FromAgent::from_proto(agent_message(delta)).unwrap_err();
        assert_eq!(
            err,
            ConvertError::Invalid("scan_delta.entries.path"),
            "path {bad:?}"
        );
    }

    let msg = pb::HubMessage {
        kind: Some(pb::hub_message::Kind::NotifyConfigServer(
            pb::NotifyConfigServer {
                request_id: "r1".to_owned(),
                paths: vec!["ok.yml".to_owned(), "../escape".to_owned()],
            },
        )),
    };
    assert_eq!(
        ToAgent::from_proto(msg).unwrap_err(),
        ConvertError::Invalid("notify_config_server.paths")
    );
}

#[test]
fn hashes_must_be_32_bytes() {
    for len in [0, 31, 33, 64] {
        let mut delta = delta_with_content(&[1]);
        delta.entries[0].hash = Bytes::from(vec![1; len]);
        let err = FromAgent::from_proto(agent_message(delta)).unwrap_err();
        assert_eq!(
            err,
            ConvertError::Invalid("scan_delta.entries.hash"),
            "length {len}"
        );
    }
    let msg = pb::AgentMessage {
        kind: Some(pb::agent_message::Kind::Heartbeat(pb::Heartbeat {
            scan_seq: 1,
            merkle_root: Bytes::from_static(b"not a hash"),
            file_count: 1,
        })),
    };
    assert_eq!(
        FromAgent::from_proto(msg).unwrap_err(),
        ConvertError::Invalid("heartbeat.merkle_root")
    );
}

#[test]
fn denied_entries_cannot_carry_bytes() {
    let mut delta = delta_with_content(&[4]);
    delta.entries[0].denied = true;
    let err = FromAgent::from_proto(agent_message(delta)).unwrap_err();
    assert_eq!(err, ConvertError::Invalid("scan_delta.entries.content"));
}

#[test]
fn a_write_without_an_expected_state_is_rejected() {
    let msg = |expected| pb::HubMessage {
        kind: Some(pb::hub_message::Kind::WriteFile(pb::WriteFile {
            request_id: "r1".to_owned(),
            path: "a.yml".to_owned(),
            expected,
            content: Bytes::from_static(b"x"),
        })),
    };
    assert_eq!(
        ToAgent::from_proto(msg(None)).unwrap_err(),
        ConvertError::Missing("write_file.expected")
    );
    assert_eq!(
        ToAgent::from_proto(msg(Some(pb::Expected { state: None }))).unwrap_err(),
        ConvertError::Missing("write_file.expected")
    );
    let bad_hash = pb::Expected {
        state: Some(pb::expected::State::Hash(Bytes::from_static(b"short"))),
    };
    assert_eq!(
        ToAgent::from_proto(msg(Some(bad_hash))).unwrap_err(),
        ConvertError::Invalid("write_file.expected.hash")
    );
}

#[test]
fn unspecified_and_unknown_enum_values_are_rejected() {
    let sync = |kind: i32| pb::AgentMessage {
        kind: Some(pb::agent_message::Kind::ClusterReport(pb::ClusterReport {
            sync_windows: vec![pb::SyncWindowEvent {
                kind,
                job: Some(pb::JobRef {
                    name: "job".to_owned(),
                    uid: "u1".to_owned(),
                }),
                at_ms: 1,
            }],
            ..Default::default()
        })),
    };
    for bad in [0, 99] {
        assert_eq!(
            FromAgent::from_proto(sync(bad)).unwrap_err(),
            ConvertError::Invalid("cluster_report.sync_windows.kind")
        );
    }

    let op = |ok: bool, error_code: i32| pb::AgentMessage {
        kind: Some(pb::agent_message::Kind::OpResult(pb::OpResult {
            request_id: "r1".to_owned(),
            ok,
            error_code,
            current_hash: None,
        })),
    };
    // Failure without a code, success with a code, and a code this build does not know are all invalid.
    for (ok, code) in [(false, 0), (true, pb::OpErrorCode::Conflict as i32), (false, 99)] {
        assert_eq!(
            FromAgent::from_proto(op(ok, code)).unwrap_err(),
            ConvertError::Invalid("op_result.error_code"),
            "ok={ok} code={code}"
        );
    }

    let audit = pb::SentinelMessage {
        kind: Some(pb::sentinel_message::Kind::Batch(pb::AuditRecordBatch {
            seq: 1,
            records: vec![pb::AuditRecord {
                path: "a.yml".to_owned(),
                operation: 0,
                ..Default::default()
            }],
        })),
    };
    assert_eq!(
        FromSentinel::from_proto(audit).unwrap_err(),
        ConvertError::Invalid("audit_record_batch.records.operation")
    );
}

#[test]
fn zero_intervals_in_agent_config_are_rejected() {
    let msg = pb::HubMessage {
        kind: Some(pb::hub_message::Kind::AgentConfig(pb::AgentConfig {
            scan_interval_secs: 0,
            heartbeat_interval_secs: 30,
            ..Default::default()
        })),
    };
    assert_eq!(
        ToAgent::from_proto(msg).unwrap_err(),
        ConvertError::Invalid("agent_config.scan_interval_secs")
    );
}

#[test]
fn served_status_must_be_an_http_status() {
    for bad in [0, 99, 600, 70_000] {
        let msg = pb::AgentMessage {
            kind: Some(pb::agent_message::Kind::ServedResponse(pb::ServedResponse {
                request_id: "r1".to_owned(),
                status: bad,
                body: Bytes::new(),
            })),
        };
        assert_eq!(
            FromAgent::from_proto(msg).unwrap_err(),
            ConvertError::Invalid("served_response.status"),
            "status {bad}"
        );
    }
}

// ------------------------------------------------------------------ S5, S7, S21: join

fn join_request() -> pb::JoinRequest {
    pb::JoinRequest {
        swimlane_id: "sit1".to_owned(),
        csr_der: Bytes::from_static(&[1, 2, 3]),
        credential: Some(pb::join_request::Credential::JoinToken(
            "s3cr3t-join-token".to_owned(),
        )),
        kind: pb::PeerKind::Agent as i32,
    }
}

#[test]
fn join_requires_a_known_kind_a_credential_and_a_valid_subject() {
    assert!(JoinParams::try_from(join_request()).is_ok());

    let mut req = join_request();
    req.kind = pb::PeerKind::Unspecified as i32;
    assert_eq!(
        JoinParams::try_from(req).unwrap_err(),
        ConvertError::Invalid("join.kind")
    );

    let mut req = join_request();
    req.kind = 42;
    assert_eq!(
        JoinParams::try_from(req).unwrap_err(),
        ConvertError::Invalid("join.kind")
    );

    let mut req = join_request();
    req.credential = None;
    assert_eq!(
        JoinParams::try_from(req).unwrap_err(),
        ConvertError::Missing("join.credential")
    );

    let mut req = join_request();
    req.swimlane_id = "../other".to_owned();
    assert_eq!(
        JoinParams::try_from(req).unwrap_err(),
        ConvertError::Invalid("join.swimlane_id")
    );

    let mut req = join_request();
    req.csr_der = Bytes::new();
    assert_eq!(
        JoinParams::try_from(req).unwrap_err(),
        ConvertError::Missing("join.csr_der")
    );

    let mut req = join_request();
    req.csr_der = Bytes::from(vec![0; limits::MAX_CSR_BYTES + 1]);
    assert_eq!(
        JoinParams::try_from(req).unwrap_err(),
        ConvertError::TooLarge("join.csr_der")
    );

    let mut req = join_request();
    req.credential = Some(pb::join_request::Credential::GoogleIdToken(
        "x".repeat(limits::MAX_TOKEN_BYTES + 1),
    ));
    assert_eq!(
        JoinParams::try_from(req).unwrap_err(),
        ConvertError::TooLarge("join.credential")
    );
}

#[test]
fn a_sentinel_join_yields_a_sentinel_subject() {
    let mut req = join_request();
    req.kind = pb::PeerKind::Sentinel as i32;
    req.swimlane_id = "nfs-vm-1".to_owned();
    let params = JoinParams::try_from(req).unwrap();
    assert_eq!(params.subject, JoinSubject::Sentinel(text("nfs-vm-1")));
}

#[test]
fn join_tokens_never_reach_debug_output() {
    let wire = join_request();
    let shown = format!("{wire:?}");
    assert!(!shown.contains("s3cr3t-join-token"), "{shown}");
    assert!(shown.contains("redacted"), "{shown}");

    // The oneof that holds the token has no `Debug` at all, so no `{:?}` of it can compile.
    static_assertions::assert_not_impl_any!(pb::join_request::Credential: std::fmt::Debug);

    let params = JoinParams::try_from(wire).unwrap();
    let shown = format!("{params:?}");
    assert!(!shown.contains("s3cr3t-join-token"), "{shown}");
    assert!(
        bool::from(params.credential.secret().ct_eq_bytes(b"s3cr3t-join-token")),
        "the verifier can still read the token"
    );
}

#[test]
fn cert_chains_are_bounded() {
    let too_many = pb::JoinResponse {
        cert_chain_der: vec![Bytes::from_static(b"c"); limits::MAX_CERT_CHAIN + 1],
        not_after_ms: 1,
    };
    assert_eq!(
        IssuedCert::try_from(too_many).unwrap_err(),
        ConvertError::TooLarge("join_response.cert_chain_der")
    );
    let empty = pb::JoinResponse {
        cert_chain_der: vec![],
        not_after_ms: 1,
    };
    assert_eq!(
        IssuedCert::try_from(empty).unwrap_err(),
        ConvertError::Missing("join_response.cert_chain_der")
    );
}

// ------------------------------------------------------------------ forward compatibility

/// Hand-encodes a length-delimited field: tag, length, payload (payloads here are shorter than 128 bytes).
fn raw_field(number: u32, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 128);
    let mut tag = (number << 3) | 2;
    let mut out = Vec::new();
    while tag >= 0x80 {
        out.push(u8::try_from(tag & 0x7f).unwrap() | 0x80);
        tag >>= 7;
    }
    out.push(u8::try_from(tag).unwrap());
    out.push(u8::try_from(payload.len()).unwrap());
    out.extend_from_slice(payload);
    out
}

#[test]
fn reserved_and_unknown_oneof_variants_are_ignored() {
    // 15 and 19 are `reserved` (kept free for additions); 14 and 99 have never been assigned. A newer peer
    // could send any of them, and an older build must skip the message, not fail the stream.
    for number in [14_u32, 15, 19] {
        let wire = raw_field(number, b"future message");
        let agent = pb::AgentMessage::decode(wire.as_slice()).unwrap();
        assert_eq!(agent.kind, None, "decoding drops field {number}");
        assert_eq!(FromAgent::from_proto(agent).unwrap(), None);

        let hub = pb::HubMessage::decode(wire.as_slice()).unwrap();
        assert_eq!(ToAgent::from_proto(hub).unwrap(), None);

        let sentinel = pb::SentinelMessage::decode(wire.as_slice()).unwrap();
        assert_eq!(FromSentinel::from_proto(sentinel).unwrap(), None);

        let ack = pb::SentinelAck::decode(wire.as_slice()).unwrap();
        assert_eq!(ToSentinel::from_proto(ack).unwrap(), None);
    }

    // A known variant followed by an unknown field still decodes: unknown fields are skipped.
    let mut wire = pb::HubMessage {
        kind: Some(pb::hub_message::Kind::Ack(pb::Ack { seq: 5 })),
    }
    .encode_to_vec();
    wire.extend(raw_field(14, b"extra"));
    let hub = pb::HubMessage::decode(wire.as_slice()).unwrap();
    assert_eq!(ToAgent::from_proto(hub).unwrap(), Some(ToAgent::Ack(5)));

    // An empty message (no variant at all) is ignored the same way.
    assert_eq!(FromAgent::from_proto(pb::AgentMessage::default()).unwrap(), None);
}

#[test]
fn reserved_numbers_are_declared_in_the_schema() {
    // Guards against someone reusing 15 to 19 for a different meaning (buf `breaking` would also catch it).
    let schema = include_str!("../../../proto/agent.proto");
    assert!(schema.matches("reserved 15 to 19;").count() >= 3);
}

// ------------------------------------------------------------------ transport: zstd and message size

mod transport {
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use hyper_util::rt::TokioIo;
    use proto::{grpc, limits, pb};
    use tonic::codegen::http::Uri;
    use tonic::transport::{Channel, Endpoint, Server};
    use tonic::{Code, Request, Response, Status};

    /// Records how the request arrived and answers with a certificate chain that compresses well.
    #[derive(Clone, Default)]
    struct Probe {
        request_encoding: Arc<Mutex<Option<String>>>,
    }

    #[tonic::async_trait]
    impl pb::agent_server::Agent for Probe {
        async fn join(
            &self,
            request: Request<pb::JoinRequest>,
        ) -> Result<Response<pb::JoinResponse>, Status> {
            let encoding = request
                .metadata()
                .get("grpc-encoding")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            *self.request_encoding.lock().unwrap() = encoding;
            Ok(Response::new(pb::JoinResponse {
                cert_chain_der: vec![Bytes::from(vec![0_u8; 8 * 1024])],
                not_after_ms: 1,
            }))
        }

        type ConnectStream = tokio_stream::Empty<Result<pb::HubMessage, Status>>;

        async fn connect(
            &self,
            _request: Request<tonic::Streaming<pb::AgentMessage>>,
        ) -> Result<Response<Self::ConnectStream>, Status> {
            Ok(Response::new(tokio_stream::empty()))
        }
    }

    /// A channel to `server`, over an in-memory duplex pipe instead of a socket.
    async fn duplex_channel(
        server: pb::agent_server::AgentServer<Probe>,
    ) -> (Channel, tokio::task::JoinHandle<()>) {
        let (client_io, server_io) = tokio::io::duplex(256 * 1024);
        let task = tokio::spawn(async move {
            let incoming = tokio_stream::once(Ok::<_, std::io::Error>(server_io));
            let _ = Server::builder().serve_with_incoming(server, incoming).await;
        });
        let mut client_io = Some(client_io);
        let channel = Endpoint::try_from("http://[::1]:50051")
            .unwrap()
            .connect_with_connector(tower::service_fn(move |_: Uri| {
                let io = client_io.take();
                async move {
                    io.map(TokioIo::new)
                        .ok_or_else(|| std::io::Error::other("the duplex pipe is single use"))
                }
            }))
            .await
            .unwrap();
        (channel, task)
    }

    fn join_request(csr_len: usize) -> pb::JoinRequest {
        pb::JoinRequest {
            swimlane_id: "sit1".to_owned(),
            csr_der: Bytes::from(vec![7_u8; csr_len]),
            credential: Some(pb::join_request::Credential::JoinToken("t".to_owned())),
            kind: pb::PeerKind::Agent as i32,
        }
    }

    #[tokio::test]
    async fn zstd_is_negotiated_over_duplex_channel() {
        let probe = Probe::default();
        let (channel, task) = duplex_channel(grpc::agent_server(probe.clone())).await;
        let mut client = grpc::agent_client(channel);

        let response = client.join(join_request(2048)).await.unwrap();

        assert_eq!(
            probe.request_encoding.lock().unwrap().as_deref(),
            Some("zstd"),
            "the request was compressed with zstd"
        );
        assert_eq!(
            response
                .metadata()
                .get("grpc-encoding")
                .and_then(|v| v.to_str().ok()),
            Some("zstd"),
            "the response was compressed with zstd"
        );
        assert_eq!(response.into_inner().cert_chain_der[0].len(), 8 * 1024);
        task.abort();
    }

    #[tokio::test]
    async fn a_server_without_zstd_refuses_a_zstd_client() {
        // Proves that the test above is about negotiation: without `accept_compressed` the call fails.
        let plain = pb::agent_server::AgentServer::new(Probe::default());
        let (channel, task) = duplex_channel(plain).await;
        let mut client = grpc::agent_client(channel);

        let status = client.join(join_request(16)).await.unwrap_err();

        assert_eq!(status.code(), Code::Unimplemented);
        task.abort();
    }

    #[tokio::test]
    async fn messages_over_the_grpc_limit_are_refused() {
        let probe = Probe::default();
        let (channel, task) = duplex_channel(grpc::agent_server(probe.clone())).await;
        let mut client = grpc::agent_client(channel);

        let status = client
            .join(join_request(limits::MAX_MESSAGE_BYTES + 1))
            .await
            .unwrap_err();

        // Whether the client's encoder or the server's decompressor notices first depends on how well the
        // payload compresses; either way the message is refused, never delivered.
        assert!(
            matches!(status.code(), Code::OutOfRange | Code::ResourceExhausted),
            "{status}"
        );
        assert!(
            probe.request_encoding.lock().unwrap().is_none(),
            "the handler never ran"
        );
        task.abort();
    }
}
