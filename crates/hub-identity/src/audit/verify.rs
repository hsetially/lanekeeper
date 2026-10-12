//! Nightly verification of the audit chain (S9).
//!
//! [`ChainVerifier`] recomputes the whole chain from the stored `event_json` and checks every checkpoint:
//!
//! - each row's `prev_hash` is the previous row's `hash` (the first row's is the genesis hash), and `hash` is
//!   `SHA-256(prev_hash || event_json)` computed with [`ports::chain_hash`]. Gaps in `seq` are expected (a rolled-back
//!   transaction uses a number) and are not looked at: the chain is followed by hashes, not by numbers. The log hands out `seq`
//!   from 1, so a row at any other position ([`FailureKind::SeqNotPositive`]) was written by hand: it is reported and skipped, and
//!   a row with no `seq` at all ([`FailureKind::EventPositionMissing`]) is reported once;
//! - for every row of `audit_checkpoints`: the chain has an event at that `seq` with that hash, the bucket holds the object for
//!   it and the object equals the row, and the signature verifies with the KMS public key;
//! - for every row whose hash is good, the denormalised columns (`actor_ref`, `action`, `at` and the rest, see [`super::columns`])
//!   equal the fields they were copied from in the stored `event_json`. Nothing chains these columns, so an owner who disabled the
//!   append-only trigger could change who an event is attributed to without touching the hash. A difference names the columns, not
//!   their values.
//!
//! # How a bad value is told from a bad database
//!
//! A stored value is a finding and never ends the walk. That covers a value that cannot be decoded (an infinite `at`, a `jsonb`
//! that `serde_json` refuses), a `NULL` where the schema forbids one (in the chain, in a copied column, in a checkpoint), and a
//! value over the bound of its column: the select sends nothing over a bound (see [`super::columns`]), so the value is never
//! transferred or held. Each is reported against its row, by column name only. What is an error is a failure of the query: a lost
//! connection, a column that is not there, a statement or lock that ran out of time. An error ends the run, but not the findings:
//! [`VerifyAborted`] carries what was found before it, the two halves (chain, checkpoints) are run whatever happens to the other,
//! and [`NightlyVerifyJob`] alerts on findings even when the run then errored ([`NightlyOutcome::Incomplete`]).
//!
//! # Memory, time and threads
//!
//! The chain is read by keyset in batches (`seq > $last ORDER BY seq LIMIT 5000`, the first batch with no `WHERE` so that no `seq`
//! is skipped), each in a transaction of its own that begins with `SET LOCAL statement_timeout` and `lock_timeout` (5 minutes and
//! 30 seconds by default), and the client gives up 30 seconds after the statement timeout. The rows are streamed and compared in
//! chunks of at most 1,024 rows or 16 MiB as sent, on a blocking thread (hashing and parsing up to a megabyte per row is CPU work),
//! so the memory held is a chunk, not a batch and not the chain. A legitimate row is a few kilobytes (the largest part of an event
//! is its diff, at most 64 KiB), about 230 of the largest fit a chunk; a row with every bounded column full is about 14 MiB.
//!
//! The first row with a bad link or hash ends the walk of the chain, because every row after a bad one fails too; the report names
//! that one position. A row whose columns differ does not end it (the chain is intact after it), so each such row is listed.
//! Checkpoints are checked one by one, each reporting its first problem. The report lists at most 100 differing columns, 100 rows
//! below position 1 and, separately, 100 failures of every other kind, so that no one kind crowds out the others.
//!
//! What it cannot see, by design: an event removed *and* its checkpoint rows removed, or a tail cut off after the newest
//! checkpoint, leave a chain that is internally consistent. The bucket's retention keeps the objects of the removed checkpoints,
//! but this job starts from the rows in the table. The newest unchecked stretch is at most an hour. Nor does it see two rows with
//! the same `seq` once the primary key is dropped, when the second falls across a batch edge.
//!
//! The row hash is recomputed from the stored text, not from a re-parse: a future change to the event type must not turn old
//! rows into false alarms.

use std::sync::Arc;
use std::time::Duration;

use domain::{ContentHash, Severity, ShortText};
use futures::TryStreamExt;
use ports::{KeyRef, LeaseError, Leases, Notification, NotificationKind, Notifier, chain_hash, genesis_hash};
use sqlx::{PgPool, Row};

use super::JobHandle;
use super::bucket::{BucketError, CheckpointBucket, OBJECT_PREFIX, object_name};
use super::checkpoint::{CheckpointObject, MAX_SIGNATURE_BYTES};
use super::columns::{ColumnSet, HASH_COLUMN_BYTES, differing_in_row, is_decode_failure, select_list};
use super::log::MAX_EVENT_JSON_BYTES;
use super::signature::{CheckpointVerifier, checkpoint_message};
use crate::metrics::IdentityMetrics;

/// Rows read per keyset batch.
pub const DEFAULT_BATCH_ROWS: usize = 5_000;
/// Checkpoint rows read per batch. Each is bounded by the select (at most a few hundred bytes), so a batch is a few hundred
/// kilobytes at the most.
const CHECKPOINT_BATCH: i64 = 500;
/// The rows of a batch are compared in chunks of at most this many rows ...
const CHUNK_ROWS: usize = 1_024;
/// ... and, whichever comes first, this many bytes as the server sent them. A chunk is what is held in memory at once, apart from
/// what comparing it needs for one row at a time. A legitimate row is a few kilobytes (an event is at most
/// [`MAX_EVENT_JSON_BYTES`] and, being typed, in practice far less than that), so a chunk is a thousand rows or thereabouts; a row
/// as large as the select lets one be (about 14 MiB, with every bounded column full) ends a chunk by itself.
const CHUNK_BYTES: usize = 16 * 1024 * 1024;
/// The bucket gets this long to answer for one object.
const BUCKET_TIMEOUT: Duration = Duration::from_secs(60);
/// The client waits this much longer than the server's statement timeout before it gives up on a query.
const CLIENT_MARGIN: Duration = Duration::from_secs(30);
/// The most failures of one class a report lists (differing columns, rows below position 1, and everything else); the rest are
/// counted by `truncated`.
const MAX_FAILURES: usize = 100;
/// The lease of the nightly job, and how long it lasts between renewals while a long run is going.
pub const LEASE_NAME: &str = "audit.verify";
const LEASE_TTL: Duration = Duration::from_secs(3600);
/// How long one query of the verifier may run, and how long it may wait for a lock.
pub const DEFAULT_STATEMENT_TIMEOUT: Duration = Duration::from_secs(300);
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// The notifier gets this long.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// What is wrong at one position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// A row's `prev_hash` is not the hash of the row before it: a row was removed, inserted or replaced.
    ChainLinkBroken,
    /// A row's `hash` is not `SHA-256(prev_hash || event_json)`: the stored event was changed.
    ChainHashMismatch,
    /// A checkpoint names a position the chain does not have: the chain was cut.
    CheckpointHeadMissing,
    /// The chain's hash at the checkpoint's position is not the one the checkpoint signed.
    CheckpointHeadMismatch,
    /// The row's object name is not the one this position has.
    CheckpointNameInvalid,
    BucketObjectMissing,
    /// The bucket's object differs from the table's row.
    BucketObjectMismatch,
    /// The bucket could not be read. The check did not complete; this is not evidence of tampering, and still an alert.
    BucketUnavailable,
    SignatureInvalid,
    /// The row's hash is good, but the named columns are not the copies of the fields of its stored event: the attribution or
    /// the time shown by anything that reads the columns is not the event's. Carries column names only (S10).
    ColumnsDiffer(ColumnSet),
    /// A row of `audit_events` has a `seq` that is not positive. The log hands out positions from 1; such a row was written with an
    /// explicit `seq`. It is not part of the chain, is skipped (the walk goes on without it) and is never trusted.
    SeqNotPositive,
    /// A row of `audit_events` has no `seq` (the primary key and the `NOT NULL` were dropped): it cannot be placed in the chain.
    /// Reported at position 0, the position of no checkpoint.
    EventPositionMissing,
    /// The same for a row of `audit_checkpoints`.
    CheckpointPositionMissing,
    /// A checkpoint row has a column that is `NULL`, cannot be decoded, or is larger than any checkpoint can hold. The checkpoint
    /// cannot be checked at all; the walk goes on with the next one. Carries column names only (S10).
    CheckpointColumnsInvalid(CheckpointColumnSet),
}

/// A column of `audit_checkpoints` that the verifier reads (besides `seq`, which is the position).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointColumn {
    HeadHash,
    KmsKeyVersion,
    Signature,
    GcsObject,
}

impl CheckpointColumn {
    /// Every column the verifier reads, in table order.
    pub const ALL: [Self; 4] = [
        Self::HeadHash,
        Self::KmsKeyVersion,
        Self::Signature,
        Self::GcsObject,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::HeadHash => "head_hash",
            Self::KmsKeyVersion => "kms_key_version",
            Self::Signature => "signature",
            Self::GcsObject => "gcs_object",
        }
    }

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of checkpoint columns. Small and `Copy`, so that [`FailureKind`] stays `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CheckpointColumnSet(u8);

impl CheckpointColumnSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn of(columns: &[CheckpointColumn]) -> Self {
        let mut set = Self::empty();
        for c in columns {
            set.insert(*c);
        }
        set
    }

    pub fn insert(&mut self, c: CheckpointColumn) {
        self.0 |= c.bit();
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, c: CheckpointColumn) -> bool {
        self.0 & c.bit() != 0
    }

    /// The names of the columns in the set, in table order.
    pub fn names(self) -> Vec<&'static str> {
        CheckpointColumn::ALL
            .into_iter()
            .filter(|c| self.contains(*c))
            .map(CheckpointColumn::name)
            .collect()
    }
}

impl std::fmt::Display for CheckpointColumnSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.names().join(", "))
    }
}

impl FailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChainLinkBroken => "chain link broken",
            Self::ChainHashMismatch => "event hash mismatch",
            Self::CheckpointHeadMissing => "checkpoint position missing from the chain",
            Self::CheckpointHeadMismatch => "checkpoint head differs from the chain",
            Self::CheckpointNameInvalid => "checkpoint object name invalid",
            Self::BucketObjectMissing => "checkpoint object missing from the bucket",
            Self::BucketObjectMismatch => "checkpoint object differs from the table",
            Self::BucketUnavailable => "checkpoint bucket unavailable",
            Self::SignatureInvalid => "checkpoint signature invalid",
            Self::ColumnsDiffer(_) => "columns differ from the stored event",
            Self::SeqNotPositive => "event position is not positive",
            Self::EventPositionMissing => "an event has no position",
            Self::CheckpointPositionMissing => "a checkpoint has no position",
            Self::CheckpointColumnsInvalid(_) => "checkpoint row has a missing or unreadable column",
        }
    }

    /// How serious the failure is, 0 the most: damage to the chain, then a checkpoint that does not hold, then a copy that differs,
    /// then a bucket that could not be read (the check did not complete, which is not evidence of anything).
    pub const fn severity(self) -> u8 {
        match self {
            Self::ChainLinkBroken
            | Self::ChainHashMismatch
            | Self::SeqNotPositive
            | Self::EventPositionMissing => 0,
            Self::CheckpointHeadMissing
            | Self::CheckpointHeadMismatch
            | Self::CheckpointNameInvalid
            | Self::BucketObjectMissing
            | Self::BucketObjectMismatch
            | Self::SignatureInvalid
            | Self::CheckpointColumnsInvalid(_)
            | Self::CheckpointPositionMissing => 1,
            Self::ColumnsDiffer(_) => 2,
            Self::BucketUnavailable => 3,
        }
    }

    /// The names of the columns the failure is about (events or checkpoint), for the log and the alert. Names, never values (S10).
    pub fn detail(self) -> Option<String> {
        match self {
            Self::ColumnsDiffer(set) => Some(set.to_string()),
            Self::CheckpointColumnsInvalid(set) => Some(set.to_string()),
            _ => None,
        }
    }

    /// Which of the report's bounded lists the failure goes in. Each list has [`MAX_FAILURES`] places of its own, so that a flood
    /// of one kind (a hundred rows with a changed copy, a hundred rows forged at a position below 1, both of which the INSERT-only
    /// role can write) never keeps the broken chain or the bad checkpoint that matters more out of the report.
    const fn list(self) -> u8 {
        match self {
            Self::ColumnsDiffer(_) => 1,
            Self::SeqNotPositive => 2,
            _ => 0,
        }
    }

    /// The columns that differ, for [`FailureKind::ColumnsDiffer`].
    pub const fn columns(self) -> Option<ColumnSet> {
        match self {
            Self::ColumnsDiffer(set) => Some(set),
            _ => None,
        }
    }
}

/// One problem and where it is (the `seq` of the event or of the checkpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyFailure {
    pub seq: i64,
    pub kind: FailureKind,
}

/// The outcome of one verification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerifyReport {
    /// Events whose link and hash were checked, up to and including the first with a bad link or hash.
    pub events_checked: u64,
    /// Keyset batches read from `audit_events`.
    pub batches: u64,
    pub checkpoints_checked: u64,
    pub failures: Vec<VerifyFailure>,
    /// More failures were found than are listed.
    pub truncated: bool,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    /// List a failure, unless its list already has [`MAX_FAILURES`] places taken. Differing columns, rows at a position below 1 and
    /// everything else (a broken chain, a checkpoint) are three lists: a hundred rows with a changed copy must not leave no room for
    /// the broken chain or the bad checkpoint that matters more.
    fn fail(&mut self, seq: i64, kind: FailureKind) {
        let taken = self
            .failures
            .iter()
            .filter(|f| f.kind.list() == kind.list())
            .count();
        if taken < MAX_FAILURES {
            self.failures.push(VerifyFailure { seq, kind });
        } else {
            self.truncated = true;
        }
    }
}

/// Why a verification could not run to the end (as opposed to finding a mismatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("the audit tables could not be read")]
    Database,
    /// A query did not answer within its time (the server cancelled it, or it was not answered at all).
    #[error("the audit tables did not answer in time")]
    Timeout,
}

/// A verification that could not run to the end, with what it had found until then. An error is not a verdict, but the findings
/// before it are still evidence, and a fault of the database must never discard them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct VerifyAborted {
    pub error: VerifyError,
    /// The findings made before the run stopped. `events_checked` and `checkpoints_checked` say how far it got.
    pub partial: VerifyReport,
}

/// Check one row of the chain. `expected_prev` is the hash of the row before it (the genesis hash for the first).
///
/// # Errors
/// [`FailureKind::ChainLinkBroken`] or [`FailureKind::ChainHashMismatch`].
pub fn check_row(
    expected_prev: &ContentHash,
    prev_hash: &ContentHash,
    hash: &ContentHash,
    event_json: &[u8],
) -> Result<(), FailureKind> {
    if prev_hash != expected_prev {
        return Err(FailureKind::ChainLinkBroken);
    }
    if *hash != chain_hash(prev_hash, event_json) {
        return Err(FailureKind::ChainHashMismatch);
    }
    Ok(())
}

fn hash_of(bytes: Vec<u8>) -> Option<ContentHash> {
    <[u8; 32]>::try_from(bytes).ok().map(ContentHash::from_bytes)
}

/// A column as it is stored; `None` when it is `NULL`, was left out of the query for its size, or its value cannot be decoded. All
/// mean that the row is not one the verifier can use. Any other failure (the column is not in the row) is a fault of the query,
/// and an error.
fn stored_column<'r, T>(
    row: &'r sqlx::postgres::PgRow,
    name: &str,
    stage: &'static str,
) -> Result<Option<T>, VerifyError>
where
    Option<T>: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    match row.try_get::<Option<T>, _>(name) {
        Ok(value) => Ok(value),
        Err(e) if is_decode_failure(&e) => Ok(None),
        Err(e) => Err(db_error(stage, &e)),
    }
}

/// A server that cancelled a query for its time (`statement_timeout`, SQLSTATE 57014) or for a lock it could not get in time
/// (`lock_timeout`, 55P03) is a timeout; any other failure of the query is a database error.
fn db_error(stage: &'static str, e: &sqlx::Error) -> VerifyError {
    let code = e
        .as_database_error()
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned));
    tracing::warn!(
        stage,
        sqlstate = code.as_deref().unwrap_or("none"),
        "audit verification could not read a table"
    );
    match code.as_deref() {
        Some("57014" | "55P03") => VerifyError::Timeout,
        _ => VerifyError::Database,
    }
}

/// The query that finds a row without a `seq`. With the primary key in place it is an index probe that finds nothing at once; only
/// when the key was dropped does it read the table.
const EVENTS_WITHOUT_SEQ: &str = "SELECT EXISTS (SELECT 1 FROM audit_events WHERE seq IS NULL)";
const CHECKPOINTS_WITHOUT_SEQ: &str = "SELECT EXISTS (SELECT 1 FROM audit_checkpoints WHERE seq IS NULL)";

/// The longest `gcs_object` a checkpoint can have: [`OBJECT_PREFIX`], a `u64` in decimal, `.json` (see [`object_name`]).
const GCS_OBJECT_LIMIT: usize = OBJECT_PREFIX.len() + 20 + ".json".len();

/// The text of the query that reads one batch of the chain: the first batch (`keyset` false, `$1` the row limit) and the later ones
/// (`$1` the last `seq` read, `$2` the row limit). Public for the tests that look at its plan and at what it sends.
///
/// The first batch has no `WHERE`: a row at the smallest `seq` an `i64` has must be read too, and `seq > i64::MIN` would skip it.
/// Every column that can be large is bounded in the select (see [`super::columns`]); a value over its bound comes back `NULL`,
/// which for the three columns of the chain means a row the chain cannot vouch for.
#[doc(hidden)]
pub fn chain_batch_sql(keyset: bool) -> String {
    let (filter, limit) = if keyset {
        ("WHERE seq > $1 ", "$2")
    } else {
        ("", "$1")
    };
    format!(
        "SELECT seq, \
         CASE WHEN octet_length(prev_hash) <= {HASH_COLUMN_BYTES} THEN prev_hash END AS prev_hash, \
         CASE WHEN octet_length(hash) <= {HASH_COLUMN_BYTES} THEN hash END AS hash, \
         CASE WHEN octet_length(event_json) <= {MAX_EVENT_JSON_BYTES} THEN event_json END AS event_json, {} \
         FROM audit_events {filter}ORDER BY seq LIMIT {limit}",
        select_list()
    )
}

/// The same for the checkpoints. A column that is `NULL` or over its bound comes back `NULL`: either way the checkpoint cannot be
/// checked.
#[doc(hidden)]
pub fn checkpoint_batch_sql(keyset: bool) -> String {
    let (filter, limit) = if keyset {
        ("WHERE c.seq > $1 ", "$2")
    } else {
        ("", "$1")
    };
    format!(
        "SELECT c.seq, \
         CASE WHEN octet_length(c.head_hash) <= {HASH_COLUMN_BYTES} THEN c.head_hash END AS head_hash, \
         CASE WHEN octet_length(c.kms_key_version) <= {} THEN c.kms_key_version END AS kms_key_version, \
         CASE WHEN octet_length(c.signature) <= {MAX_SIGNATURE_BYTES} THEN c.signature END AS signature, \
         CASE WHEN octet_length(c.gcs_object) <= {GCS_OBJECT_LIMIT} THEN c.gcs_object END AS gcs_object, \
         CASE WHEN octet_length(e.hash) <= {HASH_COLUMN_BYTES} THEN e.hash END AS chain_hash \
         FROM audit_checkpoints c LEFT JOIN audit_events e ON e.seq = c.seq {filter}ORDER BY c.seq LIMIT {limit}",
        KeyRef::MAX_LEN
    )
}

/// What the server is told before the first query of each transaction. Every query of the verifier runs in a transaction of its
/// own (a batch, or a probe), so `SET LOCAL` ends with it and nothing is left on the pooled connection for the next user.
fn set_local_sql(statement: Duration, lock: Duration) -> String {
    let ms = |d: Duration| d.as_millis().clamp(1, 3_600_000);
    format!(
        "SET LOCAL statement_timeout = '{}ms'; SET LOCAL lock_timeout = '{}ms'",
        ms(statement),
        ms(lock)
    )
}

/// The bytes a fetched row holds, as the server sent them.
fn row_bytes(row: &sqlx::postgres::PgRow) -> usize {
    (0..row.columns().len())
        .map(|i| {
            row.try_get_raw(i)
                .ok()
                .and_then(|v| v.as_bytes().ok().map(<[u8]>::len))
                .unwrap_or(0)
        })
        .sum()
}

/// What comparing a chunk of rows found.
struct Compared {
    /// Rows whose link and hash were checked.
    checked: u64,
    findings: Vec<(i64, FailureKind)>,
    /// A row with a bad link or hash was found: the walk ends.
    stop: bool,
    /// A fault of the query (not a finding).
    fault: Option<VerifyError>,
    /// The hash the next row must link to.
    next_prev: ContentHash,
}

/// Compare a chunk of rows with the chain: each row's link and hash, and, for a row whose hash is good, its copied columns. CPU
/// work (SHA-256 over up to a megabyte per row, JSON parsing of the same), so it runs in `spawn_blocking`.
fn compare_rows(rows: &[sqlx::postgres::PgRow], mut expected_prev: ContentHash) -> Compared {
    let mut out = Compared {
        checked: 0,
        findings: Vec::new(),
        stop: false,
        fault: None,
        next_prev: expected_prev,
    };
    for row in rows {
        let seq: i64 = match row.try_get::<Option<i64>, _>("seq") {
            Ok(Some(seq)) => seq,
            // A row with no position is not part of the chain; `any_without_seq` has reported that there is one. (The first batch
            // has no `WHERE`, so it can return such a row, last.)
            Ok(None) => continue,
            Err(e) => {
                out.fault = Some(db_error("chain", &e));
                break;
            }
        };
        // The log hands out positions from 1. A row at any other is not part of the chain: it is reported, skipped, and never
        // taken for the first row or for the link of the one after it.
        if seq <= 0 {
            out.findings.push((seq, FailureKind::SeqNotPositive));
            continue;
        }
        // A value that cannot be read (a `NULL` in a column that is `NOT NULL` by the schema, one over its bound) is not the hash of
        // anything: it is a bad row, found at this position, and not a failure to read the table.
        let (prev, hash, json) = match (
            stored_column::<Vec<u8>>(row, "prev_hash", "chain"),
            stored_column::<Vec<u8>>(row, "hash", "chain"),
            stored_column::<String>(row, "event_json", "chain"),
        ) {
            (Ok(prev), Ok(hash), Ok(json)) => (prev, hash, json),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                out.fault = Some(e);
                break;
            }
        };
        out.checked += 1;
        let checked = match (prev.and_then(hash_of), hash.and_then(hash_of), json.as_deref()) {
            (Some(prev), Some(hash), Some(json)) => {
                check_row(&expected_prev, &prev, &hash, json.as_bytes()).map(|()| (hash, json))
            }
            _ => Err(FailureKind::ChainHashMismatch),
        };
        match checked {
            Ok((hash, json)) => {
                expected_prev = hash;
                // The text is now vouched for by the chain, so the copies of it can be held against it. A copy that cannot be
                // read is a difference too: only a fault of the query is an error.
                match differing_in_row(row, json) {
                    Ok(differ) if differ.is_empty() => {}
                    Ok(differ) => out.findings.push((seq, FailureKind::ColumnsDiffer(differ))),
                    Err(e) => {
                        out.fault = Some(db_error("chain", &e));
                        break;
                    }
                }
            }
            Err(kind) => {
                // Every later row would fail too: name the first position and stop.
                out.findings.push((seq, kind));
                out.stop = true;
                break;
            }
        }
    }
    out.next_prev = expected_prev;
    out
}

pub struct ChainVerifier {
    pool: PgPool,
    bucket: Arc<dyn CheckpointBucket>,
    verifier: Arc<dyn CheckpointVerifier>,
    batch_rows: usize,
    statement_timeout: Duration,
    lock_timeout: Duration,
}

impl std::fmt::Debug for ChainVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainVerifier")
            .field("batch_rows", &self.batch_rows)
            .finish_non_exhaustive()
    }
}

impl ChainVerifier {
    pub fn new(
        pool: PgPool,
        bucket: Arc<dyn CheckpointBucket>,
        verifier: Arc<dyn CheckpointVerifier>,
    ) -> Self {
        Self {
            pool,
            bucket,
            verifier,
            batch_rows: DEFAULT_BATCH_ROWS,
            statement_timeout: DEFAULT_STATEMENT_TIMEOUT,
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// Rows per keyset batch (at least one).
    #[must_use]
    pub fn with_batch_rows(mut self, rows: usize) -> Self {
        self.batch_rows = rows.max(1);
        self
    }

    /// Wait at most `statement` for any one query and `lock` for any lock. Both are told to the server (`SET LOCAL`, so they end
    /// with the transaction); the client also gives up 30 seconds after `statement`, for a server that does not answer at all.
    #[must_use]
    pub fn with_timeouts(mut self, statement: Duration, lock: Duration) -> Self {
        self.statement_timeout = statement;
        self.lock_timeout = lock;
        self
    }

    /// Verify the chain and every checkpoint. A mismatch is a finding in the report. An error means the tables could not be read
    /// to the end; it carries what was found before it ([`VerifyAborted::partial`]), which is still evidence. The two halves are
    /// independent: a fault in the first does not stop the second.
    pub async fn verify(&self) -> Result<VerifyReport, VerifyAborted> {
        let mut report = VerifyReport::default();
        let chain = self.verify_chain(&mut report).await;
        let checkpoints = self.verify_checkpoints(&mut report).await;
        match chain.and(checkpoints) {
            Ok(()) => Ok(report),
            Err(error) => Err(VerifyAborted {
                error,
                partial: report,
            }),
        }
    }

    /// How long the client waits for the server to answer one step before it gives up on it.
    fn step_timeout(&self) -> Duration {
        self.statement_timeout + CLIENT_MARGIN
    }

    /// One step against the database, with the client's own time limit.
    async fn step<T>(
        &self,
        stage: &'static str,
        step: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, VerifyError> {
        match tokio::time::timeout(self.step_timeout(), step).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(db_error(stage, &e)),
            Err(_) => {
                tracing::warn!(stage, "audit verification query timed out");
                Err(VerifyError::Timeout)
            }
        }
    }

    /// A transaction with the verifier's timeouts set for it.
    async fn begin(
        &self,
        stage: &'static str,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, VerifyError> {
        let mut tx = self.step(stage, self.pool.begin()).await?;
        let set = set_local_sql(self.statement_timeout, self.lock_timeout);
        // Text with no bound arguments goes out as a simple query, which is what allows two statements in it.
        self.step(stage, sqlx::Executor::execute(&mut *tx, set.as_str()))
            .await?;
        Ok(tx)
    }

    /// Whether any row of a table has no `seq`.
    async fn any_without_seq(&self, stage: &'static str, sql: &'static str) -> Result<bool, VerifyError> {
        let mut tx = self.begin(stage).await?;
        let found: bool = self
            .step(stage, sqlx::query_scalar(sql).fetch_one(&mut *tx))
            .await?;
        self.step(stage, tx.commit()).await?;
        Ok(found)
    }

    /// Compare a chunk of rows on a blocking thread and merge what it found into the report. `Ok(true)` when the walk ends here.
    async fn compare_chunk(
        &self,
        chunk: &mut Vec<sqlx::postgres::PgRow>,
        expected_prev: &mut ContentHash,
        report: &mut VerifyReport,
    ) -> Result<bool, VerifyError> {
        if chunk.is_empty() {
            return Ok(false);
        }
        let rows = std::mem::take(chunk);
        let prev = *expected_prev;
        let compared = tokio::task::spawn_blocking(move || compare_rows(&rows, prev))
            .await
            .map_err(|_| {
                tracing::warn!(stage = "chain", "audit verification comparison did not finish");
                VerifyError::Database
            })?;
        *expected_prev = compared.next_prev;
        report.events_checked += compared.checked;
        for (seq, kind) in compared.findings {
            report.fail(seq, kind);
        }
        match compared.fault {
            Some(fault) => Err(fault),
            None => Ok(compared.stop),
        }
    }

    async fn verify_chain(&self, report: &mut VerifyReport) -> Result<(), VerifyError> {
        // A row with no `seq` is never returned by a keyset read; look for it once.
        if self.any_without_seq("chain", EVENTS_WITHOUT_SEQ).await? {
            report.fail(0, FailureKind::EventPositionMissing);
        }
        let limit = i64::try_from(self.batch_rows).unwrap_or(i64::MAX);
        let (first_sql, next_sql) = (chain_batch_sql(false), chain_batch_sql(true));
        // `None` until the first row is read: the first batch starts at the smallest `seq` there is, whatever it is.
        let mut after: Option<i64> = None;
        let mut expected_prev = genesis_hash();
        loop {
            let resumed_from = after;
            let mut tx = self.begin("chain").await?;
            let mut query = sqlx::query(if after.is_some() { &next_sql } else { &first_sql });
            if let Some(last) = after {
                query = query.bind(last);
            }
            let mut rows = query.bind(limit).fetch(&mut *tx);
            let mut in_batch = 0_usize;
            let mut chunk = Vec::new();
            let mut chunk_bytes = 0_usize;
            let mut fault = None;
            let mut stop = false;
            // The rows are read one at a time and compared in chunks that are bounded in count and in bytes, so the memory of a
            // batch is that of a chunk, whatever the rows hold.
            loop {
                let row = match tokio::time::timeout(self.step_timeout(), rows.try_next()).await {
                    Ok(Ok(Some(row))) => row,
                    Ok(Ok(None)) => break,
                    Ok(Err(e)) => {
                        fault = Some(db_error("chain", &e));
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(stage = "chain", "audit verification query timed out");
                        fault = Some(VerifyError::Timeout);
                        break;
                    }
                };
                in_batch += 1;
                match row.try_get::<Option<i64>, _>("seq") {
                    Ok(seq) => after = seq.or(after),
                    Err(e) => {
                        fault = Some(db_error("chain", &e));
                        break;
                    }
                }
                chunk_bytes += row_bytes(&row);
                chunk.push(row);
                if chunk.len() >= CHUNK_ROWS || chunk_bytes >= CHUNK_BYTES {
                    chunk_bytes = 0;
                    stop = self.compare_chunk(&mut chunk, &mut expected_prev, report).await?;
                    if stop {
                        break;
                    }
                }
            }
            // What was read before a fault is still compared: its findings are evidence.
            let last = if stop {
                Ok(true)
            } else {
                self.compare_chunk(&mut chunk, &mut expected_prev, report).await
            };
            drop(rows);
            if let Some(fault) = fault {
                return Err(fault);
            }
            if last? {
                // Every later row would fail too.
                report.batches += 1;
                return Ok(());
            }
            self.step("chain", tx.commit()).await?;
            if in_batch > 0 {
                report.batches += 1;
            }
            // A full batch with no `seq` in it (rows with none sort last, and the first batch has no `WHERE`) gives nothing to
            // continue from: the walk ends, and does not ask for the same batch for ever.
            if in_batch < self.batch_rows || after == resumed_from {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    }

    async fn verify_checkpoints(&self, report: &mut VerifyReport) -> Result<(), VerifyError> {
        if self
            .any_without_seq("checkpoints", CHECKPOINTS_WITHOUT_SEQ)
            .await?
        {
            report.fail(0, FailureKind::CheckpointPositionMissing);
        }
        let (first_sql, next_sql) = (checkpoint_batch_sql(false), checkpoint_batch_sql(true));
        let mut after: Option<i64> = None;
        loop {
            let resumed_from = after;
            let mut tx = self.begin("checkpoints").await?;
            let mut query = sqlx::query(if after.is_some() { &next_sql } else { &first_sql });
            if let Some(last) = after {
                query = query.bind(last);
            }
            // At most `CHECKPOINT_BATCH` rows, and each is bounded by the select.
            let rows = self
                .step("checkpoints", query.bind(CHECKPOINT_BATCH).fetch_all(&mut *tx))
                .await?;
            self.step("checkpoints", tx.commit()).await?;
            let n = rows.len();
            for row in rows {
                let seq = row
                    .try_get::<Option<i64>, _>("seq")
                    .map_err(|e| db_error("checkpoints", &e))?;
                // A row with no position cannot be checked; `any_without_seq` has reported that there is one. (The first batch
                // has no `WHERE`, so it can return such a row, last.)
                let Some(seq) = seq else {
                    continue;
                };
                after = Some(seq);
                // A column that is `NULL`, over its bound, or unreadable is a finding about this checkpoint, which cannot be
                // checked; the next one is.
                let head = stored_column::<Vec<u8>>(&row, "head_hash", "checkpoints")?;
                let key_version = stored_column::<String>(&row, "kms_key_version", "checkpoints")?;
                let signature = stored_column::<Vec<u8>>(&row, "signature", "checkpoints")?;
                let object = stored_column::<String>(&row, "gcs_object", "checkpoints")?;
                let chain = stored_column::<Vec<u8>>(&row, "chain_hash", "checkpoints")?;
                report.checkpoints_checked += 1;
                let mut invalid = CheckpointColumnSet::empty();
                for (column, missing) in [
                    (CheckpointColumn::HeadHash, head.is_none()),
                    (CheckpointColumn::KmsKeyVersion, key_version.is_none()),
                    (CheckpointColumn::Signature, signature.is_none()),
                    (CheckpointColumn::GcsObject, object.is_none()),
                ] {
                    if missing {
                        invalid.insert(column);
                    }
                }
                if !invalid.is_empty() {
                    report.fail(seq, FailureKind::CheckpointColumnsInvalid(invalid));
                    continue;
                }
                if let (Some(head), Some(key_version), Some(signature), Some(object)) =
                    (head, key_version, signature, object)
                {
                    if let Err(kind) = self
                        .check_checkpoint(
                            seq,
                            hash_of(head),
                            &key_version,
                            &signature,
                            &object,
                            chain.and_then(hash_of),
                        )
                        .await
                    {
                        report.fail(seq, kind);
                    }
                }
            }
            // As for the chain: a full batch with no `seq` in it has nothing to continue from.
            if i64::try_from(n).unwrap_or(i64::MAX) < CHECKPOINT_BATCH || after == resumed_from {
                return Ok(());
            }
        }
    }

    async fn check_checkpoint(
        &self,
        seq: i64,
        head: Option<ContentHash>,
        key_version: &str,
        signature: &[u8],
        object: &str,
        chain: Option<ContentHash>,
    ) -> Result<(), FailureKind> {
        // A checkpoint is written for a head, and the first head is at 1: no checkpoint belongs at any other position.
        let Some(position) = u64::try_from(seq).ok().filter(|p| *p > 0) else {
            return Err(FailureKind::CheckpointHeadMissing);
        };
        let Some(head) = head else {
            return Err(FailureKind::CheckpointHeadMismatch);
        };
        let Some(chain) = chain else {
            return Err(FailureKind::CheckpointHeadMissing);
        };
        if chain != head {
            return Err(FailureKind::CheckpointHeadMismatch);
        }
        if object != object_name(position) {
            return Err(FailureKind::CheckpointNameInvalid);
        }
        let stored = match tokio::time::timeout(BUCKET_TIMEOUT, self.bucket.get(object)).await {
            Ok(Ok(Some(bytes))) => bytes,
            Ok(Ok(None)) => return Err(FailureKind::BucketObjectMissing),
            Ok(Err(BucketError::Unavailable | BucketError::Rejected)) | Err(_) => {
                return Err(FailureKind::BucketUnavailable);
            }
            Ok(Err(BucketError::TooLarge | BucketError::InvalidName)) => {
                return Err(FailureKind::BucketObjectMismatch);
            }
        };
        let parsed = CheckpointObject::parse(&stored).map_err(|_| FailureKind::BucketObjectMismatch)?;
        if parsed.seq != position
            || parsed.head != head
            || parsed.signature != signature
            || parsed.key_version.as_str() != key_version
        {
            return Err(FailureKind::BucketObjectMismatch);
        }
        if !self
            .verifier
            .verify(key_version, &checkpoint_message(position, &head), signature)
        {
            return Err(FailureKind::SignatureInvalid);
        }
        Ok(())
    }
}

/// The failure that heads the alert: the most severe by [`FailureKind::severity`], and among equals the first found. The same
/// failure whatever order the walk found them in.
fn most_severe(failures: &[VerifyFailure]) -> Option<&VerifyFailure> {
    failures.iter().min_by_key(|f| f.kind.severity())
}

/// The one line of the alert: how many problems, and where and what the most severe is. For a failure about columns it names them if
/// the line stays inside a [`ShortText`] and counts them otherwise, so the alert is always built. `incomplete` says that the run
/// then could not read the tables to the end.
fn alert_summary(count: usize, more: &str, worst: &VerifyFailure, incomplete: bool) -> String {
    let base = format!(
        "{count}{more} problem(s) in the audit log. Most severe: {} at position {}",
        worst.kind.as_str(),
        worst.seq
    );
    let suffix = if incomplete {
        ". The run did not finish."
    } else {
        ""
    };
    let Some(columns) = worst.kind.detail() else {
        return format!("{base}{suffix}");
    };
    let named = format!("{base} ({columns}){suffix}");
    if ShortText::parse(&named).is_ok() {
        named
    } else {
        let count = columns.split(", ").count();
        format!("{base} ({count} columns){suffix}")
    }
}

/// What a nightly run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NightlyOutcome {
    /// Another replica holds the lease (it ran, or is running).
    NotLeader,
    Clean(VerifyReport),
    /// A mismatch was found; the admins were told.
    Failed(VerifyReport),
    /// The tables could not be read; nothing was verified.
    Errored,
    /// The lease ran out during a long run, so the run was abandoned.
    LeaseLost,
    /// A mismatch was found, the admins were told, and the run then could not read the tables to the end.
    Incomplete(VerifyReport),
}

/// The nightly job: one replica verifies, and a mismatch raises an alert.
pub struct NightlyVerifyJob {
    verifier: ChainVerifier,
    leases: Arc<dyn Leases>,
    notifier: Arc<dyn Notifier>,
    metrics: Arc<IdentityMetrics>,
}

impl std::fmt::Debug for NightlyVerifyJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NightlyVerifyJob").finish_non_exhaustive()
    }
}

impl NightlyVerifyJob {
    pub fn new(
        verifier: ChainVerifier,
        leases: Arc<dyn Leases>,
        notifier: Arc<dyn Notifier>,
        metrics: Arc<IdentityMetrics>,
    ) -> Self {
        Self {
            verifier,
            leases,
            notifier,
            metrics,
        }
    }

    /// Verify now if this replica holds the lease. The lease is not released afterwards: it runs out, so the other replicas, which
    /// wake at the same time, find it held and skip, and the admins hear of a mismatch once.
    pub async fn run_once(&self) -> Result<NightlyOutcome, LeaseError> {
        let Some(mut lease) = self.leases.try_acquire(LEASE_NAME, LEASE_TTL).await? else {
            return Ok(NightlyOutcome::NotLeader);
        };
        let verify = self.verifier.verify();
        tokio::pin!(verify);
        let mut renew = tokio::time::interval(LEASE_TTL / 3);
        renew.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        renew.tick().await;
        let result = loop {
            tokio::select! {
                result = &mut verify => break result,
                _ = renew.tick() => {
                    if lease.renew(LEASE_TTL).await.is_err() {
                        tracing::error!("the audit verification lease was lost; the run is abandoned");
                        return Ok(NightlyOutcome::LeaseLost);
                    }
                }
            }
        };
        let (report, aborted) = match result {
            Ok(report) => (report, false),
            Err(VerifyAborted { error, partial }) => {
                self.metrics.verify_errored();
                tracing::error!(
                    error = %error,
                    found = partial.failures.len(),
                    events = partial.events_checked,
                    checkpoints = partial.checkpoints_checked,
                    "audit chain verification could not run to the end"
                );
                if partial.is_clean() {
                    return Ok(NightlyOutcome::Errored);
                }
                // What it found before the fault is not thrown away with it.
                self.metrics
                    .verify_found(u64::try_from(partial.failures.len()).unwrap_or(u64::MAX));
                (partial, true)
            }
        };
        if !aborted {
            self.metrics
                .verify_ran(u64::try_from(report.failures.len()).unwrap_or(u64::MAX));
            if report.is_clean() {
                tracing::info!(
                    events = report.events_checked,
                    checkpoints = report.checkpoints_checked,
                    "audit chain verified"
                );
                return Ok(NightlyOutcome::Clean(report));
            }
        }
        for failure in &report.failures {
            // Column names are structure, not content; the values of the columns are never read into a message.
            let columns = failure.kind.detail().unwrap_or_default();
            tracing::error!(
                seq = failure.seq,
                reason = failure.kind.as_str(),
                columns = columns.as_str(),
                "audit chain verification failed"
            );
        }
        self.alert(&report, aborted).await;
        Ok(if aborted {
            NightlyOutcome::Incomplete(report)
        } else {
            NightlyOutcome::Failed(report)
        })
    }

    /// Tell the admins. A notification problem is logged and goes no further.
    async fn alert(&self, report: &VerifyReport, incomplete: bool) {
        let Some(worst) = most_severe(&report.failures) else {
            return;
        };
        let more = if report.truncated { "+" } else { "" };
        let summary = alert_summary(report.failures.len(), more, worst, incomplete);
        let (Ok(title), Ok(summary)) = (
            ShortText::parse("Audit log verification failed"),
            ShortText::parse(&summary),
        ) else {
            tracing::error!("audit alert could not be built");
            return;
        };
        let note = Notification {
            kind: NotificationKind::CriticalFinding,
            severity: Severity::Critical,
            swimlane: None,
            title,
            summary,
            link: None,
        };
        match tokio::time::timeout(NOTIFY_TIMEOUT, self.notifier.notify(note)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!(error = %e, "audit alert was not delivered"),
            Err(_) => tracing::error!("audit alert timed out"),
        }
    }

    /// Run [`NightlyVerifyJob::run_once`] after `first_delay` and then every `every`, until the handle is dropped.
    pub fn spawn(self: Arc<Self>, first_delay: Duration, every: Duration) -> JobHandle {
        JobHandle::spawn(async move {
            tokio::time::sleep(first_delay).await;
            loop {
                if let Err(e) = self.run_once().await {
                    tracing::error!(error = %e, "audit verification lease failed");
                }
                tokio::time::sleep(super::jittered(every)).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, canonical_json};

    use super::*;

    fn rows(n: usize) -> Vec<(ContentHash, ContentHash, Vec<u8>)> {
        let mut prev = genesis_hash();
        let mut out = Vec::new();
        for i in 0..n {
            let e = AuditEvent::new(
                domain::Timestamp::from_unix_millis(i64::try_from(i).unwrap()),
                AuditActor::System {
                    component: ShortText::parse("t").unwrap(),
                },
                AuditAction::SettingsChanged,
                AuditVia::System,
            );
            let json = canonical_json(&e).unwrap();
            let hash = chain_hash(&prev, &json);
            out.push((prev, hash, json));
            prev = hash;
        }
        out
    }

    fn first_failure(rows: &[(ContentHash, ContentHash, Vec<u8>)]) -> Option<(usize, FailureKind)> {
        let mut expected = genesis_hash();
        for (i, (prev, hash, json)) in rows.iter().enumerate() {
            match check_row(&expected, prev, hash, json) {
                Ok(()) => expected = *hash,
                Err(kind) => return Some((i, kind)),
            }
        }
        None
    }

    #[test]
    fn an_intact_chain_passes() {
        assert_eq!(first_failure(&rows(20)), None);
    }

    #[test]
    fn a_changed_event_is_a_hash_mismatch_and_a_removed_row_a_broken_link() {
        let mut r = rows(5);
        r[2].2[3] ^= 1;
        assert_eq!(first_failure(&r), Some((2, FailureKind::ChainHashMismatch)));
        let mut r = rows(5);
        r.remove(2);
        assert_eq!(first_failure(&r), Some((2, FailureKind::ChainLinkBroken)));
        let mut r = rows(5);
        r[0].0 = ContentHash::from_bytes([1; 32]);
        assert_eq!(first_failure(&r), Some((0, FailureKind::ChainLinkBroken)));
    }

    #[test]
    fn the_alert_names_the_columns_when_they_fit_and_counts_them_when_they_do_not() {
        use super::super::columns::Column;
        let few = VerifyFailure {
            seq: 5,
            kind: FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::ActorRef, Column::At])),
        };
        let line = alert_summary(1, "", &few, false);
        assert_eq!(
            line,
            "1 problem(s) in the audit log. Most severe: columns differ from the stored event at position 5 (at, actor_ref)"
        );
        // Every column, and a position and a count that use all their digits: the line must still be a valid `ShortText`.
        let all = VerifyFailure {
            seq: i64::MIN,
            kind: FailureKind::ColumnsDiffer(ColumnSet::all()),
        };
        let line = alert_summary(usize::MAX, "+", &all, true);
        assert!(ShortText::parse(&line).is_ok(), "{} bytes: {line}", line.len());
        assert!(line.ends_with("The run did not finish."), "{line}");
        assert!(line.contains(&format!("position {}", i64::MIN)), "{line}");
        // A failure of another kind is the old line.
        let other = VerifyFailure {
            seq: 3,
            kind: FailureKind::ChainHashMismatch,
        };
        assert_eq!(
            alert_summary(2, "", &other, false),
            "2 problem(s) in the audit log. Most severe: event hash mismatch at position 3"
        );
        assert_eq!(
            alert_summary(2, "", &other, true),
            "2 problem(s) in the audit log. Most severe: event hash mismatch at position 3. The run did not finish."
        );
        // A checkpoint with an unreadable column names it, as a column difference does.
        let cp = VerifyFailure {
            seq: 12,
            kind: FailureKind::CheckpointColumnsInvalid(CheckpointColumnSet::of(&[
                CheckpointColumn::Signature,
                CheckpointColumn::HeadHash,
            ])),
        };
        assert_eq!(
            alert_summary(1, "", &cp, false),
            "1 problem(s) in the audit log. Most severe: checkpoint row has a missing or unreadable column at position 12 (head_hash, signature)"
        );
    }

    #[test]
    fn the_worst_failure_heads_the_alert_whatever_the_order_found() {
        use super::super::columns::Column;
        let differ = FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::Via]));
        let at = |seq, kind| VerifyFailure { seq, kind };
        // Chain damage beats a bad checkpoint, which beats a differing column, which beats a bucket that could not be read.
        let all = [
            at(1, FailureKind::BucketUnavailable),
            at(2, differ),
            at(3, FailureKind::SignatureInvalid),
            at(4, FailureKind::ChainLinkBroken),
            at(5, FailureKind::ChainHashMismatch),
        ];
        assert_eq!(
            most_severe(&all),
            Some(&all[3]),
            "the first of the chain failures"
        );
        assert_eq!(most_severe(&all[..4]), Some(&all[3]));
        assert_eq!(most_severe(&all[..3]), Some(&all[2]));
        assert_eq!(most_severe(&all[..2]), Some(&all[1]));
        assert_eq!(most_severe(&all[..1]), Some(&all[0]));
        // Among equals, the first found.
        let two = [
            at(8, FailureKind::SignatureInvalid),
            at(9, FailureKind::BucketObjectMissing),
        ];
        assert_eq!(most_severe(&two), Some(&two[0]));
        assert_eq!(most_severe(&[]), None);
        // Every kind has a rank; the chain kinds are the first rank.
        for kind in [
            FailureKind::ChainLinkBroken,
            FailureKind::ChainHashMismatch,
            FailureKind::SeqNotPositive,
            FailureKind::EventPositionMissing,
        ] {
            assert_eq!(kind.severity(), 0, "{}", kind.as_str());
        }
        assert_eq!(differ.severity(), 2);
        assert_eq!(FailureKind::CheckpointPositionMissing.severity(), 1);
        assert_eq!(FailureKind::BucketUnavailable.severity(), 3);
    }

    #[test]
    fn a_set_of_checkpoint_columns_lists_names_in_table_order_and_never_a_value() {
        let set = CheckpointColumnSet::of(&[CheckpointColumn::GcsObject, CheckpointColumn::HeadHash]);
        assert_eq!(set.names(), ["head_hash", "gcs_object"]);
        assert_eq!(set.to_string(), "head_hash, gcs_object");
        let mut seen = CheckpointColumnSet::empty();
        assert!(seen.is_empty());
        for c in CheckpointColumn::ALL {
            assert!(!seen.contains(c));
            seen.insert(c);
        }
        assert_eq!(seen.names().len(), CheckpointColumn::ALL.len());
    }

    #[test]
    fn column_failures_never_crowd_out_the_other_kinds() {
        use super::super::columns::Column;
        let differ = FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::Via]));
        let mut report = VerifyReport::default();
        for n in 0..(MAX_FAILURES + 50) {
            report.fail(i64::try_from(n).unwrap(), differ);
        }
        // The list of differing columns is full, and says so ...
        assert_eq!(report.failures.len(), MAX_FAILURES);
        assert!(report.truncated);
        // ... and a broken chain, and as many checkpoint failures as the other list holds, still get in.
        report.fail(1_000, FailureKind::ChainHashMismatch);
        for n in 0..(MAX_FAILURES + 5) {
            report.fail(2_000 + i64::try_from(n).unwrap(), FailureKind::SignatureInvalid);
        }
        assert_eq!(report.failures.len(), 2 * MAX_FAILURES);
        assert_eq!(
            report
                .failures
                .iter()
                .filter(|f| f.kind.columns().is_none())
                .count(),
            MAX_FAILURES
        );
        assert!(report.failures.contains(&VerifyFailure {
            seq: 1_000,
            kind: FailureKind::ChainHashMismatch
        }));
        // The other way round: a full list of checkpoint failures does not keep a differing column out.
        let mut report = VerifyReport::default();
        for n in 0..(MAX_FAILURES + 5) {
            report.fail(i64::try_from(n).unwrap(), FailureKind::BucketObjectMissing);
        }
        report.truncated = false;
        report.fail(7, differ);
        assert!(report.failures.contains(&VerifyFailure { seq: 7, kind: differ }));
        assert!(!report.truncated);
    }

    #[test]
    fn rows_forged_below_position_1_do_not_crowd_out_the_chain_or_the_checkpoints() {
        let mut report = VerifyReport::default();
        for n in 0..(MAX_FAILURES + 20) {
            report.fail(-i64::try_from(n).unwrap(), FailureKind::SeqNotPositive);
        }
        assert_eq!(report.failures.len(), MAX_FAILURES);
        assert!(report.truncated);
        report.fail(7, FailureKind::ChainHashMismatch);
        report.fail(9, FailureKind::CheckpointHeadMismatch);
        report.fail(
            11,
            FailureKind::ColumnsDiffer(ColumnSet::of(&[super::super::columns::Column::Via])),
        );
        for (seq, kind) in [
            (7, FailureKind::ChainHashMismatch),
            (9, FailureKind::CheckpointHeadMismatch),
        ] {
            assert!(report.failures.contains(&VerifyFailure { seq, kind }));
        }
        assert_eq!(report.failures.len(), MAX_FAILURES + 3);
    }

    #[test]
    fn the_server_is_told_both_timeouts_for_the_transaction_only() {
        assert_eq!(
            set_local_sql(Duration::from_secs(300), Duration::from_secs(30)),
            "SET LOCAL statement_timeout = '300000ms'; SET LOCAL lock_timeout = '30000ms'"
        );
        // Zero would mean "no limit" to the server; it is never sent. Nor is anything past an hour.
        assert_eq!(
            set_local_sql(Duration::ZERO, Duration::from_secs(86_400)),
            "SET LOCAL statement_timeout = '1ms'; SET LOCAL lock_timeout = '3600000ms'"
        );
    }

    #[test]
    fn the_sql_of_a_batch_is_bounded_and_keyed_as_documented() {
        let (first, next) = (chain_batch_sql(false), chain_batch_sql(true));
        assert!(
            !first.contains("WHERE") && first.ends_with("ORDER BY seq LIMIT $1"),
            "{first}"
        );
        assert!(next.contains("WHERE seq > $1 ORDER BY seq LIMIT $2"), "{next}");
        // The three columns of the chain are bounded; `event_json` by the size the log enforces.
        for guard in [
            "CASE WHEN octet_length(prev_hash) <= 32 THEN prev_hash END AS prev_hash",
            "CASE WHEN octet_length(hash) <= 32 THEN hash END AS hash",
            "CASE WHEN octet_length(event_json) <= 1048576 THEN event_json END AS event_json",
        ] {
            assert!(first.contains(guard), "{guard}");
        }
        let (first, next) = (checkpoint_batch_sql(false), checkpoint_batch_sql(true));
        assert!(
            !first.contains("WHERE") && first.ends_with("ORDER BY c.seq LIMIT $1"),
            "{first}"
        );
        assert!(
            next.contains("WHERE c.seq > $1 ORDER BY c.seq LIMIT $2"),
            "{next}"
        );
        for guard in [
            "octet_length(c.head_hash) <= 32",
            "octet_length(c.kms_key_version) <= 512",
            "octet_length(c.signature) <= 128",
            "octet_length(c.gcs_object) <= 43",
            "octet_length(e.hash) <= 32",
        ] {
            assert!(first.contains(guard), "{guard}");
        }
        // No column is selected as it is: each is the `END AS` of a guard.
        for bare in [
            "c.head_hash,",
            "c.kms_key_version,",
            "c.signature,",
            "c.gcs_object,",
            "e.hash ",
        ] {
            assert!(
                !first.contains(&format!(", {bare}")) && !first.contains(&format!("SELECT {bare}")),
                "{bare}"
            );
        }
    }

    #[test]
    fn the_longest_object_name_is_what_the_bound_allows() {
        // `object_name` of the largest position: the bound on `gcs_object` is exactly that long.
        assert_eq!(object_name(u64::MAX).len(), GCS_OBJECT_LIMIT);
        assert_eq!(MAX_SIGNATURE_BYTES, 128);
        assert_eq!(KeyRef::MAX_LEN, 512);
    }

    #[test]
    fn the_report_keeps_a_bounded_list() {
        let mut report = VerifyReport::default();
        for n in 0..(MAX_FAILURES + 5) {
            report.fail(i64::try_from(n).unwrap(), FailureKind::BucketObjectMissing);
        }
        assert_eq!(report.failures.len(), MAX_FAILURES);
        assert!(report.truncated);
        assert!(!report.is_clean());
    }
}
