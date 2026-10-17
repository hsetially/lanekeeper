//! A sentinel on the rig: the admin's registration of an NFS VM (as the admin operations, A7, will write it), the records it sends, the
//! users its logins map to, and what the database holds afterwards.
//!
//! Time in these tests is the rig's `ManualClock` (`T0_MS`), and an observation's time is the one the scan entry says. A record's time is
//! the VM's, in the same milliseconds, so "a local edit at `T0 + 10 s`" is a record at that time and a scan that sees the file at
//! `T0 + 25 s` is the agent finding it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use async_trait::async_trait;
use domain::{
    AuditOperation, AuditRecord, AuditRecordBatch, DomainEvent, Guid, NfsPath, SentinelConfig, SentinelHello,
    ShortText, Timestamp, UserId,
};
use hub_registry::SentinelIngest;
use ports::conformance::SentinelProbe;
use ports::{AckSeq, SentinelIdentity, SentinelSink, SinkError};
use sqlx::PgPool;
use sqlx::Row;

use domain::ContentHash;

use super::rig::{Rig, delta, entry, path as nfs, root};

/// The VM of the tests and the root it watches.
pub const VM: &str = "nfs-vm-1";
pub const ROOT: &str = "/export";

pub fn text(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

pub fn identity() -> SentinelIdentity {
    SentinelIdentity::new(text(VM))
}

pub fn hello_of(vm: &str, root: &str) -> SentinelHello {
    SentinelHello {
        vm: text(vm),
        export_root: text(root),
        version: text("0.1.0"),
    }
}

/// A write of `path` by `ada_example_com`, through `sudo vim` (the login identity survives `sudo`), at `at_ms`.
pub fn record(path: &str, at_ms: i64) -> AuditRecord {
    AuditRecord {
        time: Timestamp::from_unix_millis(at_ms),
        path: NfsPath::parse(path).unwrap(),
        operation: AuditOperation::Write,
        success: true,
        login_user: text("ada_example_com"),
        effective_user: text("root"),
        exe: text("/usr/bin/vim"),
        comm: text("vim"),
    }
}

pub fn op(mut r: AuditRecord, operation: AuditOperation) -> AuditRecord {
    r.operation = operation;
    r
}

pub fn by(mut r: AuditRecord, login: &str, effective: &str) -> AuditRecord {
    r.login_user = text(login);
    r.effective_user = text(effective);
    r
}

pub fn failed(mut r: AuditRecord) -> AuditRecord {
    r.success = false;
    r
}

pub fn batch(seq: u64, records: Vec<AuditRecord>) -> AuditRecordBatch {
    AuditRecordBatch { seq, records }
}

/// What an observation's attribution is, as the table holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    pub source: String,
    pub confidence: String,
    pub actor: Option<serde_json::Value>,
    pub evidence: serde_json::Value,
}

impl Attr {
    pub fn is(&self, source: &str, confidence: &str) -> bool {
        self.source == source && self.confidence == confidence
    }

    /// The application user the actor names (`tid:oid`), when it is one.
    pub fn user(&self) -> Option<String> {
        let actor = self.actor.as_ref()?;
        (actor["kind"] == "user").then(|| actor["user"].as_str().map(str::to_owned))?
    }
}

/// One stored record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub batch_seq: i64,
    pub index: i32,
    pub at_ms: i64,
    pub path: String,
    pub operation: String,
    pub success: bool,
    pub login: Option<String>,
}

pub const TID: &str = "72f988bf-86f1-41af-91ab-2d7cd011db47";

/// The user `n` as an id.
pub fn user(n: u8) -> UserId {
    UserId::new(
        Guid::parse(TID).unwrap(),
        Guid::parse(&format!("00000000-0000-4000-8000-{n:012}")).unwrap(),
    )
}

impl Rig {
    /// The admin registers an NFS VM that serves `swimlane` and watches `root` (`sentinels`, as the admin operations will).
    pub async fn register_sentinel(&self, vm: &str, swimlane: Option<&str>, root: &str) {
        sqlx::query("INSERT INTO sentinels (vm, swimlane_id, export_root) VALUES ($1, $2, $3)")
            .bind(vm)
            .bind(swimlane)
            .bind(root)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    /// The usual setup: `nfs-vm-1` serves this rig's swimlane and watches `/export`, which is also where the agent's `hello` says the
    /// swimlane's export is, so a sentinel's paths are the swimlane's own.
    pub async fn sentinel_for_the_swimlane(&self) {
        self.register_sentinel(VM, Some(self.swimlane.as_str()), ROOT)
            .await;
    }

    pub async fn set_nfs_export(&self, export: Option<&str>) {
        sqlx::query("UPDATE swimlanes SET nfs_export = $2 WHERE id = $1")
            .bind(self.swimlane.as_str())
            .bind(export)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    pub async fn sentinel_hello(&self) -> Result<SentinelConfig, SinkError> {
        self.sentinel.hello(&identity(), hello_of(VM, ROOT)).await
    }

    pub async fn records(&self, b: AuditRecordBatch) -> Result<AckSeq, SinkError> {
        self.sentinel.records(&identity(), b).await
    }

    /// Say hello, then deliver the batch: the usual way a test gets a record in.
    pub async fn hello_and_records(&self, b: AuditRecordBatch) -> AckSeq {
        self.sentinel_hello().await.unwrap();
        self.records(b).await.unwrap()
    }

    /// A user that signed in: `status` is `active`, `pending` or `disabled`.
    pub async fn add_user(&self, n: u8, email: &str, status: &str) -> UserId {
        let id = user(n);
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, 'viewer', $5)",
        )
        .bind(id.tid().as_str())
        .bind(id.oid().as_str())
        .bind(email)
        .bind(format!("User {n}"))
        .bind(status)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    /// The admin maps an OS Login name to a user (`os_login_user_map`).
    pub async fn map_login(&self, login: &str, user: &UserId) {
        sqlx::query("INSERT INTO os_login_user_map (os_login, user_tid, user_oid) VALUES ($1, $2, $3)")
            .bind(login)
            .bind(user.tid().as_str())
            .bind(user.oid().as_str())
            .execute(&self.pool)
            .await
            .unwrap();
    }

    /// The attribution of the `nth` observation of `path` (0 is the first), in the order they were applied.
    pub async fn attribution(&self, path: &str, nth: usize) -> Attr {
        let rows = sqlx::query(
            "SELECT attribution_source, attribution_confidence, attribution_actor, attribution_evidence \
             FROM file_observations WHERE swimlane_id = $1 AND path = $2 ORDER BY id LIMIT 100",
        )
        .bind(self.swimlane.as_str())
        .bind(path)
        .fetch_all(&self.pool)
        .await
        .unwrap();
        let row = &rows[nth];
        Attr {
            source: row.get("attribution_source"),
            confidence: row.get("attribution_confidence"),
            actor: row.get("attribution_actor"),
            evidence: row.get("attribution_evidence"),
        }
    }

    /// The records the sentinel stored, in the order of their keys.
    pub async fn stored_records(&self) -> Vec<Stored> {
        sqlx::query(
            "SELECT batch_seq, record_index, (EXTRACT(EPOCH FROM observed_at) * 1000)::bigint AS at, path, operation, success, login_user \
             FROM sentinel_records ORDER BY batch_seq, record_index LIMIT 100000",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| Stored {
            batch_seq: r.get("batch_seq"),
            index: r.get("record_index"),
            at_ms: r.get("at"),
            path: r.get("path"),
            operation: r.get("operation"),
            success: r.get("success"),
            login: r.get("login_user"),
        })
        .collect()
    }

    pub async fn last_acked(&self) -> i64 {
        sqlx::query_scalar("SELECT last_acked_seq FROM sentinels WHERE vm = $1")
            .bind(VM)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// `AttributionUpdated` events in the outbox, as (path) in order.
    pub async fn attribution_updates(&self) -> Vec<String> {
        self.bus
            .published()
            .await
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::AttributionUpdated { path, .. } => Some(path.as_str().to_owned()),
                _ => None,
            })
            .collect()
    }

    /// The `attribution_upgraded` audit events, as (path, source, confidence).
    pub async fn upgrade_audits(&self) -> Vec<(Option<String>, Option<String>)> {
        sqlx::query_as(
            "SELECT path, attribution_source FROM audit_events WHERE action = 'attribution_upgraded' ORDER BY seq LIMIT 10000",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

/// The agent's side of a swimlane: one delta after another, each against the root the one before left, observed at the time the test says.
pub struct Agent<'a> {
    pub rig: &'a Rig,
    seq: u64,
    n: u8,
}

impl<'a> Agent<'a> {
    /// Say hello and deliver `files` as the first full scan (the swimlane's inventory: not a change).
    pub async fn start(rig: &'a Rig, files: &[(&str, &str)]) -> Agent<'a> {
        rig.first_scan(files).await;
        Agent { rig, seq: 1, n: 1 }
    }

    fn next(&mut self) -> (u64, ContentHash, ContentHash) {
        let base = root(self.n);
        self.seq += 1;
        self.n += 1;
        (self.seq, base, root(self.n))
    }

    /// `content` at `path`, seen by the agent at `seen_ms`.
    pub async fn change(&mut self, path: &str, content: &str, seen_ms: i64) {
        let (seq, base, new) = self.next();
        let d = delta(
            seq,
            Some(base),
            new,
            vec![entry(path, content.as_bytes(), seen_ms)],
        );
        self.rig.delta(d).await.unwrap();
    }

    /// `content` at `path`, a file the agent calls denied: its name, size and hash, and no bytes (D79).
    pub async fn change_denied(&mut self, path: &str, content: &str, seen_ms: i64) {
        let (seq, base, new) = self.next();
        let mut e = entry(path, content.as_bytes(), seen_ms);
        e.denied = true;
        e.bytes = None;
        self.rig
            .delta(delta(seq, Some(base), new, vec![e]))
            .await
            .unwrap();
    }

    /// Several files at once, all seen at `seen_ms`.
    pub async fn change_all(&mut self, changes: &[(&str, &str)], seen_ms: i64) {
        let (seq, base, new) = self.next();
        let entries = changes
            .iter()
            .map(|(p, c)| entry(p, c.as_bytes(), seen_ms))
            .collect();
        self.rig
            .delta(delta(seq, Some(base), new, entries))
            .await
            .unwrap();
    }

    /// `path` is gone. A removal carries no time of its own: it is placed at the time of the delta it comes in, which `keep` (a file that
    /// has not changed, as it is now) gives it, `seen_ms`.
    pub async fn delete_seen_at(&mut self, path: &str, keep: (&str, &str), seen_ms: i64) {
        let (seq, base, new) = self.next();
        let mut d = delta(
            seq,
            Some(base),
            new,
            vec![entry(keep.0, keep.1.as_bytes(), seen_ms)],
        );
        d.removed = vec![nfs(path)];
        self.rig.delta(d).await.unwrap();
    }
}

/// The registry's sentinel sink with a way to count what it stored, so that `ports::conformance::sentinel_sink` runs against the real
/// implementation over a real Postgres. It **forwards both calls** to the sink.
pub struct PgSentinelProbe {
    pub sink: SentinelIngest,
    pub pool: PgPool,
}

#[async_trait]
impl SentinelSink for PgSentinelProbe {
    async fn hello(&self, id: &SentinelIdentity, h: SentinelHello) -> Result<SentinelConfig, SinkError> {
        self.sink.hello(id, h).await
    }

    async fn records(&self, id: &SentinelIdentity, b: AuditRecordBatch) -> Result<AckSeq, SinkError> {
        self.sink.records(id, b).await
    }
}

#[async_trait]
impl SentinelProbe for PgSentinelProbe {
    async fn applied_records(&self, id: &SentinelIdentity) -> usize {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sentinel_records r JOIN sentinels s ON s.id = r.sentinel_id WHERE s.vm = $1",
        )
        .bind(id.name().as_str())
        .fetch_one(&self.pool)
        .await
        .unwrap();
        usize::try_from(n).unwrap()
    }
}

impl Rig {
    /// The sink with a probe over this rig's database.
    pub fn sentinel_probe(&self) -> PgSentinelProbe {
        PgSentinelProbe {
            sink: self.sentinel.clone(),
            pool: self.pool.clone(),
        }
    }
}
