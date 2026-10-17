//! Plumbing for the tool-write tests: a `TxFactory` over a pool (the suites of `ports::conformance` begin, commit and roll back through
//! it), and a probe that reads the committed registrations back, so that `ports::conformance::tool_writes_with_deletes` and
//! `tool_writes_bounded` run against the real registry over a real Postgres.
//!
//! The probe **forwards both `register` and `register_delete`** to the registry. A decorator that forwarded only `register` would inherit the
//! port's do-nothing default for deletes, and the suite would catch it (`tool_writes_with_deletes` fails for a probe whose
//! `registered_deletes` is `None`, and for one that opts in and registers nothing).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use async_trait::async_trait;
use domain::{ContentHash, Guid, NfsPath, RequestId, SwimlaneId, UserId};
use hub_registry::PgToolWrites;
use ports::conformance::ToolWritesProbe;
use ports::{OwnedTx, ToolDeleteEntry, ToolWriteEntry, ToolWriteError, ToolWrites, Tx, TxError, TxFactory};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub struct PgTxFactory(pub PgPool);

struct PgOwnedTx(Transaction<'static, Postgres>);

#[async_trait]
impl OwnedTx for PgOwnedTx {
    fn tx(&mut self) -> Tx<'_> {
        Tx::from_pg(&mut self.0)
    }

    async fn commit(self: Box<Self>) -> Result<(), TxError> {
        self.0.commit().await.map_err(|_| TxError::CommitFailed)
    }

    async fn rollback(self: Box<Self>) -> Result<(), TxError> {
        self.0.rollback().await.map_err(|_| TxError::CommitFailed)
    }
}

#[async_trait]
impl TxFactory for PgTxFactory {
    async fn begin(&self) -> Result<Box<dyn OwnedTx>, TxError> {
        let tx = self.0.begin().await.map_err(|_| TxError::Unavailable)?;
        Ok(Box::new(PgOwnedTx(tx)))
    }
}

/// The registry, plus a way to read what it committed.
pub struct Probe {
    pub registry: PgToolWrites,
    pub pool: PgPool,
}

#[async_trait]
impl ToolWrites for Probe {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.registry
            .register(tx, swimlane, path, new_hash, user, request_id)
            .await
    }

    /// Forwarded, so that the suite tests the registry's `register_delete` and not the port's default.
    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.registry
            .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
            .await
    }
}

type Row5 = (String, String, Vec<u8>, String, String, String);

async fn committed(pool: &PgPool, is_delete: bool) -> Vec<Row5> {
    sqlx::query(
        "SELECT swimlane_id, path, hash::bytea AS hash, user_tid, user_oid, request_id FROM tool_write_registrations \
         WHERE is_delete = $1 ORDER BY id LIMIT 100000",
    )
    .bind(is_delete)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| {
        (
            r.get("swimlane_id"),
            r.get("path"),
            r.get("hash"),
            r.get("user_tid"),
            r.get("user_oid"),
            r.get("request_id"),
        )
    })
    .collect()
}

fn user_of(tid: &str, oid: &str) -> UserId {
    UserId::new(Guid::parse(tid).unwrap(), Guid::parse(oid).unwrap())
}

#[async_trait]
impl ToolWritesProbe for Probe {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        committed(&self.pool, false)
            .await
            .into_iter()
            .map(|(swimlane, path, hash, tid, oid, request)| ToolWriteEntry {
                swimlane: SwimlaneId::parse(&swimlane).unwrap(),
                path: NfsPath::parse(&path).unwrap(),
                new_hash: ContentHash::from_bytes(hash.try_into().unwrap()),
                user: user_of(&tid, &oid),
                request_id: RequestId::parse(&request).unwrap(),
            })
            .collect()
    }

    /// The registry registers deletes, so the probe says so: `Some`, even when none is registered yet.
    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        Some(
            committed(&self.pool, true)
                .await
                .into_iter()
                .map(|(swimlane, path, hash, tid, oid, request)| ToolDeleteEntry {
                    swimlane: SwimlaneId::parse(&swimlane).unwrap(),
                    path: NfsPath::parse(&path).unwrap(),
                    deleted_hash: ContentHash::from_bytes(hash.try_into().unwrap()),
                    user: user_of(&tid, &oid),
                    request_id: RequestId::parse(&request).unwrap(),
                })
                .collect(),
        )
    }
}

// ------------------------------------------------------------------------------------------------ the tests of matching

use ports::Tx as PortTx;
use ports::content_hash;

use super::rig::{Rig, T, delta, entry, path, root};

pub fn user(n: u32) -> UserId {
    UserId::new(
        Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
        Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
    )
}

pub fn request(s: &str) -> RequestId {
    RequestId::parse(s).unwrap()
}

pub fn hash_of(content: &str) -> ContentHash {
    content_hash(content.as_bytes())
}

/// An open transaction of the caller (prompt 06), in which registrations are made and which commits later.
pub type CallerTx = Transaction<'static, Postgres>;

impl Rig {
    pub async fn caller(&self) -> CallerTx {
        self.pool.begin().await.unwrap()
    }

    /// `register` in `tx`, as prompt 06 does before it sends `WriteFile`.
    pub async fn register_in(&self, tx: &mut CallerTx, p: &str, content: &str, req: &str, who: u32) {
        self.tool_writes
            .register(
                &mut PortTx::from_pg(tx),
                &self.swimlane,
                &path(p),
                &hash_of(content),
                &user(who),
                &request(req),
            )
            .await
            .unwrap();
    }

    /// `register_delete` in `tx`, with the content of the version it deletes.
    pub async fn register_delete_in(&self, tx: &mut CallerTx, p: &str, deleted: &str, req: &str, who: u32) {
        self.tool_writes
            .register_delete(
                &mut PortTx::from_pg(tx),
                &self.swimlane,
                &path(p),
                &hash_of(deleted),
                &user(who),
                &request(req),
            )
            .await
            .unwrap();
    }

    /// A registration that is made and committed at once.
    pub async fn registered(&self, p: &str, content: &str, req: &str, who: u32) {
        let mut tx = self.caller().await;
        self.register_in(&mut tx, p, content, req, who).await;
        tx.commit().await.unwrap();
    }

    pub async fn registered_delete(&self, p: &str, deleted: &str, req: &str, who: u32) {
        let mut tx = self.caller().await;
        self.register_delete_in(&mut tx, p, deleted, req, who).await;
        tx.commit().await.unwrap();
    }

    /// The registrations still pending: (path, `is_delete`, request id, id order).
    pub async fn pending(&self) -> Vec<(String, bool, String)> {
        sqlx::query_as(
            "SELECT path, is_delete, request_id FROM tool_write_registrations ORDER BY id LIMIT 1000",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    /// Every observation of a path in the order the registry applied them (`id`): the hash (`None`: a deletion), its source, and its
    /// attribution (source, confidence, the user if the actor is one).
    pub async fn history(&self, p: &str) -> Vec<Seen> {
        sqlx::query(
            "SELECT hash::bytea AS hash, source, attribution_source, attribution_confidence, attribution_actor, \
                    (EXTRACT(EPOCH FROM recorded_at) * 1000)::bigint AS applied \
             FROM file_observations WHERE swimlane_id = $1 AND path = $2 ORDER BY id LIMIT 1000",
        )
        .bind(self.swimlane.as_str())
        .bind(p)
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| Seen {
            hash: r
                .get::<Option<Vec<u8>>, _>("hash")
                .map(|b| ContentHash::from_bytes(b.try_into().unwrap())),
            source: r.get("source"),
            attribution: r.get("attribution_source"),
            confidence: r.get("attribution_confidence"),
            actor: r
                .get::<Option<serde_json::Value>, _>("attribution_actor")
                .and_then(|a| a["user"].as_str().map(str::to_owned)),
            applied_ms: r.get("applied"),
        })
        .collect()
    }

    /// Set the attribution of an observation by hand, as another component (the sentinel's correlator, a sync) would have.
    pub async fn attribute_by_hand(&self, p: &str, nth: usize, source: &str, confidence: &str) {
        sqlx::query(
            "UPDATE file_observations SET attribution_source = $3, attribution_confidence = $4 \
             WHERE id = (SELECT id FROM file_observations WHERE swimlane_id = $1 AND path = $2 ORDER BY id OFFSET $5 LIMIT 1)",
        )
        .bind(self.swimlane.as_str())
        .bind(p)
        .bind(source)
        .bind(confidence)
        .bind(i64::try_from(nth).unwrap())
        .execute(&self.pool)
        .await
        .unwrap();
    }
}

/// One observation of a path, as `Rig::history` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub hash: Option<ContentHash>,
    pub source: String,
    pub attribution: String,
    pub confidence: String,
    /// The user of the actor, as `tid:oid`, when the actor is a user.
    pub actor: Option<String>,
    pub applied_ms: i64,
}

impl Seen {
    pub fn is_tool_write_of(&self, who: u32) -> bool {
        self.source == "tool_write"
            && self.attribution == "tool_write"
            && self.confidence == "certain"
            && self.actor.as_deref() == Some(user(who).to_string().as_str())
    }

    pub fn is_unattributed(&self) -> bool {
        self.attribution == "unknown" && self.confidence == "low" && self.actor.is_none()
    }
}

/// The agent's side of a swimlane: one delta after another, each at a time of its own (the agent's clock, which the registry never uses
/// to decide what a registration explains).
pub struct Script<'a> {
    pub rig: &'a Rig,
    seq: u64,
    n: u8,
    at: i64,
}

impl<'a> Script<'a> {
    /// Say hello and deliver `files` as the first full scan (the swimlane's inventory), observed at `T`.
    pub async fn start(rig: &'a Rig, files: &[(&str, &str)]) -> Script<'a> {
        rig.first_scan(files).await;
        Script {
            rig,
            seq: 1,
            n: 1,
            at: T,
        }
    }

    fn next(&mut self) -> (u64, ContentHash, ContentHash, i64) {
        let base = root(self.n);
        self.seq += 1;
        self.n += 1;
        self.at += 1_000;
        (self.seq, base, root(self.n), self.at)
    }

    /// These files changed on NFS (path, content).
    pub async fn change(&mut self, changes: &[(&str, &str)]) {
        let (seq, base, new, at) = self.next();
        let entries = changes.iter().map(|(p, c)| entry(p, c.as_bytes(), at)).collect();
        self.rig
            .delta(delta(seq, Some(base), new, entries))
            .await
            .unwrap();
    }

    /// One file changed on NFS, as the agent saw it at `observed_at_ms` (its own clock).
    pub async fn change_seen_at(&mut self, p: &str, content: &str, observed_at_ms: i64) {
        let (seq, base, new, _) = self.next();
        self.at = self.at.max(observed_at_ms);
        self.rig
            .delta(delta(
                seq,
                Some(base),
                new,
                vec![entry(p, content.as_bytes(), observed_at_ms)],
            ))
            .await
            .unwrap();
    }

    /// These files were deleted on NFS.
    pub async fn delete(&mut self, paths: &[&str]) {
        let (seq, base, new, _) = self.next();
        let mut d = delta(seq, Some(base), new, Vec::new());
        d.removed = paths.iter().map(|p| path(p)).collect();
        self.rig.delta(d).await.unwrap();
    }
}
