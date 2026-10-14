//! Settings search (T6, P7: 200 ms, D65, S11, A28, A31, P12).
//!
//! A part of a setting path, searched across swimlanes: indexed SQL over `settings_index` (trigram index on `setting_path`), limited to
//! the settings of the files as they are *now*. Two statements:
//!
//! 1. the next settings (blob and path) that match, among the blobs that are the current version of a readable file of the swimlanes
//!    asked for, in primary key order after the cursor's position, so a page reads only what it shows; the planner takes the primary
//!    key when matches are dense and the trigram index when they are rare;
//! 2. the places of those blobs: the files of the swimlanes that have them as their current version, from the covering index
//!    `(swimlane_id, nfs_hash)`.
//!
//! The same setting is a hit in every swimlane that holds the blob. Hits are ordered by (blob hash, setting path, swimlane, file
//! sequence) and the cursor is that key, so a page boundary can fall between two swimlanes' hits of the same setting. Values come from
//! the index, where a flagged value is already `[REDACTED: N chars, rule]` (A28, S13).
//!
//! **A bound the cursor imposes (A31):** a setting path longer than 255 bytes cannot be the key of a cursor, so the search does not list
//! such settings (they stay in the index, the grid and the effective view). No real configuration has a key that long.

use std::collections::HashMap;

use domain::{
    ContentHash, LineRange, NfsPath, Paged, SettingHit, SettingLocation, SettingPath, SettingValue,
    SettingsQuery, SwimlaneId, ValueType,
};
use ports::ReadError;
use sqlx::Row;

use super::cursor::{self, Context, Field, Kind, MAX_BYTES_FIELD};
use super::rows::{self, like_escape};
use super::service::Inner;
use super::settings::flag_of;
use crate::error::RegistryError;

/// The matching settings of current blobs: `$1` the `LIKE` pattern, `$2` the swimlanes, `$3` a logical file or `NULL`, `$4` and `$5` the
/// blob and setting path to resume at (inclusive; empty for the first page), `$6` how many to read, `$7` the longest path a cursor holds.
pub const PAIRS_SQL: &str = "\
SELECT si.blob_hash::bytea AS blob, si.setting_path, si.value_text, si.value_type, si.start_line, si.end_line \
FROM settings_index si \
WHERE si.setting_path ILIKE $1 \
  AND octet_length(si.setting_path) <= $7 \
  AND (si.blob_hash, si.setting_path) >= ($4::bytea, $5::text) \
  AND si.blob_hash IN ( \
        SELECT nfs_hash FROM file_state \
        WHERE swimlane_id = ANY($2::text[]) AND nfs_hash IS NOT NULL AND NOT denied \
          AND ($3::text IS NULL OR logical_file = $3)) \
ORDER BY si.blob_hash, si.setting_path LIMIT $6";

/// The files that have these blobs as their current version.
pub const PLACES_SQL: &str = "\
SELECT swimlane_id, seq, path, nfs_hash::bytea AS blob FROM file_state \
WHERE swimlane_id = ANY($1::text[]) AND nfs_hash = ANY($2::bytea[]) AND NOT denied \
  AND ($3::text IS NULL OR logical_file = $3)";

struct Pair {
    blob: ContentHash,
    path: String,
    value: String,
    ty: String,
    start: i32,
    end: i32,
}

/// One hit with its position in the order the cursor is a key of: blob, setting path, swimlane, file sequence.
struct Positioned {
    blob: Vec<u8>,
    path: String,
    lane: String,
    seq: i64,
    hit: SettingHit,
}

/// The key a cursor resumes after: blob, setting path, swimlane, file sequence.
type After = (Vec<u8>, String, String, i64);

fn after_of(fields: &[Field]) -> Result<After, ReadError> {
    cursor::arity(fields, 4).map_err(|_| ReadError::BadRequest)?;
    let blob = cursor::bytes(fields, 0)
        .map_err(|_| ReadError::BadRequest)?
        .to_vec();
    if blob.len() != 32 {
        return Err(ReadError::BadRequest);
    }
    Ok((
        blob,
        cursor::text(fields, 1).map_err(|_| ReadError::BadRequest)?,
        cursor::text(fields, 2).map_err(|_| ReadError::BadRequest)?,
        cursor::int(fields, 3).map_err(|_| ReadError::BadRequest)?,
    ))
}

/// The hits of the pairs, in the order the pairs came in (the database's own order of blob and setting path, which is also what the
/// cursor's `>=` was evaluated in), and within a setting by swimlane and file sequence (an order made here, so it is the same for
/// every page). Only the cursor's own setting can have places that were shown already: those at or before the cursor's.
fn positioned(
    pairs: &[Pair],
    by_blob: &HashMap<ContentHash, Vec<(String, i64, String)>>,
    after: Option<&After>,
) -> Result<Vec<Positioned>, RegistryError> {
    let mut hits = Vec::new();
    for p in pairs {
        let Some(here) = by_blob.get(&p.blob) else {
            continue;
        };
        let cursor_pair = after.filter(|a| a.0.as_slice() == p.blob.as_bytes().as_slice() && a.1 == p.path);
        for (lane, seq, path) in here {
            if cursor_pair.is_some_and(|a| (lane.as_str(), *seq) <= (a.2.as_str(), a.3)) {
                continue;
            }
            hits.push(Positioned {
                blob: p.blob.as_bytes().to_vec(),
                path: p.path.clone(),
                lane: lane.clone(),
                seq: *seq,
                hit: hit_of(p, lane, path)?,
            });
        }
    }
    Ok(hits)
}

impl Inner {
    /// The swimlanes searched: those asked for (each must exist), or all of them.
    async fn searched_lanes(&self, q: &SettingsQuery) -> Result<Vec<String>, ReadError> {
        if q.swimlanes.is_empty() {
            return Ok(self
                .db
                .run(
                    "read_all_swimlanes",
                    sqlx::query_scalar("SELECT id FROM swimlanes ORDER BY id LIMIT 1000")
                        .fetch_all(self.db.pool()),
                )
                .await?);
        }
        let mut asked: Vec<&str> = q.swimlanes.iter().map(SwimlaneId::as_str).collect();
        asked.sort_unstable();
        asked.dedup();
        let existing = self.existing(&asked).await?;
        if existing.len() != asked.len() {
            return Err(ReadError::NotFound);
        }
        Ok(asked.into_iter().map(str::to_owned).collect())
    }

    /// The next `want` matching (blob, setting) pairs at or after the key.
    async fn setting_pairs(
        &self,
        pattern: &str,
        lanes: &[String],
        file: Option<&str>,
        start: (&[u8], &str),
        want: i64,
    ) -> Result<Vec<Pair>, ReadError> {
        let fetched = self
            .db
            .run(
                "read_setting_pairs",
                sqlx::query(PAIRS_SQL)
                    .bind(pattern)
                    .bind(lanes)
                    .bind(file)
                    .bind(start.0)
                    .bind(start.1)
                    .bind(want)
                    .bind(i32::try_from(MAX_BYTES_FIELD).unwrap_or(255))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut pairs: Vec<Pair> = Vec::with_capacity(fetched.len());
        for row in &fetched {
            let blob: Vec<u8> = row.get("blob");
            pairs.push(Pair {
                blob: rows::hash(&blob)?,
                path: row.get("setting_path"),
                value: row.get("value_text"),
                ty: row.get("value_type"),
                start: row.get("start_line"),
                end: row.get("end_line"),
            });
        }
        Ok(pairs)
    }

    /// Where those blobs are: (swimlane, sequence, path) of the files that have each as their current version, sorted.
    async fn setting_places(
        &self,
        lanes: &[String],
        pairs: &[Pair],
        file: Option<&str>,
    ) -> Result<HashMap<ContentHash, Vec<(String, i64, String)>>, ReadError> {
        let mut seen: Vec<ContentHash> = Vec::new();
        for p in pairs {
            if !seen.contains(&p.blob) {
                seen.push(p.blob);
            }
        }
        let blobs: Vec<Vec<u8>> = seen.iter().map(|b| b.as_bytes().to_vec()).collect();
        let places = self
            .db
            .run(
                "read_setting_places",
                sqlx::query(PLACES_SQL)
                    .bind(lanes)
                    .bind(&blobs)
                    .bind(file)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut by_blob: HashMap<ContentHash, Vec<(String, i64, String)>> = HashMap::new();
        for row in &places {
            let blob: Vec<u8> = row.get("blob");
            by_blob.entry(rows::hash(&blob)?).or_default().push((
                row.get("swimlane_id"),
                row.get("seq"),
                row.get("path"),
            ));
        }
        for v in by_blob.values_mut() {
            v.sort();
        }
        Ok(by_blob)
    }

    pub async fn search_settings(&self, q: &SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        let needle = q.path_contains.as_str();
        if needle.is_empty() {
            return Err(ReadError::BadRequest);
        }
        let lanes = self.searched_lanes(q).await?;
        if lanes.is_empty() {
            return Ok(Paged::last(Vec::new()));
        }
        let context = Context::of(
            Kind::SettingsSearch,
            &[
                &needle.to_lowercase(),
                &lanes.join(","),
                q.file.as_ref().map_or("", |f| f.as_str()),
            ],
        );
        let after: Option<After> = cursor::resume(&q.page, Kind::SettingsSearch, &context)?
            .map(|f| after_of(&f))
            .transpose()?;
        let limit = q.page.limit() as usize;
        let pattern = format!("%{}%", like_escape(needle));
        let file = q.file.as_ref().map(|f| f.as_str().to_owned());
        let (start_blob, start_path) = after
            .as_ref()
            .map_or((&[][..], ""), |(b, p, _, _)| (b.as_slice(), p.as_str()));

        // Read `limit + 2` settings: the first may be the cursor's own with nothing left, and every setting of a current blob has at
        // least one place, so the rest give at least `limit + 1` hits.
        let want_pairs = i64::try_from(limit + 2).unwrap_or(i64::MAX);
        let pairs = self
            .setting_pairs(
                &pattern,
                &lanes,
                file.as_deref(),
                (start_blob, start_path),
                want_pairs,
            )
            .await?;
        if pairs.is_empty() {
            return Ok(Paged::last(Vec::new()));
        }
        let by_blob = self.setting_places(&lanes, &pairs, file.as_deref()).await?;

        let mut hits = positioned(&pairs, &by_blob, after.as_ref())?;
        let more = hits.len() > limit;
        hits.truncate(limit);
        let next = if more {
            hits.last()
                .map(|h| {
                    cursor::issue(
                        Kind::SettingsSearch,
                        &context,
                        vec![
                            Field::Bytes(h.blob.clone()),
                            Field::text(&h.path),
                            Field::text(&h.lane),
                            Field::Int(h.seq),
                        ],
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok(Paged {
            items: hits.into_iter().map(|h| h.hit).collect(),
            next,
        })
    }
}

fn hit_of(p: &Pair, lane: &str, file: &str) -> Result<SettingHit, RegistryError> {
    let ty = match p.ty.as_str() {
        "int" => ValueType::Int,
        "float" => ValueType::Float,
        "bool" => ValueType::Bool,
        "null" => ValueType::Null,
        "other" => ValueType::Other,
        _ => ValueType::String,
    };
    let path = SettingPath::parse(&p.path).map_err(|_| RegistryError::Corrupt)?;
    let lines = LineRange::new(
        u32::try_from(p.start).map_err(|_| RegistryError::Corrupt)?,
        u32::try_from(p.end).map_err(|_| RegistryError::Corrupt)?,
    )
    .map_err(|_| RegistryError::Corrupt)?;
    Ok(SettingHit {
        swimlane: SwimlaneId::parse(lane).map_err(|_| RegistryError::Corrupt)?,
        file: NfsPath::parse(file).map_err(|_| RegistryError::Corrupt)?,
        path: path.clone(),
        value: SettingValue {
            text: p.value.clone(),
            ty,
            flag: flag_of(ty, &p.value),
        },
        location: SettingLocation { path, lines },
    })
}
