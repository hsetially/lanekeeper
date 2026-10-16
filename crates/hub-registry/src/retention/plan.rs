//! The retention plan (T9, D78, D116): which blobs may go, and what every column that can name a blob means to that question.
//!
//! This file holds what is decided, as data and as the SQL that carries it out; `job.rs` runs it.
//!
//! # What may be deleted
//!
//! A blob goes only when **all** of these hold, tested together in the statement that deletes it (and once before, to plan the run):
//!
//! 1. **Its grace has passed.** `blobs.touched_at < now() - grace`. `PgBlobStore::put` refreshes `touched_at` of a blob that is stored
//!    again, so a writer that has stored a blob and not yet committed the row that references it is protected for the grace period
//!    (the contract is `docs/interfaces.md`, `BlobStore`, and migration `0008`).
//! 2. **No reference class names it.** [`NOT_REFERENCED`]: one probe for each column that can name a blob (the table [`DECISIONS`] says
//!    which are probed, and decides every other column of the schema that holds a hash, so that a column added later cannot be forgotten:
//!    `tests/retention_columns.rs` fails when the catalog holds one that is not decided here).
//! 3. **The keep rules do not keep it.** [`NOT_KEPT`]: a version of a file is kept when it is among the last `keep_versions` versions of its
//!    file, or is newer than `keep_days` (the Q33 defaults, 50 and 180, read from the `retention_settings` row); a blob is kept when any
//!    version of any file has it. Audit events are kept for ever and are not touched.
//!
//! # What is not a reference
//!
//! **An audit event does not pin a blob through its hashes** (`audit_events.hash_before`, `hash_after`). The chain keeps its own diff text
//! and its hashes (D78, last bullet); a blob is pinned by an audit event only through an explicit `blob_refs` row of kind `audit_event`
//! that the event's writer chose to add (`hub-writes` adds one for each blob that a recorded action keeps, in
//! `pipeline/execute.rs`; the registry's own writers add `baseline` rows only). Such a blob is pinned twice over: by the foreign key,
//! while a delete is in flight, and by the `blob_refs.hash` probe of [`NOT_REFERENCED`], which looks at every kind. So the keep rules
//! decide which versions survive when no row pins them, and the audit chain may name a hash whose blob is gone: the residual risk of D116.
//!
//! # Races, and how the statements close them
//!
//! A writer stores a blob (`put`, committed at once) before the transaction that references it commits. The deleter therefore
//! (a) tests the age in the **same statement** that locks or deletes the row, never from an earlier read ([`LOCK_SQL`] and
//! [`DELETE_SQL`] both carry it), (b) locks first (`FOR UPDATE SKIP LOCKED`: a row that a put is refreshing, or that a reference is being
//! written against through its foreign key, is left for the next run) and deletes **in the same transaction while it holds the lock**, and
//! (c) runs the reference check as a **second statement** after the lock, so that its snapshot sees every reference committed so far
//! (a single statement that waited for a lock would still read the snapshot it started with). The plan is a hint made without locks: it
//! can be stale by the time a chunk is deleted, and the delete tests everything again.

use std::fmt;
use std::sync::LazyLock;

use domain::ContentHash;

/// What a column of the schema that can hold a hash means to retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// `blobs.hash`: the blob itself.
    Subject,
    /// Retention reads it: a blob it names is never deleted. The column appears in [`NOT_REFERENCED`].
    Reference,
    /// A `jsonb` column whose entries name blobs, read by retention (the path says where). Appears in [`NOT_REFERENCED`].
    ReferenceJson(&'static str),
    /// The keep rules decide, not a pin of its own: a version of a file, named by an observation. Appears in [`NOT_KEPT`].
    DecidedByKeepRules(&'static str),
    /// A foreign key with `ON DELETE CASCADE` to `blobs`: derived rows that go with the blob.
    Cascades,
    /// Not a hash of a blob, or not one that anything reads as a blob. The reason is the decision.
    NotABlob(&'static str),
}

use Decision::{Cascades, DecidedByKeepRules, NotABlob, Reference, ReferenceJson, Subject};

/// Every column of type `sha256_hash` or `bytea`, and every `jsonb` column, of the schema, with what retention does about it. The
/// test `tests/retention_columns.rs::every_column_that_can_hold_a_hash_is_decided` compares this table with the live catalog both ways
/// (a column in the catalog and not here fails, and so does one here and not in the catalog), and checks that the columns decided
/// [`Reference`] or [`ReferenceJson`] are exactly the ones [`NOT_REFERENCED`] probes.
///
/// Text columns are not covered by that test. The ones that name a version by hash are `blob_refs.ref_id` (an owner, never a blob) and
/// nothing else.
pub const DECISIONS: &[(&str, Decision)] = &[
    // ----- the references retention reads
    ("blob_refs.hash", Reference),
    ("file_state.nfs_hash", Reference),
    ("file_state.baseline_hash", Reference),
    ("baselines.blob_hash", Reference),
    ("baselines.hash", Reference),
    ("pr_links.nfs_hash", Reference),
    ("intentional_divergence.nfs_hash", Reference),
    ("adoption_resolutions.nfs_hash", Reference),
    ("proposals.new_hash", Reference),
    ("drafts.new_hash", Reference),
    ("tool_write_registrations.hash", Reference),
    ("delta_staging.metadata", ReferenceJson("entries[].hash")),
    // ----- the subject, the versions and what cascades
    ("blobs.hash", Subject),
    (
        "blobs.content",
        NotABlob("the bytes of the blob itself; retention never reads them"),
    ),
    (
        "file_observations.hash",
        DecidedByKeepRules("the version an observation recorded"),
    ),
    (
        "file_observations.previous_hash",
        DecidedByKeepRules(
            "the version the file had before a sync overwrote it: an earlier observation of the same file, kept or not by its own rank and age",
        ),
    ),
    ("settings_index.blob_hash", Cascades),
    ("settings_index_blobs.blob_hash", Cascades),
    // ----- hashes that are not a blob's
    (
        "adoption_resolutions.git_hash",
        NotABlob("the hash of Git's version of the file: Git content is read from Git, never from blobs"),
    ),
    (
        "intentional_divergence.git_hash",
        NotABlob("the hash of Git's version of the file: Git content is read from Git, never from blobs"),
    ),
    (
        "file_state.git_hash",
        NotABlob("the normalised hash of Git's version of the file, read from Git"),
    ),
    (
        "file_state.nfs_drift_hash",
        NotABlob("the normalised N, derived; `nfs_hash` is the raw hash that names the blob"),
    ),
    (
        "git_tree_index.hash",
        NotABlob("the hash of a file in a Git commit: Git content is read from Git, never from blobs"),
    ),
    (
        "effective_files.hash",
        NotABlob("derived: the hash of an effective rendering, which no blob holds"),
    ),
    (
        "effective_tree_hashes.hash",
        NotABlob("derived: a hash over a directory's effective files"),
    ),
    ("agents.merkle_root", NotABlob("a Merkle root of an agent's tree")),
    (
        "_sqlx_migrations.checksum",
        NotABlob("sqlx's own record of the migrations it applied: the checksum of a migration file"),
    ),
    (
        "swimlane_merkle.hash",
        NotABlob("a hash of a directory in an agent's tree"),
    ),
    ("delta_staging.base_root", NotABlob("a Merkle root")),
    ("delta_staging.new_root", NotABlob("a Merkle root")),
    (
        "swimlane_registry.rules_fingerprint",
        NotABlob("a fingerprint of the rules a swimlane is computed under"),
    ),
    (
        "approvals.rechecked_hash",
        NotABlob(
            "the hash the file had when an approval re-checked it: compared, never read as content, and a version of that file under the keep rules (D116: an audit-like hash does not pin)",
        ),
    ),
    (
        "audit_events.hash_before",
        NotABlob(
            "D78 and D116: an audit event keeps its own diff text and its hashes and pins a blob only through a blob_refs row of kind audit_event",
        ),
    ),
    (
        "audit_events.hash_after",
        NotABlob(
            "D78 and D116: an audit event keeps its own diff text and its hashes and pins a blob only through a blob_refs row of kind audit_event",
        ),
    ),
    ("audit_events.hash", NotABlob("the hash chain of the audit log")),
    (
        "audit_events.prev_hash",
        NotABlob("the hash chain of the audit log"),
    ),
    (
        "audit_checkpoints.head_hash",
        NotABlob("the head of the audit chain"),
    ),
    (
        "audit_checkpoints.signature",
        NotABlob("a signature of the audit chain's head"),
    ),
    (
        "doc_versions.content_hash",
        NotABlob(
            "the hash of a document's extracted text, which doc_versions.body holds: documents are not in blobs (a writer that put one there would add blob_refs rows)",
        ),
    ),
    ("idempotency_keys.request_hash", NotABlob("the hash of a request")),
    ("idempotency_keys.response_body", NotABlob("a stored response")),
    ("sessions.session_hash", NotABlob("the hash of a session token")),
    ("sessions.csrf_hash", NotABlob("the hash of a CSRF token")),
    ("join_tokens.token_hash", NotABlob("the hash of a join token")),
    ("mcp_tokens.token_hmac", NotABlob("the HMAC of a personal token")),
    ("github_credentials.ciphertext", NotABlob("an encrypted token")),
    ("github_credentials.nonce", NotABlob("a nonce")),
    ("github_credentials.wrapped_dek", NotABlob("a wrapped data key")),
    // ----- jsonb columns that name no blob
    (
        "drafts.expected",
        NotABlob(
            "the hash the file had when the draft was made (compared at apply, never read as content); the draft's own content is new_hash",
        ),
    ),
    (
        "proposals.base_hashes",
        NotABlob(
            "the hash each file had when the proposal was made (compared at apply, never read as content); the proposal's own content is new_hash",
        ),
    ),
    (
        "proposals.params",
        NotABlob("the parameters of an action: paths, names and flags"),
    ),
    (
        "pr_jobs.step_data",
        NotABlob(
            "the state of a PR job (prompt 06): the content of a PR is pr_links.nfs_hash, which is read, and Git; a writer that keeps a blob hash here must add a blob_refs row",
        ),
    ),
    ("drafts.hunks", NotABlob("diff hunks, text")),
    ("drafts.setting_changes", NotABlob("setting changes, text")),
    (
        "findings.details",
        NotABlob("derived: evidence of a finding, recomputed by the engine from file_state and Git"),
    ),
    (
        "findings.locations",
        NotABlob("derived: line ranges of a finding"),
    ),
    (
        "outbox.event",
        NotABlob("a transient event, pruned by the platform"),
    ),
    ("adoptions.base_label", NotABlob("a version label")),
    ("adoptions.hints", NotABlob("environment and release hints")),
    ("adoptions.ranking", NotABlob("branch scores")),
    (
        "adoptions.secret_scan",
        NotABlob("locations and lengths of possible secrets, never values"),
    ),
    ("adoptions.tenant_commits", NotABlob("Git commit ids")),
    ("audit_events.attribution_actor", NotABlob("who made a change")),
    ("audit_events.attribution_evidence", NotABlob("names and times")),
    (
        "file_observations.attribution_actor",
        NotABlob("who made a change"),
    ),
    (
        "file_observations.attribution_evidence",
        NotABlob("names and times"),
    ),
    (
        "service_state.pending_files",
        NotABlob("paths of files a service has not picked up"),
    ),
    ("service_state.pods", NotABlob("pods of a Deployment")),
    ("services.env_values", NotABlob("allow-listed environment values")),
    (
        "severity_rules.matcher",
        NotABlob("the conditions of a severity rule"),
    ),
    ("swimlane_deployments.actor", NotABlob("who reported a sync")),
    (
        "swimlane_registry.state_counts",
        NotABlob("counts by drift state"),
    ),
];

/// The columns that retention reads (decided [`Reference`] or [`ReferenceJson`]), as `table.column`.
pub fn read_columns() -> Vec<&'static str> {
    DECISIONS
        .iter()
        .filter(|(_, d)| matches!(d, Reference | ReferenceJson(_)))
        .map(|(c, _)| *c)
        .collect()
}

/// One probe for each column retention reads. `x` is the referencing table and `b` the blob. `LIMIT 1` makes each an existence probe; the
/// planner drops it and plans an anti join, which reads each table once for a batch of blobs. The comments name the column each probe
/// reads, and a test holds them equal to [`DECISIONS`].
///
/// The jsonb probe is written so that a damaged row cannot fail the statement: `entries` that is not an array reads as none, and a hash
/// that is not 64 hex digits matches nothing (`decode` would raise on it).
pub const NOT_REFERENCED: &str = concat!(
    " AND NOT EXISTS (SELECT 1 FROM blob_refs x WHERE x.hash = b.hash LIMIT 1) /* ref:blob_refs.hash */",
    " AND NOT EXISTS (SELECT 1 FROM file_state x WHERE x.nfs_hash = b.hash LIMIT 1) /* ref:file_state.nfs_hash */",
    " AND NOT EXISTS (SELECT 1 FROM file_state x WHERE x.baseline_hash = b.hash LIMIT 1) /* ref:file_state.baseline_hash */",
    " AND NOT EXISTS (SELECT 1 FROM baselines x WHERE x.blob_hash = b.hash LIMIT 1) /* ref:baselines.blob_hash */",
    " AND NOT EXISTS (SELECT 1 FROM baselines x WHERE x.hash = b.hash LIMIT 1) /* ref:baselines.hash */",
    " AND NOT EXISTS (SELECT 1 FROM pr_links x WHERE x.nfs_hash = b.hash LIMIT 1) /* ref:pr_links.nfs_hash */",
    " AND NOT EXISTS (SELECT 1 FROM intentional_divergence x WHERE x.nfs_hash = b.hash LIMIT 1) /* ref:intentional_divergence.nfs_hash */",
    " AND NOT EXISTS (SELECT 1 FROM adoption_resolutions x WHERE x.nfs_hash = b.hash LIMIT 1) /* ref:adoption_resolutions.nfs_hash */",
    " AND NOT EXISTS (SELECT 1 FROM proposals x WHERE x.new_hash = b.hash LIMIT 1) /* ref:proposals.new_hash */",
    " AND NOT EXISTS (SELECT 1 FROM drafts x WHERE x.new_hash = b.hash LIMIT 1) /* ref:drafts.new_hash */",
    " AND NOT EXISTS (SELECT 1 FROM tool_write_registrations x WHERE x.hash = b.hash LIMIT 1) /* ref:tool_write_registrations.hash */",
    " AND NOT EXISTS (SELECT 1 FROM delta_staging x, jsonb_array_elements(CASE WHEN jsonb_typeof(x.metadata -> 'entries') = 'array' THEN x.metadata -> 'entries' ELSE '[]'::jsonb END) e ",
    "WHERE decode(CASE WHEN e ->> 'hash' ~* '^[0-9a-f]{64}$' THEN e ->> 'hash' END, 'hex') = b.hash LIMIT 1) /* ref:delta_staging.metadata */",
);

/// The keep rules. A blob is kept when some version of some file has it and that version is among the last `$2` versions of its file (the
/// versions newer than it, counted up to `$2`, are fewer than `$2`) or is newer than `$3` (Unix milliseconds: `keep_days` before the
/// registry's clock). A version's time is the later of when the agent saw it and when the hub recorded it. The probe reaches the versions
/// of a blob by `file_observations_hash_idx` and counts the newer ones by `file_observations_history_idx`, stopping at `$2`.
pub const NOT_KEPT: &str = concat!(
    " AND NOT EXISTS (SELECT 1 FROM file_observations o WHERE o.hash = b.hash AND (",
    "GREATEST(o.observed_at, o.recorded_at) > to_timestamp($3::double precision / 1000.0) ",
    "OR (SELECT count(*) FROM (SELECT 1 FROM file_observations n WHERE n.swimlane_id = o.swimlane_id AND n.path = o.path ",
    "AND (n.observed_at, n.id) > (o.observed_at, o.id) LIMIT $2::bigint) newer) < $2::bigint)) /* keep:file_observations.hash */",
);

/// The plan: the blobs that pass all three tests now, in hash order, one more than `$4 - 1` so that the caller can tell a plan that was
/// cut from one that was not. Binds: `$1` grace seconds, `$2` keep versions, `$3` keep cutoff (Unix ms), `$4` the limit.
pub static PLAN_SQL: LazyLock<String> = LazyLock::new(|| {
    [
        "SELECT b.hash::bytea, b.size FROM blobs b WHERE b.touched_at < now() - ($1::bigint * interval '1 second')",
        NOT_REFERENCED,
        NOT_KEPT,
        " ORDER BY b.hash LIMIT $4",
    ]
    .concat()
});

/// The lock: of the blobs `$2` (the chunk), those whose age is past the grace **in this statement**, taken `FOR UPDATE SKIP LOCKED`. A row
/// that a put is refreshing (or that a writer holds a `FOR KEY SHARE` lock on, because it is referencing the blob through a foreign key) is
/// skipped, and a row whose age was refreshed after the plan no longer matches and is skipped too. `$1` grace seconds, `$2` hashes.
pub const LOCK_SQL: &str = "SELECT b.hash::bytea FROM blobs b WHERE b.hash = ANY($2::bytea[]) \
     AND b.touched_at < now() - ($1::bigint * interval '1 second') ORDER BY b.hash FOR UPDATE OF b SKIP LOCKED";

/// The delete, run in the transaction that took the lock and as a statement of its own, so that its snapshot is newer than the lock: every
/// test again (age, every reference class, the keep rules) on the rows `$4` that are still locked, and the size of what went.
/// `$1` grace seconds, `$2` keep versions, `$3` keep cutoff (Unix ms), `$4` the hashes the lock returned.
pub static DELETE_SQL: LazyLock<String> = LazyLock::new(|| {
    [
        "DELETE FROM blobs b WHERE b.hash = ANY($4::bytea[]) AND b.touched_at < now() - ($1::bigint * interval '1 second')",
        NOT_REFERENCED,
        NOT_KEPT,
        " RETURNING b.size",
    ]
    .concat()
});

/// The keep rules as the `retention_settings` row says them (Q33).
pub const SETTINGS_SQL: &str =
    "SELECT keep_versions::bigint, keep_days::bigint FROM retention_settings WHERE singleton LIMIT 1";

/// Q33's defaults, used when the settings row is missing.
pub const DEFAULT_KEEP_VERSIONS: i64 = 50;
pub const DEFAULT_KEEP_DAYS: i64 = 180;

const DAY_MS: i64 = 24 * 3600 * 1000;

/// The keep rules of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepRules {
    /// The last this many versions of a file are kept.
    pub versions: i64,
    /// A version newer than this many days is kept.
    pub days: i64,
}

impl Default for KeepRules {
    fn default() -> Self {
        Self {
            versions: DEFAULT_KEEP_VERSIONS,
            days: DEFAULT_KEEP_DAYS,
        }
    }
}

impl KeepRules {
    /// Unix milliseconds `days` before `now_ms`: the cutoff of `$3`.
    pub fn cutoff_ms(&self, now_ms: i64) -> i64 {
        now_ms.saturating_sub(self.days.saturating_mul(DAY_MS))
    }
}

/// One blob that may go.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub(crate) hash: ContentHash,
    pub(crate) size: u64,
}

impl Candidate {
    pub fn hash(&self) -> &ContentHash {
        &self.hash
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

impl fmt::Debug for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // S10: a hash is never in a log line or a panic message.
        f.debug_struct("Candidate")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// What a run intends to do: the blobs that passed every test when it looked, within the batch and the byte limit, and the rules it looked
/// under. Made without a lock, so it can be stale: [`super::Retention::collect`] tests everything again.
#[derive(Clone, PartialEq, Eq)]
pub struct Plan {
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) bytes: u64,
    pub(crate) truncated: bool,
    pub(crate) rules: KeepRules,
}

impl Plan {
    /// A plan with nothing in it, under `rules`.
    pub fn empty(rules: KeepRules) -> Self {
        Self {
            candidates: Vec::new(),
            bytes: 0,
            truncated: false,
            rules,
        }
    }

    /// Blobs planned.
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Bytes of the blobs planned.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The batch or the byte limit stopped the plan: more is collectable than it holds.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The keep rules the plan was made under.
    pub fn rules(&self) -> KeepRules {
        self.rules
    }

    /// The blobs planned, in the order they will be taken.
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Whether `hash` is planned.
    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.candidates.iter().any(|c| &c.hash == hash)
    }
}

impl fmt::Debug for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Counts only (S10): which blobs is never printed.
        f.debug_struct("Plan")
            .field("blobs", &self.candidates.len())
            .field("bytes", &self.bytes)
            .field("truncated", &self.truncated)
            .field("rules", &self.rules)
            .finish()
    }
}

/// Cut the rows the plan statement returned to the run's bounds: at most `max_blobs` blobs and `max_bytes` bytes, taken in order, stopping
/// at the first blob that does not fit. `rows` holds one more row than `max_blobs` when the statement had more to give, which is how a plan
/// that was cut is told from one that was not. A row that is not a 32-byte hash is skipped (never guessed at), and a size that is not a
/// number counts as the largest, so that the blob is not taken.
pub fn cut(rows: Vec<(Vec<u8>, i64)>, max_blobs: usize, max_bytes: u64, rules: KeepRules) -> Plan {
    let mut plan = Plan::empty(rules);
    for (hash, size) in rows {
        let Ok(hash) = <[u8; 32]>::try_from(hash) else {
            continue;
        };
        let size = u64::try_from(size).unwrap_or(u64::MAX);
        if plan.candidates.len() >= max_blobs || plan.bytes.saturating_add(size) > max_bytes {
            plan.truncated = true;
            // The rest of the rows are not taken either: the order is the plan's, and a smaller blob later does not jump the queue.
            break;
        }
        plan.bytes += size;
        plan.candidates.push(Candidate {
            hash: ContentHash::from_bytes(hash),
            size,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    fn row(n: u8, size: i64) -> (Vec<u8>, i64) {
        (vec![n; 32], size)
    }

    #[test]
    fn a_plan_is_cut_at_the_batch_and_says_so() {
        let rows = vec![row(1, 10), row(2, 10), row(3, 10)];
        let plan = cut(rows.clone(), 2, 1_000, KeepRules::default());
        assert_eq!(plan.len(), 2);
        assert_eq!(plan.bytes(), 20);
        assert!(plan.truncated(), "a third row was there");
        let whole = cut(rows, 3, 1_000, KeepRules::default());
        assert_eq!(whole.len(), 3);
        assert!(!whole.truncated(), "nothing was left over");
    }

    #[test]
    fn a_plan_is_cut_at_the_byte_limit_in_order() {
        let rows = vec![row(1, 40), row(2, 40), row(3, 5)];
        let plan = cut(rows, 100, 70, KeepRules::default());
        assert_eq!(
            plan.len(),
            1,
            "the second does not fit, and the small third does not jump it"
        );
        assert_eq!(plan.bytes(), 40);
        assert!(plan.truncated());
    }

    #[test]
    fn a_blob_that_is_exactly_the_limit_is_taken() {
        let plan = cut(vec![row(1, 64)], 10, 64, KeepRules::default());
        assert_eq!(plan.len(), 1);
        assert!(!plan.truncated());
    }

    #[test]
    fn a_row_that_is_not_a_hash_is_skipped_and_a_bad_size_is_never_taken() {
        let rows = vec![(vec![1_u8; 31], 5), row(2, -1), row(3, 5)];
        let plan = cut(rows, 10, 1_000, KeepRules::default());
        assert!(
            plan.truncated(),
            "a size that is not a number counts as the largest and stops the plan"
        );
        assert!(plan.is_empty());
    }

    #[test]
    fn nothing_found_is_an_empty_plan_that_was_not_cut() {
        let plan = cut(Vec::new(), 5, 5, KeepRules::default());
        assert!(plan.is_empty());
        assert!(!plan.truncated());
        assert_eq!(plan.bytes(), 0);
    }

    #[test]
    fn the_debug_output_of_a_plan_and_of_a_candidate_has_counts_and_no_hash() {
        let plan = cut(vec![row(0xab, 7)], 5, 100, KeepRules::default());
        let text = format!("{plan:?} {:?}", plan.candidates()[0]);
        assert!(!text.contains("abab"), "{text}");
        assert!(!text.contains("ab, ab"), "{text}");
        assert!(text.contains("blobs") && text.contains("bytes"), "{text}");
    }

    #[test]
    fn the_cutoff_is_the_keep_days_before_now() {
        let rules = KeepRules {
            versions: 50,
            days: 180,
        };
        assert_eq!(rules.cutoff_ms(1_000 * DAY_MS), 820 * DAY_MS);
        assert_eq!(
            KeepRules {
                versions: 1,
                days: i64::MAX
            }
            .cutoff_ms(5),
            5 - i64::MAX,
            "an absurd setting saturates and never wraps"
        );
    }

    #[test]
    fn the_decisions_name_each_column_once() {
        let mut seen = BTreeSet::new();
        for (column, _) in DECISIONS {
            assert!(seen.insert(*column), "{column} is decided twice");
            assert!(column.split('.').count() == 2, "{column} is table.column");
        }
    }

    #[test]
    fn the_columns_retention_reads_are_the_ones_the_probes_name_and_no_other() {
        let mut named: Vec<&str> = Vec::new();
        for part in NOT_REFERENCED.split("/* ref:").skip(1) {
            if let Some(end) = part.find(" */") {
                named.push(&part[..end]);
            }
        }
        let mut named_sorted = named.clone();
        named_sorted.sort_unstable();
        let mut read = read_columns();
        read.sort_unstable();
        assert_eq!(named_sorted, read, "the probes and the decisions disagree");
        assert!(
            NOT_KEPT.contains("/* keep:file_observations.hash */"),
            "the keep rules name what they read"
        );
    }

    #[test]
    fn the_statements_carry_the_age_the_references_and_the_keep_rules() {
        for sql in [PLAN_SQL.as_str(), DELETE_SQL.as_str()] {
            assert!(
                sql.contains("touched_at < now()"),
                "the age is tested in the statement"
            );
            for column in read_columns() {
                assert!(
                    sql.contains(&format!("/* ref:{column} */")),
                    "{column} is not probed in {sql}"
                );
            }
            assert!(sql.contains("/* keep:"), "the keep rules are in the statement");
        }
        assert!(LOCK_SQL.contains("touched_at < now()"));
        assert!(LOCK_SQL.contains("FOR UPDATE OF b SKIP LOCKED"));
        assert!(DELETE_SQL.starts_with("DELETE FROM blobs b"));
        assert!(!PLAN_SQL.contains("FOR UPDATE"), "the plan takes no lock");
    }

    proptest! {
        #[test]
        fn a_cut_plan_never_exceeds_its_bounds_and_keeps_the_order(
            sizes in proptest::collection::vec(0_i64..200, 0..40),
            max_blobs in 1_usize..20,
            max_bytes in 1_u64..500,
        ) {
            let rows: Vec<(Vec<u8>, i64)> = sizes.iter().enumerate().map(|(i, s)| (vec![u8::try_from(i).unwrap_or(0); 32], *s)).collect();
            let plan = cut(rows.clone(), max_blobs, max_bytes, KeepRules::default());
            prop_assert!(plan.len() <= max_blobs);
            prop_assert!(plan.bytes() <= max_bytes);
            let taken: Vec<u64> = plan.candidates().iter().map(Candidate::size).collect();
            let expected: Vec<u64> = sizes.iter().map(|s| u64::try_from(*s).unwrap_or(0)).take(taken.len()).collect();
            prop_assert_eq!(taken, expected, "a prefix of the rows");
            let all_taken = plan.len() == rows.len();
            prop_assert_eq!(plan.truncated(), !all_taken, "truncated says exactly whether something was left");
        }
    }
}
