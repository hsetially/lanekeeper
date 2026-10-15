//! The `proposals` and `approvals` tables: inserting a pending proposal, reading proposals, and the transitions of their status.
//!
//! A proposal moves one way: `pending` to `applied`, `stale`, `rejected` or `expired`, and never back. Every transition is an
//! `UPDATE ... WHERE status = 'pending'` on a row the transaction has locked, so two deciders cannot both win, and the decision
//! is written once to `approvals` (the application role has no UPDATE or DELETE on it, migration 0004).

use std::str::FromStr;

use domain::{ContentHash, Guid, ProposalId, ProposalStatus, RequestId, SwimlaneId, UserId, Via};
use sqlx::{PgConnection, Row};

use crate::db::{ms, ts};
use crate::error::{Internal, Res};

/// A proposal to insert.
pub(crate) struct NewProposal<'a> {
    pub author: &'a UserId,
    /// One of the table's actions: `edit`, `upload`, `delete`, `revert`, `restart`, `pr`, `notify`.
    pub action: &'static str,
    pub swimlane: &'a SwimlaneId,
    pub paths: Vec<String>,
    /// Path to the hash the author expected (hex), or `"absent"`.
    pub base_hashes: serde_json::Value,
    pub new_hash: Option<ContentHash>,
    pub params: serde_json::Value,
    pub via: Via,
    pub request_id: &'a RequestId,
    pub created_ms: i64,
    pub expires_ms: i64,
}

const INSERT: &str = concat!(
    "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, base_hashes, new_hash, params, via, status, \
     request_id, created_at, expires_at) \
     VALUES ($1, $2, $3, $4, $5::text[], $6, $7, $8, $9, 'pending', $10, ",
    ts!("11"),
    ", ",
    ts!("12"),
    ") RETURNING id"
);

pub(crate) const INSERT_SQL: &str = INSERT;

/// A proposal keeps the version it proposes for as long as it lives (D78).
const BLOB_REF: &str = "INSERT INTO blob_refs (hash, ref_kind, ref_id) SELECT hash, 'proposal', $2 FROM blobs WHERE hash = $1 ON CONFLICT DO NOTHING";

pub(crate) async fn insert(conn: &mut PgConnection, p: &NewProposal<'_>) -> Res<ProposalId> {
    let id: i64 = sqlx::query_scalar(INSERT)
        .bind(p.author.tid().as_str())
        .bind(p.author.oid().as_str())
        .bind(p.action)
        .bind(p.swimlane.as_str())
        .bind(&p.paths)
        .bind(&p.base_hashes)
        .bind(p.new_hash.as_ref().map(|h| h.as_bytes().to_vec()))
        .bind(&p.params)
        .bind(p.via.as_str())
        .bind(p.request_id.as_str())
        .bind(p.created_ms)
        .bind(p.expires_ms)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Internal::db("proposal_insert", &e))?;
    if let Some(h) = &p.new_hash {
        sqlx::query(BLOB_REF)
            .bind(h.as_bytes().as_slice())
            .bind(id.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| Internal::db("proposal_blob_ref", &e))?;
    }
    Ok(ProposalId::from(id))
}

// ------------------------------------------------------------------------------------------------------------- reading

/// The decision on a proposal, from `approvals`.
#[derive(Debug, Clone)]
pub(crate) struct Decision {
    pub approver: UserId,
    pub comment: Option<String>,
}

/// What a proposal's author expected to find at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Base {
    /// The file was not there.
    Absent,
    /// The file had this hash.
    At(ContentHash),
}

/// A proposal as the tables hold it.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub id: ProposalId,
    pub author: UserId,
    /// `edit`, `upload`, `delete`, `revert`, `restart`, `pr` or `notify`.
    pub action: String,
    pub swimlane: SwimlaneId,
    pub paths: Vec<String>,
    pub base_hashes: serde_json::Value,
    pub new_hash: Option<ContentHash>,
    pub params: serde_json::Value,
    pub status: ProposalStatus,
    pub created_ms: i64,
    pub expires_ms: i64,
    pub decision: Option<Decision>,
}

impl Stored {
    /// What the author expected at `path`, if the proposal says.
    pub fn expected_at(&self, path: &str) -> Option<Base> {
        match self.base_hashes.get(path)?.as_str()? {
            "absent" => Some(Base::Absent),
            hex => ContentHash::parse(hex).ok().map(Base::At),
        }
    }

    /// The draft this proposal came from, when it came from one.
    pub fn draft(&self) -> Option<String> {
        self.params
            .get("draft")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }
}

/// The select list and the join every read of a proposal uses.
macro_rules! select_proposal {
    () => {
        concat!(
            "SELECT p.id, p.author_tid, p.author_oid, p.action, p.swimlane_id, p.paths, p.base_hashes, p.new_hash, p.params, \
             p.status, ",
            ms!("p.created_at"),
            " AS created_ms, ",
            ms!("p.expires_at"),
            " AS expires_ms, a.decision, a.approver_tid, a.approver_oid, a.reason \
             FROM proposals p LEFT JOIN approvals a ON a.proposal_id = p.id"
        )
    };
}

/// One proposal, with its decision.
pub(crate) const GET_SQL: &str = concat!(select_proposal!(), " WHERE p.id = $1");

/// A page of proposals, newest first (keyset on the id): `$1` the id to start below (or null), `$2` the status (or null), `$3`
/// the swimlane (or null), `$4` the most rows.
pub(crate) const LIST_SQL: &str = concat!(
    select_proposal!(),
    " WHERE ($1::bigint IS NULL OR p.id < $1) AND ($2::text IS NULL OR p.status = $2) \
     AND ($3::text IS NULL OR p.swimlane_id = $3) ORDER BY p.id DESC LIMIT $4"
);

fn user_of(tid: &str, oid: &str) -> Option<UserId> {
    Some(UserId::new(Guid::parse(tid).ok()?, Guid::parse(oid).ok()?))
}

fn read(row: &sqlx::postgres::PgRow) -> Option<Stored> {
    let id: i64 = row.try_get("id").ok()?;
    let tid: String = row.try_get("author_tid").ok()?;
    let oid: String = row.try_get("author_oid").ok()?;
    let status: String = row.try_get("status").ok()?;
    let new_hash: Option<Vec<u8>> = row.try_get("new_hash").ok()?;
    let decision = if row.try_get::<Option<String>, _>("decision").ok()?.is_some() {
        let approver = user_of(
            &row.try_get::<String, _>("approver_tid").ok()?,
            &row.try_get::<String, _>("approver_oid").ok()?,
        )?;
        Some(Decision {
            approver,
            comment: row.try_get("reason").ok()?,
        })
    } else {
        None
    };
    Some(Stored {
        id: ProposalId::from(id),
        author: user_of(&tid, &oid)?,
        action: row.try_get("action").ok()?,
        swimlane: SwimlaneId::parse(&row.try_get::<String, _>("swimlane_id").ok()?).ok()?,
        paths: row.try_get("paths").ok()?,
        base_hashes: row.try_get("base_hashes").ok()?,
        new_hash: match new_hash {
            Some(b) => Some(ContentHash::from_bytes(b.try_into().ok()?)),
            None => None,
        },
        params: row.try_get("params").ok()?,
        status: ProposalStatus::from_str(&status).ok()?,
        created_ms: row.try_get("created_ms").ok()?,
        expires_ms: row.try_get("expires_ms").ok()?,
        decision,
    })
}

pub(crate) async fn get(conn: &mut PgConnection, id: ProposalId) -> Res<Option<Stored>> {
    let row = sqlx::query(GET_SQL)
        .bind(id.get())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("proposal_get", &e))?;
    match row {
        None => Ok(None),
        Some(row) => read(&row)
            .map(Some)
            .ok_or_else(|| Internal::new("proposal_get", "row")),
    }
}

/// Filters of a list.
pub(crate) struct Filter<'a> {
    pub below: Option<i64>,
    pub status: Option<ProposalStatus>,
    pub swimlane: Option<&'a SwimlaneId>,
}

/// At most `limit` proposals matching `filter`, newest first. A row that does not read is left out and logged.
pub(crate) async fn list(conn: &mut PgConnection, filter: &Filter<'_>, limit: i64) -> Res<Vec<Stored>> {
    let rows = sqlx::query(LIST_SQL)
        .bind(filter.below)
        .bind(filter.status.map(ProposalStatus::as_str))
        .bind(filter.swimlane.map(SwimlaneId::as_str))
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| Internal::db("proposal_list", &e))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        if let Some(p) = read(row) {
            out.push(p);
        } else {
            tracing::warn!("a proposal row could not be read");
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------- transitions

/// The status of a proposal, locked until the transaction ends.
pub(crate) const LOCK_SQL: &str = "SELECT status FROM proposals WHERE id = $1 FOR UPDATE";

pub(crate) async fn lock_status(conn: &mut PgConnection, id: ProposalId) -> Res<Option<ProposalStatus>> {
    let status: Option<String> = sqlx::query_scalar(LOCK_SQL)
        .bind(id.get())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("proposal_lock", &e))?;
    match status {
        None => Ok(None),
        Some(s) => ProposalStatus::from_str(&s)
            .map(Some)
            .map_err(|_| Internal::new("proposal_lock", "status")),
    }
}

const SET_STATUS: &str = concat!(
    "UPDATE proposals SET status = $2, applied_at = CASE WHEN $2 = 'applied' THEN ",
    ts!("3"),
    " ELSE applied_at END WHERE id = $1 AND status = 'pending'"
);

/// Move a pending proposal to `to`. Returns whether it moved: false means it was not pending (the caller holds the row lock, so
/// this is a defect of the caller, not a race).
pub(crate) async fn set_status(
    conn: &mut PgConnection,
    id: ProposalId,
    to: ProposalStatus,
    now_ms: i64,
) -> Res<bool> {
    let done = sqlx::query(SET_STATUS)
        .bind(id.get())
        .bind(to.as_str())
        .bind(now_ms)
        .execute(&mut *conn)
        .await
        .map_err(|e| Internal::db("proposal_status", &e))?;
    Ok(done.rows_affected() == 1)
}

const INSERT_DECISION: &str = concat!(
    "INSERT INTO approvals (proposal_id, decision, approver_tid, approver_oid, reason, rechecked_hash, decided_at) \
     VALUES ($1, $2, $3, $4, $5, $6, ",
    ts!("7"),
    ")"
);

/// What an admin decided, to be written once.
pub(crate) struct NewDecision<'a> {
    pub proposal: ProposalId,
    pub approved: bool,
    pub approver: &'a UserId,
    pub comment: Option<&'a str>,
    /// The hash found at the path when the approval re-checked it.
    pub rechecked: Option<ContentHash>,
    pub at_ms: i64,
}

pub(crate) async fn insert_decision(conn: &mut PgConnection, d: &NewDecision<'_>) -> Res<()> {
    sqlx::query(INSERT_DECISION)
        .bind(d.proposal.get())
        .bind(if d.approved { "approved" } else { "rejected" })
        .bind(d.approver.tid().as_str())
        .bind(d.approver.oid().as_str())
        .bind(d.comment)
        .bind(d.rechecked.as_ref().map(|h| h.as_bytes().to_vec()))
        .bind(d.at_ms)
        .execute(&mut *conn)
        .await
        .map_err(|e| Internal::db("approval_insert", &e))?;
    Ok(())
}

/// Pending proposals whose time has run out, oldest first: `$1` is now, `$2` the most rows. Read through the partial index on
/// `expires_at`.
pub(crate) const EXPIRED_SQL: &str = concat!(
    "SELECT id FROM proposals WHERE status = 'pending' AND expires_at <= ",
    ts!("1"),
    " ORDER BY expires_at LIMIT $2"
);

pub(crate) async fn expired(conn: &mut PgConnection, now_ms: i64, limit: i64) -> Res<Vec<ProposalId>> {
    let ids: Vec<i64> = sqlx::query_scalar(EXPIRED_SQL)
        .bind(now_ms)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| Internal::db("proposal_expired", &e))?;
    Ok(ids.into_iter().map(ProposalId::from).collect())
}
