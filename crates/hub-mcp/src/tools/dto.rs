//! The shapes of the answers (the output schemas of `docs/mcp-tools.md`), and how the engine's and the ports' types become them.
//!
//! Every field that can hold text from a file or a doc is a [`RedactedText`], whose only constructor runs the secret scan (S14b), so
//! a tool cannot return unscanned text: it does not compile. A value is redacted *before* it is cut to its length, so a secret that
//! the cut would have split is never shown in part.
//!
//! The types here are built to serialise to exactly the contract's JSON (`additionalProperties: false`, every `required` field
//! present, `null` where the schema says `oneOf [x, null]`); `tests/mcp_auth/contract_schemas.rs` validates every tool's result
//! against its schema with a validator that is not ours.

use domain::{
    AgentStatus, Attribution, ChangeKind, ChannelName, CompareState, ContentHash, DriftState, FileClass,
    FileEntry, FileKind, FileRole, FindingKind, LineRange, LogicalFile, NfsPath, ObservationSource,
    PickupState, ServiceRef, ServiceState, SettingLocation, SettingPath, Severity, ShortText, SwimlaneId,
    SwimlaneSummary, TenantId, Timestamp, ValueFlag, ValueType, Version,
};
use serde::Serialize;

use crate::output::cap::cut_chars;
use crate::output::{RedactError, RedactedText, Redactor};
use crate::tools::denied::Denied;

/// The largest integer a JSON number carries without loss: the contract's cap on sizes and ids.
pub const MAX_SAFE_INT: u64 = 9_007_199_254_740_991;

/// `n` as a size the contract allows.
#[must_use]
pub fn safe(n: u64) -> u64 {
    n.min(MAX_SAFE_INT)
}

/// A short text the server made from typed parts, cut to 256 characters (the contract's `ShortText`).
fn short(text: &str) -> String {
    cut_chars(text, 256).0.to_owned()
}

// ---------------------------------------------------------------------------------------------------------------------
// Swimlanes and services
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DriftCountDto {
    pub state: DriftState,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwimlaneDto {
    pub id: SwimlaneId,
    pub display_name: ShortText,
    pub tenants: Vec<TenantId>,
    pub agent: AgentStatus,
    pub last_scan_at: Option<Timestamp>,
    pub base_version: Option<String>,
    pub drift_counts: Vec<DriftCountDto>,
    pub finding_count: u32,
}

impl From<&SwimlaneSummary> for SwimlaneDto {
    fn from(s: &SwimlaneSummary) -> Self {
        Self {
            id: s.id.clone(),
            display_name: s.display_name.clone(),
            tenants: s.tenants.iter().take(64).cloned().collect(),
            agent: s.agent,
            last_scan_at: s.last_scan_at,
            base_version: s.base_version.as_ref().map(|v| short(&v.to_string())),
            drift_counts: s
                .drift_counts
                .iter()
                .filter(|(_, n)| *n > 0)
                .take(8)
                .map(|(state, count)| DriftCountDto {
                    state: *state,
                    count: *count,
                })
                .collect(),
            finding_count: s.finding_count,
        }
    }
}

/// The most files a service lists as pending.
pub const MAX_PENDING_FILES: usize = 50;

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStateDto {
    pub service: ServiceRef,
    pub pickup: PickupState,
    pub pending_since: Option<Timestamp>,
    pub live_by: Option<Timestamp>,
    pub notifications_enabled: bool,
    pub cache_ttl_secs: u32,
    pub pending_files: Vec<NfsPath>,
}

impl From<&ServiceState> for ServiceStateDto {
    fn from(s: &ServiceState) -> Self {
        Self {
            service: s.service.clone(),
            pickup: s.pickup,
            pending_since: s.pending_since,
            live_by: s.live_by,
            notifications_enabled: s.notifications_enabled,
            cache_ttl_secs: s.cache_ttl_secs.min(31_536_000),
            pending_files: s.pending_files.iter().take(MAX_PENDING_FILES).cloned().collect(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Files, attribution
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ActorDto {
    pub kind: &'static str,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributionDto {
    pub source: domain::AttributionSource,
    pub confidence: domain::Confidence,
    pub actor: Option<ActorDto>,
    pub executable: Option<ShortText>,
}

impl From<&Attribution> for AttributionDto {
    fn from(a: &Attribution) -> Self {
        let actor = a.actor.as_ref().map(|actor| match actor {
            domain::Actor::User { user } => ActorDto {
                kind: "user",
                id: short(&user.to_string()),
            },
            domain::Actor::OsLogin { login } => ActorDto {
                kind: "os_login",
                id: short(login.as_str()),
            },
            domain::Actor::Job { job } => ActorDto {
                kind: "job",
                id: short(job.name()),
            },
        });
        Self {
            source: a.source,
            confidence: a.confidence,
            actor,
            executable: a.evidence.executable.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntryDto {
    pub path: NfsPath,
    pub is_dir: bool,
    pub kind: FileKind,
    pub class: FileClass,
    pub role: FileRole,
    pub size: u64,
    pub hash: Option<ContentHash>,
    pub drift: DriftState,
    pub content_withheld: bool,
    pub attribution: Option<AttributionDto>,
    pub severity: Option<Severity>,
    pub pickup_state: Option<PickupState>,
}

impl FileEntryDto {
    /// The entry as the contract shows it. A file that is denied by its class, by the port's flag or by its name (D79) shows as
    /// `denied`, withheld, with the name, size and hash it is known by.
    #[must_use]
    pub fn new(e: &FileEntry, denied: &Denied) -> Self {
        let is_denied = e.class == FileClass::Denied || e.content_withheld || denied.path(&e.path);
        Self {
            path: e.path.clone(),
            is_dir: e.is_dir,
            kind: e.kind,
            class: if is_denied && !e.is_dir {
                FileClass::Denied
            } else {
                e.class
            },
            role: e.role,
            size: safe(e.size),
            hash: e.hash,
            drift: e.drift,
            // A denied file is withheld whatever the entry's flag says: the class is the rule (D79). A folder has no content.
            content_withheld: is_denied && !e.is_dir,
            attribution: e.attribution.as_ref().map(AttributionDto::from),
            severity: e.severity,
            pickup_state: e.pickup_state,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionDto {
    pub hash: ContentHash,
    pub observed_at: Timestamp,
    pub source: ObservationSource,
    pub size: u64,
    pub attribution: AttributionDto,
    pub severity: Option<Severity>,
    pub commit: Option<String>,
}

impl From<&Version> for VersionDto {
    fn from(v: &Version) -> Self {
        Self {
            hash: v.hash,
            observed_at: v.observed_at,
            source: v.source,
            size: safe(v.size),
            attribution: AttributionDto::from(&v.attribution),
            severity: v.severity,
            commit: v.commit.as_ref().map(|c| c.as_str().to_owned()),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Settings and diffs
// ---------------------------------------------------------------------------------------------------------------------

/// The longest setting value in a list of values, in characters. A single item must always fit the cap, so lists cut harder than
/// the contract's 2,048.
pub const LIST_VALUE_CHARS: usize = 512;
/// The longest value in a grid cell: a row can have 64 of them.
pub const CELL_VALUE_CHARS: usize = 96;
/// The longest value where one value is the answer.
pub const VALUE_CHARS: usize = 2048;

#[derive(Debug, Clone, Serialize)]
pub struct SettingValueDto {
    pub text: RedactedText,
    #[serde(rename = "type")]
    pub ty: ValueType,
    pub redacted: bool,
    pub flag: Option<ValueFlag>,
}

/// A value, redacted and then cut to `max_chars`. Returns the value and how many secrets were replaced.
///
/// # Errors
///
/// [`RedactError`] when the scan cannot finish: the value is then not shown.
pub fn setting_value(
    redactor: &Redactor,
    value: &domain::SettingValue,
    max_chars: usize,
) -> Result<(SettingValueDto, u32), RedactError> {
    let redacted = redactor.text(&value.text)?;
    let text = redacted.text.truncated(max_chars);
    Ok((
        SettingValueDto {
            text,
            ty: value.ty,
            redacted: redacted.redacted > 0,
            flag: value.flag,
        },
        redacted.redacted,
    ))
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingChangeDto {
    pub path: SettingPath,
    pub kind: ChangeKind,
    pub left: Option<SettingValueDto>,
    pub right: Option<SettingValueDto>,
    pub left_location: Option<SettingLocation>,
    pub right_location: Option<SettingLocation>,
}

/// # Errors
///
/// [`RedactError`] when a value cannot be scanned.
pub fn setting_change(
    redactor: &Redactor,
    change: &domain::SettingChange,
    max_chars: usize,
) -> Result<(SettingChangeDto, u32), RedactError> {
    let mut count = 0;
    let mut side = |value: &Option<domain::SettingValue>| -> Result<Option<SettingValueDto>, RedactError> {
        value
            .as_ref()
            .map(|v| {
                let (dto, n) = setting_value(redactor, v, max_chars)?;
                count += n;
                Ok(dto)
            })
            .transpose()
    };
    let left = side(&change.left)?;
    let right = side(&change.right)?;
    Ok((
        SettingChangeDto {
            path: change.path.clone(),
            kind: change.kind,
            left,
            right,
            left_location: change.left_location.clone(),
            right_location: change.right_location.clone(),
        },
        count,
    ))
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffLineDto {
    pub kind: domain::DiffLineKind,
    pub text: RedactedText,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffHunkDto {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLineDto>,
}

/// The most lines of a hunk in a result: the contract's `maxItems`.
pub const MAX_HUNK_LINES: usize = 400;
/// The longest line of a hunk, in characters.
pub const HUNK_LINE_CHARS: usize = 1024;

/// A hunk with its lines scanned as one text (so a block that spans lines is seen whole), then cut.
///
/// # Errors
///
/// [`RedactError`] when the scan cannot finish, or when replacing a secret would change the number of lines.
pub fn diff_hunk(redactor: &Redactor, hunk: &domain::DiffHunk) -> Result<(DiffHunkDto, u32), RedactError> {
    let lines: Vec<&domain::DiffLine> = hunk.lines.iter().take(MAX_HUNK_LINES).collect();
    let joined = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let redacted = redactor.text(&joined)?;
    let texts = redacted.text.lines();
    if texts.len() != lines.len().max(1) {
        return Err(RedactError);
    }
    let out = lines
        .iter()
        .zip(&texts)
        .map(|(line, text)| DiffLineDto {
            kind: line.kind,
            text: text.truncated(HUNK_LINE_CHARS),
        })
        .collect();
    Ok((
        DiffHunkDto {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            lines: out,
        },
        redacted.redacted,
    ))
}

// ---------------------------------------------------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FindingDto {
    pub id: u64,
    pub kind: FindingKind,
    pub swimlane: SwimlaneId,
    pub path: Option<NfsPath>,
    pub severity: Severity,
    pub rule_ids: Vec<ShortText>,
    pub message: ShortText,
    pub locations: Vec<SettingLocation>,
    pub detected_at: Timestamp,
}

impl From<&domain::Finding> for FindingDto {
    fn from(f: &domain::Finding) -> Self {
        Self {
            id: safe(u64::try_from(f.id.get()).unwrap_or(0)),
            kind: f.kind,
            swimlane: f.swimlane.clone(),
            path: f.path.clone(),
            severity: f.severity,
            rule_ids: f.rule_ids.iter().take(20).cloned().collect(),
            message: f.message.clone(),
            locations: f.locations.iter().take(20).cloned().collect(),
            detected_at: f.detected_at,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Smaller shapes used by several tools
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SourceDto {
    pub path: NfsPath,
    pub rank: u32,
}

impl From<&domain::EffectiveSource> for SourceDto {
    fn from(s: &domain::EffectiveSource) -> Self {
        Self {
            path: s.path.clone(),
            rank: s.rank.clamp(1, 1000),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TreeEntryDto {
    pub file: LogicalFile,
    pub state: CompareState,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinesDto {
    pub start: u32,
    pub end: u32,
}

impl From<LineRange> for LinesDto {
    fn from(r: LineRange) -> Self {
        Self {
            start: r.start(),
            end: r.end(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelOpt(pub Option<ChannelName>);
