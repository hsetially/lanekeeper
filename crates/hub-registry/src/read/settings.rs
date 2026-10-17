//! The setting rows of a blob, for the reads that never parse YAML (D65, P7, S13).
//!
//! Grid, effective config, settings search and the setting changes of a compare are built on `settings_index`: a row for every setting
//! of every unique structured blob, written once, off the acknowledgement path (T2). A read gets the rows of a blob in one statement
//! (one row per blob, the columns as arrays, so a 6,000-line file is one row on the wire and not six thousand) and keeps them in a
//! cache keyed by the blob's hash, which never goes stale.
//!
//! # What the rows are
//!
//! - **Flagged values are already redacted.** The index stores a value that the secret scan flagged as `[REDACTED: N chars, rule]`
//!   (S13), so everything built from these rows shows the marker and never the value (A28). The same marker is stored for any two
//!   flagged values of one length and rule, so two *different* secrets compare equal as rows. The compare therefore reads the real
//!   bytes of a blob that has a flagged row (see [`super::compare`]); the grid and the effective config do not, and a change between
//!   two secrets of the same length and rule is hidden by "only differing" there (a recorded residual risk, T10).
//! - **A blob the index has not reached yet** (no marker row: the indexer runs after the acknowledgement, the sweeper catches up every
//!   few minutes) is indexed here, in memory, from its bytes with the same function the indexer uses ([`crate::index::index_blob`]),
//!   so a read right after a change shows the change and never an empty file. Nothing is written: the indexer's own write is
//!   idempotent and will come.
//! - **The `yes`/`no`/`on`/`off` flag** is not stored in the index, so it is read back from the text: a string value with one of the
//!   twelve spellings the engine flags. A *quoted* `"yes"` is a plain string to the engine and gets the flag here too; the flag only
//!   asks the reader to look twice, and an over-flag is the safe side.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use domain::{ContentHash, LineRange, SettingPath, SettingValue, ValueFlag, ValueType};
use engine::{FlatSetting, Syntax};
use futures::stream::{self, StreamExt};
use ports::ReadError;
use tracing::warn;

use super::array_bin::{self, Array};
use super::cache::BlobRows;
use super::service::Inner;
use crate::index::{Indexed, index_blob};
use crate::metrics::CacheName;

/// The rows of up to this many blobs are read in one statement: a few, so that a read of many blobs is several statements that the
/// database works on at once, and a single large blob (6,000 settings and more) is a statement of its own.
pub const ROWS_PER_STATEMENT: usize = 2;
/// How many of those statements are in flight at once for one read.
const STATEMENTS_AT_ONCE: usize = 4;

/// One row per blob: its marker (`NULL` when the indexer has not seen the blob) and its settings as arrays. The arrays are in primary
/// key order, which is path order; nothing downstream needs file order (the merge and the diff sort by path).
pub const ROWS_SQL: &str = "\
SELECT b.h::bytea AS hash, m.parse_ok, r.paths, r.vals, r.types, r.starts, r.ends, r.rules \
FROM unnest($1::bytea[]) AS b(h) \
LEFT JOIN settings_index_blobs m ON m.blob_hash = b.h \
LEFT JOIN LATERAL ( \
  SELECT array_agg(setting_path) AS paths, array_agg(value_text) AS vals, array_agg(value_type) AS types, \
         array_agg(start_line) AS starts, array_agg(end_line) AS ends, array_agg(secret_rule) AS rules \
  FROM settings_index s WHERE s.blob_hash = b.h) r ON true LIMIT $2";

/// What one statement found: the rows of the blobs the index has, and the blobs it has not reached.
#[derive(Default)]
struct Loaded {
    rows: Vec<(ContentHash, BlobRows)>,
    unindexed: Vec<(ContentHash, Syntax)>,
}

enum Converted {
    /// The indexer has not seen the blob.
    Unindexed,
    Rows(BlobRows),
}

/// The array in a column of a [`ROWS_SQL`] row: binary, as the driver asks the server for every column, and empty for the `NULL` that
/// `array_agg` gives over no rows.
fn column<'r>(row: &'r sqlx::postgres::PgRow, name: &str, oid: u32) -> Result<Array<'r>, ReadError> {
    use sqlx::{Row as _, ValueRef as _};
    let unavailable = |what: &str| {
        warn!(column = name, what, "a column of an index row cannot be read");
        ReadError::Unavailable
    };
    let value = row.try_get_raw(name).map_err(|_| unavailable("missing"))?;
    if value.is_null() {
        return Ok(Array::empty());
    }
    if value.format() != sqlx::postgres::PgValueFormat::Binary {
        return Err(unavailable("not binary"));
    }
    let bytes = value.as_bytes().map_err(|_| unavailable("no bytes"))?;
    Array::parse(bytes, oid).map_err(|_| unavailable("not an array of the type"))
}

/// An array of an index row that is cut, or holds a null where there is none.
fn malformed(_: array_bin::Malformed) -> ReadError {
    warn!("an array of an index row is cut or holds a null where there is none");
    ReadError::Unavailable
}

/// The next element of an array that has `n` of them (the caller has checked the count).
fn next<'a>(elements: &mut array_bin::Elements<'a>) -> Result<Option<&'a [u8]>, ReadError> {
    elements.next().ok_or(ReadError::Unavailable)?.map_err(malformed)
}

/// The settings of a blob out of the six arrays of its row, read in place: the strings made are the path (checked by the domain, which
/// copies it) and the value, and nothing else.
fn settings_of_row(row: &sqlx::postgres::PgRow) -> Result<BlobRows, ReadError> {
    let (paths, vals, types) = (
        column(row, "paths", array_bin::TEXT)?,
        column(row, "vals", array_bin::TEXT)?,
        column(row, "types", array_bin::TEXT)?,
    );
    let (starts, ends, rules) = (
        column(row, "starts", array_bin::INT4)?,
        column(row, "ends", array_bin::INT4)?,
        column(row, "rules", array_bin::TEXT)?,
    );
    let n = paths.len();
    if [vals.len(), types.len(), starts.len(), ends.len(), rules.len()]
        .iter()
        .any(|len| *len != n)
    {
        warn!(rows = n, "the columns of an index row do not line up");
        return Err(ReadError::Unavailable);
    }
    let mut settings = Vec::with_capacity(n);
    let mut secret = Vec::with_capacity(n);
    let (mut paths, mut vals, mut types) = (paths.iter(), vals.iter(), types.iter());
    let (mut starts, mut ends, mut rules) = (starts.iter(), ends.iter(), rules.iter());
    // Each array has `n` elements (checked above), and the iterators say so when they do not.
    for _ in 0..n {
        let path = array_bin::text(next(&mut paths)?).map_err(malformed)?;
        let value = array_bin::text(next(&mut vals)?).map_err(malformed)?;
        let ty = array_bin::text(next(&mut types)?).map_err(malformed)?;
        let start = array_bin::int4(next(&mut starts)?).map_err(malformed)?;
        let end = array_bin::int4(next(&mut ends)?).map_err(malformed)?;
        // Only whether a rule flagged the value is read, and it is the null that says it.
        let rule = next(&mut rules)?;
        if let Some(s) = setting(path, value, ty, start, end) {
            settings.push(s);
            secret.push(rule.is_some());
        }
    }
    Ok(BlobRows::new(settings, secret, false))
}

/// One row of [`ROWS_SQL`] as the rows of its blob.
fn convert_row(row: &sqlx::postgres::PgRow) -> Result<(ContentHash, Converted), ReadError> {
    use sqlx::Row as _;
    let hash = crate::read::rows::hash(&row.get::<Vec<u8>, _>("hash"))?;
    let parse_ok: Option<bool> = row.get("parse_ok");
    let rows = match parse_ok {
        None => return Ok((hash, Converted::Unindexed)),
        Some(false) => BlobRows::new(Vec::new(), Vec::new(), true),
        Some(true) => settings_of_row(row)?,
    };
    Ok((hash, Converted::Rows(rows)))
}

/// The type of a stored `value_type`.
fn value_type(name: &str) -> ValueType {
    match name {
        "int" => ValueType::Int,
        "float" => ValueType::Float,
        "bool" => ValueType::Bool,
        "null" => ValueType::Null,
        "other" => ValueType::Other,
        _ => ValueType::String,
    }
}

/// The flag the engine gives a plain `yes`, `no`, `on` or `off`, read back from the text (the index does not store it).
pub fn flag_of(ty: ValueType, text: &str) -> Option<ValueFlag> {
    let boolean_like = matches!(
        text,
        "yes" | "Yes" | "YES" | "no" | "No" | "NO" | "on" | "On" | "ON" | "off" | "Off" | "OFF"
    );
    (ty == ValueType::String && boolean_like).then_some(ValueFlag::YamlBooleanLike)
}

/// A setting from the parts of an index row. `None` for a row that cannot be one (a path or a line range the domain refuses): it is
/// left out, as a setting that the engine's own types cannot carry.
fn setting(path: &str, value: &str, ty: &str, start: i32, end: i32) -> Option<FlatSetting> {
    let ty = value_type(ty);
    Some(FlatSetting {
        path: SettingPath::parse(path).ok()?,
        value: SettingValue {
            text: value.to_owned(),
            ty,
            flag: flag_of(ty, value),
        },
        lines: LineRange::new(u32::try_from(start).ok()?, u32::try_from(end).ok()?).ok()?,
        via_alias: false,
    })
}

/// The rows of a blob that was indexed here, in memory.
fn rows_from_index(indexed: Indexed) -> BlobRows {
    match indexed {
        Indexed::Unparseable => BlobRows::new(Vec::new(), Vec::new(), true),
        Indexed::Rows(rows) => {
            let mut settings = Vec::with_capacity(rows.len());
            let mut secret = Vec::with_capacity(rows.len());
            for row in &rows {
                if let Some(s) = setting(&row.path, &row.value, row.ty, row.start_line, row.end_line) {
                    settings.push(s);
                    secret.push(row.secret_rule.is_some());
                }
            }
            BlobRows::new(settings, secret, false)
        }
    }
}

impl Inner {
    /// The rows of these blobs, each blob once. `syntax` tells how to read a blob that the index has not reached (a blob that is not
    /// structured has no rows and is not asked for).
    pub async fn rows_of(
        &self,
        wanted: &[(ContentHash, Syntax)],
    ) -> Result<HashMap<ContentHash, Arc<BlobRows>>, ReadError> {
        let mut found: HashMap<ContentHash, Arc<BlobRows>> = HashMap::with_capacity(wanted.len());
        let mut missing: Vec<(ContentHash, Syntax)> = Vec::new();
        let mut asked: std::collections::HashSet<ContentHash> =
            std::collections::HashSet::with_capacity(wanted.len());
        for (hash, syntax) in wanted {
            if !asked.insert(*hash) {
                continue;
            }
            if let Some(rows) = self.caches.rows.get(hash) {
                self.caches.count(CacheName::Rows, true);
                found.insert(*hash, rows);
            } else {
                self.caches.count(CacheName::Rows, false);
                missing.push((*hash, *syntax));
            }
        }
        // The statements run side by side, and each answer is turned into rows on the read pool, off the async threads (rule 4): a large
        // blob is tens of thousands of strings.
        let loaded: Vec<Loaded> = stream::iter(
            missing
                .chunks(ROWS_PER_STATEMENT)
                .map(<[(ContentHash, Syntax)]>::to_vec),
        )
        .map(|chunk| async move { self.load_rows(chunk).await })
        .buffer_unordered(STATEMENTS_AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()?;
        let mut unindexed: Vec<(ContentHash, Syntax)> = Vec::new();
        for Loaded {
            rows,
            unindexed: more,
        } in loaded
        {
            for (hash, blob_rows) in rows {
                self.keep(&mut found, hash, blob_rows);
            }
            unindexed.extend(more);
        }
        if !unindexed.is_empty() {
            self.index_in_memory(&unindexed, &mut found).await?;
        }
        Ok(found)
    }

    /// One statement: the rows of these blobs, converted. The blobs the index has not reached are returned for the caller to index.
    async fn load_rows(&self, chunk: Vec<(ContentHash, Syntax)>) -> Result<Loaded, ReadError> {
        let hashes: Vec<Vec<u8>> = chunk.iter().map(|(h, _)| h.as_bytes().to_vec()).collect();
        let fetched = self
            .db
            .run(
                "read_rows",
                sqlx::query(ROWS_SQL)
                    .bind(&hashes)
                    .bind(i64::try_from(chunk.len()).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let pool = Arc::clone(&self.pool);
        let converted = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            pool.install(|| {
                fetched
                    .into_par_iter()
                    .map(|row| convert_row(&row))
                    .collect::<Result<Vec<_>, ReadError>>()
            })
        })
        .await
        .map_err(|_| {
            warn!("a row conversion task on the blocking pool did not finish");
            ReadError::Unavailable
        })??;
        let mut loaded = Loaded::default();
        for (hash, outcome) in converted {
            match outcome {
                Converted::Unindexed => {
                    if let Some((_, syntax)) = chunk.iter().find(|(h, _)| *h == hash) {
                        loaded.unindexed.push((hash, *syntax));
                    }
                }
                Converted::Rows(rows) => loaded.rows.push((hash, rows)),
            }
        }
        Ok(loaded)
    }

    fn keep(&self, found: &mut HashMap<ContentHash, Arc<BlobRows>>, hash: ContentHash, rows: BlobRows) {
        let rows = Arc::new(rows);
        self.caches.rows.insert(hash, Arc::clone(&rows));
        found.insert(hash, rows);
    }

    /// Index blobs the settings indexer has not reached, from their bytes, on the read pool (rule 4). A blob the store does not have is
    /// left out: the caller sees it as a file with no settings, as it would a file that does not parse.
    async fn index_in_memory(
        &self,
        unindexed: &[(ContentHash, Syntax)],
        found: &mut HashMap<ContentHash, Arc<BlobRows>>,
    ) -> Result<(), ReadError> {
        let hashes: Vec<ContentHash> = unindexed.iter().map(|(h, _)| *h).collect();
        let blobs = self.get_many_bounded(&hashes).await?;
        let work: Vec<(ContentHash, Syntax, Bytes)> = unindexed
            .iter()
            .filter_map(|(h, s)| blobs.get(h).map(|b| (*h, *s, b.clone())))
            .collect();
        let (pool, limits, scan) = (Arc::clone(&self.pool), self.limits.clone(), self.scan.clone());
        let indexed = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            pool.install(|| {
                work.par_iter()
                    .map(|(hash, syntax, bytes)| {
                        (*hash, rows_from_index(index_blob(bytes, *syntax, &limits, &scan)))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .await
        .map_err(|_| {
            warn!("an indexing task on the blocking pool did not finish");
            ReadError::Unavailable
        })?;
        for (hash, rows) in indexed {
            self.keep(found, hash, rows);
        }
        Ok(())
    }

    /// The rows of bytes that are not (necessarily) in the store, such as a file read from Git: indexed in memory with the indexer's own
    /// function, on the read pool, and kept in the cache under the hash of the bytes.
    pub async fn rows_from_bytes(
        &self,
        hash: ContentHash,
        syntax: Syntax,
        bytes: Bytes,
    ) -> Result<Arc<BlobRows>, ReadError> {
        if let Some(rows) = self.caches.rows.get(&hash) {
            self.caches.count(CacheName::Rows, true);
            return Ok(rows);
        }
        self.caches.count(CacheName::Rows, false);
        let (pool, limits, scan) = (Arc::clone(&self.pool), self.limits.clone(), self.scan.clone());
        let rows = tokio::task::spawn_blocking(move || {
            pool.install(|| rows_from_index(index_blob(&bytes, syntax, &limits, &scan)))
        })
        .await
        .map_err(|_| {
            warn!("an indexing task on the blocking pool did not finish");
            ReadError::Unavailable
        })?;
        let rows = Arc::new(rows);
        self.caches.rows.insert(hash, Arc::clone(&rows));
        Ok(rows)
    }

    /// `BlobStore::get_many` in pieces of the port's limit, with the blob timeout on each call, as a map.
    pub async fn get_many_bounded(
        &self,
        hashes: &[ContentHash],
    ) -> Result<HashMap<ContentHash, Bytes>, ReadError> {
        let mut out = HashMap::with_capacity(hashes.len());
        for chunk in hashes.chunks(ports::MAX_GET_MANY) {
            let got = tokio::time::timeout(self.config.blob_timeout, self.blobs.get_many(chunk))
                .await
                .map_err(|_| {
                    warn!(op = "read_blobs", why = "timed_out", "blob read failed");
                    ReadError::Unavailable
                })?
                .map_err(|e| {
                    warn!(op = "read_blobs", ?e, "blob read failed");
                    ReadError::Unavailable
                })?;
            out.extend(got);
        }
        Ok(out)
    }

    /// One blob, with the blob timeout. `None` when the store does not have it.
    pub async fn get_blob(&self, hash: &ContentHash) -> Result<Option<Bytes>, ReadError> {
        tokio::time::timeout(self.config.blob_timeout, self.blobs.get(hash))
            .await
            .map_err(|_| {
                warn!(op = "read_blob", why = "timed_out", "blob read failed");
                ReadError::Unavailable
            })?
            .map_err(|e| {
                warn!(op = "read_blob", ?e, "blob read failed");
                ReadError::Unavailable
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_is_given_to_a_plain_yes_and_to_nothing_else() {
        for t in ["yes", "No", "ON", "off"] {
            assert_eq!(
                flag_of(ValueType::String, t),
                Some(ValueFlag::YamlBooleanLike),
                "{t}"
            );
        }
        assert_eq!(flag_of(ValueType::Bool, "true"), None);
        assert_eq!(
            flag_of(ValueType::Bool, "yes"),
            None,
            "a typed bool is not the 1.1 spelling"
        );
        assert_eq!(
            flag_of(ValueType::String, "YeS"),
            None,
            "only the three spellings the engine knows"
        );
        assert_eq!(flag_of(ValueType::String, "maybe"), None);
    }

    #[test]
    fn a_row_the_domain_cannot_carry_is_left_out() {
        assert!(setting("a.b", "v", "string", 1, 1).is_some());
        assert!(setting("a.b", "v", "string", 0, 1).is_none(), "lines start at 1");
        assert!(
            setting("a.b", "v", "string", 3, 2).is_none(),
            "a range does not run backwards"
        );
        assert!(
            setting("", "v", "string", 1, 1).is_none(),
            "a setting path is not empty"
        );
        assert_eq!(value_type("nonsense"), ValueType::String);
    }
}
