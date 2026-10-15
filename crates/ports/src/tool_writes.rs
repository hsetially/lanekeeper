//! The tool-write registration port (D73): the write path tells the registry which hash it is about to put on NFS, so
//! that the next observation of that hash is attributed as a tool write with certainty, and not guessed from NFS
//! activity.
//!
//! Called by prompt 06 (`hub-writes`), implemented by prompt 05 (`hub-registry`). Like [`crate::AuditLog::record`], it
//! takes the caller's open transaction: the registration exists only if the transaction that carries the write's
//! audit event commits.
//!
//! # Ordering: the observation can come first
//!
//! A registration becomes visible when its transaction commits, and 06 commits after the file is written (the
//! `WriteFile` command has returned). So the agent can see the new file, and report it, before the registration is
//! visible. The registry (05) must therefore also match an observation that arrives before the commit becomes visible
//! (a late-committed registration): it keeps an observation it could not attribute for at least
//! [`ToolWriteEntry::TTL_SECS`] and attributes it as a tool write when a matching registration commits within that
//! time. Matching is by swimlane, path and hash.
//!
//! # A registration that nothing matches expires
//!
//! An unmatched registration is not kept forever, because it could be attributed to a write that was not the tool's and
//! because the queue is bounded:
//!
//! - the registry drops it [`ToolWriteEntry::TTL_SECS`] after the commit that made it visible;
//! - a newer observation of the same path with another hash invalidates it at once: that state replaced the one the
//!   registration was for, so a later write of the registered hash is not the tool's write;
//! - dropping a registration is not an error and is not audited; the write's own audit event already is.
//!
//! The port has no way to observe, so these rules are tested where the registry is implemented (05).

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};

use crate::Tx;

/// Why a registration could not be made. Carries no path and no hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ToolWriteError {
    /// The transaction belongs to another store.
    #[error("transaction is not backed by the expected store")]
    WrongTx,
    /// Too many registrations are waiting to be matched with an observation (every queue is bounded). It is the answer of
    /// `register`, never of the commit: a registration the registry accepted is not dropped for lack of room, and a
    /// repeat of one it already holds is a no-op, not `Full`.
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

impl ToolWriteEntry {
    /// How long a registration waits for its observation, and how long an observation waits for its registration: 15
    /// minutes. Long enough for the agent to report a written file and for a commit that comes late; short enough that a
    /// stale entry cannot be attributed to a later write or fill the bounded queue.
    pub const TTL_SECS: u64 = 15 * 60;
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
