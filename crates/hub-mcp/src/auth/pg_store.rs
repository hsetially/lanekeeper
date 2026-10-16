//! [`TokenStore`] over Postgres (`mcp_tokens`, migration 0005; S7, S9, P12).
//!
//! Runs as `lanekeeper_app`: SELECT and INSERT, and UPDATE of `revoked_at` and `last_used_at` only, so the owner, the hash, the label
//! and the end of a token are fixed when the row is written, and nothing deletes one. Statements are runtime-checked (no `query!`),
//! as in `hub-identity`, and every one is bound and bounded: a timeout over the whole unit of work, and a `LIMIT` on the list.
//! A dropped future drops its transaction, which rolls back, so a timeout leaves nothing half-written.
//!
//! **Time.** A new token's `created_at` and `expires_at` come from the database's clock (`now()`), so they satisfy the table's
//! constraints whatever the hub's clock says; a revocation's `revoked_at` does too. Only `last_used_at` takes the hub's `now`.
//!
//! **Audit.** The audit event is recorded last in the transaction (the chain lock is held to the commit) through the caller's
//! [`AuditLog`], on the same connection (rule 8, D80).

use std::time::Duration;

use async_trait::async_trait;
use domain::{Guid, Timestamp, UserId};
use ports::{AuditLog, Tx};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

use super::store::{
    Label, MakeEvent, NewToken, RevokeOutcome, StoreError, TOUCH_AFTER_MILLIS, TokenId, TokenRecord,
    TokenStore,
};
use super::token::TokenHmac;

/// The longest any unit of work takes before it is abandoned.
const DB_TIMEOUT: Duration = Duration::from_secs(5);

/// The columns of a token row, with the times as milliseconds.
const COLUMNS: &str = "id, user_tid, user_oid, token_hmac, label, \
    (extract(epoch from created_at) * 1000)::bigint AS created_ms, \
    (extract(epoch from expires_at) * 1000)::bigint AS expires_ms, \
    (extract(epoch from revoked_at) * 1000)::bigint AS revoked_ms, \
    (extract(epoch from last_used_at) * 1000)::bigint AS used_ms";

/// Tokens in Postgres.
#[derive(Clone)]
pub struct PgTokenStore {
    pool: PgPool,
}

impl std::fmt::Debug for PgTokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgTokenStore").finish_non_exhaustive()
    }
}

impl PgTokenStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The class of a database error, for the log. The text of the error (which can hold a value) is never logged.
fn unavailable(op: &'static str, error: &sqlx::Error) -> StoreError {
    let state = error
        .as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
        .unwrap_or_else(|| "none".to_owned());
    tracing::warn!(op, sqlstate = %state, "token store database error");
    StoreError::Unavailable
}

async fn bounded<T>(
    op: &'static str,
    work: impl Future<Output = Result<T, StoreError>>,
) -> Result<T, StoreError> {
    if let Ok(result) = tokio::time::timeout(DB_TIMEOUT, work).await {
        result
    } else {
        tracing::warn!(op, "token store operation timed out");
        Err(StoreError::Unavailable)
    }
}

/// A row of `mcp_tokens` (selected with [`COLUMNS`]). A stored value that is not valid is a damaged table, not user input.
fn record_from_row(row: &PgRow) -> Result<TokenRecord, StoreError> {
    let damaged = || {
        tracing::warn!("a token row holds a value that is not valid");
        StoreError::Unavailable
    };
    let id: i64 = row.try_get("id").map_err(|_| damaged())?;
    let tid: String = row.try_get("user_tid").map_err(|_| damaged())?;
    let oid: String = row.try_get("user_oid").map_err(|_| damaged())?;
    let hmac: Vec<u8> = row.try_get("token_hmac").map_err(|_| damaged())?;
    let label: String = row.try_get("label").map_err(|_| damaged())?;
    let created: i64 = row.try_get("created_ms").map_err(|_| damaged())?;
    let expires: i64 = row.try_get("expires_ms").map_err(|_| damaged())?;
    let revoked: Option<i64> = row.try_get("revoked_ms").map_err(|_| damaged())?;
    let used: Option<i64> = row.try_get("used_ms").map_err(|_| damaged())?;
    let owner = match (Guid::parse(&tid), Guid::parse(&oid)) {
        (Ok(tid), Ok(oid)) => UserId::new(tid, oid),
        _ => return Err(damaged()),
    };
    let hmac: [u8; 32] = hmac.try_into().map_err(|_| damaged())?;
    Ok(TokenRecord {
        id: TokenId::new(id).ok_or_else(damaged)?,
        owner,
        hmac: TokenHmac::from_bytes(hmac),
        label: Label::parse(&label).map_err(|_| damaged())?,
        created_at: Timestamp::from_unix_millis(created),
        expires_at: Timestamp::from_unix_millis(expires),
        revoked_at: revoked.map(Timestamp::from_unix_millis),
        last_used_at: used.map(Timestamp::from_unix_millis),
    })
}

/// The statement of [`TokenStore::list`]. A function and not a constant so that the test of its plan runs the very same text.
fn list_sql() -> String {
    format!(
        "SELECT {COLUMNS} FROM mcp_tokens \
         WHERE user_tid = $1 AND user_oid = $2 AND ($3::bigint IS NULL OR id < $3) \
         ORDER BY id DESC LIMIT $4"
    )
}

/// The statement of [`TokenStore::find_for_auth`]: read the row, and move `last_used_at` of a live token when it is stale. The
/// write happens at most once per token per ten seconds whatever the number of calls and replicas, and never for a revoked or an
/// expired token.
fn find_sql() -> String {
    format!(
        "WITH hit AS (SELECT {COLUMNS}, last_used_at AS used_at, revoked_at AS rev_at, expires_at AS exp_at \
                      FROM mcp_tokens WHERE token_hmac = $1), \
              touched AS (UPDATE mcp_tokens m SET last_used_at = to_timestamp($2::bigint / 1000.0) FROM hit \
                          WHERE m.id = hit.id AND hit.rev_at IS NULL AND hit.exp_at > to_timestamp($2::bigint / 1000.0) \
                            AND (hit.used_at IS NULL \
                                 OR hit.used_at < to_timestamp(($2::bigint - $3::bigint) / 1000.0)) \
                          RETURNING m.id) \
         SELECT id, user_tid, user_oid, token_hmac, label, created_ms, expires_ms, revoked_ms, used_ms FROM hit"
    )
}

/// The query plans of the two statements that run on every call or every page, for the EXPLAIN tests (P12). Each runs
/// `EXPLAIN (FORMAT JSON)` on exactly the statement the store runs, with the same binds, so a test that checks the plan checks the
/// real query. Nothing in the hub calls these; they write nothing.
pub mod explain {
    use domain::{Timestamp, UserId};
    use sqlx::PgConnection;

    use super::{StoreError, TOUCH_AFTER_MILLIS, TokenHmac, TokenId, find_sql, list_sql, unavailable};

    /// The plan of the lookup by hash.
    pub async fn find(
        conn: &mut PgConnection,
        hmac: &TokenHmac,
        now: Timestamp,
    ) -> Result<serde_json::Value, StoreError> {
        let row: (serde_json::Value,) = sqlx::query_as(&format!("EXPLAIN (FORMAT JSON) {}", find_sql()))
            .bind(&hmac.as_bytes()[..])
            .bind(now.unix_millis())
            .bind(TOUCH_AFTER_MILLIS)
            .fetch_one(conn)
            .await
            .map_err(|e| unavailable("explain find", &e))?;
        Ok(row.0)
    }

    /// The plan of a page of the owner's list.
    pub async fn list(
        conn: &mut PgConnection,
        owner: &UserId,
        before: Option<TokenId>,
        limit: i64,
    ) -> Result<serde_json::Value, StoreError> {
        let row: (serde_json::Value,) = sqlx::query_as(&format!("EXPLAIN (FORMAT JSON) {}", list_sql()))
            .bind(owner.tid().to_string())
            .bind(owner.oid().to_string())
            .bind(before.map(TokenId::get))
            .bind(limit)
            .fetch_one(conn)
            .await
            .map_err(|e| unavailable("explain list", &e))?;
        Ok(row.0)
    }
}

#[async_trait]
impl TokenStore for PgTokenStore {
    async fn create(
        &self,
        new: NewToken,
        audit: &dyn AuditLog,
        event: MakeEvent<'_>,
    ) -> Result<TokenRecord, StoreError> {
        bounded("create token", async {
            let max_live = i64::try_from(new.max_live).unwrap_or(i64::MAX);
            let lifetime_secs = i64::try_from(new.lifetime.as_secs()).unwrap_or(i64::MAX);
            let tid = new.owner.tid().to_string();
            let oid = new.owner.oid().to_string();
            let mut tx = self.pool.begin().await.map_err(|e| unavailable("begin", &e))?;
            // One creation at a time per owner, so that two requests at once cannot both pass the limit below. The lock ends with
            // the transaction.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(format!("lanekeeper.mcp_tokens:{}", new.owner))
                .execute(&mut *tx)
                .await
                .map_err(|e| unavailable("lock owner", &e))?;
            let live: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM mcp_tokens \
                 WHERE user_tid = $1 AND user_oid = $2 AND revoked_at IS NULL AND expires_at > now()",
            )
            .bind(&tid)
            .bind(&oid)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| unavailable("count live tokens", &e))?;
            if live >= max_live {
                return Err(StoreError::LimitReached);
            }
            let row = sqlx::query(&format!(
                "INSERT INTO mcp_tokens (user_tid, user_oid, token_hmac, label, expires_at) \
                 VALUES ($1, $2, $3, $4, now() + ($5::bigint * interval '1 second')) RETURNING {COLUMNS}"
            ))
            .bind(&tid)
            .bind(&oid)
            .bind(&new.hmac.as_bytes()[..])
            .bind(new.label.as_str())
            .bind(lifetime_secs)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| unavailable("insert token", &e))?;
            let record = record_from_row(&row)?;
            // Last: the audit log's chain lock is held until the commit.
            audit
                .record(&mut Tx::from_pg(&mut tx), event(&record))
                .await
                .map_err(|_| StoreError::Unavailable)?;
            tx.commit().await.map_err(|e| unavailable("commit", &e))?;
            Ok(record)
        })
        .await
    }

    async fn revoke(
        &self,
        owner: &UserId,
        id: TokenId,
        audit: &dyn AuditLog,
        event: MakeEvent<'_>,
    ) -> Result<RevokeOutcome, StoreError> {
        bounded("revoke token", async {
            let tid = owner.tid().to_string();
            let oid = owner.oid().to_string();
            let mut tx = self.pool.begin().await.map_err(|e| unavailable("begin", &e))?;
            let revoked = sqlx::query(&format!(
                "UPDATE mcp_tokens SET revoked_at = now() \
                 WHERE id = $1 AND user_tid = $2 AND user_oid = $3 AND revoked_at IS NULL RETURNING {COLUMNS}"
            ))
            .bind(id.get())
            .bind(&tid)
            .bind(&oid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| unavailable("revoke", &e))?;
            if let Some(row) = revoked {
                let record = record_from_row(&row)?;
                audit
                    .record(&mut Tx::from_pg(&mut tx), event(&record))
                    .await
                    .map_err(|_| StoreError::Unavailable)?;
                tx.commit().await.map_err(|e| unavailable("commit", &e))?;
                return Ok(RevokeOutcome::Revoked(record));
            }
            // Nothing was changed. Either the token is the owner's and was revoked already, or it is not theirs (or not there).
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM mcp_tokens WHERE id = $1 AND user_tid = $2 AND user_oid = $3)",
            )
            .bind(id.get())
            .bind(&tid)
            .bind(&oid)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| unavailable("check token", &e))?;
            Ok(if exists {
                RevokeOutcome::AlreadyRevoked
            } else {
                RevokeOutcome::NotFound
            })
        })
        .await
    }

    async fn list(
        &self,
        owner: &UserId,
        before: Option<TokenId>,
        limit: usize,
    ) -> Result<Vec<TokenRecord>, StoreError> {
        bounded("list tokens", async {
            let rows = sqlx::query(&list_sql())
                .bind(owner.tid().to_string())
                .bind(owner.oid().to_string())
                .bind(before.map(TokenId::get))
                .bind(i64::try_from(limit.min(500)).unwrap_or(500))
                .fetch_all(&self.pool)
                .await
                .map_err(|e| unavailable("list tokens", &e))?;
            rows.iter().map(record_from_row).collect()
        })
        .await
    }

    async fn find_for_auth(
        &self,
        hmac: &TokenHmac,
        now: Timestamp,
    ) -> Result<Option<TokenRecord>, StoreError> {
        bounded("find token", async {
            // One statement: read the row, and move `last_used_at` of a live token when it is stale. The write happens at most once
            // per token per ten seconds whatever the number of calls and replicas, and never for a revoked or an expired token.
            let row = sqlx::query(&find_sql())
                .bind(&hmac.as_bytes()[..])
                .bind(now.unix_millis())
                .bind(TOUCH_AFTER_MILLIS)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| unavailable("find token", &e))?;
            row.as_ref().map(record_from_row).transpose()
        })
        .await
    }
}
