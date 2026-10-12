//! The `sessions` table (T3; S2).
//!
//! The session id is 256 random bits from the system generator. The cookie holds it; the table holds its SHA-256, so a copy of
//! the table, a backup or a log of a query is no session. The CSRF token is a second random value, stored the same way.
//!
//! Every statement here is one indexed row, or a bounded batch of them: `session_hash` is the primary key, and `expires_at`
//! and the user are indexed (P12). Nothing selects a whole table.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use domain::{Guid, Role, Secret, Timestamp, UserId, UserStatus};
use sqlx::postgres::{PgConnection, PgRow};
use sqlx::{PgPool, Row};

use super::cookie::{RandomToken, TokenHash};
use super::{SessionError, SessionPolicy};
use crate::clock::Clock;

/// A lookup is a primary-key read. If the database does not answer in this time, the request fails closed.
const DB_TIMEOUT: Duration = Duration::from_secs(3);
/// The most batches one sweep runs, so a huge backlog cannot hold a connection for ever; the next sweep carries on.
const MAX_SWEEP_BATCHES: u32 = 1_000;
/// The widest sweep batch, whatever the caller asks for.
const MAX_SWEEP_BATCH: u32 = 5_000;

/// The cookie values of a session that was just created. Shown to the browser once; the server keeps only their hashes.
pub struct NewSession {
    pub session: Secret<String>,
    pub csrf: Secret<String>,
    /// The hash of the session id: the row's key.
    pub hash: TokenHash,
    pub expires_at: Timestamp,
}

impl std::fmt::Debug for NewSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewSession").finish_non_exhaustive()
    }
}

/// A session row joined with its user's current role and status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub user: UserId,
    pub role: Option<Role>,
    pub status: UserStatus,
    pub csrf_hash: TokenHash,
    pub last_seen_at: Timestamp,
    pub expires_at: Timestamp,
}

/// What a sweep removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    pub deleted: u64,
    pub batches: u32,
}

/// The statement behind [`SessionStore::lookup`]. Kept as one constant so the EXPLAIN test explains the real query.
const LOOKUP_SQL: &str = "SELECT s.user_tid, s.user_oid, s.csrf_hash, \
     (extract(epoch from s.last_seen_at) * 1000)::bigint AS last_seen_ms, \
     (extract(epoch from s.expires_at) * 1000)::bigint AS expires_ms, u.role, u.status \
     FROM sessions s JOIN users u ON u.tid = s.user_tid AND u.oid = s.user_oid WHERE s.session_hash = $1";

#[derive(Clone)]
pub struct SessionStore {
    pool: PgPool,
    clock: Arc<dyn Clock>,
    absolute_ms: i64,
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStore").finish_non_exhaustive()
    }
}

/// A database error, logged by operation and SQLSTATE only: the message can quote values.
pub(crate) fn db_err(op: &'static str, e: &sqlx::Error) -> SessionError {
    let state = e
        .as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
        .unwrap_or_else(|| "none".to_owned());
    tracing::warn!(op, sqlstate = %state, "session database error");
    SessionError::Unavailable
}

/// Run `f` with the store's timeout. A dropped future rolls back its transaction, so a timeout leaves nothing half-written.
async fn bounded<T>(
    op: &'static str,
    f: impl std::future::Future<Output = Result<T, SessionError>>,
) -> Result<T, SessionError> {
    if let Ok(r) = tokio::time::timeout(DB_TIMEOUT, f).await {
        r
    } else {
        tracing::warn!(op, "session database operation timed out");
        Err(SessionError::Unavailable)
    }
}

impl SessionStore {
    pub fn new(pool: PgPool, clock: Arc<dyn Clock>, policy: &SessionPolicy) -> Self {
        Self {
            pool,
            clock,
            absolute_ms: i64::try_from(policy.absolute.as_millis()).unwrap_or(i64::MAX),
        }
    }

    /// Create a session for `user` inside the caller's transaction, so that the sign-in's other writes commit with it.
    pub async fn create_in(
        &self,
        conn: &mut PgConnection,
        user: &UserId,
    ) -> Result<NewSession, SessionError> {
        let (session, csrf) = fresh_tokens()?;
        let hash = session.hash();
        let now = self.clock.now();
        let expires_at = Timestamp::from_unix_millis(now.unix_millis().saturating_add(self.absolute_ms));
        bounded("create session", async {
            sqlx::query(
                "INSERT INTO sessions (session_hash, user_tid, user_oid, csrf_hash, created_at, last_seen_at, expires_at) \
                 VALUES ($1, $2, $3, $4, to_timestamp($5::bigint / 1000.0), to_timestamp($5::bigint / 1000.0), \
                         to_timestamp($6::bigint / 1000.0))",
            )
            .bind(hash.as_bytes().as_slice())
            .bind(user.tid().as_str())
            .bind(user.oid().as_str())
            .bind(csrf.hash().as_bytes().as_slice())
            .bind(now.unix_millis())
            .bind(expires_at.unix_millis())
            .execute(conn)
            .await
            .map_err(|e| db_err("create session", &e))?;
            Ok(())
        })
        .await?;
        Ok(NewSession {
            session: session.to_text(),
            csrf: csrf.to_text(),
            hash,
            expires_at,
        })
    }

    /// Delete one session inside the caller's transaction. True when there was one.
    pub async fn delete_in(&self, conn: &mut PgConnection, hash: &TokenHash) -> Result<bool, SessionError> {
        bounded("delete session", async {
            let done = sqlx::query("DELETE FROM sessions WHERE session_hash = $1")
                .bind(hash.as_bytes().as_slice())
                .execute(conn)
                .await
                .map_err(|e| db_err("delete session", &e))?;
            Ok(done.rows_affected() > 0)
        })
        .await
    }

    /// Delete one session. True when there was one; a second call is a no-op that says false.
    pub async fn delete(&self, hash: &TokenHash) -> Result<bool, SessionError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| db_err("delete session", &e))?;
        self.delete_in(&mut conn, hash).await
    }

    /// The session with this hash and its user's standing, if there is one. Expiry is not judged here.
    pub async fn lookup(&self, hash: &TokenHash) -> Result<Option<SessionRow>, SessionError> {
        bounded("lookup session", async {
            let row = sqlx::query(LOOKUP_SQL)
                .bind(hash.as_bytes().as_slice())
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| db_err("lookup session", &e))?;
            row.as_ref().map(row_to_session).transpose()
        })
        .await
    }

    /// Remove sessions past their absolute expiry, `batch` rows per statement (at most 5,000). A batch that comes back short
    /// ends the sweep, as does the cap of 1,000 batches; the next sweep carries on.
    pub async fn sweep_expired(&self, batch: u32) -> Result<SweepReport, SessionError> {
        let batch = batch.clamp(1, MAX_SWEEP_BATCH);
        let now = self.clock.now().unix_millis();
        let mut report = SweepReport {
            deleted: 0,
            batches: 0,
        };
        while report.batches < MAX_SWEEP_BATCHES {
            let removed = bounded("sweep sessions", async {
                // SKIP LOCKED: two replicas sweeping at once take different rows instead of waiting for each other.
                let done = sqlx::query(
                    "DELETE FROM sessions WHERE session_hash IN ( \
                       SELECT session_hash FROM sessions WHERE expires_at <= to_timestamp($1::bigint / 1000.0) \
                       ORDER BY expires_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
                )
                .bind(now)
                .bind(i64::from(batch))
                .execute(&self.pool)
                .await
                .map_err(|e| db_err("sweep sessions", &e))?;
                Ok(done.rows_affected())
            })
            .await?;
            report.deleted += removed;
            report.batches += 1;
            if removed < u64::from(batch) {
                break;
            }
        }
        Ok(report)
    }

    /// The plan of [`SessionStore::lookup`], for the EXPLAIN test (P12). Nothing in the hub calls it.
    pub async fn explain_lookup(&self, hash: &TokenHash) -> Result<serde_json::Value, SessionError> {
        bounded("explain session lookup", async {
            let plan: (serde_json::Value,) = sqlx::query_as(&format!("EXPLAIN (FORMAT JSON) {LOOKUP_SQL}"))
                .bind(hash.as_bytes().as_slice())
                .fetch_one(&self.pool)
                .await
                .map_err(|e| db_err("explain session lookup", &e))?;
            Ok(plan.0)
        })
        .await
    }
}

/// Delete every session of `user` inside the caller's transaction. A role or status change calls it, so the deletion commits
/// or rolls back with the change and its audit event (S2). Returns how many there were.
pub async fn delete_all_for_user(conn: &mut PgConnection, user: &UserId) -> Result<u64, SessionError> {
    let done = sqlx::query("DELETE FROM sessions WHERE user_tid = $1 AND user_oid = $2")
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .execute(conn)
        .await
        .map_err(|e| db_err("end sessions", &e))?;
    Ok(done.rows_affected())
}

/// Fresh session and CSRF values, or `Crypto`.
fn fresh_tokens() -> Result<(RandomToken, RandomToken), SessionError> {
    let session = RandomToken::generate().ok_or(SessionError::Crypto)?;
    let csrf = RandomToken::generate().ok_or(SessionError::Crypto)?;
    Ok((session, csrf))
}

/// A row of [`LOOKUP_SQL`]. A stored value that does not parse is a damaged database, not user input.
fn row_to_session(row: &PgRow) -> Result<SessionRow, SessionError> {
    let bad = |e: sqlx::Error| db_err("read session", &e);
    let tid: String = row.try_get("user_tid").map_err(bad)?;
    let oid: String = row.try_get("user_oid").map_err(bad)?;
    let csrf: Vec<u8> = row.try_get("csrf_hash").map_err(bad)?;
    let last_seen: i64 = row.try_get("last_seen_ms").map_err(bad)?;
    let expires: i64 = row.try_get("expires_ms").map_err(bad)?;
    let role: Option<String> = row.try_get("role").map_err(bad)?;
    let status: String = row.try_get("status").map_err(bad)?;
    let damaged = || {
        tracing::warn!("a session row holds a value that does not parse");
        SessionError::Unavailable
    };
    Ok(SessionRow {
        user: UserId::new(
            Guid::parse(&tid).map_err(|_| damaged())?,
            Guid::parse(&oid).map_err(|_| damaged())?,
        ),
        role: match role {
            None => None,
            Some(r) => Some(Role::from_str(&r).map_err(|_| damaged())?),
        },
        status: UserStatus::from_str(&status).map_err(|_| damaged())?,
        csrf_hash: TokenHash::from_bytes(csrf.try_into().map_err(|_| damaged())?),
        last_seen_at: Timestamp::from_unix_millis(last_seen),
        expires_at: Timestamp::from_unix_millis(expires),
    })
}
