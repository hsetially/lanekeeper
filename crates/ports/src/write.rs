//! The write service port (implemented by 06, used by REST and MCP).
//!
//! There is no blind write: every request states the hash it expects (rule 11). A mismatch writes nothing
//! and answers [`WriteOutcome::Conflict`]. Retrying a request with the same idempotency key returns the
//! first outcome and writes nothing more (D66).

use async_trait::async_trait;
use domain::{
    DeleteRequest, Draft, DraftId, EditRequest, PrRequest, RestartRequest, RevertRequest, UploadRequest,
    WriteCtx, WriteOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    /// The user's role is too low, or the user is pending or disabled (S4).
    #[error("not allowed")]
    Forbidden,
    /// A draft that does not exist or is not the caller's.
    #[error("not found")]
    NotFound,
    #[error("request is not valid")]
    BadRequest,
    /// The idempotency key was already used for a different request.
    #[error("idempotency key was used for another request")]
    IdempotencyConflict,
    #[error("write service is unavailable")]
    Unavailable,
}

/// Role floors: `edit`, `upload`, `delete`, `revert`, `raise_pr`, `propose_draft` and `apply_draft` need
/// Editor; `restart` needs Operator. Users with `requires_approval` get [`WriteOutcome::ProposalCreated`]
/// instead of a write, except for `propose_draft`.
#[async_trait]
pub trait WriteService: Send + Sync + 'static {
    async fn edit(&self, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError>;
    async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError>;
    async fn delete(&self, ctx: &WriteCtx, r: DeleteRequest) -> Result<WriteOutcome, WriteError>;
    async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError>;
    async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError>;
    async fn raise_pr(&self, ctx: &WriteCtx, r: PrRequest) -> Result<WriteOutcome, WriteError>;
    /// Prepares a change with no side effects at all.
    async fn propose_draft(&self, ctx: &WriteCtx, r: EditRequest) -> Result<Draft, WriteError>;
    /// Applies a draft written by the same user, re-checking its expected hash.
    async fn apply_draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<WriteOutcome, WriteError>;
}
