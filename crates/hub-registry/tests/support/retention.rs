//! The retention job over a real Postgres (T9), with the helpers that seed what a reference class is and read what is left.
//!
//! Rows are written as the **database owner** where a test needs a shape that the registry's own writers would not produce (a blob that is
//! a hundred days old, a row in every class that can name a blob): the grants of the app role are exercised by the job itself, which
//! always runs as `lanekeeper_app`.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::ContentHash;
use hub_registry::{ManualClock, RegistryConfig, RegistryMetrics, Retention, RetentionDeps};
use ports::BlobStore;
use sqlx::{PgPool, Row};

use super::rig::Rig;

pub const DAY: f64 = 86_400.0;
/// A blob that has been untouched for far longer than the grace period.
pub const OLD: f64 = 100.0 * DAY;
/// A blob stored a moment ago.
pub const FRESH: f64 = 60.0;

/// The kinds of `blob_refs.ref_kind`.
pub const REF_KINDS: [&str; 5] = ["baseline", "audit_event", "proposal", "draft", "pr_link"];

/// Everything that can name a blob, and what names one only by looks. (A staged part is stamped a minute before the registry's clock, so
/// that the staging sweep, which runs first, finds it fresh.) `pins` says whether retention must treat it as a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Attach {
    BlobRef(usize),
    FileStateNfs,
    FileStateBaseline,
    BaselineHash,
    BaselineBlob,
    PrLink,
    Intentional,
    AdoptionResolution,
    Proposal,
    Draft,
    ToolWrite,
    Staged,
    // Hashes that are not a blob's: they must not keep one.
    FileStateGit,
    FileStateDrift,
    IntentionalGit,
    ResolutionGit,
    TreeIndex,
    EffectiveFile,
    MerkleDir,
    StagedElsewhere,
}

impl Attach {
    /// Every class that pins a blob, then every look-alike.
    pub fn all() -> Vec<Attach> {
        let mut all: Vec<Attach> = (0..REF_KINDS.len()).map(Attach::BlobRef).collect();
        all.extend([
            Attach::FileStateNfs,
            Attach::FileStateBaseline,
            Attach::BaselineHash,
            Attach::BaselineBlob,
            Attach::PrLink,
            Attach::Intentional,
            Attach::AdoptionResolution,
            Attach::Proposal,
            Attach::Draft,
            Attach::ToolWrite,
            Attach::Staged,
            Attach::FileStateGit,
            Attach::FileStateDrift,
            Attach::IntentionalGit,
            Attach::ResolutionGit,
            Attach::TreeIndex,
            Attach::EffectiveFile,
            Attach::MerkleDir,
            Attach::StagedElsewhere,
        ]);
        all
    }

    /// The column (`table.column`) whose value this class puts the blob's hash in: what `DECISIONS` calls it.
    pub fn column(self) -> &'static str {
        match self {
            Attach::BlobRef(_) => "blob_refs.hash",
            Attach::FileStateNfs => "file_state.nfs_hash",
            Attach::FileStateBaseline => "file_state.baseline_hash",
            Attach::BaselineHash => "baselines.hash",
            Attach::BaselineBlob => "baselines.blob_hash",
            Attach::PrLink => "pr_links.nfs_hash",
            Attach::Intentional => "intentional_divergence.nfs_hash",
            Attach::AdoptionResolution => "adoption_resolutions.nfs_hash",
            Attach::Proposal => "proposals.new_hash",
            Attach::Draft => "drafts.new_hash",
            Attach::ToolWrite => "tool_write_registrations.hash",
            Attach::Staged | Attach::StagedElsewhere => "delta_staging.metadata",
            Attach::FileStateGit => "file_state.git_hash",
            Attach::FileStateDrift => "file_state.nfs_drift_hash",
            Attach::IntentionalGit => "intentional_divergence.git_hash",
            Attach::ResolutionGit => "adoption_resolutions.git_hash",
            Attach::TreeIndex => "git_tree_index.hash",
            Attach::EffectiveFile => "effective_files.hash",
            Attach::MerkleDir => "swimlane_merkle.hash",
        }
    }

    pub fn pins(self) -> bool {
        !matches!(
            self,
            Attach::FileStateGit
                | Attach::FileStateDrift
                | Attach::IntentionalGit
                | Attach::ResolutionGit
                | Attach::TreeIndex
                | Attach::EffectiveFile
                | Attach::MerkleDir
                | Attach::StagedElsewhere
        )
    }

    /// The classes whose foreign key would also refuse the delete (so that a missed check shows as a failed transaction, not a lost row).
    pub fn has_foreign_key(self) -> bool {
        matches!(self, Attach::BlobRef(_) | Attach::Proposal | Attach::Draft)
    }
}

pub struct Gc {
    pub rig: Rig,
    pub retention: Retention,
    pub config: RegistryConfig,
}

/// A retention job over the rig's pool, clock and ingestion.
pub fn retention_of(rig: &Rig, config: RegistryConfig, metrics: RegistryMetrics) -> Retention {
    Retention::new(RetentionDeps {
        pool: rig.pool.clone(),
        ingest: rig.ingest.clone(),
        clock: Arc::clone(&rig.clock) as _,
        config,
        metrics,
    })
    .unwrap()
}

impl Gc {
    pub async fn new() -> Option<Self> {
        Self::with(RegistryConfig::default()).await
    }

    pub async fn with(config: RegistryConfig) -> Option<Self> {
        let rig = Rig::with("sit1", config.clone()).await?;
        let retention = retention_of(&rig, config.clone(), rig.metrics.clone());
        Some(Self {
            rig,
            retention,
            config,
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.rig.pool
    }

    /// Run SQL as the owner of the database.
    pub async fn admin(&self, sql: &str) {
        let mut admin = self.rig.db.admin().await;
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }

    /// Store a blob as the hub does (the real `put`); its age is now.
    pub async fn put(&self, text: &str) -> ContentHash {
        self.rig.blobs.put(Bytes::from(text.to_owned())).await.unwrap()
    }

    /// A blob that has been untouched for `secs`.
    pub async fn aged(&self, text: &str, secs: f64) -> ContentHash {
        let hash = self.put(text).await;
        self.age(&hash, secs).await;
        hash
    }

    /// An old blob (past any grace period).
    pub async fn old(&self, text: &str) -> ContentHash {
        self.aged(text, OLD).await
    }

    /// Set the age of a blob, as the app role can (its one UPDATE on `blobs` is `touched_at`).
    pub async fn age(&self, hash: &ContentHash, secs: f64) {
        let done = sqlx::query(
            "UPDATE blobs SET touched_at = now() - make_interval(secs => $2::double precision) WHERE hash = $1::bytea",
        )
        .bind(hash.as_bytes().as_slice())
        .bind(secs)
        .execute(self.pool())
        .await
        .unwrap();
        assert_eq!(done.rows_affected(), 1, "the blob is stored");
    }

    pub async fn exists(&self, hash: &ContentHash) -> bool {
        self.rig.blobs.has(hash).await
    }

    pub async fn all_blobs(&self) -> BTreeSet<ContentHash> {
        let rows: Vec<Vec<u8>> = sqlx::query_scalar("SELECT hash::bytea FROM blobs")
            .fetch_all(self.pool())
            .await
            .unwrap();
        rows.into_iter()
            .map(|b| ContentHash::from_bytes(b.try_into().unwrap()))
            .collect()
    }

    pub async fn touched_secs_ago(&self, hash: &ContentHash) -> f64 {
        sqlx::query_scalar(
            "SELECT extract(epoch FROM now() - touched_at)::double precision FROM blobs WHERE hash = $1::bytea",
        )
        .bind(hash.as_bytes().as_slice())
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    /// The content of a blob, read straight from the table.
    pub async fn content(&self, hash: &ContentHash) -> Option<Vec<u8>> {
        sqlx::query_scalar("SELECT content::bytea FROM blobs WHERE hash = $1::bytea")
            .bind(hash.as_bytes().as_slice())
            .fetch_optional(self.pool())
            .await
            .unwrap()
    }

    /// A user for the rows that name one.
    pub async fn user(&self) {
        self.admin(
            "INSERT INTO users (tid, oid, email, display_name, role, status) \
             VALUES ('t1', 'o1', 'ada@example.com', 'Ada', 'viewer', 'active') ON CONFLICT DO NOTHING",
        )
        .await;
    }

    /// The time `days` before the registry's clock, in Unix milliseconds.
    pub fn days_ago_ms(days: f64) -> i64 {
        ManualClock::T0_MS - (days * 86_400_000.0) as i64
    }

    /// One observation of `path` with `hash` (or a deletion when `None`), `days` before the registry's clock. Both times of the row are that
    /// instant.
    pub async fn observe(&self, path: &str, hash: Option<&ContentHash>, days: f64) {
        let at = Self::days_ago_ms(days);
        sqlx::query(
            "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, recorded_at) \
             VALUES ('sit1', $1, $2::bytea, 'scan', to_timestamp($3::double precision / 1000.0), to_timestamp($3::double precision / 1000.0))",
        )
        .bind(path)
        .bind(hash.map(|h| h.as_bytes().to_vec()))
        .bind(at)
        .execute(self.pool())
        .await
        .unwrap();
    }

    /// Make the row of one reference class that names `hash`. `n` makes the row's own key unique.
    pub async fn attach(&self, how: Attach, hash: &ContentHash, n: usize) {
        self.user().await;
        let path = format!("seed/{n}.yml");
        let h = hex::encode(hash.as_bytes());
        let other = hex::encode(ports::content_hash(format!("another {n}").as_bytes()).as_bytes());
        let file_state = |col: &str| {
            format!(
                "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, {col}, state) \
                 VALUES ('sit1', '{path}', '{path}', 'base', 'structured', '\\x{h}', 'in_sync')"
            )
        };
        let sql = match how {
            Attach::BlobRef(k) => format!(
                "INSERT INTO blob_refs (hash, ref_kind, ref_id) VALUES ('\\x{h}', '{}', 'ref-{n}')",
                REF_KINDS[k]
            ),
            Attach::FileStateNfs => file_state("nfs_hash"),
            Attach::FileStateBaseline => file_state("baseline_hash"),
            Attach::FileStateGit => file_state("git_hash"),
            Attach::FileStateDrift => file_state("nfs_drift_hash"),
            Attach::BaselineHash => format!(
                "INSERT INTO baselines (swimlane_id, path, hash, repo, commit_id) VALUES ('sit1', '{path}', '\\x{h}', 'base', 'abcdef0')"
            ),
            Attach::BaselineBlob => format!(
                "INSERT INTO baselines (swimlane_id, path, hash, repo, commit_id, blob_hash) \
                 VALUES ('sit1', '{path}', '\\x{other}', 'base', 'abcdef0', '\\x{h}')"
            ),
            Attach::PrLink => format!(
                "INSERT INTO pr_links (swimlane_id, path, nfs_hash, repo, branch, pr_number) \
                 VALUES ('sit1', '{path}', '\\x{h}', 'base', 'b', {})",
                n + 1
            ),
            Attach::Intentional | Attach::IntentionalGit => {
                let column = if how == Attach::Intentional {
                    "nfs_hash"
                } else {
                    "git_hash"
                };
                format!(
                    "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state) \
                     VALUES ('sit1', '{path}', '{path}', 'base', 'structured', 'intentional_divergence'); \
                     INSERT INTO intentional_divergence (swimlane_id, path, {column}, marked_by_tid, marked_by_oid) \
                     VALUES ('sit1', '{path}', '\\x{h}', 't1', 'o1')"
                )
            }
            Attach::AdoptionResolution | Attach::ResolutionGit => {
                let column = if how == Attach::AdoptionResolution {
                    "nfs_hash"
                } else {
                    "git_hash"
                };
                format!(
                    "INSERT INTO adoptions (id, swimlane_id, reason, status) OVERRIDING SYSTEM VALUE VALUES ({id}, 'sit1', 'initial', 'confirmed'); \
                     INSERT INTO adoption_resolutions (adoption_id, path, resolution, {column}) \
                     VALUES ({id}, '{path}', 'nfs_is_right', '\\x{h}')",
                    id = 1000 + n
                )
            }
            Attach::Proposal => format!(
                "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, new_hash, via, expires_at) \
                 VALUES ('t1', 'o1', 'edit', 'sit1', ARRAY['{path}'], '\\x{h}', 'ui', now() + interval '3 days')"
            ),
            Attach::Draft => format!(
                "INSERT INTO drafts (id, author_tid, author_oid, swimlane_id, path, expected, new_hash, expires_at) \
                 VALUES ('draft-{n}', 't1', 'o1', 'sit1', '{path}', '{{}}', '\\x{h}', now() + interval '3 days')"
            ),
            Attach::ToolWrite => format!(
                "INSERT INTO tool_write_registrations (swimlane_id, path, hash, is_delete, user_tid, user_oid, request_id, registered_at) \
                 VALUES ('sit1', '{path}', '\\x{h}', false, 't1', 'o1', 'req-{n}', now())"
            ),
            Attach::Staged => format!(
                "INSERT INTO delta_staging (swimlane_id, seq, part, base_root, new_root, metadata, staged_at) \
                 VALUES ('sit1', {n}, 0, NULL, '\\x{other}', '{{\"v\":1,\"entries\":[{{\"hash\":\"{h}\"}}],\"removed\":[],\"skipped\":[],\"bytes\":0}}', \
                         to_timestamp({staged}::double precision / 1000.0))",
                staged = ManualClock::T0_MS - 60_000
            ),
            Attach::StagedElsewhere => format!(
                "INSERT INTO delta_staging (swimlane_id, seq, part, base_root, new_root, metadata, staged_at) \
                 VALUES ('sit1', {n}, 0, NULL, '\\x{other}', '{{\"v\":1,\"entries\":[],\"removed\":[\"{h}\"],\"skipped\":[\"{h}\"],\"bytes\":0}}', \
                         to_timestamp({staged}::double precision / 1000.0))",
                staged = ManualClock::T0_MS - 60_000
            ),
            Attach::TreeIndex => format!(
                "INSERT INTO git_tree_index (repo, commit_id, path, object_id, hash) \
                 VALUES ('base', 'abcdef0123', '{path}', '{obj}', '\\x{h}')",
                obj = "a".repeat(40)
            ),
            Attach::EffectiveFile => format!(
                "INSERT INTO effective_files (swimlane_id, logical_file, hash) VALUES ('sit1', '{path}', '\\x{h}')"
            ),
            Attach::MerkleDir => format!(
                "INSERT INTO swimlane_merkle (swimlane_id, dir_path, hash) VALUES ('sit1', '{path}', '\\x{h}')"
            ),
        };
        self.admin(&sql).await;
    }

    /// A settings row for the blob, so that deleting it cascades.
    pub async fn index_rows(&self, hash: &ContentHash, rows: usize) {
        let h = hex::encode(hash.as_bytes());
        self.admin(&format!(
            "INSERT INTO settings_index_blobs (blob_hash, setting_count) VALUES ('\\x{h}', {rows}); \
             INSERT INTO settings_index (blob_hash, setting_path, value_text, value_type, start_line, end_line) \
             SELECT '\\x{h}', 'a.b' || g, 'v', 'string', 1, 1 FROM generate_series(1, {rows}) g"
        ))
        .await;
    }

    /// Empty every table a case of a property test writes, keeping the swimlane and the users.
    pub async fn clear(&self) {
        self.admin(
            "TRUNCATE blobs, blob_refs, file_state, baselines, file_observations, pr_links, intentional_divergence, adoptions, \
             adoption_resolutions, proposals, drafts, delta_staging, tool_write_registrations, git_tree_index, effective_files, \
             swimlane_merkle, settings_index, settings_index_blobs, sync_windows CASCADE",
        )
        .await;
    }

    /// Set the Q33 settings row.
    pub async fn keep(&self, versions: i32, days: i32) {
        sqlx::query("UPDATE retention_settings SET keep_versions = $1, keep_days = $2")
            .bind(versions)
            .bind(days)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// The rows of every table of the database as one comparable value (a digest of each row's text and its `xmin`, so that even an
    /// update that changes nothing shows), by table. `leases` and `outbox` are included: a dry run must not write them either.
    pub async fn dump(&self) -> Vec<(String, i64, String)> {
        let mut admin = self.rig.db.admin().await;
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND NOT c.relispartition ORDER BY 1",
        )
        .fetch_all(&mut admin)
        .await
        .unwrap();
        let mut out = Vec::new();
        for table in tables {
            let row = sqlx::query(&format!(
                "SELECT count(*)::bigint, coalesce(md5(string_agg(t::text || '/' || t.xmin::text, '|' ORDER BY t::text)), '') FROM {table} t"
            ))
            .fetch_one(&mut admin)
            .await
            .unwrap();
            out.push((table, row.get::<i64, _>(0), row.get::<String, _>(1)));
        }
        out
    }

    /// Wait (on a condition, never a delay) until some backend of this database is waiting on a lock in a statement that starts with
    /// `query_prefix`.
    pub async fn wait_for_lock_wait(&self, query_prefix: &str) {
        let mut admin = self.rig.db.admin().await;
        for _ in 0..1500 {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() \
                 AND wait_event_type = 'Lock' AND query LIKE $1",
            )
            .bind(format!("{query_prefix}%"))
            .fetch_one(&mut admin)
            .await
            .unwrap();
            if waiting > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no statement starting {query_prefix:?} ever waited on a lock");
    }
}
