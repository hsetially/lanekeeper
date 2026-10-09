//! Write-path DTOs. There is no blind write: every request states what it expects to find (rule 11).

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{
    AuditId, ContentHash, DraftId, IdempotencyKey, NfsPath, ProposalId, ProposalStatus, RequestId,
    ServiceRef, ShortText, SwimlaneId, Timestamp, User, UserId, Via,
    compare::{DiffHunk, SettingChange},
};

/// What the caller expects to find on NFS before the write. A mismatch is a conflict and writes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Expected {
    /// The file must not exist (a create).
    Absent,
    /// The file must have exactly this hash.
    Hash { hash: ContentHash },
}

impl Expected {
    /// True when `current` (the hash found on NFS, `None` if absent) is what the caller expected.
    pub fn matches(&self, current: Option<&ContentHash>) -> bool {
        match (self, current) {
            (Self::Absent, None) => true,
            (Self::Hash { hash }, Some(c)) => hash == c,
            _ => false,
        }
    }
}

/// Context every write carries: who, through which door, and the keys that make it retry-safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCtx {
    pub user: User,
    pub via: Via,
    pub idempotency_key: IdempotencyKey,
    pub request_id: RequestId,
}

/// Replace or create a file. `content` is the full new content, line endings already restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub content: Bytes,
}

/// Upload a (possibly binary) file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub content: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    /// The hash being deleted. Deleting needs a hash; there is no "delete whatever is there".
    pub expected: ContentHash,
}

/// Restore an earlier version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub to: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartRequest {
    pub swimlane: SwimlaneId,
    pub service: ServiceRef,
}

/// Raise a PR for NFS-only changes (D77). At most 100 paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRequest {
    pub swimlane: SwimlaneId,
    pub paths: Vec<NfsPath>,
    pub title: ShortText,
    pub body: Option<String>,
}

impl PrRequest {
    pub const MAX_PATHS: usize = 100;
}

/// Why a write was refused without being attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// The file is denied (D79).
    Denied,
    TooLarge,
    /// Structured content that does not parse.
    InvalidContent,
    UnknownSwimlane,
    AgentUnavailable,
    /// The request is not allowed for this user or state.
    NotAllowed,
}

/// The result of a write (`docs/interfaces.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WriteOutcome {
    /// Done. `hash` is the new content hash (absent for a delete or restart).
    Applied {
        hash: Option<ContentHash>,
        audit_id: AuditId,
    },
    /// The user requires approval: a proposal was created instead of writing.
    ProposalCreated {
        id: ProposalId,
    },
    /// The expected state did not match. Nothing was written. `current` is what is there now.
    Conflict {
        current: Option<ContentHash>,
    },
    /// A sync window or another lock is open.
    Locked {
        until: Option<Timestamp>,
    },
    Rejected {
        reason: RejectReason,
    },
}

impl WriteOutcome {
    /// True when the request did what was asked, or queued it for approval.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Applied { .. } | Self::ProposalCreated { .. })
    }
}

/// A prepared change with no side effects (MCP `propose_change`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub id: DraftId,
    pub author: UserId,
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub new_hash: ContentHash,
    pub hunks: Vec<DiffHunk>,
    pub setting_changes: Vec<SettingChange>,
    pub status: ProposalStatus,
    pub expires_at: Timestamp,
}
