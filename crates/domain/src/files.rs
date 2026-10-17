//! Read DTOs for swimlanes, files, versions, effective config and service pickup state.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{
    AgentStatus, Attribution, ChannelName, CommitId, ContentHash, DriftState, EolStyle, FileClass, FileKind,
    FileRole, GitRef, JobRef, LogicalFile, NfsPath, ObservationSource, PickupState, RepoKind, ServiceRef,
    SettingLocation, SettingPath, Severity, ShortText, SwimlaneId, TenantId, TextEncoding, Timestamp,
    UserRef, VersionLabel,
};

/// A row of the swimlane list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwimlaneSummary {
    pub id: SwimlaneId,
    pub display_name: ShortText,
    /// The tenants deployed to this swimlane (a set, Q34).
    pub tenants: Vec<TenantId>,
    pub agent: AgentStatus,
    pub last_scan_at: Option<Timestamp>,
    pub base_version: Option<VersionLabel>,
    /// Counts by drift state, for the list badges. Only non-zero states are listed.
    pub drift_counts: Vec<(DriftState, u32)>,
    pub finding_count: u32,
    /// The environment tier of the swimlane (`swimlanes.tier`), when one is set. Absent on the wire when `None`
    /// (`contract/08-web`, `CG4a`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ShortText>,
    /// The display name of the project the swimlane belongs to, when it has one. Absent on the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ShortText>,
    /// Whether the swimlane's baseline is adopted, and by whom and when. Absent on the wire when `None`, which only a producer
    /// written before this field says: the registry states it for every swimlane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineStatus>,
    /// How many services are in each pickup state that is not [`PickupState::Live`], for the list badges and the pending
    /// column. It is an aggregate worked out in one query for all swimlanes, never a call per swimlane, and it agrees with
    /// what [`crate::ServiceState`] lists per service (`RegistryRead::pending_restarts`): only non-zero states are listed,
    /// each once. Empty (and absent on the wire) when nothing is waiting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pickup_counts: Vec<(PickupState, u32)>,
    /// The sync window that is open on the swimlane now, if any (D72). Absent on the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syncing: Option<SyncingInfo>,
}

/// Where a swimlane's baseline stands (D17): not adopted, being adopted, or adopted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineState {
    /// No baseline: editing is locked until someone adopts one.
    Unconfirmed,
    /// An adoption is open (it is being worked through, or it waits for review). The baseline is not confirmed yet.
    InProgress,
    /// Adopted and confirmed: editing is unlocked.
    Confirmed,
}

impl BaselineState {
    /// Every variant, in declaration order.
    pub const ALL: [Self; 3] = [Self::Unconfirmed, Self::InProgress, Self::Confirmed];

    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unconfirmed => "unconfirmed",
            Self::InProgress => "in_progress",
            Self::Confirmed => "confirmed",
        }
    }
}

/// The baseline of a swimlane: its state and, once confirmed, who confirmed it and when. Both are always written, `null` when
/// there is nobody and no time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineStatus {
    pub state: BaselineState,
    /// Who confirmed it. `None` unless the state is [`BaselineState::Confirmed`].
    pub by: Option<UserRef>,
    /// When it was confirmed. `None` unless the state is [`BaselineState::Confirmed`].
    pub at: Option<Timestamp>,
}

/// A sync window that is open on a swimlane: the Kubernetes Job that is syncing it and when the window opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncingInfo {
    pub job: JobRef,
    pub since: Timestamp,
}

/// Line endings, encoding and BOM of a file. Writes keep the original (rule 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFormat {
    pub eol: EolStyle,
    pub encoding: TextEncoding,
    pub bom: bool,
}

/// A file or directory in a swimlane tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: NfsPath,
    pub is_dir: bool,
    pub kind: FileKind,
    pub class: FileClass,
    pub role: FileRole,
    pub size: u64,
    /// `None` for directories.
    pub hash: Option<ContentHash>,
    pub drift: DriftState,
    pub format: Option<FileFormat>,
    /// True for denied files (D79): content endpoints answer "content withheld" and writes are refused.
    pub content_withheld: bool,
    pub attribution: Option<Attribution>,
    pub severity: Option<Severity>,
    pub pickup_state: Option<PickupState>,
    /// The file without the tenant suffix of its name: the same as `path` for a base file, and `path` with `-<tenant>`
    /// taken out for a tenant file. `None` for a directory. The registry computes it with the path mapping's reverse
    /// direction and the swimlane's tenant set; a client never derives it (`contract/08-web`, CG2). Absent on the wire when
    /// `None`, and JSON written before the field existed reads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_file: Option<LogicalFile>,
    /// The tenant whose file this is: set when the name ends in `-<tenant>` for a tenant of the swimlane (Q34). `None` for a
    /// base file and for a directory. Absent on the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<TenantId>,
}

/// Which copy of a file to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ContentSource {
    /// The current NFS version.
    Nfs,
    /// The adopted baseline.
    Baseline,
    /// A stored version, by hash.
    Blob { hash: ContentHash },
    /// A file at a Git ref.
    Git { repo: RepoKind, git_ref: GitRef },
}

/// File content. Not `Serialize`: content is streamed by the edge, never embedded in JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub path: NfsPath,
    pub hash: ContentHash,
    pub class: FileClass,
    pub format: FileFormat,
    pub size: u64,
    /// `None` when `withheld` (denied file) or when the caller asked for metadata only.
    pub bytes: Option<Bytes>,
    pub withheld: bool,
}

/// One version of a file in its history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub hash: ContentHash,
    pub observed_at: Timestamp,
    pub source: ObservationSource,
    pub size: u64,
    pub attribution: Attribution,
    pub severity: Option<Severity>,
    /// The Git commit, for versions read from Git.
    pub commit: Option<CommitId>,
}

/// Where one effective setting or file body came from, highest precedence first (D82).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveSource {
    pub path: NfsPath,
    /// 1 is the highest precedence.
    pub rank: u32,
}

/// One setting of a property view, with the file that supplied it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveSetting {
    pub path: SettingPath,
    /// The value as written in the source.
    pub value: String,
    pub source: NfsPath,
    pub location: Option<SettingLocation>,
}

/// What the config-server would serve for `(file, tenant, channel)`, computed by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveConfig {
    pub swimlane: SwimlaneId,
    pub file: LogicalFile,
    pub tenant: TenantId,
    pub channel: Option<ChannelName>,
    pub role: FileRole,
    /// Hash of the rendered content.
    pub hash: ContentHash,
    pub sources: Vec<EffectiveSource>,
    /// For property sources: the merged settings. Empty for resource files.
    pub settings: Vec<EffectiveSetting>,
    /// `${...}` placeholders left as written because their keys are not in the property view.
    pub unresolved_placeholders: Vec<ShortText>,
}

/// What to compute for [`EffectiveConfig`]: a file, the tenant whose view it is, optionally a channel (D82, D83), and
/// optionally only a part of the settings. A swimlane has a set of tenants (Q34), so the tenant is always named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveQuery {
    pub file: LogicalFile,
    pub tenant: TenantId,
    pub channel: Option<ChannelName>,
    /// Only the settings at or below this path: a setting is below it when its path is the prefix or continues it after a
    /// `.` or a `[`. A property source only; a resource file has no settings, so the prefix selects nothing of it.
    pub setting_prefix: Option<SettingPath>,
}

/// The files of a swimlane in a drift state, for `RegistryRead::drift` (the REST `listDrift`, and the drift filter of a folder
/// listing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftQuery {
    /// Only files in this state. `None`: every state except [`DriftState::InSync`], so the files that are not in sync.
    pub state: Option<DriftState>,
    /// Only files below this folder, at any depth. `None`: the whole swimlane.
    pub prefix: Option<NfsPath>,
}

/// Pickup state of one consuming service (D85).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceState {
    pub service: ServiceRef,
    pub pickup: PickupState,
    /// When the oldest pending change was detected.
    pub pending_since: Option<Timestamp>,
    /// For `live_within_ttl`: when the client cache expires ("by HH:MM").
    pub live_by: Option<Timestamp>,
    pub notifications_enabled: bool,
    /// `CONFIG_CLIENT_CACHE_TTL` in seconds (default 20 minutes).
    pub cache_ttl_secs: u32,
    /// Files whose changes this service has not picked up yet (bounded by the producer).
    pub pending_files: Vec<NfsPath>,
}
