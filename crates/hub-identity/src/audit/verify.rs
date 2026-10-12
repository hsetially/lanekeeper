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
//! value over the bound of its column or one the select does not vet: the select decides in the database, before anything is sent,
//! and never evaluates anything on a value that can fail or grow with it (see [`super::columns`] for the checks, and why a `jsonb`
//! number such as `1e131000` is the case that needed them). Each is reported against its row, by column name only.
//!
//! What is not a finding about a value is a failure of the query, and there are two kinds:
//!
//! - **The database cannot be read now**: a lost connection, a statement or lock that ran out of time, a resource limit. It ends
//!   the run as an error ([`VerifyError::Database`], [`VerifyError::Timeout`]), but not the findings: [`VerifyAborted`] carries
//!   what was found before it, the two halves (chain, checkpoints) are run whatever happens to the other, and
//!   [`NightlyVerifyJob`] alerts on findings even when the run then errored ([`NightlyOutcome::Incomplete`]). Such a run says
//!   nothing to the admins by itself, because the next night usually reads fine; but the job counts the nights, and from the
//!   second in a row it tells them (below).
//! - **The tables are not what the verifier reads**: the server refuses the query with SQLSTATE class 42 (an undefined column,
//!   function or operator, a datatype mismatch, an undefined table, a privilege taken away), or returns a column of another type
//!   than the one it is read as. The queries are fixed text, so this is never a value's fault, and nothing transient does it: an
//!   owner dropped, renamed or retyped a column, which blinds the verifier (`ALTER COLUMN hash TYPE int` makes every query fail).
//!   That is [`VerifyError::SchemaChanged`], and it is a finding as well as an error ([`FailureKind::SchemaChanged`], at position
//!   0, carrying only the SQLSTATE): the run alerts that night as [`NightlyOutcome::Incomplete`] and raises
//!   `audit_verify_schema_changes_total`. A migration that changes the audit tables must change the verifier first, or it will
//!   raise this alert; the append-only design allows no migration that does.
//!
//! When a retype leaves the query working, what the verifier says is by the values: if every value is still what the event says
//! the log is clean (a `text` column made `varchar` has not been tampered with), and if a value can no longer be read as the type
//! it is read as, that column differs (`approval_id` made `int`, `actor_ref` made `bytea`).
//!
//! # Nights without a verdict
//!
//! [`NightlyVerifyJob`] counts, in memory, the nights in a row that ended without a verdict ([`NightlyOutcome::Errored`] and
//! [`NightlyOutcome::Incomplete`]). From the [`DEFAULT_ALERT_AFTER_FAILED_RUNS`]th in a row (two; [`NightlyVerifyJob::with_alert_after`])
//! it tells the admins, through the notifier that carries every other alert, that verification "could not complete N nights in a
//! row", with the number and the class of the last error (a kind, or the SQLSTATE) and nothing else, and it does so every night
//! until a run completes. A run that ran to the end ([`NightlyOutcome::Clean`], [`NightlyOutcome::Failed`]) ends the count. A run
//! that was not the leader's ([`NightlyOutcome::NotLeader`]) or was abandoned when the lease was lost ([`NightlyOutcome::LeaseLost`])
//! neither counts nor ends it: it learned nothing about the database.
//!
//! **The count is not persistent, and that has a cost.** The `Leases` port holds no payload and `db/migrations/` is a contract path,
//! so the count lives in the job: a restart of the pod, or the lease passing to another replica, starts it again from zero. A pod
//! that restarts every day and fails every night never reaches two, and two replicas that take the lease on alternate nights never
//! do either. The count is therefore also exposed so that Prometheus, which keeps history, can alert independently of this job and
//! of the notifier: `audit_verify_unfinished_runs_in_a_row` (a gauge, with the same limit), `audit_verify_errors_total` and
//! `audit_verify_runs_total` (counters, read by `increase`, which survives resets); the expressions are in [`crate::metrics`]. A
//! durable count needs a small table, `audit_verify_state`, in a contract-change PR (see the plan's follow-ups).
//!
//! # Memory, time and threads
//!
//! The chain is read by keyset in batches (`seq >= $last ORDER BY seq LIMIT 5000`, the first batch with no `WHERE` so that no `seq`
//! is skipped), each in a transaction of its own that begins with `SET LOCAL statement_timeout` and `lock_timeout` (5 minutes and
//! 30 seconds by default), and the client gives up 30 seconds after the statement timeout. The rows are streamed and compared in
//! chunks of at most 1,024 rows or 16 MiB as sent, on a blocking thread (hashing and parsing up to a megabyte per row is CPU work),
//! so the memory held is a chunk, not a batch and not the chain. A legitimate row is a few kilobytes (the largest part of an event
//! is its diff, at most 64 KiB), about 230 of the largest fit a chunk; a row with every bounded column full is about 14 MiB.
//!
//! **The statement timeout counts the time the client spends comparing.** The rows of a batch are streamed: the server sends them
//! as the client takes them, and blocks when the client does not, and a statement that is blocked on its client is still running,
//! so `statement_timeout` runs through the comparison of every chunk as well as through the query. For legitimate rows this is
//! nothing: a batch of 5,000 events of a few kilobytes was verified in 51 ms in a release build (round 3, one run), against a
//! timeout of five minutes. A batch of the largest rows the bounds allow is another matter (up to 14 MB a row, hashed and parsed
//! on this side while the statement waits), and the run would then end as a timeout, which is an error and not a finding. How
//! close a batch of hostile rows (events of a megabyte, which the table's `CHECK` lets the INSERT-only role write) can come to
//! five minutes was not measured; it is the cost of not holding a batch in memory, and the nights without a verdict are counted.
//!
//! # Positions used twice
//!
//! A batch starts *at* the last position the one before it read, so that all the rows that share a position are in one batch (a
//! `seq > $last` would skip the second of two rows at the position that ended a batch, and the chain would show it only as a
//! broken link one row later, or not at all). The rows of that position that were already compared are skipped, not compared
//! twice. A row whose position is not after the previous row's is [`FailureKind::EventPositionDuplicated`] (or
//! [`FailureKind::CheckpointPositionDuplicated`]), once per position. The same position twice is a damaged log: the log hands out
//! each once, and only a dropped primary key lets it happen. In a table without the key the order of the rows that share a
//! position is not promised, so the report may also name a broken link next to the repeated position. A position held by more
//! rows than a batch (5,000 by default) is reported when a whole batch is rows at it; the rows of it beyond what a batch reaches
//! are not compared, and as the chain then does not link, the first row after it is a broken link.
//!
//! The first row with a bad link or hash ends the walk of the chain, because every row after a bad one fails too; the report names
//! that one position. A row whose columns differ does not end it (the chain is intact after it), so each such row is listed.
//! Checkpoints are checked one by one, each reporting its first problem. The report lists at most 100 differing columns, 100 rows
//! below position 1, 100 repeated positions and, separately, 100 failures of every other kind, so that no one kind crowds out
//! the others.
//!
//! What it cannot see, by design: an event removed *and* its checkpoint rows removed, or a tail cut off after the newest
//! checkpoint, leave a chain that is internally consistent. The bucket's retention keeps the objects of the removed checkpoints,
//! but this job starts from the rows in the table. The newest unchecked stretch is at most an hour.
//!
//! The row hash is recomputed from the stored text, not from a re-parse: a future change to the event type must not turn old
//! rows into false alarms.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
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
pub const DEFAULT_CHECKPOINT_BATCH_ROWS: usize = 500;
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
/// How many nights in a row the verification may end without a verdict (an error, or a run that found something and then could
/// not finish) before the admins are told that it is not completing.
pub const DEFAULT_ALERT_AFTER_FAILED_RUNS: u32 = 2;

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
    /// Two or more rows of `audit_events` have the same `seq` (the primary key was dropped). The log hands out each position once,
    /// so one of them was written by hand. Reported at that position, once.
    EventPositionDuplicated,
    /// The same for `audit_checkpoints`.
    CheckpointPositionDuplicated,
    /// The database refused a query of the verifier as not fitting its tables, or returned a column of another type than the one it
    /// reads: a column was dropped, renamed or retyped. Nothing transient does that, and it blinds the verifier, so it is a
    /// finding as soon as it happens and not an error that waits for the next night. Carries the SQLSTATE (the class of the
    /// refusal) and no text of the database's message, which can quote a value (S10). Reported at position 0.
    SchemaChanged(Option<SqlState>),
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
            Self::EventPositionDuplicated => "two events have the same position",
            Self::CheckpointPositionDuplicated => "two checkpoints have the same position",
            Self::SchemaChanged(_) => "the audit tables no longer have the shape the verifier reads",
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
            | Self::EventPositionMissing
            | Self::EventPositionDuplicated
            | Self::SchemaChanged(_) => 0,
            Self::CheckpointHeadMissing
            | Self::CheckpointHeadMismatch
            | Self::CheckpointNameInvalid
            | Self::BucketObjectMissing
            | Self::BucketObjectMismatch
            | Self::SignatureInvalid
            | Self::CheckpointColumnsInvalid(_)
            | Self::CheckpointPositionMissing
            | Self::CheckpointPositionDuplicated => 1,
            Self::ColumnsDiffer(_) => 2,
            Self::BucketUnavailable => 3,
        }
    }

    /// The names of the columns the failure is about (events or checkpoint), for the log and the alert. Names, never values (S10).
    pub fn detail(self) -> Option<String> {
        match self {
            Self::ColumnsDiffer(set) => Some(set.to_string()),
            Self::CheckpointColumnsInvalid(set) => Some(set.to_string()),
            Self::SchemaChanged(Some(code)) => Some(format!("SQLSTATE {code}")),
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
            Self::EventPositionDuplicated | Self::CheckpointPositionDuplicated => 3,
            _ => 0,
        }
    }

    /// Whether the failure is about a place in the log. One about the tables themselves is not.
    pub const fn has_position(self) -> bool {
        !matches!(self, Self::SchemaChanged(_))
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

/// A SQLSTATE: the five characters with which a server names the class of an error. A code is structure, never a value, so it may
/// be logged and put in an alert (S10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlState([u8; 5]);

impl SqlState {
    /// `code` if it is a SQLSTATE (five ASCII digits or upper-case letters).
    pub fn parse(code: &str) -> Option<Self> {
        let bytes: [u8; 5] = code.as_bytes().try_into().ok()?;
        bytes
            .iter()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
            .then_some(Self(bytes))
    }

    pub fn as_str(&self) -> &str {
        // Only ASCII digits and capitals get in (see `parse`).
        std::str::from_utf8(&self.0).unwrap_or("?????")
    }
}

impl std::fmt::Display for SqlState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
    /// The database refused a query as not fitting its tables, or returned a column of another type than the verifier reads (a
    /// column dropped, renamed or retyped; SQLSTATE class 42). No transient fault looks like this: the tables are not what the
    /// verifier was written for, and the run is reported as a finding at once (see [`FailureKind::SchemaChanged`]).
    #[error("the audit tables do not have the shape the verifier reads")]
    SchemaChanged(Option<SqlState>),
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

/// Which kind of failure a failed query is.
///
/// - A server that cancelled a query for its time (`statement_timeout`, SQLSTATE 57014) or for a lock it could not get in time
///   (`lock_timeout`, 55P03) is a [`VerifyError::Timeout`].
/// - A query the server refused as not fitting its tables is [`VerifyError::SchemaChanged`]: SQLSTATE class 42, "syntax error or
///   access rule violation", of which an undefined column (42703), an undefined function or operator (42883), a datatype mismatch
///   (42804), an undefined table (42P01) and a coercion that cannot be made (42846) are the ones a dropped, renamed or retyped
///   column causes. The verifier's queries are fixed text, so class 42 is never the fault of a value; it is the database not
///   being the one the text was written for (or a privilege taken away, 42501). So are the errors of reading a column that came
///   back with a type other than the one it is read as (`ColumnNotFound`, `ColumnDecode` of a column that holds no copy: the
///   position and the flags). Connection, resource (53), operator-intervention (57P) and transaction (40) errors are not class 42
///   and stay [`VerifyError::Database`].
/// - Anything else is a [`VerifyError::Database`].
///
/// Only the SQLSTATE is kept, never the server's message, which can quote a value.
fn db_error(stage: &'static str, e: &sqlx::Error) -> VerifyError {
    let code = e
        .as_database_error()
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned));
    tracing::warn!(
        stage,
        sqlstate = code.as_deref().unwrap_or("none"),
        "audit verification could not read a table"
    );
    classify(e, code.as_deref())
}

fn classify(e: &sqlx::Error, code: Option<&str>) -> VerifyError {
    match code {
        Some("57014" | "55P03") => VerifyError::Timeout,
        Some(c) if c.starts_with("42") => VerifyError::SchemaChanged(SqlState::parse(c)),
        Some(_) => VerifyError::Database,
        None => match e {
            sqlx::Error::ColumnNotFound(_)
            | sqlx::Error::ColumnIndexOutOfBounds { .. }
            | sqlx::Error::ColumnDecode { .. }
            | sqlx::Error::Decode(_)
            | sqlx::Error::TypeNotFound { .. } => VerifyError::SchemaChanged(None),
            _ => VerifyError::Database,
        },
    }
}

/// The query that finds a row without a `seq`. With the primary key in place it is an index probe that finds nothing at once; only
/// when the key was dropped does it read the table.
const EVENTS_WITHOUT_SEQ: &str = "SELECT EXISTS (SELECT 1 FROM audit_events WHERE seq IS NULL)";
const CHECKPOINTS_WITHOUT_SEQ: &str = "SELECT EXISTS (SELECT 1 FROM audit_checkpoints WHERE seq IS NULL)";

/// The longest `gcs_object` a checkpoint can have: [`OBJECT_PREFIX`], a `u64` in decimal, `.json` (see [`object_name`]).
const GCS_OBJECT_LIMIT: usize = OBJECT_PREFIX.len() + 20 + ".json".len();

/// Where a batch of the chain or of the checkpoints starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// At the smallest `seq` there is, whatever it is: no `WHERE`, so that a row at the smallest `seq` an `i64` has is read too
    /// (`seq > i64::MIN` would skip it).
    First,
    /// At the last `seq` the batch before read, that one included (`>=`): the rows that share it are all read again, in one
    /// batch, whichever side of the edge they were on. The ones already compared are skipped (see [`verify_chain`]).
    At,
    /// After it (`>`): only when a whole batch was rows at one position, so that `>=` would ask for the same rows again.
    After,
}

impl Resume {
    /// The `WHERE` of a batch, over the column `seq` (qualified as `column`), and the placeholder of the limit.
    fn clauses(self, column: &str) -> (String, &'static str) {
        match self {
            Self::First => (String::new(), "$1"),
            Self::At => (format!("WHERE {column} >= $1 "), "$2"),
            Self::After => (format!("WHERE {column} > $1 "), "$2"),
        }
    }
}

/// The text of the query that reads one batch of the chain: the first batch (`keyset` false, `$1` the row limit) and the later ones
/// (`$1` the `seq` of the last row read, which is read again, and `$2` the row limit). Public for the tests that look at its plan
/// and at what it sends.
///
/// Every column that can be large is bounded in the select (see [`super::columns`]); a value over its bound comes back `NULL`,
/// which for the three columns of the chain means a row the chain cannot vouch for.
#[doc(hidden)]
pub fn chain_batch_sql(keyset: bool) -> String {
    chain_sql(if keyset { Resume::At } else { Resume::First })
}

/// The batch that starts strictly after `$1` (limit `$2`).
fn chain_batch_after_sql() -> String {
    chain_sql(Resume::After)
}

fn chain_sql(resume: Resume) -> String {
    let (filter, limit) = resume.clauses("seq");
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
///
/// The hash of the event at the checkpoint's position is looked up, not joined: with two events at a position (see
/// [`FailureKind::EventPositionDuplicated`]) a join would make two rows of the one checkpoint, and the verifier would report a
/// checkpoint twice that exists once. The lookup prefers the event whose hash is the one the checkpoint signed.
#[doc(hidden)]
pub fn checkpoint_batch_sql(keyset: bool) -> String {
    checkpoint_sql(if keyset { Resume::At } else { Resume::First })
}

fn checkpoint_batch_after_sql() -> String {
    checkpoint_sql(Resume::After)
}

fn checkpoint_sql(resume: Resume) -> String {
    let (filter, limit) = resume.clauses("c.seq");
    format!(
        "SELECT c.seq, \
         CASE WHEN octet_length(c.head_hash) <= {HASH_COLUMN_BYTES} THEN c.head_hash END AS head_hash, \
         CASE WHEN octet_length(c.kms_key_version) <= {} THEN c.kms_key_version END AS kms_key_version, \
         CASE WHEN octet_length(c.signature) <= {MAX_SIGNATURE_BYTES} THEN c.signature END AS signature, \
         CASE WHEN octet_length(c.gcs_object) <= {GCS_OBJECT_LIMIT} THEN c.gcs_object END AS gcs_object, \
         j.chain_hash AS chain_hash \
         FROM audit_checkpoints c LEFT JOIN LATERAL ( \
           SELECT CASE WHEN octet_length(e.hash) <= {HASH_COLUMN_BYTES} THEN e.hash END AS chain_hash \
           FROM audit_events e WHERE e.seq = c.seq ORDER BY (e.hash = c.head_hash) DESC NULLS LAST LIMIT 1) j ON true \
         {filter}ORDER BY c.seq LIMIT {limit}",
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

/// What the walk of the chain carries from one row to the next, across chunks and batches.
#[derive(Debug, Clone, Copy)]
struct ChainState {
    /// The hash the next row must link to.
    expected_prev: ContentHash,
    /// The position of the last row compared. Rows come back in order of position, so a row whose position is not after it has
    /// the position of the one before.
    last_seq: Option<i64>,
    /// The position last reported as repeated: three rows at one position are one finding, and a batch that starts by reading
    /// again the rows it already compared does not report it twice.
    repeat_reported: Option<i64>,
}

impl ChainState {
    fn new() -> Self {
        Self {
            expected_prev: genesis_hash(),
            last_seq: None,
            repeat_reported: None,
        }
    }

    /// Whether `seq` is a position the walk has already been at, and has not said so yet. Notes it.
    fn repeats(&mut self, seq: i64) -> bool {
        let repeated = self.last_seq.is_some_and(|last| seq <= last) && self.repeat_reported != Some(seq);
        if repeated {
            self.repeat_reported = Some(seq);
        }
        self.last_seq = Some(seq);
        repeated
    }
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
    state: ChainState,
}

/// Compare a chunk of rows with the chain: each row's link and hash, and, for a row whose hash is good, its copied columns. CPU
/// work (SHA-256 over up to a megabyte per row, JSON parsing of the same), so it runs in `spawn_blocking`.
fn compare_rows(rows: &[sqlx::postgres::PgRow], mut state: ChainState) -> Compared {
    let mut out = Compared {
        checked: 0,
        findings: Vec::new(),
        stop: false,
        fault: None,
        state,
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
        // The log hands out each position once. A second row at one (the primary key was dropped) is reported, and compared like
        // any other: it links to the chain or it does not.
        if state.repeats(seq) {
            out.findings.push((seq, FailureKind::EventPositionDuplicated));
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
                check_row(&state.expected_prev, &prev, &hash, json.as_bytes()).map(|()| (hash, json))
            }
            _ => Err(FailureKind::ChainHashMismatch),
        };
        match checked {
            Ok((hash, json)) => {
                state.expected_prev = hash;
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
    out.state = state;
    out
}

/// What a keyset walk carries from one batch to the next: where the next batch starts, and how many of its first rows the batch
/// before already compared (they share the position it starts at, which it reads again).
#[derive(Debug, Default)]
struct Cursor {
    /// The position of the last row read; `None` until one is.
    after: Option<i64>,
    reread: usize,
    /// The next batch starts strictly after `after`: a whole batch was rows at one position.
    strict: bool,
}

/// What the end of a batch means for the walk.
enum Advance {
    Done,
    Continue,
    /// A whole batch was rows at this one position: it is used more than once, and the next batch starts after it.
    Repeated(i64),
}

impl Cursor {
    fn skip(&self) -> usize {
        if self.strict { 0 } else { self.reread }
    }

    /// Decide where the next batch starts, after a batch of `rows` rows (of at most `limit`) of which `seen` is the account.
    fn advance(&mut self, seen: &BatchSeen, rows: usize, limit: usize, no_seq: bool) -> Advance {
        // Rows with no `seq` sort last: nothing is after one. And a short batch is the end of the table.
        if no_seq || rows < limit {
            return Advance::Done;
        }
        if seen.last == self.after {
            // A whole batch of rows at one position, nothing new to continue from by `>=`. (A batch holds at least two rows, so
            // this is a position used twice.)
            let Some(position) = seen.last else {
                return Advance::Done;
            };
            self.strict = true;
            self.reread = 0;
            return Advance::Repeated(position);
        }
        self.after = seen.last;
        self.strict = false;
        self.reread = seen.tail;
        Advance::Continue
    }
}

/// What one batch has read, as far as positions go.
#[derive(Debug)]
struct BatchSeen {
    resumed_from: Option<i64>,
    skip: usize,
    /// The position of the last row read.
    last: Option<i64>,
    /// How many rows at that position the batch has read (those it skipped included).
    tail: usize,
}

impl BatchSeen {
    fn new(cursor: &Cursor) -> Self {
        Self {
            resumed_from: cursor.after,
            skip: cursor.skip(),
            last: cursor.after,
            tail: 0,
        }
    }

    /// Note a row at `seq`. `true` when it is one the batch before already compared, and is to be skipped.
    fn read(&mut self, seq: i64) -> bool {
        self.tail = if self.last == Some(seq) { self.tail + 1 } else { 1 };
        self.last = Some(seq);
        if self.skip > 0 && self.resumed_from == Some(seq) {
            self.skip -= 1;
            return true;
        }
        false
    }
}

/// The three texts of a keyset walk's query: the first batch, one that starts at the last position read, and one after it.
struct BatchSql {
    first: String,
    at: String,
    after: String,
}

impl BatchSql {
    fn of(&self, cursor: &Cursor) -> &str {
        match (cursor.after, cursor.strict) {
            (None, _) => &self.first,
            (Some(_), false) => &self.at,
            (Some(_), true) => &self.after,
        }
    }
}

/// What reading and comparing one batch of the chain came to.
struct ChainBatch {
    seen: BatchSeen,
    rows: usize,
    saw_no_seq: bool,
    /// A row with a bad link or hash was found: the walk ends.
    stop: bool,
}

pub struct ChainVerifier {
    pool: PgPool,
    bucket: Arc<dyn CheckpointBucket>,
    verifier: Arc<dyn CheckpointVerifier>,
    batch_rows: usize,
    checkpoint_batch_rows: usize,
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
            checkpoint_batch_rows: DEFAULT_CHECKPOINT_BATCH_ROWS,
            statement_timeout: DEFAULT_STATEMENT_TIMEOUT,
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// Rows per keyset batch of the chain (at least two: a batch of one row could never show that a position is used twice).
    #[must_use]
    pub fn with_batch_rows(mut self, rows: usize) -> Self {
        self.batch_rows = rows.max(2);
        self
    }

    /// Rows per keyset batch of the checkpoints (at least two, for the same reason).
    #[must_use]
    pub fn with_checkpoint_batch_rows(mut self, rows: usize) -> Self {
        self.checkpoint_batch_rows = rows.max(2);
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
        // A query refused for the shape of the tables is an error that ends the run and also a finding: it is not transient, and a
        // verifier that cannot read the tables verifies nothing. (Once per distinct refusal: both halves can be refused alike.)
        for error in [&chain, &checkpoints]
            .into_iter()
            .filter_map(|r| r.as_ref().err())
        {
            if let VerifyError::SchemaChanged(code) = error {
                let kind = FailureKind::SchemaChanged(*code);
                if !report.failures.iter().any(|f| f.kind == kind) {
                    report.fail(0, kind);
                }
            }
        }
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
        state: &mut ChainState,
        report: &mut VerifyReport,
    ) -> Result<bool, VerifyError> {
        if chunk.is_empty() {
            return Ok(false);
        }
        let rows = std::mem::take(chunk);
        let before = *state;
        let compared = tokio::task::spawn_blocking(move || compare_rows(&rows, before))
            .await
            .map_err(|_| {
                tracing::warn!(stage = "chain", "audit verification comparison did not finish");
                VerifyError::Database
            })?;
        *state = compared.state;
        report.events_checked += compared.checked;
        for (seq, kind) in compared.findings {
            report.fail(seq, kind);
        }
        match compared.fault {
            Some(fault) => Err(fault),
            None => Ok(compared.stop),
        }
    }

    /// Walk the chain by `seq`, a batch at a time.
    ///
    /// A batch starts *at* the last `seq` the one before read, not after it, so that all the rows that share a position are in one
    /// batch: with `seq > $last` a second row at the position that ended a batch was never read. The rows of that position that the
    /// batch before had compared (`reread`: how many it read at its last position) come first, and are skipped, not compared twice.
    /// If a whole batch is rows at one position `>=` would ask for the same rows again, so the position is reported and the next
    /// batch starts after it (`strict`); a position held by more rows than a batch is therefore compared only as far as a batch
    /// reaches into it, which the report shows as a repeated position and, as the chain then does not link, a broken link.
    async fn verify_chain(&self, report: &mut VerifyReport) -> Result<(), VerifyError> {
        // A row with no `seq` is never returned by a keyset read; look for it once.
        if self.any_without_seq("chain", EVENTS_WITHOUT_SEQ).await? {
            report.fail(0, FailureKind::EventPositionMissing);
        }
        let sql = BatchSql {
            first: chain_batch_sql(false),
            at: chain_batch_sql(true),
            after: chain_batch_after_sql(),
        };
        let mut cursor = Cursor::default();
        let mut state = ChainState::new();
        loop {
            let batch = self.chain_batch(&cursor, &sql, &mut state, report).await?;
            if batch.rows > 0 || batch.stop {
                report.batches += 1;
            }
            if batch.stop {
                // Every later row would fail too.
                return Ok(());
            }
            match cursor.advance(&batch.seen, batch.rows, self.batch_rows, batch.saw_no_seq) {
                Advance::Done => return Ok(()),
                Advance::Continue => {}
                Advance::Repeated(position) => {
                    // Said so, if the comparison has not.
                    if state.repeat_reported != Some(position) {
                        report.fail(position, FailureKind::EventPositionDuplicated);
                        state.repeat_reported = Some(position);
                    }
                }
            }
            tokio::task::yield_now().await;
        }
    }

    /// Read one batch of the chain in a transaction of its own and compare it, a chunk at a time.
    async fn chain_batch(
        &self,
        cursor: &Cursor,
        sql: &BatchSql,
        state: &mut ChainState,
        report: &mut VerifyReport,
    ) -> Result<ChainBatch, VerifyError> {
        let limit = i64::try_from(self.batch_rows).unwrap_or(i64::MAX);
        let mut tx = self.begin("chain").await?;
        let mut query = sqlx::query(sql.of(cursor));
        if let Some(last) = cursor.after {
            query = query.bind(last);
        }
        let mut rows = query.bind(limit).fetch(&mut *tx);
        let mut seen = BatchSeen::new(cursor);
        let mut in_batch = 0_usize;
        let mut saw_no_seq = false;
        let mut chunk = Vec::new();
        let mut chunk_bytes = 0_usize;
        let mut fault = None;
        let mut stop = false;
        // The rows are read one at a time and compared in chunks that are bounded in count and in bytes, so the memory of a batch
        // is that of a chunk, whatever the rows hold.
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
                // The rows of the position this batch starts at that the batch before compared.
                Ok(Some(seq)) if seen.read(seq) => continue,
                Ok(Some(_)) => {}
                Ok(None) => saw_no_seq = true,
                Err(e) => {
                    fault = Some(db_error("chain", &e));
                    break;
                }
            }
            chunk_bytes += row_bytes(&row);
            chunk.push(row);
            if chunk.len() >= CHUNK_ROWS || chunk_bytes >= CHUNK_BYTES {
                chunk_bytes = 0;
                stop = self.compare_chunk(&mut chunk, state, report).await?;
                if stop {
                    break;
                }
            }
        }
        // What was read before a fault is still compared: its findings are evidence.
        let last = if stop {
            Ok(true)
        } else {
            self.compare_chunk(&mut chunk, state, report).await
        };
        drop(rows);
        if let Some(fault) = fault {
            return Err(fault);
        }
        let stop = last?;
        if !stop {
            self.step("chain", tx.commit()).await?;
        }
        Ok(ChainBatch {
            seen,
            rows: in_batch,
            saw_no_seq,
            stop,
        })
    }

    async fn verify_checkpoints(&self, report: &mut VerifyReport) -> Result<(), VerifyError> {
        if self
            .any_without_seq("checkpoints", CHECKPOINTS_WITHOUT_SEQ)
            .await?
        {
            report.fail(0, FailureKind::CheckpointPositionMissing);
        }
        let limit = i64::try_from(self.checkpoint_batch_rows).unwrap_or(i64::MAX);
        let sql = BatchSql {
            first: checkpoint_batch_sql(false),
            at: checkpoint_batch_sql(true),
            after: checkpoint_batch_after_sql(),
        };
        // As for the chain: a batch starts at the last position read, rereads the rows it already checked, and skips them.
        let mut cursor = Cursor::default();
        let mut last_seq: Option<i64> = None;
        let mut repeat_reported: Option<i64> = None;
        loop {
            let mut tx = self.begin("checkpoints").await?;
            let mut query = sqlx::query(sql.of(&cursor));
            if let Some(last) = cursor.after {
                query = query.bind(last);
            }
            // At most `checkpoint_batch_rows` rows, and each is bounded by the select.
            let rows = self
                .step("checkpoints", query.bind(limit).fetch_all(&mut *tx))
                .await?;
            self.step("checkpoints", tx.commit()).await?;
            let n = rows.len();
            let mut seen = BatchSeen::new(&cursor);
            let mut saw_no_seq = false;
            for row in rows {
                let seq = row
                    .try_get::<Option<i64>, _>("seq")
                    .map_err(|e| db_error("checkpoints", &e))?;
                // A row with no position cannot be checked; `any_without_seq` has reported that there is one. (The first batch
                // has no `WHERE`, so it can return such a row, last.)
                let Some(seq) = seq else {
                    saw_no_seq = true;
                    continue;
                };
                if seen.read(seq) {
                    continue;
                }
                // The log hands out each position once, checkpoints included.
                if last_seq.is_some_and(|last| seq <= last) && repeat_reported != Some(seq) {
                    report.fail(seq, FailureKind::CheckpointPositionDuplicated);
                    repeat_reported = Some(seq);
                }
                last_seq = Some(seq);
                self.check_checkpoint_row(&row, seq, report).await?;
            }
            match cursor.advance(&seen, n, self.checkpoint_batch_rows, saw_no_seq) {
                Advance::Done => return Ok(()),
                Advance::Continue => {}
                Advance::Repeated(position) => {
                    if repeat_reported != Some(position) {
                        report.fail(position, FailureKind::CheckpointPositionDuplicated);
                        repeat_reported = Some(position);
                    }
                }
            }
        }
    }

    /// Check one row of the checkpoints: its columns can be read, and the checkpoint holds.
    async fn check_checkpoint_row(
        &self,
        row: &sqlx::postgres::PgRow,
        seq: i64,
        report: &mut VerifyReport,
    ) -> Result<(), VerifyError> {
        // A column that is `NULL`, over its bound, or unreadable is a finding about this checkpoint, which cannot be checked; the
        // next one is.
        let head = stored_column::<Vec<u8>>(row, "head_hash", "checkpoints")?;
        let key_version = stored_column::<String>(row, "kms_key_version", "checkpoints")?;
        let signature = stored_column::<Vec<u8>>(row, "signature", "checkpoints")?;
        let object = stored_column::<String>(row, "gcs_object", "checkpoints")?;
        let chain = stored_column::<Vec<u8>>(row, "chain_hash", "checkpoints")?;
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
            return Ok(());
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
        Ok(())
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
    let base = if worst.kind.has_position() {
        format!(
            "{count}{more} problem(s) in the audit log. Most severe: {} at position {}",
            worst.kind.as_str(),
            worst.seq
        )
    } else {
        format!(
            "{count}{more} problem(s) in the audit log. Most severe: {}",
            worst.kind.as_str()
        )
    };
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

/// The line of the alert that the verification is not completing: how many nights, and the class of the last error. The class is
/// the kind of the error, and the SQLSTATE for a refusal of the tables' shape: never a text of the database's, which can quote a
/// value (S10).
fn unfinished_summary(nights: u32, error: Option<VerifyError>) -> String {
    let class = match error {
        Some(VerifyError::Database) => "the audit tables could not be read".to_owned(),
        Some(VerifyError::Timeout) => "the audit tables did not answer in time".to_owned(),
        Some(VerifyError::SchemaChanged(Some(code))) => {
            format!("the audit tables changed shape, SQLSTATE {code}")
        }
        Some(VerifyError::SchemaChanged(None)) => "the audit tables changed shape".to_owned(),
        None => "unknown".to_owned(),
    };
    let plural = if nights == 1 { "night" } else { "nights" };
    format!("Audit log verification could not complete {nights} {plural} in a row (last error: {class}).")
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
    alert_after: u32,
    /// Runs in a row, by this job, that ended without a verdict. In memory (see the module docs).
    unfinished_in_a_row: AtomicU32,
    lease_renew_every: Duration,
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
            alert_after: DEFAULT_ALERT_AFTER_FAILED_RUNS,
            unfinished_in_a_row: AtomicU32::new(0),
            lease_renew_every: LEASE_TTL / 3,
        }
    }

    /// Tell the admins that the verification is not completing once it has ended without a verdict `runs` nights in a row (the
    /// default is [`DEFAULT_ALERT_AFTER_FAILED_RUNS`]). 0 and 1 are the same: every unfinished night alerts.
    #[must_use]
    pub fn with_alert_after(mut self, runs: u32) -> Self {
        self.alert_after = runs;
        self
    }

    /// How often the lease is renewed while a run goes on. For tests that need a run to lose its lease without waiting for it.
    #[doc(hidden)]
    #[must_use]
    pub fn with_lease_renew_every(mut self, every: Duration) -> Self {
        self.lease_renew_every = every.max(Duration::from_millis(1));
        self
    }

    /// Verify now if this replica holds the lease. The lease is not released afterwards: it runs out, so the other replicas, which
    /// wake at the same time, find it held and skip, and the admins hear of a mismatch once.
    ///
    /// The run is also counted: a run that ends without a verdict ([`NightlyOutcome::Errored`] or
    /// [`NightlyOutcome::Incomplete`]) is one more night in a row on which the audit log was not verified, and from the
    /// [`NightlyVerifyJob::with_alert_after`]-th in a row the admins are told so, every night until a run completes. A run that
    /// ran to the end ([`NightlyOutcome::Clean`] or [`NightlyOutcome::Failed`]) ends the count. A run that did not happen
    /// ([`NightlyOutcome::NotLeader`]) or was abandoned because the lease was lost ([`NightlyOutcome::LeaseLost`]) neither counts
    /// nor ends it: another replica decides that night, and nothing was learned about the database.
    pub async fn run_once(&self) -> Result<NightlyOutcome, LeaseError> {
        let (outcome, error) = self.run_verification().await?;
        self.count(&outcome, error).await;
        Ok(outcome)
    }

    async fn run_verification(&self) -> Result<(NightlyOutcome, Option<VerifyError>), LeaseError> {
        let Some(mut lease) = self.leases.try_acquire(LEASE_NAME, LEASE_TTL).await? else {
            return Ok((NightlyOutcome::NotLeader, None));
        };
        let verify = self.verifier.verify();
        tokio::pin!(verify);
        let mut renew = tokio::time::interval(self.lease_renew_every);
        renew.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        renew.tick().await;
        let result = loop {
            tokio::select! {
                result = &mut verify => break result,
                _ = renew.tick() => {
                    if lease.renew(LEASE_TTL).await.is_err() {
                        tracing::error!("the audit verification lease was lost; the run is abandoned");
                        return Ok((NightlyOutcome::LeaseLost, None));
                    }
                }
            }
        };
        let (report, aborted) = match result {
            Ok(report) => (report, None),
            Err(VerifyAborted { error, partial }) => {
                self.metrics.verify_errored();
                if matches!(error, VerifyError::SchemaChanged(_)) {
                    self.metrics.verify_schema_changed();
                }
                tracing::error!(
                    error = %error,
                    found = partial.failures.len(),
                    events = partial.events_checked,
                    checkpoints = partial.checkpoints_checked,
                    "audit chain verification could not run to the end"
                );
                if partial.is_clean() {
                    return Ok((NightlyOutcome::Errored, Some(error)));
                }
                // What it found before the fault is not thrown away with it.
                self.metrics
                    .verify_found(u64::try_from(partial.failures.len()).unwrap_or(u64::MAX));
                (partial, Some(error))
            }
        };
        if aborted.is_none() {
            self.metrics
                .verify_ran(u64::try_from(report.failures.len()).unwrap_or(u64::MAX));
            if report.is_clean() {
                tracing::info!(
                    events = report.events_checked,
                    checkpoints = report.checkpoints_checked,
                    "audit chain verified"
                );
                return Ok((NightlyOutcome::Clean(report), None));
            }
        }
        for failure in &report.failures {
            // Column names and a SQLSTATE are structure, not content; the values of the columns are never read into a message.
            let detail = failure.kind.detail().unwrap_or_default();
            tracing::error!(
                seq = failure.seq,
                reason = failure.kind.as_str(),
                detail = detail.as_str(),
                "audit chain verification failed"
            );
        }
        self.alert(&report, aborted.is_some()).await;
        Ok(if aborted.is_some() {
            (NightlyOutcome::Incomplete(report), aborted)
        } else {
            (NightlyOutcome::Failed(report), None)
        })
    }

    /// Keep the count of nights in a row without a verdict, and tell the admins once it reaches the threshold. The count is in
    /// memory, on this job: see the module docs for what that costs.
    async fn count(&self, outcome: &NightlyOutcome, error: Option<VerifyError>) {
        let runs = match outcome {
            NightlyOutcome::NotLeader | NightlyOutcome::LeaseLost => return,
            NightlyOutcome::Clean(_) | NightlyOutcome::Failed(_) => 0,
            NightlyOutcome::Errored | NightlyOutcome::Incomplete(_) => {
                match self
                    .unfinished_in_a_row
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_add(1)))
                {
                    Ok(before) | Err(before) => before.saturating_add(1),
                }
            }
        };
        if runs == 0 {
            self.unfinished_in_a_row.store(0, Ordering::SeqCst);
        }
        self.metrics.set_verify_unfinished_in_a_row(u64::from(runs));
        if runs > 0 && runs >= self.alert_after {
            tracing::error!(
                nights = runs,
                "audit chain verification has not completed for several nights in a row"
            );
            self.alert_unfinished(runs, error).await;
        }
    }

    /// Tell the admins that the verification is not completing. A notification problem is logged and goes no further.
    async fn alert_unfinished(&self, nights: u32, error: Option<VerifyError>) {
        let (Ok(title), Ok(summary)) = (
            ShortText::parse("Audit log verification is not completing"),
            ShortText::parse(&unfinished_summary(nights, error)),
        ) else {
            tracing::error!("audit alert could not be built");
            return;
        };
        self.notify(Notification {
            kind: NotificationKind::CriticalFinding,
            severity: Severity::Critical,
            swimlane: None,
            title,
            summary,
            link: None,
        })
        .await;
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
        self.notify(Notification {
            kind: NotificationKind::CriticalFinding,
            severity: Severity::Critical,
            swimlane: None,
            title,
            summary,
            link: None,
        })
        .await;
    }

    async fn notify(&self, note: Notification) {
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
        let (first, next, after) = (
            chain_batch_sql(false),
            chain_batch_sql(true),
            chain_batch_after_sql(),
        );
        assert!(
            !first.contains("WHERE") && first.ends_with("ORDER BY seq LIMIT $1"),
            "{first}"
        );
        // A batch starts AT the last position the one before read, so that rows sharing a position are all in one batch ...
        assert!(next.contains("WHERE seq >= $1 ORDER BY seq LIMIT $2"), "{next}");
        // ... and AFTER it only when a whole batch was one position.
        assert!(after.contains("WHERE seq > $1 ORDER BY seq LIMIT $2"), "{after}");
        // The three columns of the chain are bounded; `event_json` by the size the log enforces.
        for guard in [
            "CASE WHEN octet_length(prev_hash) <= 32 THEN prev_hash END AS prev_hash",
            "CASE WHEN octet_length(hash) <= 32 THEN hash END AS hash",
            "CASE WHEN octet_length(event_json) <= 1048576 THEN event_json END AS event_json",
        ] {
            assert!(first.contains(guard), "{guard}");
        }
        let (first, next, after) = (
            checkpoint_batch_sql(false),
            checkpoint_batch_sql(true),
            checkpoint_batch_after_sql(),
        );
        // (The lookup of the event at the position has a `WHERE` of its own, on `e.seq`.)
        assert!(
            !first.contains("c.seq >") && first.ends_with("ORDER BY c.seq LIMIT $1"),
            "{first}"
        );
        assert!(
            next.contains("WHERE c.seq >= $1 ORDER BY c.seq LIMIT $2"),
            "{next}"
        );
        assert!(
            after.contains("WHERE c.seq > $1 ORDER BY c.seq LIMIT $2"),
            "{after}"
        );
        // The event at the checkpoint's position is looked up (one row), not joined (one per event there), and prefers the event
        // whose hash the checkpoint signed.
        assert!(
            first.contains("LEFT JOIN LATERAL (")
                && first
                    .contains("WHERE e.seq = c.seq ORDER BY (e.hash = c.head_hash) DESC NULLS LAST LIMIT 1"),
            "{first}"
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

    /// The type after every `::` in a SQL text of the verifier (a cast, or a type on a literal): a name, and its arguments if it has
    /// any.
    fn casts_in(sql: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = sql;
        while let Some(at) = rest.find("::") {
            let after = &rest[at + 2..];
            let mut end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if after[end..].starts_with('(') {
                end += after[end..]
                    .find(')')
                    .map_or(after.len() - end, |close| close + 1);
            }
            out.push(after[..end].to_owned());
            rest = &after[end..];
        }
        out
    }

    #[test]
    fn nothing_the_verifier_selects_is_cast_or_rendered_except_what_is_vetted_first() {
        // Every cast in the verifier's queries is one that cannot fail or grow on a stored value. A new cast that can would be a way
        // for one row to end every run (round 4: `col::text` on a `jsonb`), so each one is listed here with the reason.
        let texts = [
            chain_batch_sql(false),
            chain_batch_sql(true),
            chain_batch_after_sql(),
            checkpoint_batch_sql(false),
            checkpoint_batch_sql(true),
            checkpoint_batch_after_sql(),
            EVENTS_WITHOUT_SEQ.to_owned(),
            CHECKPOINTS_WITHOUT_SEQ.to_owned(),
            set_local_sql(Duration::from_secs(1), Duration::from_secs(1)),
        ];
        let mut seen = std::collections::BTreeMap::<String, usize>::new();
        for text in &texts {
            for cast in casts_in(text) {
                *seen.entry(cast).or_default() += 1;
            }
        }
        // The first chain batch holds each once; `jsonb` is the sentinel (a literal) and `text` the last check of each `jsonb`
        // column (twice, one for each column), after the checks above it; `numeric(40,0)` and `text` are the time, selected only
        // when `isfinite(at)`, whose epoch in microseconds is at most 20 digits; `int4` is the mask of flags (a sum of small
        // constants). Nothing else is cast: the text, `bytea` and hash columns are measured with `octet_length`, which reads a
        // length from the header, and `approval_id` and `seq` are selected as they are.
        let in_one_text = |cast: &str| {
            casts_in(&chain_batch_sql(false))
                .iter()
                .filter(|c| c.as_str() == cast)
                .count()
        };
        assert_eq!(
            in_one_text("jsonb"),
            2 * 5,
            "a sentinel for each of the five rejections, in each of the two columns"
        );
        assert_eq!(
            in_one_text("text"),
            1 + 2,
            "the time, and the last check of each jsonb"
        );
        assert_eq!(in_one_text("numeric(40,0)"), 1);
        assert_eq!(in_one_text("int4"), 1);
        let allowed = ["jsonb", "text", "numeric(40,0)", "int4"];
        for cast in seen.keys() {
            assert!(
                allowed.contains(&cast.as_str()),
                "a new cast in a query of the verifier: ::{cast}"
            );
        }
        // The checkpoint query casts nothing at all, and neither do the probes and the settings.
        for text in [
            checkpoint_batch_sql(false),
            checkpoint_batch_sql(true),
            checkpoint_batch_after_sql(),
            EVENTS_WITHOUT_SEQ.to_owned(),
            CHECKPOINTS_WITHOUT_SEQ.to_owned(),
            set_local_sql(Duration::from_secs(1), Duration::from_secs(1)),
        ] {
            assert_eq!(casts_in(&text), Vec::<String>::new(), "{text}");
        }
        // And every `::text` of a `jsonb` comes after the checks that make it safe, in the one `CASE` of its column.
        let list = chain_batch_sql(false);
        for column in ["attribution_actor", "attribution_evidence"] {
            let render = list.find(&format!("{column}::text")).unwrap();
            for check in [
                format!("pg_column_size({column}) >"),
                format!("pg_column_size({column} #> '{{}}') >"),
                format!("jsonb_path_exists({column},"),
                format!("jsonb_array_length(jsonb_path_query_array({column},"),
            ] {
                let at = list.find(&check).unwrap_or_else(|| panic!("{check} is missing"));
                assert!(at < render, "{check} must come before the column is printed");
            }
        }
    }

    #[test]
    fn the_longest_object_name_is_what_the_bound_allows() {
        // `object_name` of the largest position: the bound on `gcs_object` is exactly that long.
        assert_eq!(object_name(u64::MAX).len(), GCS_OBJECT_LIMIT);
        assert_eq!(MAX_SIGNATURE_BYTES, 128);
        assert_eq!(KeyRef::MAX_LEN, 512);
    }

    /// A database error with the given SQLSTATE and a message that quotes a value, as a server's message can.
    #[derive(Debug)]
    struct Refusal {
        code: &'static str,
        message: String,
    }

    impl std::fmt::Display for Refusal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for Refusal {}

    impl sqlx::error::DatabaseError for Refusal {
        fn message(&self) -> &str {
            &self.message
        }

        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(std::borrow::Cow::Borrowed(self.code))
        }

        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    fn refused_with(code: &'static str) -> VerifyError {
        db_error(
            "test",
            &sqlx::Error::Database(Box::new(Refusal {
                code,
                message: "column \"a-secret-value\" does not exist".to_owned(),
            })),
        )
    }

    #[test]
    fn a_refusal_for_the_shape_of_the_tables_is_told_from_a_fault_by_its_sqlstate() {
        // Class 42: the statement does not fit the tables. These are what a dropped, renamed or retyped column causes, and the
        // others of the class are the same kind of thing (the text of the statement is fixed, so it is never a value's fault).
        for code in [
            "42703", "42883", "42804", "42P01", "42846", "42809", "42501", "42601", "42704", "42P18", "42725",
        ] {
            assert_eq!(
                refused_with(code),
                VerifyError::SchemaChanged(SqlState::parse(code)),
                "{code}"
            );
        }
        // Time: the statement or the lock ran out.
        for code in ["57014", "55P03"] {
            assert_eq!(refused_with(code), VerifyError::Timeout, "{code}");
        }
        // Everything else is a fault of the database or the connection, and stays an error: a lost connection (08), resources (53),
        // the server going away (57P), a serialisation failure or deadlock (40), data exceptions (22), integrity (23), a missing
        // database (3D), a feature not supported (0A), an internal error (XX).
        for code in [
            "08006", "08001", "08003", "53100", "53200", "53300", "40001", "40P01", "57P01", "57P03",
            "25P02", "XX000", "22003", "23505", "0A000", "3D000", "54000", "54001",
        ] {
            assert_eq!(refused_with(code), VerifyError::Database, "{code}");
        }
    }

    #[test]
    fn a_column_that_comes_back_as_another_type_or_not_at_all_is_the_same_refusal_and_other_errors_are_not() {
        let source = || -> Box<dyn std::error::Error + Send + Sync> { "mismatched types".into() };
        for e in [
            sqlx::Error::ColumnNotFound("seq".to_owned()),
            sqlx::Error::ColumnIndexOutOfBounds { index: 3, len: 2 },
            sqlx::Error::ColumnDecode {
                index: "seq".to_owned(),
                source: source(),
            },
            sqlx::Error::Decode(source()),
            sqlx::Error::TypeNotFound {
                type_name: "x".to_owned(),
            },
        ] {
            assert_eq!(db_error("test", &e), VerifyError::SchemaChanged(None), "{e:?}");
        }
        for e in [
            sqlx::Error::PoolClosed,
            sqlx::Error::PoolTimedOut,
            sqlx::Error::WorkerCrashed,
            sqlx::Error::RowNotFound,
            sqlx::Error::Io(std::io::Error::other("reset")),
            sqlx::Error::Protocol("bad".to_owned()),
        ] {
            assert_eq!(db_error("test", &e), VerifyError::Database, "{e:?}");
        }
    }

    #[test]
    fn a_sqlstate_is_five_digits_or_capitals_and_the_server_message_is_never_kept() {
        assert_eq!(
            SqlState::parse("42P01").map(|c| c.to_string()),
            Some("42P01".to_owned())
        );
        for bad in [
            "",
            "4",
            "42P0",
            "42P011",
            "42p01",
            "42 01",
            "42-01",
            "42é1",
            "４２P01",
        ] {
            assert_eq!(SqlState::parse(bad), None, "{bad:?}");
        }
        // The error carries the code and nothing of the message.
        let e = refused_with("42703");
        assert!(!format!("{e:?}{e}").contains("a-secret-value"));
        let kind = FailureKind::SchemaChanged(SqlState::parse("42703"));
        assert!(!format!("{kind:?}{}", kind.as_str()).contains("a-secret-value"));
    }

    #[test]
    fn the_alert_for_a_changed_shape_names_the_sqlstate_and_no_position() {
        let worst = VerifyFailure {
            seq: 0,
            kind: FailureKind::SchemaChanged(SqlState::parse("42883")),
        };
        let line = alert_summary(1, "", &worst, true);
        assert_eq!(
            line,
            "1 problem(s) in the audit log. Most severe: the audit tables no longer have the shape the verifier reads (SQLSTATE 42883). The run did not finish."
        );
        assert!(ShortText::parse(&line).is_ok());
        let none = VerifyFailure {
            seq: 0,
            kind: FailureKind::SchemaChanged(None),
        };
        assert_eq!(
            alert_summary(2, "+", &none, true),
            "2+ problem(s) in the audit log. Most severe: the audit tables no longer have the shape the verifier reads. The run did not finish."
        );
        assert_eq!(worst.kind.severity(), 0);
        assert!(!worst.kind.has_position());
        assert!(FailureKind::ChainHashMismatch.has_position());
    }

    #[test]
    fn the_alert_that_the_verification_is_not_completing_names_the_class_of_the_error_and_nothing_else() {
        assert_eq!(
            unfinished_summary(2, Some(VerifyError::Timeout)),
            "Audit log verification could not complete 2 nights in a row (last error: the audit tables did not answer in time)."
        );
        assert_eq!(
            unfinished_summary(1, Some(VerifyError::Database)),
            "Audit log verification could not complete 1 night in a row (last error: the audit tables could not be read)."
        );
        assert_eq!(
            unfinished_summary(3, Some(VerifyError::SchemaChanged(SqlState::parse("42883")))),
            "Audit log verification could not complete 3 nights in a row (last error: the audit tables changed shape, SQLSTATE 42883)."
        );
        assert_eq!(
            unfinished_summary(3, Some(VerifyError::SchemaChanged(None))),
            "Audit log verification could not complete 3 nights in a row (last error: the audit tables changed shape)."
        );
        // However many nights, and whatever the class, the line is a valid `ShortText`, so the alert is always built.
        for error in [
            None,
            Some(VerifyError::Database),
            Some(VerifyError::Timeout),
            Some(VerifyError::SchemaChanged(SqlState::parse("42P01"))),
        ] {
            let line = unfinished_summary(u32::MAX, error);
            assert!(ShortText::parse(&line).is_ok(), "{} bytes: {line}", line.len());
        }
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
