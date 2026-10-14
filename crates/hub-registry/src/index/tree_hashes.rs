//! Effective tree hashes (T2, D65, D67, P7): one hash per directory of a swimlane's effective tree, and the effective hash of each
//! logical file, so that comparing two swimlanes descends only into directories whose hashes differ.
//!
//! # What is stored
//!
//! - `effective_tree_hashes (swimlane_id, dir_path, hash)`: the hash of every directory that holds a served file, the root being
//!   `dir_path ''`. The encoding is the engine's (`engine::effective_tree_hash`); this crate never computes one.
//! - `effective_files (swimlane_id, logical_file, hash)`: the leaves under those directories, which a compare needs once it has found
//!   a directory that differs.
//!
//! Both come from an [`engine::LaneState`]. They are derived facts: the pipeline (T3) is their only writer and can rebuild them from
//! the source facts and Git, which is what the nightly job does ([`TreeHashStore::sync`]).
//!
//! # Incremental
//!
//! After a recompute of some logical files, [`TreeHashStore::apply`] writes only what those files can have changed: the files
//! themselves and the directories above them (the engine rehashed exactly those). A row is rewritten only when its hash differs
//! (`ON CONFLICT DO UPDATE ... WHERE hash IS DISTINCT FROM`), so a recompute that changes nothing writes nothing, and a directory that
//! lost its last file is deleted. [`TreeHashStore::sync`] does the same for a whole swimlane by reading what is stored and writing the
//! difference; it is the repair, and the proof that `apply` kept the tables right.
//!
//! The caller passes its open transaction, so the tables commit with the rest of the pipeline's changes. Nothing here is audited: they
//! are derived columns (plan A12, D93).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use domain::{ContentHash, LogicalFile, NfsPath, SwimlaneId};
use engine::{LaneState, MAX_LANE_FILES};

use crate::clock::Clock;
use crate::config::RegistryConfig;
use crate::db::{Db, Txn, ts};
use crate::error::RegistryError;
use crate::metrics::RegistryMetrics;

/// The reads of a swimlane's stored tree, as constants so that the EXPLAIN snapshot test takes exactly what runs (P12). Both are served
/// by the primary key `(swimlane_id, dir_path)` and `(swimlane_id, logical_file)`.
pub const LOAD_DIRS_SQL: &str = "SELECT dir_path, hash::bytea FROM effective_tree_hashes WHERE swimlane_id = $1 ORDER BY dir_path LIMIT $2";
pub const LOAD_FILES_SQL: &str = "SELECT logical_file, hash::bytea FROM effective_files WHERE swimlane_id = $1 ORDER BY logical_file LIMIT $2";

/// What a call wrote, in rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeWrite {
    pub dirs_written: u64,
    pub dirs_deleted: u64,
    pub files_written: u64,
    pub files_deleted: u64,
}

impl TreeWrite {
    pub fn total(&self) -> u64 {
        self.dirs_written + self.dirs_deleted + self.files_written + self.files_deleted
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// What the tables hold for a swimlane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTree {
    /// Directory (the root is `""`) to its hash.
    pub dirs: BTreeMap<String, ContentHash>,
    /// Logical file to its effective hash.
    pub files: BTreeMap<String, ContentHash>,
}

#[derive(Clone)]
pub struct TreeHashStore {
    db: Db,
    clock: Arc<dyn Clock>,
    metrics: RegistryMetrics,
    rows: usize,
}

impl std::fmt::Debug for TreeHashStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeHashStore").finish_non_exhaustive()
    }
}

/// The directory of a logical file: everything before the last `/`, or `""` for a file in the root.
fn dir_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Every directory above the files, the root included, each once.
fn ancestors<'a>(files: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    for file in files {
        let mut dir = dir_of(file);
        // A directory that is known has known ancestors.
        while dirs.insert(dir.to_owned()) && !dir.is_empty() {
            dir = dir_of(dir);
        }
    }
    dirs
}

fn raw(hash: &ContentHash) -> Vec<u8> {
    hash.as_bytes().to_vec()
}

fn hash_of(bytes: &[u8]) -> Option<ContentHash> {
    <[u8; 32]>::try_from(bytes).ok().map(ContentHash::from_bytes)
}

impl TreeHashStore {
    pub fn new(db: Db, clock: Arc<dyn Clock>, config: &RegistryConfig, metrics: RegistryMetrics) -> Self {
        Self {
            db,
            clock,
            metrics,
            rows: config.rows_per_statement,
        }
    }

    /// Bring the rows of the files in `touched` and of the directories above them in line with `state`. A touched file that the state
    /// no longer serves is deleted, and so is a directory that no longer holds a file.
    ///
    /// `touched` must name every logical file whose effective hash may have changed since the tables were last right; a file that is
    /// not named is not looked at.
    pub async fn apply(
        &self,
        tx: &mut Txn,
        state: &LaneState,
        touched: &[LogicalFile],
    ) -> Result<TreeWrite, RegistryError> {
        let swimlane = state.swimlane();
        let mut file_rows = Vec::new();
        let mut file_gone = Vec::new();
        for file in touched {
            match state.effective_hash(file) {
                Some(hash) => file_rows.push((file.as_str().to_owned(), hash)),
                None => file_gone.push(file.as_str().to_owned()),
            }
        }
        let mut dir_rows = Vec::new();
        let mut dir_gone = Vec::new();
        for dir in ancestors(touched.iter().map(LogicalFile::as_str)) {
            let hash = NfsPath::parse_prefix(&dir)
                .ok()
                .and_then(|path| state.directory_hash(&path));
            match hash {
                Some(hash) => dir_rows.push((dir, hash)),
                None => dir_gone.push(dir),
            }
        }
        self.write(tx, swimlane, dir_rows, dir_gone, file_rows, file_gone)
            .await
    }

    /// Make the stored rows of the swimlane equal to the state's tree exactly: read what is stored, write the difference.
    pub async fn sync(&self, tx: &mut Txn, state: &LaneState) -> Result<TreeWrite, RegistryError> {
        let swimlane = state.swimlane();
        let stored = self.load_in(tx, swimlane).await?;
        let want_dirs: BTreeMap<String, ContentHash> = state
            .tree_hashes()
            .iter()
            .map(|(dir, hash)| (dir.as_str().to_owned(), *hash))
            .collect();
        let want_files: BTreeMap<String, ContentHash> = state
            .effective_hashes()
            .map(|(file, hash)| (file.as_str().to_owned(), hash))
            .collect();
        let (dir_rows, dir_gone) = difference(&want_dirs, &stored.dirs);
        let (file_rows, file_gone) = difference(&want_files, &stored.files);
        self.write(tx, swimlane, dir_rows, dir_gone, file_rows, file_gone)
            .await
    }

    /// What the tables hold for a swimlane, in the caller's transaction.
    pub async fn load_in(&self, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<StoredTree, RegistryError> {
        let limit = i64::try_from(MAX_LANE_FILES).unwrap_or(i64::MAX);
        let dirs: Vec<(String, Vec<u8>)> = self
            .db
            .run(
                "tree_load_dirs",
                sqlx::query_as(LOAD_DIRS_SQL)
                    .bind(swimlane.as_str())
                    .bind(limit)
                    .fetch_all(&mut **tx),
            )
            .await?;
        let files: Vec<(String, Vec<u8>)> = self
            .db
            .run(
                "tree_load_files",
                sqlx::query_as(LOAD_FILES_SQL)
                    .bind(swimlane.as_str())
                    .bind(limit)
                    .fetch_all(&mut **tx),
            )
            .await?;
        let collect = |rows: Vec<(String, Vec<u8>)>| -> Result<BTreeMap<String, ContentHash>, RegistryError> {
            rows.into_iter()
                .map(|(name, bytes)| hash_of(&bytes).map(|h| (name, h)).ok_or(RegistryError::Corrupt))
                .collect()
        };
        Ok(StoredTree {
            dirs: collect(dirs)?,
            files: collect(files)?,
        })
    }

    /// What the tables hold for a swimlane, read on its own connection.
    pub async fn load(&self, swimlane: &SwimlaneId) -> Result<StoredTree, RegistryError> {
        let mut tx = self.db.begin().await?;
        let stored = self.load_in(&mut tx, swimlane).await?;
        self.db.commit(tx).await?;
        Ok(stored)
    }

    async fn write(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        dir_rows: Vec<(String, ContentHash)>,
        dir_gone: Vec<String>,
        file_rows: Vec<(String, ContentHash)>,
        file_gone: Vec<String>,
    ) -> Result<TreeWrite, RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let mut out = TreeWrite::default();
        for chunk in dir_rows.chunks(self.rows) {
            out.dirs_written += self.upsert(tx, Table::Dirs, swimlane, chunk, now_ms).await?;
        }
        for chunk in file_rows.chunks(self.rows) {
            out.files_written += self.upsert(tx, Table::Files, swimlane, chunk, now_ms).await?;
        }
        for chunk in dir_gone.chunks(self.rows) {
            out.dirs_deleted += self.delete(tx, Table::Dirs, swimlane, chunk).await?;
        }
        for chunk in file_gone.chunks(self.rows) {
            out.files_deleted += self.delete(tx, Table::Files, swimlane, chunk).await?;
        }
        self.metrics.tree_rows(out.total());
        Ok(out)
    }

    /// Insert or update the rows whose hash differs; a row that already holds the hash is not touched.
    async fn upsert(
        &self,
        tx: &mut Txn,
        table: Table,
        swimlane: &SwimlaneId,
        rows: &[(String, ContentHash)],
        now_ms: i64,
    ) -> Result<u64, RegistryError> {
        let names: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
        let hashes: Vec<Vec<u8>> = rows.iter().map(|(_, h)| raw(h)).collect();
        let sql = table.upsert_sql();
        let done = self
            .db
            .run(
                "tree_upsert",
                sqlx::query(&sql)
                    .bind(swimlane.as_str())
                    .bind(&names)
                    .bind(&hashes)
                    .bind(now_ms)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(done.rows_affected())
    }

    async fn delete(
        &self,
        tx: &mut Txn,
        table: Table,
        swimlane: &SwimlaneId,
        names: &[String],
    ) -> Result<u64, RegistryError> {
        let done = self
            .db
            .run(
                "tree_delete",
                sqlx::query(table.delete_sql())
                    .bind(swimlane.as_str())
                    .bind(names)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(done.rows_affected())
    }
}

/// The two tables, with the statements that write them. Written out in full so that no SQL is built from a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Table {
    Dirs,
    Files,
}

impl Table {
    fn upsert_sql(self) -> String {
        match self {
            Self::Dirs => format!(
                "INSERT INTO effective_tree_hashes AS t (swimlane_id, dir_path, hash, computed_at) \
                 SELECT $1, x.n, x.h, {at} FROM unnest($2::text[], $3::bytea[]) AS x(n, h) \
                 ON CONFLICT (swimlane_id, dir_path) DO UPDATE SET hash = EXCLUDED.hash, computed_at = EXCLUDED.computed_at \
                 WHERE t.hash IS DISTINCT FROM EXCLUDED.hash",
                at = ts("$4"),
            ),
            Self::Files => format!(
                "INSERT INTO effective_files AS t (swimlane_id, logical_file, hash, computed_at) \
                 SELECT $1, x.n, x.h, {at} FROM unnest($2::text[], $3::bytea[]) AS x(n, h) \
                 ON CONFLICT (swimlane_id, logical_file) DO UPDATE SET hash = EXCLUDED.hash, computed_at = EXCLUDED.computed_at \
                 WHERE t.hash IS DISTINCT FROM EXCLUDED.hash",
                at = ts("$4"),
            ),
        }
    }

    fn delete_sql(self) -> &'static str {
        match self {
            Self::Dirs => {
                "DELETE FROM effective_tree_hashes WHERE swimlane_id = $1 AND dir_path = ANY($2::text[])"
            }
            Self::Files => {
                "DELETE FROM effective_files WHERE swimlane_id = $1 AND logical_file = ANY($2::text[])"
            }
        }
    }
}

/// What to write to make `stored` equal `want`: the entries that are new or differ, and the names that are not wanted.
fn difference(
    want: &BTreeMap<String, ContentHash>,
    stored: &BTreeMap<String, ContentHash>,
) -> (Vec<(String, ContentHash)>, Vec<String>) {
    let rows = want
        .iter()
        .filter(|(name, hash)| stored.get(*name) != Some(*hash))
        .map(|(name, hash)| (name.clone(), *hash))
        .collect();
    let gone = stored
        .keys()
        .filter(|name| !want.contains_key(*name))
        .cloned()
        .collect();
    (rows, gone)
}
