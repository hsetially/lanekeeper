//! Proposals: a change by a user who `requires_approval` waits for an admin (D37). T1 creates them; T4 decides them.
//!
//! - **Approve** ([`approve`]): only an admin, never the author. The file is looked at again (the proposal holds the hash its
//!   author expected); if it has changed the proposal is `stale` and nothing is written, otherwise the change is written like
//!   any other write and the proposal is `applied`, in the transaction that records the audit event (which names both the
//!   author and the approver).
//! - **Reject** ([`approve::reject`]) and **expire** ([`expire`], a leased job): the other ways out of `pending`.
//! - **Every transition** publishes `ProposalChanged` in the transaction that makes it (D80), which is what moves the screens
//!   over SSE.
//! - **List**: a page of proposals, newest first, with the hunks and the settings an approver reads. Flagged values are masked
//!   ([`view`]).

use std::sync::Arc;

use async_trait::async_trait;
use domain::{
    ContentHash, Cursor, DiffHunk, Expected, NfsPath, Page, Paged, ProposalId, ProposalStatus, SettingChange,
    Severity, ShortText, SwimlaneId, Timestamp, User, UserId, WriteCtx, WriteOutcome,
};
use ports::WriteError;
use serde::Serialize;

use crate::service::WriteHub;

pub(crate) mod approve;
pub(crate) mod expire;
pub(crate) mod list;
pub(crate) mod store;
pub(crate) mod view;

pub use expire::{ExpireProposals, ExpireReport};

/// The hash a proposal shows when its action has no new content (a delete, a restart): all zeros, like the first link of the audit
/// chain. The `Proposal` schema of the API has no way to say "none".
pub const NO_HASH: ContentHash = ContentHash::from_bytes([0; 32]);

/// What a list of proposals is asked for.
#[derive(Debug, Clone)]
pub struct ProposalQuery {
    pub status: Option<ProposalStatus>,
    pub swimlane: Option<SwimlaneId>,
    pub page: Page,
}

/// A proposal as the API shows it (the `Proposal` schema of `api/openapi.yaml`).
///
/// The schema describes a proposal for the content of one file. For the others the fields say what they can: `path` is the first
/// path of the proposal (`namespace/name` for a restart), `expected` the hash its author expected there, and `new_hash` is
/// [`NO_HASH`] when the action writes no content. The `action` is not in the schema; see the report of T4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProposalView {
    pub id: i64,
    pub author: UserId,
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub new_hash: ContentHash,
    pub status: ProposalStatus,
    pub hunks: Vec<DiffHunk>,
    pub setting_changes: Vec<SettingChange>,
    pub severity: Option<Severity>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub decided_by: Option<UserId>,
    pub decision_comment: Option<ShortText>,
}

/// How a rejection ended.
#[derive(Debug, Clone, PartialEq)]
pub enum RejectResult {
    /// The proposal is rejected; this is how it reads now.
    Rejected(Box<ProposalView>),
    /// Nothing was changed: the proposal is not pending, a lease is held, and so on. The outcome says why.
    Refused(WriteOutcome),
}

/// The decisions on proposals and the list of them: what the proposal routes call. [`WriteHub`] implements it.
#[async_trait]
pub trait ProposalDesk: Send + Sync + 'static {
    /// A page of proposals. Viewer and above.
    async fn list_proposals(&self, user: &User, q: ProposalQuery) -> Result<Paged<ProposalView>, WriteError>;

    /// Approve a proposal: apply it if the file is as its author expected, mark it stale if not. Admin only, and never the
    /// author. The outcome is that of the write it carries out.
    async fn approve_proposal(
        &self,
        ctx: &WriteCtx,
        id: ProposalId,
        comment: Option<ShortText>,
    ) -> Result<WriteOutcome, WriteError>;

    /// Reject a proposal, with a reason. Admin only, and never the author.
    async fn reject_proposal(
        &self,
        ctx: &WriteCtx,
        id: ProposalId,
        reason: ShortText,
    ) -> Result<RejectResult, WriteError>;
}

#[async_trait]
impl ProposalDesk for WriteHub {
    async fn list_proposals(&self, user: &User, q: ProposalQuery) -> Result<Paged<ProposalView>, WriteError> {
        list::list(&self.inner, user, q).await
    }

    async fn approve_proposal(
        &self,
        ctx: &WriteCtx,
        id: ProposalId,
        comment: Option<ShortText>,
    ) -> Result<WriteOutcome, WriteError> {
        use tracing::Instrument;
        let span = tracing::info_span!(
            "write.approve",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            proposal = id.get(),
        );
        approve::approve(&self.inner, ctx, id, comment)
            .instrument(span)
            .await
    }

    async fn reject_proposal(
        &self,
        ctx: &WriteCtx,
        id: ProposalId,
        reason: ShortText,
    ) -> Result<RejectResult, WriteError> {
        use tracing::Instrument;
        let span = tracing::info_span!(
            "write.reject",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            proposal = id.get(),
        );
        approve::reject(&self.inner, ctx, id, reason)
            .instrument(span)
            .await
    }
}

impl WriteHub {
    /// The job that marks proposals (and drafts) whose time has run out. Run it under a lease (D63).
    pub fn expirer(&self) -> ExpireProposals {
        ExpireProposals::new(Arc::clone(&self.inner))
    }
}

/// The cursor of a page that goes on below proposal `id`.
pub(crate) fn cursor_below(id: i64) -> Option<Cursor> {
    Cursor::parse(&id.to_string()).ok()
}
