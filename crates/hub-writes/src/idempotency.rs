//! Idempotency (D68): a write request carries a key, and a retry with the same key and the same request returns the stored
//! response and writes nothing more. The same key with another request is refused.
//!
//! The row of a request is made first (the **claim**), before anything is sent, so two requests with one key cannot both run.
//! While the request runs the row has no response; it holds the *intent* instead, which is what a later request needs to
//! finish a write that landed on NFS while the transaction that records it did not commit (plan A1). When the request ends,
//! the response replaces the intent, in the same transaction that records the audit event, or the row is removed when what
//! happened is not an outcome (a lock, an agent that could not be reached).
//!
//! The request hash is SHA-256 over an action tag and length-prefixed fields; the content is hashed first and only its digest
//! goes in, so the row holds nothing of the request (S10).

use std::fmt;

use domain::{
    ContentHash, DeleteRequest, EditRequest, Expected, NfsPath, RequestId, RestartRequest, RevertRequest,
    SwimlaneId, UploadRequest, UserId, ValidationIssue, WriteCtx, WriteOutcome,
};
use ports::AuditEvent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::db::{ms, ts};
use crate::error::{Internal, Res};

/// SHA-256 over the parts of a request that make it the same request.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RequestHash([u8; 32]);

impl RequestHash {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// For the benchmark of the statements (`crate::probe`): a hash that depends only on `n`.
    pub(crate) fn probe(n: u64) -> Self {
        let mut b = HashBuilder::new("probe/1");
        b.field(&n.to_be_bytes());
        b.finish()
    }

    /// The hash of an `edit`: the swimlane, the path, what is expected there, a digest of the content, and the line ending
    /// style asked for.
    pub(crate) fn of_edit(r: &EditRequest) -> Self {
        let mut b = HashBuilder::new("edit/1");
        b.field(r.swimlane.as_str().as_bytes());
        b.field(r.path.as_str().as_bytes());
        b.expected(&r.expected);
        b.field(&Sha256::digest(&r.content));
        b.field(r.change_eol.map_or("", |e| e.as_str()).as_bytes());
        b.finish()
    }

    /// The hash of an `upload`: what an `edit` has, without the line ending style. The preview token is not part of it: it
    /// proves the upload was confirmed, it does not make it another upload, so a retry with a fresh token is the same request.
    pub(crate) fn of_upload(r: &UploadRequest) -> Self {
        let mut b = HashBuilder::new("upload/1");
        b.field(r.swimlane.as_str().as_bytes());
        b.field(r.path.as_str().as_bytes());
        b.expected(&r.expected);
        b.field(&Sha256::digest(&r.content));
        b.finish()
    }

    /// The hash of a `delete`: the swimlane, the path and the hash being deleted.
    pub(crate) fn of_delete(r: &DeleteRequest) -> Self {
        let mut b = HashBuilder::new("delete/1");
        b.field(r.swimlane.as_str().as_bytes());
        b.field(r.path.as_str().as_bytes());
        b.field(r.expected.as_bytes());
        b.finish()
    }

    /// The hash of a `revert`: the swimlane, the path, what is expected there, the version restored and the style asked for.
    pub(crate) fn of_revert(r: &RevertRequest) -> Self {
        let mut b = HashBuilder::new("revert/1");
        b.field(r.swimlane.as_str().as_bytes());
        b.field(r.path.as_str().as_bytes());
        b.expected(&r.expected);
        b.field(r.to.as_bytes());
        b.field(r.change_eol.map_or("", |e| e.as_str()).as_bytes());
        b.finish()
    }

    /// The hash of a `restart`: the swimlane and the service.
    pub(crate) fn of_restart(r: &RestartRequest) -> Self {
        let mut b = HashBuilder::new("restart/1");
        b.field(r.swimlane.as_str().as_bytes());
        b.field(r.service.namespace().as_bytes());
        b.field(r.service.name().as_bytes());
        b.finish()
    }
}

impl fmt::Debug for RequestHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RequestHash({})", ContentHash::from_bytes(self.0))
    }
}

/// Builds a [`RequestHash`]. Every field is preceded by its length, so no two different lists of fields give the same bytes.
pub(crate) struct HashBuilder(Sha256);

impl HashBuilder {
    pub fn new(tag: &str) -> Self {
        let mut b = Self(Sha256::new());
        b.field(tag.as_bytes());
        b
    }

    pub fn field(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
        self
    }

    pub fn expected(&mut self, e: &Expected) -> &mut Self {
        match e {
            Expected::Absent => self.field(b"absent"),
            Expected::Hash { hash } => self.field(b"hash").field(hash.as_bytes()),
        }
    }

    pub fn finish(self) -> RequestHash {
        RequestHash(self.0.finalize().into())
    }
}

/// Who sent which key.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ClaimKey {
    tid: String,
    oid: String,
    key: String,
}

impl ClaimKey {
    pub fn of(ctx: &WriteCtx) -> Self {
        Self {
            tid: ctx.user.id.tid().as_str().to_owned(),
            oid: ctx.user.id.oid().as_str().to_owned(),
            key: ctx.idempotency_key.as_str().to_owned(),
        }
    }

    fn from_row(tid: String, oid: String, key: String) -> Self {
        Self { tid, oid, key }
    }

    /// For the benchmark of the statements (`crate::probe`).
    pub(crate) fn probe(user: &UserId, key: &str) -> Self {
        Self {
            tid: user.tid().as_str().to_owned(),
            oid: user.oid().as_str().to_owned(),
            key: key.to_owned(),
        }
    }
}

impl fmt::Debug for ClaimKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaimKey").finish_non_exhaustive()
    }
}

/// How far a request got when it left its claim unfinished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    /// The command may be on its way, and the request that sent it may still be running.
    Sent,
    /// The request that sent it has given up without knowing whether it landed. Nobody is running it any more.
    Ambiguous,
}

/// What the sent command did to the file, which is how a later request finds out whether it landed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IntentOp {
    /// A file was written: it landed when the file has `new_hash`.
    #[default]
    Write,
    /// A file was deleted: it landed when the file is gone. `new_hash` is the hash that was deleted.
    Delete,
}

/// What a request sent to the agent and what recording it would write: enough to finish the write without the request.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Intent {
    pub v: u8,
    pub phase: Phase,
    /// Absent in an intent written before deletes existed, which is a write.
    #[serde(default)]
    pub op: IntentOp,
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub new_hash: ContentHash,
    pub user: UserId,
    pub request_id: RequestId,
    /// The audit event the write will record. Its diff is already redacted.
    pub event: AuditEvent,
    pub warnings: Vec<ValidationIssue>,
}

impl Intent {
    pub const VERSION: u8 = 1;
}

impl fmt::Debug for Intent {
    // The event holds a diff of the file: the intent prints ids and the phase only (S10).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Intent")
            .field("phase", &self.phase)
            .field("swimlane", &self.swimlane)
            .field("path", &self.path)
            .field("new_hash", &self.new_hash)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

/// What the table says about a key.
pub(crate) enum Claim {
    /// The row is ours: nobody has used the key.
    Fresh,
    /// Another request used the key for this request and finished: its response.
    Replay(WriteOutcome),
    /// Another request used the key for another request.
    Conflict,
    /// The same request is running (or was abandoned): the intent it has left, if any, and when it began.
    InFlight { created_ms: i64, body: Option<Vec<u8>> },
}

/// A row of an unfinished claim, for the reconcile job.
pub(crate) struct Unfinished {
    pub key: ClaimKey,
    pub body: Option<Vec<u8>>,
}

const CLAIM: &str = concat!(
    "INSERT INTO idempotency_keys (user_tid, user_oid, key, request_hash, created_at, expires_at) \
     VALUES ($1, $2, $3, $4, ",
    ts!("5"),
    ", ",
    ts!("6"),
    ") ON CONFLICT (user_tid, user_oid, key) DO NOTHING RETURNING 1"
);

const LOOKUP: &str = concat!(
    "SELECT request_hash, response_status, response_body, ",
    ms!("created_at"),
    " AS created_ms, ",
    ms!("expires_at"),
    " AS expires_ms FROM idempotency_keys WHERE user_tid = $1 AND user_oid = $2 AND key = $3"
);

/// Removes a finished row that has expired.
const DROP_EXPIRED: &str = concat!(
    "DELETE FROM idempotency_keys WHERE user_tid = $1 AND user_oid = $2 AND key = $3 \
     AND response_status IS NOT NULL AND expires_at <= ",
    ts!("4")
);

const SET_INTENT: &str = "UPDATE idempotency_keys SET response_body = $4 \
     WHERE user_tid = $1 AND user_oid = $2 AND key = $3 AND response_status IS NULL";

const FINISH: &str = "UPDATE idempotency_keys SET response_status = $4, response_body = $5 \
     WHERE user_tid = $1 AND user_oid = $2 AND key = $3 AND response_status IS NULL";

const RELEASE: &str = "DELETE FROM idempotency_keys \
     WHERE user_tid = $1 AND user_oid = $2 AND key = $3 AND response_status IS NULL";

/// Unfinished claims whose `expires_at` is before `$1`. Every claim expires `idempotency_ttl` after it began, so this is "began
/// more than `stale_after` ago" when `$1` is now minus `stale_after` plus the ttl, read through the index on `expires_at`.
pub(crate) const STALE_CLAIMS: &str = concat!(
    "SELECT user_tid, user_oid, key, response_body FROM idempotency_keys \
     WHERE response_status IS NULL AND expires_at < ",
    ts!("1"),
    " ORDER BY expires_at LIMIT $2"
);

/// Finished rows that have expired, oldest first, with their primary key.
const PURGE: &str = concat!(
    "DELETE FROM idempotency_keys k USING ( \
       SELECT user_tid, user_oid, key FROM idempotency_keys \
       WHERE response_status IS NOT NULL AND expires_at < ",
    ts!("1"),
    " ORDER BY expires_at LIMIT $2) d \
     WHERE k.user_tid = d.user_tid AND k.user_oid = d.user_oid AND k.key = d.key"
);

/// The row lock a transaction takes before it records a write: the claim must still be unfinished.
pub(crate) const LOCK_CLAIM: &str = "SELECT response_status, response_body FROM idempotency_keys \
     WHERE user_tid = $1 AND user_oid = $2 AND key = $3 FOR UPDATE";

/// The statement that completes a claim inside the recording transaction.
pub(crate) const FINISH_IN_TX: &str = FINISH;

pub(crate) fn queries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("idempotency_claim", CLAIM),
        ("idempotency_lookup", LOOKUP),
        ("idempotency_lock_claim", LOCK_CLAIM),
        ("idempotency_stale_claims", STALE_CLAIMS),
        ("idempotency_purge", PURGE),
    ]
}

/// The status stored with a response, for people reading the table: the HTTP status the outcome maps to.
pub(crate) fn status_of(o: &WriteOutcome) -> i32 {
    match o {
        WriteOutcome::Applied { .. } => 200,
        WriteOutcome::ProposalCreated { .. } => 202,
        WriteOutcome::Conflict { .. } => 409,
        WriteOutcome::Locked { .. } => 423,
        WriteOutcome::Rejected { .. } => 422,
    }
}

#[derive(Clone)]
pub(crate) struct IdempotencyStore {
    pool: PgPool,
}

impl fmt::Debug for IdempotencyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdempotencyStore").finish_non_exhaustive()
    }
}

impl IdempotencyStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Try to make the row. [`Claim::Fresh`] means it is ours. An expired, finished row is removed and the claim is tried again.
    pub async fn claim(&self, key: &ClaimKey, hash: &RequestHash, now_ms: i64, ttl_ms: i64) -> Res<Claim> {
        for _ in 0..3 {
            let made: Option<i32> = sqlx::query_scalar(CLAIM)
                .bind(&key.tid)
                .bind(&key.oid)
                .bind(&key.key)
                .bind(hash.as_bytes().as_slice())
                .bind(now_ms)
                .bind(now_ms.saturating_add(ttl_ms))
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| Internal::db("claim", &e))?;
            if made.is_some() {
                return Ok(Claim::Fresh);
            }
            let Some(row) = self.lookup(key).await? else {
                // Released between the insert and the read: try again.
                continue;
            };
            if row.status.is_some() && row.expires_ms <= now_ms {
                sqlx::query(DROP_EXPIRED)
                    .bind(&key.tid)
                    .bind(&key.oid)
                    .bind(&key.key)
                    .bind(now_ms)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| Internal::db("claim", &e))?;
                continue;
            }
            return Ok(Self::classify(row, hash));
        }
        Err(Internal::new("claim", "contention"))
    }

    /// What the row says now, for a request that is waiting on it.
    pub async fn peek(&self, key: &ClaimKey, hash: &RequestHash) -> Res<Option<Claim>> {
        Ok(self.lookup(key).await?.map(|row| Self::classify(row, hash)))
    }

    fn classify(row: Row, hash: &RequestHash) -> Claim {
        if row.request_hash != hash.as_bytes().as_slice() {
            return Claim::Conflict;
        }
        match (row.status, row.body) {
            (Some(_), Some(body)) => match serde_json::from_slice::<WriteOutcome>(&body) {
                Ok(o) => Claim::Replay(o),
                // A stored response that does not read is a defect of ours: do not guess, ask the caller to retry later.
                Err(_) => Claim::InFlight {
                    created_ms: row.created_ms,
                    body: None,
                },
            },
            (_, body) => Claim::InFlight {
                created_ms: row.created_ms,
                body,
            },
        }
    }

    async fn lookup(&self, key: &ClaimKey) -> Res<Option<Row>> {
        let row: Option<LookupRow> = sqlx::query_as(LOOKUP)
            .bind(&key.tid)
            .bind(&key.oid)
            .bind(&key.key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Internal::db("claim_lookup", &e))?;
        Ok(
            row.map(|(request_hash, status, body, created_ms, expires_ms)| Row {
                request_hash,
                status,
                body,
                created_ms,
                expires_ms,
            }),
        )
    }

    /// Keep the intent with the claim, before the command is sent.
    pub async fn set_intent(&self, key: &ClaimKey, intent: &Intent) -> Res<()> {
        let body = serde_json::to_vec(intent).map_err(|_| Internal::new("intent", "encode"))?;
        sqlx::query(SET_INTENT)
            .bind(&key.tid)
            .bind(&key.oid)
            .bind(&key.key)
            .bind(body)
            .execute(&self.pool)
            .await
            .map_err(|e| Internal::db("intent", &e))?;
        Ok(())
    }

    /// Store the response of a request that changed nothing but has an answer worth replaying (a conflict, a rejection).
    pub async fn finish(&self, key: &ClaimKey, outcome: &WriteOutcome) -> Res<()> {
        let body = serde_json::to_vec(outcome).map_err(|_| Internal::new("finish", "encode"))?;
        sqlx::query(FINISH)
            .bind(&key.tid)
            .bind(&key.oid)
            .bind(&key.key)
            .bind(status_of(outcome))
            .bind(body)
            .execute(&self.pool)
            .await
            .map_err(|e| Internal::db("finish", &e))?;
        Ok(())
    }

    /// Remove an unfinished claim: what happened is not an outcome, so the key is free for the next try.
    pub async fn release(&self, key: &ClaimKey) -> Res<()> {
        sqlx::query(RELEASE)
            .bind(&key.tid)
            .bind(&key.oid)
            .bind(&key.key)
            .execute(&self.pool)
            .await
            .map_err(|e| Internal::db("release", &e))?;
        Ok(())
    }

    /// Unfinished claims that began before `now_ms - stale_after_ms`, oldest first, at most `limit`.
    pub async fn stale(
        &self,
        now_ms: i64,
        stale_after_ms: i64,
        ttl_ms: i64,
        limit: i64,
    ) -> Res<Vec<Unfinished>> {
        let cutoff = now_ms.saturating_sub(stale_after_ms).saturating_add(ttl_ms);
        let rows: Vec<(String, String, String, Option<Vec<u8>>)> = sqlx::query_as(STALE_CLAIMS)
            .bind(cutoff)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Internal::db("stale_claims", &e))?;
        Ok(rows
            .into_iter()
            .map(|(tid, oid, key, body)| Unfinished {
                key: ClaimKey::from_row(tid, oid, key),
                body,
            })
            .collect())
    }

    /// Delete finished rows that expired before `now_ms`, at most `limit`. An unfinished row is never deleted here: it may be
    /// a write that landed and has not been audited yet.
    pub async fn purge(&self, now_ms: i64, limit: i64) -> Res<u64> {
        let done = sqlx::query(PURGE)
            .bind(now_ms)
            .bind(limit)
            .execute(&self.pool)
            .await
            .map_err(|e| Internal::db("purge", &e))?;
        Ok(done.rows_affected())
    }
}

/// `request_hash`, `response_status`, `response_body`, `created_ms`, `expires_ms`.
type LookupRow = (Vec<u8>, Option<i32>, Option<Vec<u8>>, i64, i64);

struct Row {
    request_hash: Vec<u8>,
    status: Option<i32>,
    body: Option<Vec<u8>>,
    created_ms: i64,
    expires_ms: i64,
}

/// Lock the claim of the transaction and read its state: `Some(outcome)` when it is already finished (the transaction has
/// nothing to record), `None` when it is still unfinished and now locked until the transaction ends.
pub(crate) async fn lock_claim(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    key: &ClaimKey,
) -> Res<Option<WriteOutcome>> {
    let row: Option<(Option<i32>, Option<Vec<u8>>)> = sqlx::query_as(LOCK_CLAIM)
        .bind(&key.tid)
        .bind(&key.oid)
        .bind(&key.key)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| Internal::db("lock_claim", &e))?;
    match row {
        // The claim was cleared while this write was on its way: there is nothing to attach the outcome to. The caller
        // reports the failure; the write itself is on NFS and is found by the next scan.
        None => Err(Internal::new("lock_claim", "claim missing")),
        Some((None, _)) => Ok(None),
        Some((Some(_), Some(body))) => serde_json::from_slice(&body)
            .map(Some)
            .map_err(|_| Internal::new("lock_claim", "stored response")),
        Some((Some(_), None)) => Err(Internal::new("lock_claim", "stored response")),
    }
}

/// Store the response of a claim inside the transaction that recorded the change.
pub(crate) async fn finish_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    key: &ClaimKey,
    outcome: &WriteOutcome,
) -> Res<()> {
    let body = serde_json::to_vec(outcome).map_err(|_| Internal::new("finish", "encode"))?;
    let done = sqlx::query(FINISH_IN_TX)
        .bind(&key.tid)
        .bind(&key.oid)
        .bind(&key.key)
        .bind(status_of(outcome))
        .bind(body)
        .execute(&mut **tx)
        .await
        .map_err(|e| Internal::db("finish", &e))?;
    if done.rows_affected() == 1 {
        Ok(())
    } else {
        Err(Internal::new("finish", "claim changed"))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use proptest::prelude::*;

    use super::*;

    fn edit(swimlane: &str, path: &str, expected: Expected, content: &[u8]) -> EditRequest {
        EditRequest {
            swimlane: SwimlaneId::parse(swimlane).unwrap(),
            path: NfsPath::parse(path).unwrap(),
            expected,
            content: Bytes::copy_from_slice(content),
            change_eol: None,
        }
    }

    #[test]
    fn the_hash_changes_with_every_part_of_the_request() {
        let base = edit("sit1", "a.yml", Expected::Absent, b"x");
        let h0 = RequestHash::of_edit(&base);
        assert_eq!(h0, RequestHash::of_edit(&base.clone()));
        let others = [
            edit("sit2", "a.yml", Expected::Absent, b"x"),
            edit("sit1", "b.yml", Expected::Absent, b"x"),
            edit("sit1", "a.yml", Expected::Absent, b"y"),
            edit(
                "sit1",
                "a.yml",
                Expected::Hash {
                    hash: ContentHash::from_bytes([0; 32]),
                },
                b"x",
            ),
            EditRequest {
                change_eol: Some(domain::ChangeEol::Crlf),
                ..base.clone()
            },
        ];
        for o in others {
            assert_ne!(h0, RequestHash::of_edit(&o));
        }
    }

    proptest! {
        /// Moving a byte from one field to the next changes the hash: the fields have boundaries.
        #[test]
        fn field_boundaries_cannot_collide(a in proptest::collection::vec(any::<u8>(), 1..40), b in proptest::collection::vec(any::<u8>(), 0..40), cut in 0usize..40) {
            let cut = cut % a.len();
            let (left, moved) = a.split_at(cut);
            let mut one = HashBuilder::new("t");
            one.field(&a).field(&b);
            let mut other_b = moved.to_vec();
            other_b.extend_from_slice(&b);
            let mut two = HashBuilder::new("t");
            two.field(left).field(&other_b);
            // The two lists carry the same bytes in the same order, cut in different places.
            prop_assume!(!moved.is_empty());
            prop_assert_ne!(one.finish(), two.finish());
        }

        #[test]
        fn the_tag_is_part_of_the_hash(f in proptest::collection::vec(any::<u8>(), 0..40)) {
            let mut a = HashBuilder::new("edit/1");
            a.field(&f);
            let mut b = HashBuilder::new("upload/1");
            b.field(&f);
            prop_assert_ne!(a.finish(), b.finish());
        }
    }

    #[test]
    fn debug_of_the_intent_and_the_hash_show_no_content() {
        let mut ev = ports::AuditEvent::new(
            domain::Timestamp::from_unix_millis(1),
            ports::AuditActor::System {
                component: domain::ShortText::parse("t").unwrap(),
            },
            ports::AuditAction::FileEdited,
            ports::AuditVia::Ui,
        );
        ev.diff = Some("-secret-text-marker".to_owned());
        let intent = Intent {
            v: Intent::VERSION,
            phase: Phase::Sent,
            op: IntentOp::Write,
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            path: NfsPath::parse("a.yml").unwrap(),
            new_hash: ContentHash::from_bytes([1; 32]),
            user: ports::conformance::sample::user_id(1),
            request_id: RequestId::parse("r").unwrap(),
            event: ev,
            warnings: Vec::new(),
        };
        assert!(!format!("{intent:?}").contains("secret-text-marker"));
        let key = ClaimKey {
            tid: "t".into(),
            oid: "o".into(),
            key: "private-key-marker".into(),
        };
        assert!(!format!("{key:?}").contains("private-key-marker"));
    }
}
