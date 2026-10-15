//! Write-path DTOs. There is no blind write: every request states what it expects to find (rule 11).

use std::num::NonZeroU32;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{
    AppName, AuditId, ChangeEol, ContentHash, DraftId, IdempotencyKey, IssueKind, NfsPath, PreviewId,
    ProposalId, ProposalStatus, RequestId, ServiceRef, ShortText, SwimlaneId, TenantId, Timestamp, User,
    UserId, Via,
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

/// Replace or create a file. `content` is the full new content as the caller has it; the service restores the
/// file's own line endings, encoding and BOM before it writes (rule 11), unless `change_eol` asks for another style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub content: Bytes,
    /// Convert the file to this line ending style. `None` keeps the style the file has (a new file takes the
    /// dominant style of its folder). Only an explicit value ever changes a file's style.
    pub change_eol: Option<ChangeEol>,
}

/// Upload a (possibly binary) file, at most 2 MiB (the engine's `Limits::input_bytes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub content: Bytes,
    /// From `WriteService::preview_effect` for this user, swimlane, path, expected state and content. An upload without a
    /// valid one is refused ([`RejectReason::NotAllowed`]); it is an `Option` so that the refusal is an outcome
    /// and not a parse failure.
    pub preview: Option<PreviewId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    /// The hash being deleted. Deleting needs a hash; there is no "delete whatever is there".
    pub expected: ContentHash,
    /// From `WriteService::preview_effect` for this user, swimlane, path and hash. Same rule as for an upload, which a
    /// preview also binds to the content.
    pub preview: Option<PreviewId>,
}

/// Restore an earlier version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub expected: Expected,
    pub to: ContentHash,
    /// Convert the restored content to this line ending style. `None` writes the stored version as it was.
    pub change_eol: Option<ChangeEol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartRequest {
    pub swimlane: SwimlaneId,
    pub service: ServiceRef,
}

/// Where a PR goes (D43). The user chooses explicitly; nothing is guessed from the path alone when it is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrTarget {
    /// The branch of one tenant (its id equals the branch name, D84).
    Tenant { branch: TenantId },
    /// The base repository's default branch.
    Base,
}

/// Raise a PR for NFS-only changes (D77). At most 100 paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRequest {
    pub swimlane: SwimlaneId,
    pub paths: Vec<NfsPath>,
    pub title: ShortText,
    pub body: Option<String>,
    /// The branch the PR goes to. `None`: a tenant file goes to its own tenant's branch and a base file to the base
    /// branch.
    pub target: Option<PrTarget>,
}

impl PrRequest {
    pub const MAX_PATHS: usize = 100;
}

/// Tell the config-server that these files changed (its `/update-resources`, D86). Audited, Operator only (Q37).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyRequest {
    pub swimlane: SwimlaneId,
    pub paths: Vec<NfsPath>,
}

impl NotifyRequest {
    /// At most this many paths, like a PR.
    pub const MAX_PATHS: usize = 100;
}

/// What to preview: the upload or the delete that a later request will present the token for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewChange {
    Upload { expected: Expected, content: Bytes },
    Delete { expected: ContentHash },
}

/// Ask what an upload or a delete would do, before anybody confirms it (D46).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub change: PreviewChange,
}

/// Who reads the file a change touches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewScope {
    /// A file in the root folder: every service asks for it.
    EveryService,
    /// A file below the folder of an application.
    Application { app: AppName },
    /// A file below a folder whose name is not an application name.
    Folder { path: NfsPath },
}

/// A consequence the person confirming a change should see (D46, D83, D85). The wire names do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewWarning {
    /// A tenant file is deleted and the base file takes over.
    FallsBackToBase,
    /// The only file there is gets deleted: the service finds nothing.
    NoFileLeft,
    /// A new tenant file in `whole_file` mode: from now on it is a copy that falls behind (C1).
    ForksFromBase,
    /// The file's suffix is a tenant the swimlane does not deploy: nobody reads it.
    NeverRead { suffix: TenantId },
    /// A property source changes: a notification or a restart is needed (D85).
    NeedsNotifyOrRestart,
    /// The file is written into a folder that does not exist: the config-server restarts to see it (D83).
    NewFolderNeedsConfigServerRestart,
    /// `channels.yml` changes: the config-server restarts to read it (D83).
    ChannelsYmlNeedsConfigServerRestart,
}

/// One setting a preview says would differ. A value the secret scan flags is **masked**: the side of the change that
/// holds it has the marker `[REDACTED: N chars, rule]` as its `text` (the engine's `SecretMatch::marker`, the same text
/// the MCP tools use, S14b), and `flagged` is true. Nothing in a preview reveals a flagged value; revealing stays the one
/// audited action, `/files/reveal` (D79).
///
/// `Debug` prints the setting path, the kind and `flagged`, never either side's text: a value the scan did not flag may
/// still be a secret, and the text of a change does not belong in a log (S10), like [`ChangePreview`]'s.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewedChange {
    /// The change, with every flagged value replaced by its marker.
    pub change: SettingChange,
    /// True when the scan flagged `change.left` or `change.right`.
    pub flagged: bool,
}

impl std::fmt::Debug for PreviewedChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewedChange")
            .field("path", &self.change.path)
            .field("kind", &self.change.kind)
            .field("flagged", &self.flagged)
            .finish_non_exhaustive()
    }
}

/// The answer to a [`PreviewRequest`]: what the engine says the change does, and the token that later lets the
/// upload or delete through. Facts come from the engine (rule 7); this is their wire form.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePreview {
    pub preview_id: PreviewId,
    /// `issued + TTL_SECS`.
    pub expires_at: Timestamp,
    pub scope: PreviewScope,
    pub warnings: Vec<PreviewWarning>,
    /// The settings that would differ in the effective config, sorted by path, bounded by the engine. A flagged value
    /// is masked (see [`PreviewedChange`]); an unmasked secret in this list is a defect.
    pub changes: Vec<PreviewedChange>,
    /// All of them, listed or not.
    pub changes_total: u32,
    /// What the services get would differ.
    pub effective_changed: bool,
}

impl ChangePreview {
    /// How long a preview token stays valid (ten minutes).
    pub const TTL_SECS: u64 = 600;
}

impl std::fmt::Debug for ChangePreview {
    // A change carries values, and a value may be a secret: only the number of changes is printed (S10).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangePreview")
            .field("preview_id", &self.preview_id)
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .field("warnings", &self.warnings)
            .field("changes", &self.changes.len())
            .field("changes_total", &self.changes_total)
            .field("effective_changed", &self.effective_changed)
            .finish()
    }
}

/// One thing the validation of content found: where, and which rule. It never holds the text of the line, because the
/// line may be the secret (S10, S13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationIssue {
    pub kind: IssueKind,
    /// 1-based, when the issue has a place. Zero is not a line: it is refused when read and cannot be built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<NonZeroU32>,
    /// The secret rule's name for [`IssueKind::SecretMatch`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<ShortText>,
}

impl ValidationIssue {
    /// The most issues one outcome carries; the rest are dropped, the count in the audit event is not.
    pub const MAX_PER_OUTCOME: usize = 50;
}

/// Why a write was refused without being attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// The file is denied (D79).
    Denied,
    TooLarge,
    /// The content or request is invalid for a reason that has no name of its own (for example, a revert to a
    /// version that was never on NFS). Structured content that does not parse is [`RejectReason::ParseError`].
    InvalidContent,
    UnknownSwimlane,
    AgentUnavailable,
    /// The request is not allowed for this user or state, or an upload or delete came without a valid preview.
    NotAllowed,
    /// The value-based secret scan matched (S13). `Rejected::issues` has the lines and rules, never the text.
    SecretDetected,
    /// Structured content that does not parse. `Rejected::issues` has the place.
    ParseError,
}

/// The result of a write (`docs/interfaces.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WriteOutcome {
    /// Done. `hash` is the new content hash (absent for a delete or restart).
    Applied {
        hash: Option<ContentHash>,
        audit_id: AuditId,
        /// Things worth knowing that did not stop the write (duplicate keys). Absent in JSON written before this
        /// field existed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<ValidationIssue>,
    },
    /// The user requires approval: a proposal was created instead of writing.
    ProposalCreated { id: ProposalId },
    /// The expected state did not match. Nothing was written. `current` is what is there now.
    Conflict { current: Option<ContentHash> },
    /// A sync window or another lock is open.
    Locked { until: Option<Timestamp> },
    Rejected {
        reason: RejectReason,
        /// Why, with places. Empty when the reason says it all. Absent in JSON written before this field existed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        issues: Vec<ValidationIssue>,
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
