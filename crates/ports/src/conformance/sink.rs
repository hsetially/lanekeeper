use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AuditOperation, AuditRecord, AuditRecordBatch, ClusterReport, ContentHash, Heartbeat, HeartbeatAction,
    Hello, ScanDelta, ScanEntry, SentinelHello, ShortText, Timestamp,
};

use super::sample::{hash, nfs, swimlane};
use crate::{AckSeq, AgentIdentity, ReportSink, SentinelIdentity, SentinelSink, SinkError, content_hash};

fn text(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

fn hello(sl: &str) -> Hello {
    Hello {
        agent_version: text("0.1.0"),
        swimlane: swimlane(sl),
        cluster: text("gke-sit1"),
        project: text("proj"),
        nfs_server: text("nfs.internal"),
        export: text("/export"),
        mount_root: text("/mnt/config"),
    }
}

fn heartbeat(root: ContentHash) -> Heartbeat {
    Heartbeat {
        scan_seq: 1,
        merkle_root: root,
        file_count: 1,
    }
}

fn entry(path: &str, content: &'static [u8]) -> ScanEntry {
    ScanEntry {
        path: nfs(path),
        hash: content_hash(content),
        size: content.len() as u64,
        mtime: Timestamp::from_unix_millis(1),
        observed_at: Timestamp::from_unix_millis(2),
        denied: false,
        bytes: Some(Bytes::from_static(content)),
    }
}

fn delta(
    seq: u64,
    base: Option<ContentHash>,
    new_root: ContentHash,
    entries: Vec<ScanEntry>,
    more: bool,
) -> ScanDelta {
    ScanDelta {
        seq,
        base_root: base,
        new_root,
        entries,
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more,
        part: 0,
    }
}

/// - Nothing is accepted before `hello`; a `hello` naming another swimlane than the sender's identity is an
///   `IdentityMismatch` (S5).
/// - The first heartbeat asks for a full scan; a known root asks for nothing; a changed root asks for a
///   delta since the known root.
/// - A multi-part delta is applied only by its last part.
/// - A denied file that carries bytes, or bytes that do not match their hash, make the delta `Invalid` and
///   apply nothing.
/// - Agents are independent of each other.
pub async fn report_sink<S: ReportSink + ?Sized>(s: &S) {
    let id = AgentIdentity::new(swimlane("sit1"));
    let other = AgentIdentity::new(swimlane("sit2"));
    let root_a = hash(b"root-a");
    let root_b = hash(b"root-b");

    assert_eq!(
        s.heartbeat(&id, heartbeat(root_a)).await,
        Err(SinkError::HelloRequired),
        "heartbeat before hello"
    );
    assert_eq!(
        s.delta(&id, delta(1, None, root_a, vec![], false)).await,
        Err(SinkError::HelloRequired),
        "delta before hello"
    );
    assert_eq!(
        s.hello(&other, hello("sit1")).await.unwrap_err(),
        SinkError::IdentityMismatch,
        "hello for another swimlane"
    );

    let config = s.hello(&id, hello("sit1")).await.expect("hello");
    assert!(config.scan_interval_secs > 0 && config.heartbeat_interval_secs > 0 && config.max_file_bytes > 0);
    assert!(
        !config.deny_globs.is_empty(),
        "denied-file globs are part of the config (D79)"
    );

    assert_eq!(
        s.heartbeat(&id, heartbeat(root_a)).await.unwrap(),
        HeartbeatAction::RequestFullScan,
        "first heartbeat after hello"
    );
    s.delta(
        &id,
        delta(1, None, root_a, vec![entry("a/b.yml", b"x: 1")], false),
    )
    .await
    .expect("full delta");
    assert_eq!(
        s.heartbeat(&id, heartbeat(root_a)).await.unwrap(),
        HeartbeatAction::None
    );
    assert_eq!(
        s.heartbeat(&id, heartbeat(root_b)).await.unwrap(),
        HeartbeatAction::RequestDelta { since_root: root_a },
        "changed root"
    );

    // A delta in two parts: only the last part applies it.
    s.delta(
        &id,
        delta(2, Some(root_a), root_b, vec![entry("c.yml", b"y: 1")], true),
    )
    .await
    .expect("first part");
    assert_eq!(
        s.heartbeat(&id, heartbeat(root_b)).await.unwrap(),
        HeartbeatAction::RequestDelta { since_root: root_a },
        "a delta with more parts to come is not applied yet"
    );
    s.delta(&id, delta(3, Some(root_a), root_b, vec![], false))
        .await
        .expect("last part");
    assert_eq!(
        s.heartbeat(&id, heartbeat(root_b)).await.unwrap(),
        HeartbeatAction::None
    );

    // Invalid payloads change nothing.
    let mut denied = entry("keys/app.pem", b"-----BEGIN-----");
    denied.denied = true;
    assert_eq!(
        s.delta(&id, delta(4, Some(root_b), hash(b"root-c"), vec![denied], false))
            .await,
        Err(SinkError::Invalid),
        "denied file with bytes"
    );
    let mut forged = entry("d.yml", b"z: 1");
    forged.hash = hash(b"something else");
    assert_eq!(
        s.delta(&id, delta(5, Some(root_b), hash(b"root-c"), vec![forged], false))
            .await,
        Err(SinkError::Invalid),
        "bytes that do not match their hash"
    );
    assert_eq!(
        s.heartbeat(&id, heartbeat(root_b)).await.unwrap(),
        HeartbeatAction::None,
        "rejected deltas leave the root alone"
    );

    s.cluster(
        &id,
        ClusterReport {
            full: true,
            deployments: vec![],
            release_hints: vec![],
            config_server_started_at: None,
            sync_windows: vec![],
        },
    )
    .await
    .expect("cluster report");

    assert_eq!(
        s.heartbeat(&other, heartbeat(root_a)).await,
        Err(SinkError::HelloRequired),
        "sit2 never said hello, whatever sit1 did"
    );
}

/// What the sentinel conformance suite needs to see.
#[async_trait]
pub trait SentinelProbe: SentinelSink {
    /// Records applied for this sentinel so far (a re-sent batch counts once).
    async fn applied_records(&self, id: &SentinelIdentity) -> usize;
}

fn record(path: &str) -> AuditRecord {
    AuditRecord {
        time: Timestamp::from_unix_millis(10),
        path: nfs(path),
        operation: AuditOperation::Write,
        success: true,
        login_user: text("ada"),
        effective_user: text("root"),
        exe: text("/usr/bin/vim"),
        comm: text("vim"),
    }
}

fn batch(seq: u64, n: usize) -> AuditRecordBatch {
    AuditRecordBatch {
        seq,
        records: (0..n).map(|i| record(&format!("f{i}.yml"))).collect(),
    }
}

/// - `records` before `hello` is `HelloRequired`.
/// - A batch is applied once and acknowledged with its sequence number.
/// - A re-sent batch is acknowledged (with the highest sequence so far) and not applied again (D74).
/// - A batch over the configured size is `TooLarge`.
pub async fn sentinel_sink<S: SentinelProbe + ?Sized>(s: &S) {
    let id = SentinelIdentity::new(text("nfs-vm-1"));
    let hello = SentinelHello {
        vm: text("nfs-vm-1"),
        export_root: text("/export"),
        version: text("0.1.0"),
    };
    assert_eq!(
        s.records(&id, batch(1, 1)).await,
        Err(SinkError::HelloRequired),
        "records before hello"
    );
    let config = s.hello(&id, hello).await.expect("hello");
    assert!(config.max_batch_records > 0 && config.flush_interval_secs > 0);

    assert_eq!(s.records(&id, batch(1, 2)).await.unwrap(), AckSeq::new(1));
    assert_eq!(s.applied_records(&id).await, 2);
    assert_eq!(
        s.records(&id, batch(1, 2)).await.unwrap(),
        AckSeq::new(1),
        "re-send is acknowledged"
    );
    assert_eq!(s.applied_records(&id).await, 2, "re-send is not applied again");
    assert_eq!(s.records(&id, batch(2, 1)).await.unwrap(), AckSeq::new(2));
    assert_eq!(s.applied_records(&id).await, 3);
    assert_eq!(
        s.records(&id, batch(1, 2)).await.unwrap(),
        AckSeq::new(2),
        "an old batch is acknowledged with the highest sequence so far"
    );
    assert_eq!(s.applied_records(&id).await, 3);

    let too_many = config.max_batch_records as usize + 1;
    assert_eq!(
        s.records(&id, batch(3, too_many)).await,
        Err(SinkError::TooLarge),
        "batch over max_batch_records"
    );
    assert_eq!(
        s.applied_records(&id).await,
        3,
        "a rejected batch applies nothing"
    );
}
