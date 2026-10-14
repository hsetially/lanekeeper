//! The tool-write registration port (D73): the write path tells the registry which hash it is about to put on NFS, so
//! that the next observation of that hash is attributed as a tool write with certainty, and not guessed from NFS
//! activity.
//!
//! Called by prompt 06 (`hub-writes`), implemented by prompt 05 (`hub-registry`). Like [`crate::AuditLog::record`], it
//! takes the caller's open transaction: the registration exists only if the transaction that carries the write's
//! audit event commits.

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};

use crate::Tx;

/// Why a registration could not be made. Carries no path and no hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ToolWriteError {
    /// The transaction belongs to another store.
    #[error("transaction is not backed by the expected store")]
    WrongTx,
    /// Too many registrations are waiting to be matched with an observation (every queue is bounded).
    #[error("too many tool writes are waiting to be observed")]
    Full,
    #[error("tool write registry is unavailable")]
    Unavailable,
}

/// A registration as stored: what the registry will match the next observation against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolWriteEntry {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub new_hash: ContentHash,
    pub user: UserId,
    pub request_id: RequestId,
}

#[async_trait]
pub trait ToolWrites: Send + Sync + 'static {
    /// Register that `user` wrote `new_hash` to `path` in `swimlane`, through request `request_id`, inside `tx`.
    ///
    /// - The registration exists only if `tx` commits; a rolled-back transaction leaves nothing behind.
    /// - Registering the same (swimlane, path, hash, request) again is a no-op, so a retry that completes an
    ///   interrupted write does not register twice.
    /// - After the commit, the registry attributes the next observation of this hash at this path as
    ///   `ObservationSource::ToolWrite`, with this user as the actor and confidence `certain` (D73).
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError>;
}
