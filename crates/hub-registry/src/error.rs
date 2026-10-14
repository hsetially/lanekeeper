//! The errors of the registry (rule 3, S21).
//!
//! An error names what failed and carries nothing else: no SQL, no path, no file content, no driver text. The cause goes to the
//! log as a fixed word (see [`crate::db::Db`]), with the ids the caller already has. At the edge the closed [`ports::SinkError`]
//! set (or, from T6, the closed problem set of the REST edge) is all a caller ever sees.

use ports::{AuditError, BlobError, BusError, GitError, SinkError, TxError, WrongTxKind};

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
}

impl From<AuditError> for RegistryError {
    fn from(e: AuditError) -> Self {
        Self::Audit(e)
    }
}

impl From<BusError> for RegistryError {
    fn from(e: BusError) -> Self {
        match e {
            BusError::WrongTx => Self::WrongTx,
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
            | RegistryError::WrongTx => Self::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            RegistryError::WrongTx,
        ] {
            assert_eq!(SinkError::from(internal), SinkError::Unavailable, "{internal}");
        }
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
            RegistryError::WrongTx,
        ] {
            let text = e.to_string();
            assert!(text.len() < 80 && !text.contains("SELECT"), "{text}");
        }
    }
}
