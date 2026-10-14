//! The errors of the registry (rule 3, S21).
//!
//! An error names what failed and carries nothing else: no SQL, no path, no file content, no driver text. The cause goes to the
//! log as a fixed word (see [`crate::db::Db`]), with the ids the caller already has. At the edge the closed [`ports::SinkError`]
//! set (or, from T6, the closed problem set of the REST edge) is all a caller ever sees.

use ports::{AuditError, BlobError, BusError, GitError, ReadError, SinkError, TxError, WrongTxKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The database refused or could not be reached.
    #[error("the database is unavailable")]
    Database,
    /// A database call did not finish in time.
    #[error("the database did not answer in time")]
    Timeout,
    /// A stored value is not one this code wrote (a hash of the wrong length, a state that is not in the vocabulary).
    #[error("a stored value is not valid")]
    Corrupt,
    #[error("the audit log is unavailable")]
    Audit(AuditError),
    #[error("the event bus is unavailable")]
    Bus(BusError),
    #[error("the blob store is unavailable")]
    Blob(BlobError),
    /// The mirror could not answer (T2): a commit it does not have, or a repository it cannot read.
    #[error("the Git mirror could not answer")]
    Git(GitError),
    /// A transaction of another store was handed to a Postgres implementation: always a wiring bug.
    #[error("the transaction is not backed by Postgres")]
    WrongTx,
    /// A bound of the registry itself is reached (the number of live streams), so the request is refused rather than queued.
    #[error("the registry is at capacity")]
    Capacity,
    /// A payload is well formed but not acceptable.
    #[error("the payload is not valid")]
    Invalid,
    #[error("the payload is too large")]
    TooLarge,
    /// Nothing is known under that name (an unknown swimlane).
    #[error("not found")]
    NotFound,
    /// The stream has not said `hello` yet.
    #[error("hello is required first")]
    HelloRequired,
    /// The payload names a swimlane other than the sender's own.
    #[error("the payload does not match the sender identity")]
    IdentityMismatch,
    /// An invariant of the registry itself did not hold: a bug here, never something a caller did. Two causes: a batch of audit or
    /// outbox events that a port refused as too large after the registry had cut it to the port's limits itself, and a task on the
    /// blocking pool that did not finish. The error says nothing more; the stage that failed is in the log (see
    /// [`RegistryError::batch_bug`]).
    #[error("an internal error")]
    Internal,
}

impl RegistryError {
    /// A port refused a batch for size, which the registry's own splitting should make impossible (`ports::split_batches` for the
    /// audit chain, `MAX_EVENT_BATCH` for the outbox). It is a bug in the registry and not the caller's doing, so it is
    /// [`RegistryError::Internal`], and the `stage` goes to the log as a fixed word, with nothing of the batch.
    fn batch_bug(stage: &'static str) -> Self {
        tracing::error!(
            stage,
            "a batch the registry cut is over a port limit: a bug in the registry"
        );
        Self::Internal
    }

    /// A task on the blocking pool did not finish (it panicked, or the runtime is shutting down). Nothing of its input is known here,
    /// so the error is [`RegistryError::Internal`] and the `stage` is the one word that goes to the log.
    pub(crate) fn off_runtime(stage: &'static str) -> Self {
        tracing::error!(stage, "a task on the blocking pool did not finish");
        Self::Internal
    }
}

impl From<AuditError> for RegistryError {
    fn from(e: AuditError) -> Self {
        match e {
            AuditError::BatchTooLarge => Self::batch_bug("audit_batch"),
            AuditError::TooLarge | AuditError::WrongTx | AuditError::Encode | AuditError::Unavailable => {
                Self::Audit(e)
            }
        }
    }
}

impl From<BusError> for RegistryError {
    fn from(e: BusError) -> Self {
        match e {
            BusError::WrongTx => Self::WrongTx,
            BusError::BatchTooLarge => Self::batch_bug("event_batch"),
            BusError::Unavailable => Self::Bus(e),
        }
    }
}

impl From<BlobError> for RegistryError {
    fn from(e: BlobError) -> Self {
        match e {
            BlobError::TooLarge => Self::TooLarge,
            BlobError::TooMany | BlobError::Full | BlobError::Unavailable => Self::Blob(e),
        }
    }
}

impl From<GitError> for RegistryError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

impl From<WrongTxKind> for RegistryError {
    fn from(_: WrongTxKind) -> Self {
        Self::WrongTx
    }
}

impl From<TxError> for RegistryError {
    fn from(_: TxError) -> Self {
        Self::Database
    }
}

impl From<RegistryError> for SinkError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::Invalid => Self::Invalid,
            RegistryError::TooLarge => Self::TooLarge,
            RegistryError::NotFound | RegistryError::IdentityMismatch => Self::IdentityMismatch,
            RegistryError::HelloRequired => Self::HelloRequired,
            RegistryError::Database
            | RegistryError::Timeout
            | RegistryError::Corrupt
            | RegistryError::Audit(_)
            | RegistryError::Bus(_)
            | RegistryError::Blob(_)
            | RegistryError::Git(_)
            | RegistryError::Capacity
            | RegistryError::Internal
            | RegistryError::WrongTx => Self::Unavailable,
        }
    }
}

/// What a reader sees is the closed [`ReadError`] set (T6): a missing thing, a request that is not valid, or "try later". The cause
/// of the last was logged by whoever failed, as a fixed word.
impl From<RegistryError> for ReadError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::NotFound => Self::NotFound,
            RegistryError::Invalid | RegistryError::TooLarge => Self::BadRequest,
            RegistryError::Database
            | RegistryError::Timeout
            | RegistryError::Corrupt
            | RegistryError::Audit(_)
            | RegistryError::Bus(_)
            | RegistryError::Blob(_)
            | RegistryError::Git(_)
            | RegistryError::WrongTx
            | RegistryError::Capacity
            | RegistryError::HelloRequired
            | RegistryError::IdentityMismatch
            | RegistryError::Internal => Self::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_reader_sees_is_the_closed_read_set() {
        assert_eq!(ReadError::from(RegistryError::NotFound), ReadError::NotFound);
        assert_eq!(ReadError::from(RegistryError::Invalid), ReadError::BadRequest);
        assert_eq!(ReadError::from(RegistryError::TooLarge), ReadError::BadRequest);
        for internal in [
            RegistryError::Database,
            RegistryError::Timeout,
            RegistryError::Corrupt,
            RegistryError::Blob(BlobError::Unavailable),
            RegistryError::Git(GitError::Unavailable),
            RegistryError::Internal,
            RegistryError::Capacity,
        ] {
            assert_eq!(ReadError::from(internal), ReadError::Unavailable, "{internal}");
        }
    }

    #[test]
    fn what_a_caller_sees_is_the_closed_sink_set() {
        assert_eq!(SinkError::from(RegistryError::Invalid), SinkError::Invalid);
        assert_eq!(SinkError::from(RegistryError::TooLarge), SinkError::TooLarge);
        assert_eq!(
            SinkError::from(RegistryError::NotFound),
            SinkError::IdentityMismatch
        );
        assert_eq!(
            SinkError::from(RegistryError::IdentityMismatch),
            SinkError::IdentityMismatch
        );
        assert_eq!(
            SinkError::from(RegistryError::HelloRequired),
            SinkError::HelloRequired
        );
        for internal in [
            RegistryError::Database,
            RegistryError::Timeout,
            RegistryError::Corrupt,
            RegistryError::Audit(AuditError::Unavailable),
            RegistryError::Bus(BusError::Unavailable),
            RegistryError::Blob(BlobError::Unavailable),
            RegistryError::Git(GitError::Unavailable),
            RegistryError::Capacity,
            RegistryError::Internal,
            RegistryError::WrongTx,
        ] {
            assert_eq!(SinkError::from(internal), SinkError::Unavailable, "{internal}");
        }
    }

    #[test]
    fn a_batch_the_registry_cut_too_big_is_an_internal_error_and_not_the_callers() {
        // The registry splits its batches itself, so a port that refuses one for size has found a bug in the registry. It is neither a
        // client error (`Invalid`, `TooLarge`: the agent would drop the delta) nor a port that is down (`Audit`, `Bus`).
        assert_eq!(
            RegistryError::from(AuditError::BatchTooLarge),
            RegistryError::Internal
        );
        assert_eq!(
            RegistryError::from(BusError::BatchTooLarge),
            RegistryError::Internal
        );
        // What the agent is told is the closed set, and a bug is "try again later", with nothing of the batch.
        assert_eq!(
            SinkError::from(RegistryError::from(AuditError::BatchTooLarge)),
            SinkError::Unavailable
        );
        assert_eq!(
            SinkError::from(RegistryError::from(BusError::BatchTooLarge)),
            SinkError::Unavailable
        );
    }

    #[test]
    fn the_other_port_errors_keep_their_meaning() {
        for e in [
            AuditError::TooLarge,
            AuditError::WrongTx,
            AuditError::Encode,
            AuditError::Unavailable,
        ] {
            assert_eq!(RegistryError::from(e), RegistryError::Audit(e));
        }
        assert_eq!(RegistryError::from(BusError::WrongTx), RegistryError::WrongTx);
        assert_eq!(
            RegistryError::from(BusError::Unavailable),
            RegistryError::Bus(BusError::Unavailable)
        );
    }

    #[test]
    fn an_oversize_blob_is_too_large_and_other_blob_failures_are_not() {
        assert_eq!(RegistryError::from(BlobError::TooLarge), RegistryError::TooLarge);
        assert_eq!(
            RegistryError::from(BlobError::Full),
            RegistryError::Blob(BlobError::Full)
        );
    }

    #[test]
    fn no_error_text_carries_input() {
        for e in [
            RegistryError::Database,
            RegistryError::Timeout,
            RegistryError::Corrupt,
            RegistryError::Invalid,
            RegistryError::TooLarge,
            RegistryError::NotFound,
            RegistryError::Git(GitError::NotFound),
            RegistryError::Capacity,
            RegistryError::Internal,
            RegistryError::WrongTx,
        ] {
            let text = e.to_string();
            assert!(text.len() < 80 && !text.contains("SELECT"), "{text}");
        }
    }
}
