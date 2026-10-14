//! Response bodies that are not the domain type itself (T6).
//!
//! Most responses are the domain types, which serialize to the contract's JSON (the contract test walks every response against its
//! schema, so a drift between the two is a failing test). Four are not:
//!
//! - **`FileContent`** is "not `Serialize`: content is streamed by the edge, never embedded in JSON". The contract's `FileContent` has
//!   the decoded text, for structured and text files, with the original line endings; null for binary, denied and undecodable files.
//! - **`SettingChange`** has a `severity` in the contract (nullable) that the domain type does not carry. T9 attaches severity to every
//!   change; until then it is null. The shape is the contract's and nothing else is added.
//! - **`SwimlaneDetail`** is the summary plus its services and the auto-notify setting (the spec's `allOf`).
//! - **`ServedFile`** (plan A4): what the config-server answered, with the engine's role and sources. It has no domain type.

use bytes::Bytes;
use domain::{
    AppName, BinaryCompare, ChannelName, CompareRef, Comparison, ContentHash, DiffHunk, FileClass,
    FileContent, FileFormat, FileRole, LogicalFile, NfsPath, ServiceState, SettingChange, SettingLocation,
    SettingValue, Severity, ShortText, SwimlaneId, SwimlaneSummary, TenantId,
};
use engine::{Limits, decode_text};
use serde::Serialize;

/// `GET /api/v1/swimlanes/{id}`: the summary, its services and whether notifications go out by themselves.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SwimlaneDetail {
    #[serde(flatten)]
    pub summary: SwimlaneSummary,
    pub services: Vec<ServiceState>,
    pub auto_notify: bool,
}

/// The contract's `FileContent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileContentDto {
    pub path: NfsPath,
    pub hash: ContentHash,
    pub class: FileClass,
    pub format: FileFormat,
    pub size: u64,
    pub content_withheld: bool,
    /// Decoded text with the file's own line endings; `None` for binary and withheld files, and for bytes that do not decode.
    pub text: Option<String>,
}

impl FileContentDto {
    pub fn new(content: &FileContent, limits: &Limits) -> Self {
        let text = match (&content.bytes, content.class, content.withheld) {
            (Some(bytes), FileClass::Structured | FileClass::Text, false) => {
                decode_text(bytes, limits).ok().map(|d| d.text.into_owned())
            }
            _ => None,
        };
        Self {
            path: content.path.clone(),
            hash: content.hash,
            class: content.class,
            format: content.format,
            size: content.size,
            content_withheld: content.withheld,
            text,
        }
    }
}

/// The contract's `SettingChange`: the domain's, with the severity the contract has and T9 fills.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingChangeDto {
    pub path: domain::SettingPath,
    pub kind: domain::ChangeKind,
    pub left: Option<SettingValue>,
    pub right: Option<SettingValue>,
    pub left_location: Option<SettingLocation>,
    pub right_location: Option<SettingLocation>,
    pub severity: Option<Severity>,
}

impl From<&SettingChange> for SettingChangeDto {
    fn from(c: &SettingChange) -> Self {
        Self {
            path: c.path.clone(),
            kind: c.kind,
            left: c.left.clone(),
            right: c.right.clone(),
            left_location: c.left_location.clone(),
            right_location: c.right_location.clone(),
            severity: None,
        }
    }
}

/// The contract's `Comparison`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComparisonDto {
    pub left: CompareRef,
    pub right: CompareRef,
    pub file: Option<LogicalFile>,
    pub class: FileClass,
    pub identical: bool,
    pub eol_only: bool,
    pub hunks: Vec<DiffHunk>,
    pub setting_changes: Vec<SettingChangeDto>,
    pub binary: Option<BinaryCompare>,
}

impl From<&Comparison> for ComparisonDto {
    fn from(c: &Comparison) -> Self {
        Self {
            left: c.left.clone(),
            right: c.right.clone(),
            file: c.file.clone(),
            class: c.class,
            identical: c.identical,
            eol_only: c.eol_only,
            hunks: c.hunks.clone(),
            setting_changes: c.setting_changes.iter().map(SettingChangeDto::from).collect(),
            binary: c.binary.clone(),
        }
    }
}

/// The contract's `ServedFile` (plan A4): a file as the config-server serves it, asked for through the agent. It never uses
/// environment values; `${KEY}` is replaced only where the config-server's property view has the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServedFile {
    pub swimlane: SwimlaneId,
    pub application: AppName,
    pub tenant: TenantId,
    pub channel: Option<ChannelName>,
    pub file: NfsPath,
    pub role: FileRole,
    pub hash: ContentHash,
    pub content_withheld: bool,
    pub text: Option<String>,
    pub sources: Vec<domain::EffectiveSource>,
    pub unresolved_placeholders: Vec<ShortText>,
}

/// The bytes of a served answer as text (UTF-8, or what `decode_text` makes of them), or `None` when they are not text.
pub fn served_text(bytes: &Bytes, limits: &Limits) -> Option<String> {
    decode_text(bytes, limits).ok().map(|d| d.text.into_owned())
}
