# Code patterns

## Contents

1. Port implementation skeleton
2. A write with audit and outbox
3. Keyset pagination
4. CPU work from async
5. Bounded retry with jitter
6. EXPLAIN snapshot test

## 1. Port implementation skeleton

```rust
use async_trait::async_trait;
use ports::{BlobStore, BlobError};
use domain::ContentHash;
use bytes::Bytes;

/// Postgres-backed blob store (prompt 03b, T8).
pub struct PgBlobStore { pool: sqlx::PgPool, cache: moka::future::Cache<ContentHash, Bytes> }

#[async_trait]
impl BlobStore for PgBlobStore {
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError> {
        let hash = ContentHash::sha256(&bytes);
        // One statement, on the pool and never inside a caller's transaction. Storing a blob that already exists refreshes its
        // age (`blobs.touched_at`, migration 0008), so that retention cannot collect it before the transaction that references it
        // commits. A plain `INSERT ... ON CONFLICT DO NOTHING` is wrong here, and so is a bare `ON CONFLICT DO UPDATE ... WHERE`
        // (it locks the row and writes WAL on every re-put). Use `PUT_SQL` and `TOUCH_INTERVAL`, as `PgBlobStore::put` does
        // (crates/hub-platform/src/blobs/store.rs); the rules for a collector are in docs/interfaces.md, under `BlobStore`.
        sqlx::query(PUT_SQL)
            .bind(hash.as_bytes().as_slice()).bind(bytes.as_ref()).bind(bytes.len() as i64)
            .bind(TOUCH_INTERVAL.as_secs() as i64)
            .execute(&self.pool).await.map_err(BlobError::db)?;
        Ok(hash)
    }
    // get / get_many: check the cache first, then a batched SELECT ... WHERE hash = ANY($1)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn conforms() { ports::conformance::blob_store(test_store().await).await; }
}
```

## 2. A write with audit and outbox

```rust
let mut tx = pool.begin().await?;
let applied = repo.apply_edit(&mut tx, &edit).await?;                 // state change
audit.record(&mut tx, AuditEvent::edit(&ctx, &edit, &applied)).await?; // exactly one event
bus.publish_in_tx(&mut tx, DomainEvent::FileObserved { /* ids and hashes only */ }).await?;
tx.commit().await?;                                                   // fan-out happens after this
```

## 3. Keyset pagination

```rust
// cursor = base64(json!({ "ts": last.observed_at, "id": last.id }))
sqlx::query_as!(Row,
  "SELECT id, observed_at, path, hash FROM file_observations
   WHERE swimlane = $1 AND (observed_at, id) < ($2, $3)
   ORDER BY observed_at DESC, id DESC LIMIT $4",
   swimlane, cursor_ts, cursor_id, limit.min(500) as i64)
```

## 4. CPU work from async

```rust
let findings = tokio::task::spawn_blocking(move || {
    engine::checks::run_batch(&inputs) // uses rayon internally
}).await.map_err(JobError::join)?;
```

## 5. Bounded retry with jitter

```rust
for attempt in 0..MAX_ATTEMPTS {
    match tokio::time::timeout(CALL_TIMEOUT, call()).await {
        Ok(Ok(v)) => return Ok(v),
        _ if attempt + 1 < MAX_ATTEMPTS => {
            let cap = BASE.saturating_mul(1 << attempt).min(MAX_BACKOFF);
            tokio::time::sleep(rand_duration(Duration::ZERO, cap)).await; // full jitter
        }
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(CallError::Timeout),
    }
}
```

## 6. EXPLAIN snapshot test

```rust
#[sqlx::test]
async fn tree_query_uses_index(pool: PgPool) {
    seed_target_scale(&pool).await;
    let plan: serde_json::Value = sqlx::query_scalar("EXPLAIN (FORMAT JSON) SELECT ...").fetch_one(&pool).await.unwrap();
    assert!(!plan.to_string().contains("\"Seq Scan\""), "tree query must not seq-scan nfs_files");
}
```
