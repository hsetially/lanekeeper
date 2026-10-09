//! Read DTOs for swimlanes, files, versions, effective config and service pickup state.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{
    AgentStatus, Attribution, ChannelName, CommitId, ContentHash, DriftState, EolStyle, FileClass, FileKind,
    FileRole, GitRef, LogicalFile, NfsPath, ObservationSource, PickupState, RepoKind, ServiceRef,
    SettingLocation, SettingPath, Severity, ShortText, SwimlaneId, TenantId, TextEncoding, Timestamp,
    VersionLabel,
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
