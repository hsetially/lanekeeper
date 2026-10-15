//! The `proposals` table: inserting a pending proposal. Deciding, expiring and listing come with T4.

use domain::{ContentHash, ProposalId, RequestId, SwimlaneId, UserId, Via};
use sqlx::PgConnection;

use crate::db::ts;
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
