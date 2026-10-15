//! The file tree and the drift list (T6, P7: 100 ms, P12).
//!
//! # The tree
//!
//! The direct children of a prefix, ordered by name, keyset on the name. One aggregate statement reads them from `file_state`: a
//! range over `(swimlane_id, path text_pattern_ops)` for the prefix, the first path component after the prefix as the child, grouped
//! by child. A file is a child with no further `/`; a folder is a child that has descendants, and its drift state is the worst
//! among them (A25: `conflict`, `nfs_ahead`, `git_ahead`, `unknown`, `untracked`, `intentional_divergence`, `in_sync`), its severity the worst
//! among them. A second statement reads the rows of the files of the page with `= ANY($1)`, with the attribution of the observation
//! each row points at, so a page costs two statements however long it is. The names are ordered bytewise (`COLLATE "C"`), the order
//! of `NfsPath`, whatever the database's locale is.
//!
//! A folder entry has no file facts. The domain type has no "directory" variant, so a folder carries `kind: base`, `class:
//! structured`, `role: resource`, size 0 and no hash, no format, no attribution and no pickup state: what is true of a folder is
//! `is_dir`, its path, its drift state and its severity.
//!
//! # The drift list
//!
//! The files of a swimlane that are not in sync (or are in the one state asked for), flat, **most serious first** (D76, D103: the rules'
//! severity of each file's pending change; a row nobody has rated comes last) and, among equals, in the order the rows were created
//! (`file_state.seq`). The keyset is the pair (severity rank, `seq`): short, so that it fits a cursor (A31), and it totally orders the
//! list, so a page resumes exactly where the last one ended. The statement already sorted the swimlane's non-synced rows by `seq` (they
//! are read through `file_state_state_idx`, which is not in that order); a different sort key costs the same.

use domain::{
    Cursor, DriftState, FileClass, FileEntry, FileKind, FileRole, NfsPath, Page, Paged, SwimlaneId,
};
use engine::DenyMatcher;
use ports::ReadError;
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::cursor::{self, Context, Field, Kind};
use super::rows::{
    self, SEVERITY_RANK_TOP, fs_severity_rank_sql, like_escape, severity_rank_sql, state_rank_sql,
};
use super::service::Inner;
use crate::error::RegistryError;

/// The children of a prefix: `$1` swimlane, `$2` the `LIKE` pattern of the prefix, `$3` the character position where the part after the
/// prefix starts, `$4` a drift state or `NULL`, `$5` the most rows read, `$6` the name after which to resume or `NULL`, `$7` the
/// page size plus one.
pub const CHILDREN_SQL: &str = concat!(
    "SELECT child, bool_or(is_dir) AS is_dir, min(rank)::int AS rank, max(sev)::int AS sev FROM ( \
       SELECT split_part(rest, '/', 1) AS child, strpos(rest, '/') > 0 AS is_dir, ",
    state_rank_sql!(),
    " AS rank, ",
    severity_rank_sql!(),
    " AS sev FROM ( \
         SELECT substr(path, $3) AS rest, state, severity FROM file_state \
         WHERE swimlane_id = $1 AND path LIKE $2 AND ($4::text IS NULL OR state = $4) LIMIT $5) f \
     ) g \
     WHERE child <> '' \
     GROUP BY child \
     HAVING ($6::text IS NULL OR child COLLATE \"C\" > $6) \
     ORDER BY child COLLATE \"C\" LIMIT $7"
);

/// The columns of a file entry. `fs` is `file_state`, `o` the observation the row points at.
macro_rules! entry_columns {
    () => {
        "fs.path, fs.kind, fs.class, fs.role, fs.nfs_hash::bytea AS hash, fs.size, fs.state, fs.severity, fs.eol_style, \
         fs.encoding, fs.bom, fs.denied, o.attribution_source, o.attribution_confidence, o.attribution_actor, \
         o.attribution_evidence"
    };
}

/// The rows of the files of a page of the tree.
pub const FILE_ROWS_SQL: &str = concat!(
    "SELECT ",
    entry_columns!(),
    " FROM file_state fs LEFT JOIN file_observations o ON o.id = fs.last_observation_id \
      WHERE fs.swimlane_id = $1 AND fs.path = ANY($2::text[])"
);

/// The drift list: `$1` swimlane, `$2` the `seq` after which to resume among the rows of the severity `$6` (0 for the first page), `$3` a
/// state or `NULL` (every state but in sync), `$4` the `LIKE` pattern of the prefix, `$5` the page size plus one, `$6` the severity rank at
/// which to resume ([`SEVERITY_RANK_TOP`] for the first page). Most serious first, then by `seq`.
pub const DRIFT_SQL: &str = concat!(
    "SELECT fs.seq, ",
    fs_severity_rank_sql!(),
    " AS sev_rank, ",
    entry_columns!(),
    " FROM file_state fs LEFT JOIN file_observations o ON o.id = fs.last_observation_id \
      WHERE fs.swimlane_id = $1 AND ((",
    fs_severity_rank_sql!(),
    " < $6) OR (",
    fs_severity_rank_sql!(),
    " = $6 AND fs.seq > $2)) \
        AND ((($3::text) IS NULL AND fs.state <> 'in_sync') OR fs.state = $3) \
        AND fs.path LIKE $4 \
      ORDER BY ",
    fs_severity_rank_sql!(),
    " DESC, fs.seq LIMIT $5"
);

/// The `LIKE` pattern of "below this prefix": `prefix/%`, or everything for the root.
pub(crate) fn below(prefix: &NfsPath) -> String {
    if prefix.is_root() {
        "%".to_owned()
    } else {
        format!("{}/%", like_escape(prefix.as_str()))
    }
}

/// The 1-based character position of the first character after `prefix/`.
fn rest_position(prefix: &NfsPath) -> i32 {
    if prefix.is_root() {
        1
    } else {
        i32::try_from(prefix.as_str().chars().count() + 2).unwrap_or(i32::MAX)
    }
}

/// A file entry from a row of [`FILE_ROWS_SQL`] or [`DRIFT_SQL`].
pub(crate) fn file_entry(row: &PgRow, deny: &DenyMatcher) -> Result<FileEntry, RegistryError> {
    let path: String = row.get("path");
    let kind: String = row.get("kind");
    let class: String = row.get("class");
    let role: Option<String> = row.get("role");
    let hash: Option<Vec<u8>> = row.get("hash");
    let size: Option<i64> = row.get("size");
    let state: String = row.get("state");
    let severity: Option<String> = row.get("severity");
    let (eol, encoding, bom): (Option<String>, Option<String>, Option<bool>) =
        (row.get("eol_style"), row.get("encoding"), row.get("bom"));
    let flagged: bool = row.get("denied");
    let source: Option<String> = row.get("attribution_source");
    let confidence: Option<String> = row.get("attribution_confidence");
    let actor: Option<serde_json::Value> = row.get("attribution_actor");
    let evidence: Option<serde_json::Value> = row.get("attribution_evidence");

    let attribution = match (source, confidence) {
        (Some(source), Some(confidence)) => Some(rows::attribution(
            &source,
            &confidence,
            actor,
            evidence.unwrap_or(serde_json::Value::Null),
        )),
        _ => None,
    };
    let path = NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?;
    let class = rows::class(&class)?;
    // The class says `denied` for a denied file, and its content is withheld (D79): the flag, the class, or the hub's own globs (D104).
    let denied = crate::denied::is_denied(deny, &path, flagged, class);
    Ok(FileEntry {
        path,
        is_dir: false,
        kind: rows::kind(&kind)?,
        class: if denied { FileClass::Denied } else { class },
        role: rows::role(role.as_deref())?,
        size: u64::try_from(size.unwrap_or(0)).unwrap_or(0),
        hash: rows::opt_hash(hash.as_deref())?,
        drift: rows::state(&state)?,
        format: rows::file_format(eol.as_deref(), encoding.as_deref(), bom)?,
        content_withheld: denied,
        attribution,
        severity: rows::severity(severity.as_deref())?,
        pickup_state: None,
    })
}

/// A folder entry.
fn dir_entry(path: NfsPath, drift: DriftState, severity: Option<domain::Severity>) -> FileEntry {
    FileEntry {
        path,
        is_dir: true,
        kind: FileKind::Base,
        class: FileClass::Structured,
        role: FileRole::Resource,
        size: 0,
        hash: None,
        drift,
        format: None,
        content_withheld: false,
        attribution: None,
        severity,
        pickup_state: None,
    }
}

impl Inner {
    /// The direct children of `prefix`, with the drift filter of the REST operation.
    pub async fn tree_page(
        &self,
        swimlane: &SwimlaneId,
        prefix: &NfsPath,
        drift: Option<DriftState>,
        page: &Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.require_swimlane(swimlane).await?;
        let context = Context::of(
            Kind::Tree,
            &[
                swimlane.as_str(),
                prefix.as_str(),
                drift.map_or("", DriftState::as_str),
            ],
        );
        let after = match cursor::resume(page, Kind::Tree, &context)? {
            Some(fields) => {
                cursor::arity(&fields, 1).map_err(|_| ReadError::BadRequest)?;
                Some(cursor::text(&fields, 0).map_err(|_| ReadError::BadRequest)?)
            }
            None => None,
        };
        let limit = page.limit() as usize;
        let children = self
            .db
            .run(
                "read_tree",
                sqlx::query(CHILDREN_SQL)
                    .bind(swimlane.as_str())
                    .bind(below(prefix))
                    .bind(rest_position(prefix))
                    .bind(drift.map(DriftState::as_str))
                    .bind(i64::from(self.config.tree_scan_rows))
                    .bind(after)
                    .bind(i64::try_from(limit + 1).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let has_more = children.len() > limit;
        let page_rows = &children[..children.len().min(limit)];

        // The files of the page, in one statement.
        let mut file_paths: Vec<String> = Vec::new();
        let mut wanted: Vec<(String, bool, i32, Option<i32>)> = Vec::with_capacity(page_rows.len());
        for row in page_rows {
            let child: String = row.get("child");
            let is_dir: bool = row.get("is_dir");
            let rank: i32 = row.get("rank");
            let sev: Option<i32> = row.get("sev");
            let full = if prefix.is_root() {
                child.clone()
            } else {
                format!("{prefix}/{child}")
            };
            if !is_dir {
                file_paths.push(full.clone());
            }
            wanted.push((full, is_dir, rank, sev));
        }
        let mut files = std::collections::HashMap::with_capacity(file_paths.len());
        if !file_paths.is_empty() {
            let fetched = self
                .db
                .run(
                    "read_tree_files",
                    sqlx::query(FILE_ROWS_SQL)
                        .bind(swimlane.as_str())
                        .bind(&file_paths)
                        .fetch_all(self.db.pool()),
                )
                .await?;
            for row in &fetched {
                let entry = file_entry(row, &self.deny)?;
                files.insert(entry.path.as_str().to_owned(), entry);
            }
        }
        let mut items = Vec::with_capacity(wanted.len());
        for (full, is_dir, rank, sev) in wanted {
            let path = NfsPath::parse(&full).map_err(|_| RegistryError::Corrupt)?;
            if is_dir {
                items.push(dir_entry(
                    path,
                    rows::state_of_rank(rank),
                    rows::severity_of_rank(sev),
                ));
            } else if let Some(entry) = files.remove(&full) {
                items.push(entry);
            }
            // A file that was deleted between the two statements is not on the page; the next read is right.
        }
        let next = if has_more {
            page_rows
                .last()
                .map(|r| {
                    let child: String = r.get("child");
                    cursor::issue(Kind::Tree, &context, vec![Field::text(&child)])
                })
                .transpose()?
        } else {
            None
        };
        Ok(Paged { items, next })
    }

    /// The files that are not in sync (or are in `state`) below `prefix`.
    pub async fn drift_page(
        &self,
        swimlane: &SwimlaneId,
        state: Option<DriftState>,
        prefix: &NfsPath,
        page: &Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.require_swimlane(swimlane).await?;
        let context = Context::of(
            Kind::Drift,
            &[
                swimlane.as_str(),
                state.map_or("", DriftState::as_str),
                prefix.as_str(),
            ],
        );
        // Where the list resumes: the severity rank and the `seq` of the last row of the page before (the top and 0 for the first page).
        let (rank_after, seq_after) = match cursor::resume(page, Kind::Drift, &context)? {
            Some(fields) => {
                cursor::arity(&fields, 2).map_err(|_| ReadError::BadRequest)?;
                let rank = cursor::int(&fields, 0).map_err(|_| ReadError::BadRequest)?;
                // A rank outside the ones the list has was not issued by it.
                if !(-1..i64::from(SEVERITY_RANK_TOP)).contains(&rank) {
                    return Err(ReadError::BadRequest);
                }
                (rank, cursor::int(&fields, 1).map_err(|_| ReadError::BadRequest)?)
            }
            None => (i64::from(SEVERITY_RANK_TOP), 0),
        };
        let limit = page.limit() as usize;
        let fetched = self
            .db
            .run(
                "read_drift",
                sqlx::query(DRIFT_SQL)
                    .bind(swimlane.as_str())
                    .bind(seq_after)
                    .bind(state.map(DriftState::as_str))
                    .bind(below(prefix))
                    .bind(i64::try_from(limit + 1).unwrap_or(i64::MAX))
                    .bind(i32::try_from(rank_after).unwrap_or(SEVERITY_RANK_TOP))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let has_more = fetched.len() > limit;
        let kept = &fetched[..fetched.len().min(limit)];
        let items = kept
            .iter()
            .map(|r| file_entry(r, &self.deny))
            .collect::<Result<Vec<_>, _>>()?;
        let next: Option<Cursor> = if has_more {
            kept.last()
                .map(|r| {
                    let seq: i64 = r.get("seq");
                    let rank: i32 = r.get("sev_rank");
                    cursor::issue(
                        Kind::Drift,
                        &context,
                        vec![Field::Int(i64::from(rank)), Field::Int(seq)],
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok(Paged { items, next })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pattern_of_a_prefix_escapes_what_like_means() {
        assert_eq!(below(&NfsPath::root()), "%");
        assert_eq!(below(&NfsPath::parse("a/b").unwrap()), "a/b/%");
        assert_eq!(below(&NfsPath::parse("a_b%c").unwrap()), "a\\_b\\%c/%");
    }

    #[test]
    fn the_part_after_the_prefix_starts_after_the_slash() {
        assert_eq!(rest_position(&NfsPath::root()), 1);
        // "a/" is two characters, so the rest starts at the third.
        assert_eq!(rest_position(&NfsPath::parse("a").unwrap()), 3);
        // Characters, not bytes: `é/` is two characters.
        assert_eq!(rest_position(&NfsPath::parse("é").unwrap()), 3);
    }
}
