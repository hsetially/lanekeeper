//! A test double of the `ToolWrites` port that is transactional on a real Postgres connection: the registration is a
//! `file_observations` row with `source = 'tool_write'`, written in the caller's transaction, so it exists only if that
//! transaction commits (the fake parks it in an in-memory transaction, which a Postgres `Tx` cannot be).

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};
use ports::{ToolWriteError, ToolWrites, Tx};
use sqlx::PgPool;

#[derive(Debug, Default)]
pub struct PgToolWrites;

#[async_trait]
impl ToolWrites for PgToolWrites {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        _request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        let conn = tx.pg().map_err(|_| ToolWriteError::WrongTx)?;
        sqlx::query(
            "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, attribution_source, \
             attribution_confidence, attribution_actor) \
             VALUES ($1, $2, $3, 'tool_write', now(), 'tool_write', 'certain', $4) ON CONFLICT DO NOTHING",
        )
        .bind(swimlane.as_str())
        .bind(path.as_str())
        .bind(new_hash.as_bytes().as_slice())
        .bind(serde_json::json!({ "kind": "user", "user": user.to_string() }))
        .execute(&mut *conn)
        .await
        .map_err(|_| ToolWriteError::Unavailable)?;
        Ok(())
    }
}

/// The hashes registered so far, newest last.
pub async fn registered(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT path, hash FROM file_observations WHERE source = 'tool_write' ORDER BY id LIMIT 1000",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|(p, h)| (p, ContentHash::from_bytes(h.try_into().unwrap()).to_string()))
    .collect()
}
