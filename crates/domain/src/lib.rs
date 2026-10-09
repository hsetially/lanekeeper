//! Shared domain types (validated newtypes, enums, DTOs, the Secret wrapper). No I/O.
//!
//! Owned by prompt 01. Read `AGENTS.md` and `prompts/01*.md` before changing this crate.
//! This crate is a contract path: changes go through the `lanekeeper-contract-change` process.
//!
//! Every value that crosses a trust boundary (a path, a ref, an id) has a validating constructor,
//! so the rest of the system never handles a raw `String` where a typed value exists (S11).
//! Error types carry no input text, so a rejected secret-looking value never reaches a log (S21).
#![forbid(unsafe_code)]
// Documenting `# Errors` on every one-line validating constructor adds noise; the error enums are the docs.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

mod attribution;
mod compare;
mod compare_ref;
mod docs;
mod enums;
mod events;
mod files;
mod findings;
mod git;
mod ids;
mod mapping;
mod page;
mod paths;
mod secret;
mod serve;
mod setting;
mod text;
mod time;
mod user;
mod wire;
mod write;

pub use attribution::{Actor, Attribution, Evidence};
pub use compare::{
    BinaryCompare, ChangeKind, CompareState, Comparison, DiffHunk, DiffLine, DiffLineKind, Grid, GridCell,
    GridQuery, GridRow, InlineRange, SettingChange, SettingValue, TreeCompareEntry, TreeComparison,
    ValueFlag, ValueType,
};
pub use compare_ref::{CompareRef, CompareRefError, GitRef, GitRefError};
pub use docs::{DocGrepHit, DocHit, DocText, FeatureQuery, FeatureStatus, FlagStatus, FlagValue};
pub use enums::{
    AgentStatus, AttributionSource, Confidence, DriftState, EolStyle, FileClass, FileKind, FileRole,
    FindingKind, MergeMode, ObservationSource, PickupState, PrJobState, PrState, ProposalStatus, RepoKind,
    Role, Severity, TextEncoding, UnknownVariant, UserStatus, Via,
};
pub use events::DomainEvent;
pub use files::{
    ContentSource, EffectiveConfig, EffectiveSetting, EffectiveSource, FileContent, FileEntry, FileFormat,
    ServiceState, SwimlaneSummary, Version,
};
pub use findings::{Finding, FindingsQuery, SettingHit, SettingsQuery, TextHit, TextQuery, TextResults};
pub use git::{
    BranchInfo, BranchKind, CommitInfo, PathChange, PathChangeKind, TagInfo, TreeEntry, TreeIndex,
    VersionLabel,
};
pub use ids::{
    AuditId, CommitId, ContentHash, DocId, DraftId, FindingId, Guid, IdError, IdempotencyKey, JobRef,
    PrJobId, PrLinkId, ProposalId, RequestId, ServiceRef, SwimlaneId, TenantId, UserId,
};
pub use mapping::{MapError, PathMappingRule, ReverseMapping, TenantSet, TenantSetError};
pub use page::{Cursor, Page, PageError, Paged};
pub use paths::{DocPath, LineRange, LogicalFile, NfsPath, PathError, RepoPath, SettingPath};
pub use secret::Secret;
pub use serve::{AppName, ChannelName, ServeRequest};
pub use setting::SettingLocation;
pub use text::{ShortText, TextError};
pub use time::Timestamp;
pub use user::User;
pub use wire::{
    AgentConfig, AgentReply, AuditOperation, AuditRecord, AuditRecordBatch, ClusterReport, DeploymentInfo,
    EnvValue, Heartbeat, HeartbeatAction, Hello, HubCommand, OpError, OpResult, PodInfo, ReleaseHint,
    ScanDelta, ScanEntry, SentinelConfig, SentinelHello, SkippedEntry, SyncWindowEvent, SyncWindowKind,
};
pub use write::{
    DeleteRequest, Draft, EditRequest, Expected, PrRequest, RejectReason, RestartRequest, RevertRequest,
    UploadRequest, WriteCtx, WriteOutcome,
};
