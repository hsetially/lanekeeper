//! Settings search (T6, P7: 200 ms, D65, S11, A28, A31, P12).
//!
//! A part of a setting path, searched across swimlanes: indexed SQL over `settings_index` (trigram index on `setting_path`), limited to
//! the settings of the files as they are *now*. Statements:
//!
//! 0. over a few swimlanes or in one file: the blobs that are the current version of a readable file there (from the covering index
//!    `(swimlane_id, nfs_hash)`), at most `search_listed_blobs` of them (20,000 by default);
//! 1. the next settings (blob and path) that match, among those blobs (or, over many swimlanes, among all), in primary key order after
//!    the cursor's position, so a page reads only what it shows; the planner takes the primary key when matches are dense and the
//!    trigram index when they are rare (every one of these statements is planned for its own values: `persistent(false)`, because a
//!    prepared statement that has been run a few times is given one generic plan, which for a rare needle walks the whole primary key);
//! 2. the places of those blobs: the files of the swimlanes that have them as their current version, from the same index.
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
use futures::stream::{self, StreamExt};
use ports::ReadError;
use sqlx::Row;

use super::cursor::{self, Context, Field, Kind, MAX_BYTES_FIELD};
use super::rows::{self, like_escape};
use super::service::Inner;
use super::settings::flag_of;
use crate::error::RegistryError;

/// The blobs that are the current version of a readable file of these swimlanes, sorted: `$1` the swimlanes, `$2` a logical file or
/// `NULL`, `$3` how many to read at most. An index-only scan of the covering index `(swimlane_id, nfs_hash)` (a few thousand blobs for
/// eight swimlanes of the target scale, about 20 ms), or the files' own index when `$2` is given.
pub const LANE_BLOBS_SQL: &str = "\
SELECT DISTINCT nfs_hash::bytea AS blob FROM file_state \
WHERE swimlane_id = ANY($1::text[]) AND nfs_hash IS NOT NULL AND NOT denied AND ($2::text IS NULL OR logical_file = $2) \
ORDER BY 1 LIMIT $3";

/// The matching settings of a list of blobs: `$1` the `LIKE` pattern, `$2` the blobs, `$3` and `$4` the blob and setting path to resume
/// at (inclusive; empty for the first page), `$5` how many to read, `$6` the longest path a cursor holds. The planner takes the primary
/// key's order when matches are dense (a page is read from the first blobs) and the trigram index when they are rare.
pub const PAIRS_SQL: &str = "\
SELECT si.blob_hash::bytea AS blob, si.setting_path, si.value_text, si.value_type, si.start_line, si.end_line \
FROM settings_index si \
WHERE si.setting_path ILIKE $1 \
  AND octet_length(si.setting_path) <= $6 \
  AND si.blob_hash = ANY($2::bytea[]) \
  AND (si.blob_hash, si.setting_path) >= ($3::bytea, $4::text) \
ORDER BY si.blob_hash, si.setting_path LIMIT $5";

/// The matching settings, not tested: `$1` the `LIKE` pattern, `$2` and `$3` the blob and setting path to resume at (inclusive), `$4`
/// how many to read, `$5` the longest path a cursor holds. Used over many swimlanes with no file: the places of the blobs found (one
/// probe of `file_state` each, `PLACES_SQL`) are what says which are current, and the search asks for more while a batch has too few.
pub const PAIRS_UNTESTED_SQL: &str = "\
SELECT si.blob_hash::bytea AS blob, si.setting_path, si.value_text, si.value_type, si.start_line, si.end_line \
FROM settings_index si \
WHERE si.setting_path ILIKE $1 \
  AND octet_length(si.setting_path) <= $5 \
  AND (si.blob_hash, si.setting_path) >= ($2::bytea, $3::text) \
ORDER BY si.blob_hash, si.setting_path LIMIT $4";

/// How many blobs one statement of places asks about, and how many such statements run at once for a page.
pub const PLACES_PER_STATEMENT: usize = 25;
const PLACES_AT_ONCE: usize = 4;

/// The most batches of settings one request looks at when the batches are not tested in SQL: a request does a bounded amount of work, and
/// a page that the batches did not fill is returned short, with the cursor of where the search got to.
const MAX_BATCHES: usize = 16;

/// The files that have these blobs as their current version: `$1` the swimlanes, `$2` the blobs, `$3` a logical file or `NULL`. At most
/// 10,000 places are listed for one blob: a blob that is the current version of more files than that (an empty file in thousands of
/// folders) lists its first places in (swimlane, sequence) order; no real configuration comes near it, and the bound is rule 5's. Each blob
/// is probed on its own (`LATERAL` with a `LIMIT`, which keeps the planner from turning the probes into one scan of the table): the
/// covering index `(swimlane_id, nfs_hash)` answers every probe, so no plan reads the whole of `file_state`.
pub const PLACES_SQL: &str = "\
SELECT fs.swimlane_id, fs.seq, fs.path, b.h::bytea AS blob \
FROM unnest($2::bytea[]) AS b(h) \
CROSS JOIN LATERAL ( \
  SELECT f.swimlane_id, f.seq, f.path FROM file_state f \
  WHERE f.swimlane_id = ANY($1::text[]) AND f.nfs_hash = b.h AND NOT f.denied AND ($3::text IS NULL OR f.logical_file = $3) \
  ORDER BY f.swimlane_id, f.seq LIMIT 10000) fs";

/// What the settings are read among.
enum Among {
    /// The blobs that are current in the swimlanes asked for (a list, sorted): every setting found has a place.
    Listed(Vec<Vec<u8>>),
    /// All the index: a setting found may be of a blob that is no file's now.
    Everything,
}

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

/// What the batches found: the hits, where the search got to, and whether there is nothing after it.
struct Scan {
    hits: Vec<Positioned>,
    scanned: Option<(Vec<u8>, String)>,
    exhausted: bool,
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

    /// The blobs that are the current version of a readable file of these swimlanes (and of this file), or `Everything` when there are
    /// more than `search_listed_blobs`: a search that has more reads the settings first and asks where they are, as over many swimlanes.
    async fn lane_blobs(&self, lanes: &[String], file: Option<&str>) -> Result<Among, ReadError> {
        let most = self.config.search_listed_blobs;
        let blobs: Vec<Vec<u8>> = self
            .db
            .run(
                "read_setting_lane_blobs",
                sqlx::query_scalar(LANE_BLOBS_SQL)
                    .persistent(false)
                    .bind(lanes)
                    .bind(file)
                    .bind(i64::try_from(most.saturating_add(1)).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(if blobs.len() > most {
            Among::Everything
        } else {
            Among::Listed(blobs)
        })
    }

    /// The next `want` matching (blob, setting) pairs at or after the key, among the blobs listed or all.
    async fn setting_pairs(
        &self,
        among: &Among,
        pattern: &str,
        start: (&[u8], &str),
        want: i64,
    ) -> Result<Vec<Pair>, ReadError> {
        let max_path = i32::try_from(MAX_BYTES_FIELD).unwrap_or(255);
        let fetched = match among {
            Among::Listed(blobs) => {
                self.db
                    .run(
                        "read_setting_pairs",
                        sqlx::query(PAIRS_SQL)
                            .persistent(false)
                            .bind(pattern)
                            .bind(blobs)
                            .bind(start.0)
                            .bind(start.1)
                            .bind(want)
                            .bind(max_path)
                            .fetch_all(self.db.pool()),
                    )
                    .await?
            }
            Among::Everything => {
                self.db
                    .run(
                        "read_setting_pairs",
                        sqlx::query(PAIRS_UNTESTED_SQL)
                            .persistent(false)
                            .bind(pattern)
                            .bind(start.0)
                            .bind(start.1)
                            .bind(want)
                            .bind(max_path)
                            .fetch_all(self.db.pool()),
                    )
                    .await?
            }
        };
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
        // A page of settings is up to 102 blobs, each probed in every swimlane asked for: several statements of a few blobs each, side
        // by side, so that no statement is long and the page waits for the longest of them, not the sum.
        let blobs: Vec<Vec<u8>> = seen.iter().map(|b| b.as_bytes().to_vec()).collect();
        let places: Vec<sqlx::postgres::PgRow> =
            stream::iter(blobs.chunks(PLACES_PER_STATEMENT).map(<[Vec<u8>]>::to_vec))
                .map(|chunk| async move {
                    self.db
                        .run(
                            "read_setting_places",
                            sqlx::query(PLACES_SQL)
                                .bind(lanes)
                                .bind(&chunk)
                                .bind(file)
                                .fetch_all(self.db.pool()),
                        )
                        .await
                })
                .buffer_unordered(PLACES_AT_ONCE)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
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

    /// The hits from the key on, at least `limit + 1` of them if there are that many (and if the batches allowed are enough).
    ///
    /// A batch is `limit + 2` settings: the first may be the cursor's own with nothing left of it. Among the blobs listed, every setting
    /// of a batch has a place, so one batch gives `limit + 1` hits if there are that many; among all, a batch may have settings of
    /// blobs that are no file's now, and the next batch is read (a bounded number of times) until there are enough.
    async fn scan_settings(
        &self,
        among: &Among,
        pattern: &str,
        (lanes, file): (&[String], Option<&str>),
        (start_blob, start_path): (&[u8], &str),
        after: Option<&After>,
        limit: usize,
    ) -> Result<Scan, ReadError> {
        let batch = limit + 2;
        let tested = matches!(among, Among::Listed(_));
        let mut hits: Vec<Positioned> = Vec::new();
        let mut at = (start_blob.to_vec(), start_path.to_owned());
        let mut scanned: Option<(Vec<u8>, String)> = None;
        let mut exhausted = false;
        for round in 0..if tested { 1 } else { MAX_BATCHES } {
            let mut pairs = self
                .setting_pairs(
                    among,
                    pattern,
                    (&at.0, &at.1),
                    i64::try_from(batch).unwrap_or(i64::MAX),
                )
                .await?;
            let fetched = pairs.len();
            // A later batch starts at the last setting of the one before, which is not read twice.
            if let Some((blob, path)) = &scanned {
                if pairs
                    .first()
                    .is_some_and(|p| p.blob.as_bytes().as_slice() == blob.as_slice() && p.path == *path)
                {
                    pairs.remove(0);
                }
            }
            if pairs.is_empty() {
                exhausted = true;
                break;
            }
            let by_blob = self.setting_places(lanes, &pairs, file).await?;
            hits.extend(positioned(
                &pairs,
                &by_blob,
                if round == 0 { after } else { None },
            )?);
            if let Some(last) = pairs.last() {
                scanned = Some((last.blob.as_bytes().to_vec(), last.path.clone()));
                at = (last.blob.as_bytes().to_vec(), last.path.clone());
            }
            if hits.len() > limit {
                break;
            }
            if fetched < batch {
                exhausted = true;
                break;
            }
        }
        Ok(Scan {
            hits,
            scanned,
            exhausted,
        })
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
        let among = if file.is_some() || lanes.len() <= self.config.search_tested_lanes {
            self.lane_blobs(&lanes, file.as_deref()).await?
        } else {
            Among::Everything
        };
        if matches!(&among, Among::Listed(blobs) if blobs.is_empty()) {
            return Ok(Paged::last(Vec::new()));
        }
        let tested = matches!(among, Among::Listed(_));
        let Scan {
            mut hits,
            scanned,
            exhausted,
        } = self
            .scan_settings(
                &among,
                &pattern,
                (&lanes, file.as_deref()),
                (start_blob, start_path),
                after.as_ref(),
                limit,
            )
            .await?;
        let more = hits.len() > limit;
        hits.truncate(limit);
        let key = if more {
            hits.last().map(|h| {
                vec![
                    Field::Bytes(h.blob.clone()),
                    Field::text(&h.path),
                    Field::text(&h.lane),
                    Field::Int(h.seq),
                ]
            })
        } else if !exhausted && !tested {
            // The batches did not fill the page: it is returned as it is, with the cursor of the setting the search got to (after all
            // of its places: `~` sorts after every swimlane id).
            scanned.as_ref().map(|(blob, path)| {
                vec![
                    Field::Bytes(blob.clone()),
                    Field::text(path),
                    Field::text("~"),
                    Field::Int(i64::MAX),
                ]
            })
        } else {
            None
        };
        let next = key
            .map(|k| cursor::issue(Kind::SettingsSearch, &context, k))
            .transpose()?;
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
