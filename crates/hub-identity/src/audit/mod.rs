//! The tamper-evident audit log (T5; S9, D38, D56).
//!
//! - [`log`]: [`PgAuditLog`], the `AuditLog` port over the caller's transaction. An advisory lock, the head of the chain, the hash
//!   `SHA-256(prev_hash || canonical JSON)`, the insert. Call `record` last in a transaction.
//! - [`checkpoint`]: every hour, the head is signed by KMS ([`signature`]) and the signed checkpoint goes to a locked bucket
//!   ([`bucket`]) and to `audit_checkpoints`.
//! - [`verify`]: every night, the whole chain is recomputed from the stored `event_json` and checked against every checkpoint,
//!   its bucket object and its signature, and holds the columns that copy each event ([`columns`]) against the stored text. A
//!   mismatch alerts the admins, raises a metric and logs the position (and, for columns, their names).
//! - [`shipper`]: committed events are copied, hash included, to Cloud Logging as one structured line each.
//!
//! Appending is in the caller's transaction; the jobs run on one replica at a time under leases ([`ports::Leases`]).

use std::future::Future;
use std::time::Duration;

use crate::session::AbortOnDrop;

pub mod bucket;
pub mod checkpoint;
pub mod columns;
pub mod log;
pub mod metadata;
pub mod shipper;
pub mod signature;
pub mod verify;

pub use bucket::{
    AccessTokenSource, BucketError, CheckpointBucket, GcsCheckpointBucket, MemoryBucket, PutOutcome,
};
pub use checkpoint::{CheckpointDeps, CheckpointError, CheckpointJob, CheckpointObject, CheckpointOutcome};
pub use columns::{Column, ColumnSet};
pub use log::{CHAIN_LOCK_KEY, PgAuditLog};
pub use metadata::MetadataTokenSource;
pub use shipper::{AuditLogShipper, ShipOutcome};
pub use signature::{CheckpointVerifier, EcdsaP256Verifier, checkpoint_digest, checkpoint_message};
pub use verify::{
    ChainVerifier, CheckpointColumn, CheckpointColumnSet, FailureKind, NightlyOutcome, NightlyVerifyJob,
    VerifyAborted, VerifyError, VerifyFailure, VerifyReport,
};

/// A background job that stops when its owner drops the handle.
#[derive(Debug)]
pub struct JobHandle {
    _task: AbortOnDrop,
}

impl JobHandle {
    pub(crate) fn spawn(task: impl Future<Output = ()> + Send + 'static) -> Self {
        Self {
            _task: AbortOnDrop::spawn(task),
        }
    }
}

/// `every` plus up to a tenth of it, at random, so that replicas started together do not wake together.
pub(crate) fn jittered(every: Duration) -> Duration {
    let extra_ms = u64::try_from(every.as_millis() / 10).unwrap_or(0);
    every + Duration::from_millis(fastrand::u64(0..=extra_ms))
}
