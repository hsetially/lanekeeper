//! The versions of one file, newest first (T6, P12).
//!
//! `file_observations` has an index for exactly this, `(swimlane_id, path, observed_at DESC, id DESC)`, and the page is a keyset over
//! `(observed_at, id)`: the time in whole milliseconds (the replay key of ingestion compares the same value) and the observation's
//! id. A deletion has no hash and is not a version of the content, so it is not listed. The current version is the newest.

use domain::{ContentHash, NfsPath, Page, Paged, SwimlaneId, Version};
use ports::ReadError;
use sqlx::Row;

use super::cursor::{self, Context, Field, Kind};
use super::rows;
use super::service::Inner;
use crate::db::{ms, ts};
use crate::error::RegistryError;

/// `$1` swimlane, `$2` path, `$3` and `$4` the time (in milliseconds) and id after which to resume (`NULL` for the first page), `$5` the
/// page size plus one.
pub fn history_sql() -> String {
    format!(
        "SELECT o.id, {observed} AS at_ms, o.hash::bytea AS hash, COALESCE(o.size, 0) AS size, o.source, o.severity, \
                o.attribution_source, o.attribution_confidence, o.attribution_actor, o.attribution_evidence \
         FROM file_observations o \
         WHERE o.swimlane_id = $1 AND o.path = $2 AND o.hash IS NOT NULL \
           AND ($3::bigint IS NULL OR (o.observed_at, o.id) < ({after}, $4::bigint)) \
         ORDER BY o.observed_at DESC, o.id DESC LIMIT $5",
        observed = ms("o.observed_at"),
        after = ts("$3"),
    )
}

impl Inner {
    pub async fn history_page(
        &self,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        page: &Page,
    ) -> Result<Paged<Version>, ReadError> {
        self.require_swimlane(swimlane).await?;
        let context = Context::of(Kind::History, &[swimlane.as_str(), path.as_str()]);
        let (after_ms, after_id): (Option<i64>, Option<i64>) =
            match cursor::resume(page, Kind::History, &context)? {
                Some(fields) => {
                    cursor::arity(&fields, 2).map_err(|_| ReadError::BadRequest)?;
                    (
                        Some(cursor::int(&fields, 0).map_err(|_| ReadError::BadRequest)?),
                        Some(cursor::int(&fields, 1).map_err(|_| ReadError::BadRequest)?),
                    )
                }
                None => (None, None),
            };
        let limit = page.limit() as usize;
        let fetched = self
            .db
            .run(
                "read_history",
                sqlx::query(&history_sql())
                    .bind(swimlane.as_str())
                    .bind(path.as_str())
                    .bind(after_ms)
                    .bind(after_id)
                    .bind(i64::try_from(limit + 1).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let has_more = fetched.len() > limit;
        let kept = &fetched[..fetched.len().min(limit)];
        let mut items = Vec::with_capacity(kept.len());
        for row in kept {
            let hash: Vec<u8> = row.get("hash");
            let size: i64 = row.get("size");
            let source: String = row.get("source");
            let severity: Option<String> = row.get("severity");
            let (a_source, a_conf, a_actor, a_evidence): (
                String,
                String,
                Option<serde_json::Value>,
                serde_json::Value,
            ) = (
                row.get("attribution_source"),
                row.get("attribution_confidence"),
                row.get("attribution_actor"),
                row.get("attribution_evidence"),
            );
            items.push(Version {
                hash: rows::hash(&hash)?,
                observed_at: domain::Timestamp::from_unix_millis(row.get::<i64, _>("at_ms")),
                source: rows::observation_source(&source)?,
                size: u64::try_from(size).unwrap_or(0),
                attribution: rows::attribution(&a_source, &a_conf, a_actor, a_evidence),
                severity: rows::severity(severity.as_deref())?,
                commit: None,
            });
        }
        let next = if has_more {
            kept.last()
                .map(|r| {
                    let (at, id): (i64, i64) = (r.get("at_ms"), r.get("id"));
                    cursor::issue(Kind::History, &context, vec![Field::Int(at), Field::Int(id)])
                })
                .transpose()?
        } else {
            None
        };
        Ok(Paged { items, next })
    }

    /// Whether `hash` is a version this path has had: the current one, one in its history, or the one behind its baseline.
    pub async fn is_version_of(
        &self,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        hash: &ContentHash,
    ) -> Result<bool, RegistryError> {
        self.db
            .run(
                "read_is_version",
                sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM file_observations WHERE swimlane_id = $1 AND path = $2 AND hash = $3::bytea) \
                         OR EXISTS (SELECT 1 FROM file_state WHERE swimlane_id = $1 AND path = $2 AND nfs_hash = $3::bytea) \
                         OR EXISTS (SELECT 1 FROM baselines WHERE swimlane_id = $1 AND path = $2 AND blob_hash = $3::bytea)",
                )
                .bind(swimlane.as_str())
                .bind(path.as_str())
                .bind(hash.as_bytes().as_slice())
                .fetch_one(self.db.pool()),
            )
            .await
    }
}
