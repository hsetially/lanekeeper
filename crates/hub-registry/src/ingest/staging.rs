//! `delta_staging`: the acknowledged parts of a multi-part scan delta that are not applied yet (A14, P15).
//!
//! The agent sends one logical delta as several messages and the hub acknowledges each message when the sink accepts it
//! (`hub_platform::ack`), so a part that is acknowledged must not be lost to a restart of the replica that holds the stream. A
//! part therefore goes to the database (its blobs are already in `blobs`, its metadata here) before its acknowledgement is
//! sent, and the part with `more == false` applies all of them. A stream that ends earlier applies nothing: the rows wait, and
//! are dropped on the next `hello` of the swimlane and by the sweep after `staging_ttl`.
//!
//! Rows are keyed by `(swimlane, seq)`, so a part sent again replaces itself, and they are grouped into a logical delta by the
//! pair of roots every part of one delta shares.

use domain::{ContentHash, SwimlaneId};

use super::validate::{StagedPart, Totals, Validated};
use crate::db::{Db, ts};
use crate::error::RegistryError;

/// A staged part, as read back.
#[derive(Debug, Clone)]
pub(crate) struct Staged {
    pub seq: u64,
    pub part: u32,
    pub data: StagedPart,
}

fn root(r: Option<ContentHash>) -> Option<Vec<u8>> {
    r.map(|h| h.as_bytes().to_vec())
}

/// What the staged parts of the logical delta with these roots hold, not counting the part with `except_seq` (which a retry
/// replaces).
pub(crate) async fn totals(
    db: &Db,
    swimlane: &SwimlaneId,
    base_root: Option<ContentHash>,
    new_root: ContentHash,
    except_seq: u64,
) -> Result<Totals, RegistryError> {
    let row: (i64, i64, i64) = db
        .run(
            "staging_totals",
            sqlx::query_as(
                "SELECT count(*)::bigint, \
                        COALESCE(sum((metadata->>'bytes')::bigint), 0)::bigint, \
                        COALESCE(sum(jsonb_array_length(metadata->'entries') + jsonb_array_length(metadata->'removed') \
                                     + jsonb_array_length(metadata->'skipped')), 0)::bigint \
                 FROM delta_staging \
                 WHERE swimlane_id = $1 AND base_root IS NOT DISTINCT FROM $2::bytea AND new_root = $3::bytea AND seq <> $4",
            )
            .bind(swimlane.as_str())
            .bind(root(base_root))
            .bind(new_root.as_bytes().as_slice())
            .bind(i64::try_from(except_seq).map_err(|_| RegistryError::Invalid)?)
            .fetch_one(db.pool()),
        )
        .await?;
    let n = |v: i64| u64::try_from(v).unwrap_or(0);
    Ok(Totals {
        parts: n(row.0),
        bytes: n(row.1),
        items: n(row.2),
    })
}

/// Keep one part. Idempotent: the same `(swimlane, seq)` replaces the earlier row.
pub(crate) async fn stage(
    db: &Db,
    swimlane: &SwimlaneId,
    v: &Validated,
    now_ms: i64,
) -> Result<(), RegistryError> {
    let metadata = serde_json::to_value(&v.staged).map_err(|_| RegistryError::Invalid)?;
    let sql = format!(
        "INSERT INTO delta_staging (swimlane_id, seq, part, base_root, new_root, metadata, staged_at) \
         VALUES ($1, $2, $3, $4::bytea, $5::bytea, $6, {}) \
         ON CONFLICT (swimlane_id, seq) DO UPDATE SET \
           part = EXCLUDED.part, base_root = EXCLUDED.base_root, new_root = EXCLUDED.new_root, \
           metadata = EXCLUDED.metadata, staged_at = EXCLUDED.staged_at",
        ts("$7")
    );
    db.run(
        "stage_part",
        sqlx::query(&sql)
            .bind(swimlane.as_str())
            .bind(i64::try_from(v.seq).map_err(|_| RegistryError::Invalid)?)
            .bind(i32::try_from(v.part).map_err(|_| RegistryError::Invalid)?)
            .bind(root(v.base_root))
            .bind(v.new_root.as_bytes().as_slice())
            .bind(metadata)
            .bind(now_ms)
            .execute(db.pool()),
    )
    .await?;
    Ok(())
}

/// The staged parts of the logical delta with these roots, in the order they arrived.
pub(crate) async fn load(
    db: &Db,
    swimlane: &SwimlaneId,
    base_root: Option<ContentHash>,
    new_root: ContentHash,
    limit: usize,
) -> Result<Vec<Staged>, RegistryError> {
    let rows: Vec<(i64, i32, serde_json::Value)> = db
        .run(
            "staging_load",
            sqlx::query_as(
                "SELECT seq, part, metadata FROM delta_staging \
                 WHERE swimlane_id = $1 AND base_root IS NOT DISTINCT FROM $2::bytea AND new_root = $3::bytea \
                 ORDER BY seq LIMIT $4",
            )
            .bind(swimlane.as_str())
            .bind(root(base_root))
            .bind(new_root.as_bytes().as_slice())
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(db.pool()),
        )
        .await?;
    rows.into_iter()
        .map(|(seq, part, value)| {
            let data: StagedPart = serde_json::from_value(value).map_err(|_| RegistryError::Corrupt)?;
            if data.v != StagedPart::VERSION {
                return Err(RegistryError::Corrupt);
            }
            Ok(Staged {
                seq: u64::try_from(seq).map_err(|_| RegistryError::Corrupt)?,
                part: u32::try_from(part).map_err(|_| RegistryError::Corrupt)?,
                data,
            })
        })
        .collect()
}

/// Drop everything staged for a swimlane (a new stream starts), and everything older than `older_than_ms` anywhere.
pub(crate) async fn discard(
    db: &Db,
    swimlane: &SwimlaneId,
    older_than_ms: i64,
) -> Result<u64, RegistryError> {
    let sql = format!(
        "DELETE FROM delta_staging WHERE swimlane_id = $1 OR staged_at < {}",
        ts("$2")
    );
    let done = db
        .run(
            "staging_discard",
            sqlx::query(&sql)
                .bind(swimlane.as_str())
                .bind(older_than_ms)
                .execute(db.pool()),
        )
        .await?;
    Ok(done.rows_affected())
}

/// Drop the staged parts of one logical delta, once it is applied or refused.
pub(crate) async fn clear(
    db: &Db,
    swimlane: &SwimlaneId,
    base_root: Option<ContentHash>,
    new_root: ContentHash,
) -> Result<(), RegistryError> {
    db.run(
        "staging_clear",
        sqlx::query(
            "DELETE FROM delta_staging \
             WHERE swimlane_id = $1 AND base_root IS NOT DISTINCT FROM $2::bytea AND new_root = $3::bytea",
        )
        .bind(swimlane.as_str())
        .bind(root(base_root))
        .bind(new_root.as_bytes().as_slice())
        .execute(db.pool()),
    )
    .await?;
    Ok(())
}
