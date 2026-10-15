//! The write service port (implemented by 06, used by REST and MCP).
//!
//! There is no blind write: every request states the hash it expects (rule 11). A mismatch writes nothing
//! and answers [`WriteOutcome::Conflict`]. Retrying a request with the same idempotency key returns the
//! first outcome and writes nothing more (D66).

use async_trait::async_trait;
use domain::{
    ChangePreview, DeleteRequest, Draft, DraftId, EditRequest, NotifyRequest, PrRequest, PreviewRequest,
    RestartRequest, RevertRequest, UploadRequest, WriteCtx, WriteOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    /// The user's role is too low, or the user is pending or disabled (S4).
    #[error("not allowed")]
    Forbidden,
    /// A draft that does not exist or is not the caller's, or (for `preview_effect`) a swimlane that does not exist.
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

/// Role floors: `edit`, `upload`, `delete`, `revert`, `raise_pr`, `propose_draft`, `apply_draft` and
/// `preview_effect` need Editor; `restart` and `notify` need Operator. Users with `requires_approval` get
/// [`WriteOutcome::ProposalCreated`] instead of a write, except for `propose_draft` and `preview_effect`, which
/// change nothing.
///
/// `upload` and `delete` need a valid preview (D46): the `preview` of the request must be one that `preview_effect`
/// issued to the same user, less than [`ChangePreview::TTL_SECS`] ago, for the same swimlane and path and the same
/// expected state: for an `upload`, `expected` (absent, or a hash) and the content; for a `delete`, the hash. A token
/// presented with any of those changed is as good as no token. Without a valid one the outcome is
/// [`domain::RejectReason::NotAllowed`] (never a `Conflict`) and nothing is written; a refused attempt does not use the
/// token up. Both doors need it: REST and MCP (07 calls `preview_effect` to build its confirmation text).
///
/// Content (an edit, a draft, an upload, a preview) is at most 2 MiB, the engine's `Limits::input_bytes`
/// (`docs/performance.md`), or less when the agent's `max_file_bytes` is lower.
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
    /// What an upload or a delete would do, from the engine, and the token that lets that upload or delete
    /// through. It writes nothing to NFS and records no audit event, so it is not idempotency-keyed; the only state it
    /// may keep is the issued token, until it runs out. `effective_changed` is judged on the content as it would be
    /// written (after the file's line endings are restored) against the file as it is now, so a delete of a file that
    /// is not there is no change. A value the secret scan flags is masked in `changes` ([`domain::PreviewedChange`]).
    /// Content over the size limit is refused.
    async fn preview_effect(&self, ctx: &WriteCtx, r: PreviewRequest) -> Result<ChangePreview, WriteError>;
    /// Tells the config-server that these files changed (D86). Audited, and Operator only, because its endpoint
    /// has no authentication (Q37). For a user who `requires_approval` it becomes a proposal. Not to be confused with
    /// [`crate::Notifier::notify`], the port that sends Teams and other notifications; the two share a verb and
    /// nothing else.
    async fn notify(&self, ctx: &WriteCtx, r: NotifyRequest) -> Result<WriteOutcome, WriteError>;

    /// A draft as it is now, for its author only: the MCP `apply_change` shows the person what they are about to apply
    /// before they confirm. Editor, like [`WriteService::propose_draft`] and [`WriteService::apply_draft`]. No side effects
    /// and no audit event (reading is not audited), and it is not idempotency-keyed. [`WriteError::NotFound`] for a draft
    /// that does not exist or is not the caller's; a draft that was applied or has expired is still returned, with its
    /// status. `severity` and `effects` are the engine's, as of the call.
    ///
    /// Additive (`contract/07-mcp`, G7): the default answers [`WriteError::Unavailable`], and the conformance suite
    /// ([`crate::conformance::write_service`]) fails an implementation that keeps it. A wrapper of `WriteService` must
    /// forward it.
    async fn draft(&self, _ctx: &WriteCtx, _id: DraftId) -> Result<Draft, WriteError> {
        Err(WriteError::Unavailable)
    }
}
