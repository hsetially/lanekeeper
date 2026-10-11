//! Nightly verification of the audit chain (S9).
//!
//! [`ChainVerifier`] recomputes the whole chain from the stored `event_json` and checks every checkpoint:
//!
//! - each row's `prev_hash` is the previous row's `hash` (the first row's is the genesis hash), and `hash` is
//!   `SHA-256(prev_hash || event_json)` computed with [`ports::chain_hash`]. Gaps in `seq` are expected (a rolled-back
//!   transaction uses a number) and are not looked at: the chain is followed by hashes, not by numbers;
//! - for every row of `audit_checkpoints`: the chain has an event at that `seq` with that hash, the bucket holds the object for
//!   it and the object equals the row, and the signature verifies with the KMS public key.
//!
//! The chain is read by keyset in batches (`seq > $last ORDER BY seq LIMIT 5000`) and streamed row by row, so memory use does
//! not depend on its length. The first bad row ends the walk of the chain, because every row after a bad one fails too; the
//! report names that one position. Checkpoints are checked one by one, each reporting its first problem, up to a bound.
//!
//! What it cannot see, by design: an event removed *and* its checkpoint rows removed, or a tail cut off after the newest
//! checkpoint, leave a chain that is internally consistent. The bucket's retention keeps the objects of the removed checkpoints,
//! but this job starts from the rows in the table. The newest unchecked stretch is at most an hour.
//!
//! The row hash is recomputed from the stored text, not from a re-parse: a future change to the event type must not turn old
//! rows into false alarms.

use std::sync::Arc;
use std::time::Duration;

use domain::{ContentHash, Severity, ShortText};
use futures::TryStreamExt;
use ports::{LeaseError, Leases, Notification, NotificationKind, Notifier, chain_hash, genesis_hash};
use sqlx::{PgPool, Row};

use super::JobHandle;
use super::bucket::{BucketError, CheckpointBucket, object_name};
use super::checkpoint::CheckpointObject;
use super::signature::{CheckpointVerifier, checkpoint_message};
use crate::metrics::IdentityMetrics;

/// Rows read per keyset batch.
pub const DEFAULT_BATCH_ROWS: usize = 5_000;
/// Checkpoint rows read per batch.
const CHECKPOINT_BATCH: i64 = 500;
/// The most failures a report lists; the rest are counted by `truncated`.
const MAX_FAILURES: usize = 100;
/// The lease of the nightly job, and how long it lasts between renewals while a long run is going.
pub const LEASE_NAME: &str = "audit.verify";
const LEASE_TTL: Duration = Duration::from_secs(3600);
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
    /// Events whose link and hash were checked, up to and including the first bad one.
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

    fn fail(&mut self, seq: i64, kind: FailureKind) {
        if self.failures.len() < MAX_FAILURES {
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

fn db_error(stage: &'static str, e: &sqlx::Error) -> VerifyError {
    let code = e
        .as_database_error()
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned));
    tracing::warn!(
        stage,
        sqlstate = code.as_deref().unwrap_or("none"),
        "audit verification could not read a table"
    );
    VerifyError::Database
}

pub struct ChainVerifier {
    pool: PgPool,
    bucket: Arc<dyn CheckpointBucket>,
    verifier: Arc<dyn CheckpointVerifier>,
    batch_rows: usize,
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
        }
    }

    /// Rows per keyset batch (at least one).
    #[must_use]
    pub fn with_batch_rows(mut self, rows: usize) -> Self {
        self.batch_rows = rows.max(1);
        self
    }

    /// Verify the chain and every checkpoint. A mismatch is a finding in the report; an error means the tables could not be read.
    pub async fn verify(&self) -> Result<VerifyReport, VerifyError> {
        let mut report = VerifyReport::default();
        self.verify_chain(&mut report).await?;
        self.verify_checkpoints(&mut report).await?;
        Ok(report)
    }

    async fn verify_chain(&self, report: &mut VerifyReport) -> Result<(), VerifyError> {
        let limit = i64::try_from(self.batch_rows).unwrap_or(i64::MAX);
        let mut after = 0_i64;
        let mut expected_prev = genesis_hash();
        loop {
            let mut rows = sqlx::query(
                "SELECT seq, prev_hash, hash, event_json FROM audit_events WHERE seq > $1 ORDER BY seq LIMIT $2",
            )
            .bind(after)
            .bind(limit)
            .fetch(&self.pool);
            let mut in_batch = 0_usize;
            while let Some(row) = rows.try_next().await.map_err(|e| db_error("chain", &e))? {
                in_batch += 1;
                let seq: i64 = row.try_get("seq").map_err(|e| db_error("chain", &e))?;
                let prev: Vec<u8> = row.try_get("prev_hash").map_err(|e| db_error("chain", &e))?;
                let hash: Vec<u8> = row.try_get("hash").map_err(|e| db_error("chain", &e))?;
                let json: String = row.try_get("event_json").map_err(|e| db_error("chain", &e))?;
                after = seq;
                report.events_checked += 1;
                let checked = match (hash_of(prev), hash_of(hash)) {
                    (Some(prev), Some(hash)) => {
                        check_row(&expected_prev, &prev, &hash, json.as_bytes()).map(|()| hash)
                    }
                    _ => Err(FailureKind::ChainHashMismatch),
                };
                match checked {
                    Ok(hash) => expected_prev = hash,
                    Err(kind) => {
                        // Every later row would fail too: name the first position and stop.
                        report.fail(seq, kind);
                        report.batches += 1;
                        return Ok(());
                    }
                }
            }
            drop(rows);
            if in_batch > 0 {
                report.batches += 1;
            }
            if in_batch < self.batch_rows {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    }

    async fn verify_checkpoints(&self, report: &mut VerifyReport) -> Result<(), VerifyError> {
        let mut after = 0_i64;
        loop {
            let rows = sqlx::query(
                "SELECT c.seq, c.head_hash, c.kms_key_version, c.signature, c.gcs_object, e.hash AS chain_hash \
                 FROM audit_checkpoints c LEFT JOIN audit_events e ON e.seq = c.seq \
                 WHERE c.seq > $1 ORDER BY c.seq LIMIT $2",
            )
            .bind(after)
            .bind(CHECKPOINT_BATCH)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| db_error("checkpoints", &e))?;
            let n = rows.len();
            for row in rows {
                let seq: i64 = row.try_get("seq").map_err(|e| db_error("checkpoints", &e))?;
                after = seq;
                let head: Vec<u8> = row
                    .try_get("head_hash")
                    .map_err(|e| db_error("checkpoints", &e))?;
                let key_version: String = row
                    .try_get("kms_key_version")
                    .map_err(|e| db_error("checkpoints", &e))?;
                let signature: Vec<u8> = row
                    .try_get("signature")
                    .map_err(|e| db_error("checkpoints", &e))?;
                let object: String = row
                    .try_get("gcs_object")
                    .map_err(|e| db_error("checkpoints", &e))?;
                let chain: Option<Vec<u8>> = row
                    .try_get("chain_hash")
                    .map_err(|e| db_error("checkpoints", &e))?;
                report.checkpoints_checked += 1;
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
            if i64::try_from(n).unwrap_or(i64::MAX) < CHECKPOINT_BATCH {
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
        let (Some(head), Ok(position)) = (head, u64::try_from(seq)) else {
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
        let stored = match self.bucket.get(object).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(FailureKind::BucketObjectMissing),
            Err(BucketError::Unavailable | BucketError::Rejected) => {
                return Err(FailureKind::BucketUnavailable);
            }
            Err(BucketError::TooLarge | BucketError::InvalidName) => {
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
        let report = match result {
            Ok(report) => report,
            Err(e) => {
                self.metrics.verify_errored();
                tracing::error!(error = %e, "audit chain verification could not run");
                return Ok(NightlyOutcome::Errored);
            }
        };
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
        for failure in &report.failures {
            tracing::error!(
                seq = failure.seq,
                reason = failure.kind.as_str(),
                "audit chain verification failed"
            );
        }
        self.alert(&report).await;
        Ok(NightlyOutcome::Failed(report))
    }

    /// Tell the admins. A notification problem is logged and goes no further.
    async fn alert(&self, report: &VerifyReport) {
        let Some(first) = report.failures.first() else {
            return;
        };
        let more = if report.truncated { "+" } else { "" };
        let summary = format!(
            "{}{more} problem(s) in the audit log. First: {} at position {}",
            report.failures.len(),
            first.kind.as_str(),
            first.seq
        );
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
