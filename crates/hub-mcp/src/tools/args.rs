//! The arguments of every tool, parsed into typed values (S11).
//!
//! One struct per tool, `deny_unknown_fields`, every field a domain parser (`SwimlaneId`, `NfsPath`, `LogicalFile`, `SettingPath`,
//! `DocPath`, `CompareRef`, `ContentHash`, `Cursor`, `ChannelName`, `TenantId`, `ShortText`, `LineRange`), so a raw string never
//! reaches a port. The rule that this file keeps and `tests/mcp_auth/contract_schemas.rs` proves by property: **nothing the typed
//! parser accepts is rejected by the document's schema.** The parser may be stricter than the schema (and is, where the domain
//! parser normalises a path or the schema counts characters and the parser counts bytes), never looser:
//!
//! - an optional field that is present as `null` is refused (the schemas have no `null` for them);
//! - a string is held to the schema's `maxLength` in bytes before the domain parser sees it, because the domain parsers normalise
//!   first (a path with `//` in it) and the schema counts the text as sent;
//! - a hash is lower case, a channel or application name starts with a letter, a digit or `_`, a line range is at most ten
//!   million, because the schemas say so and the domain parsers are wider;
//! - a list is unique after parsing, not only as text.
//!
//! This file depends on `domain`, `serde` and `serde_json` and on nothing else of the crate, so the fuzz target `mcp_tool_args`
//! can include it as it is (`crates/hub-mcp/fuzz/`).

use std::collections::BTreeSet;
use std::fmt;

use domain::{
    ChangeEol, ContentHash, Cursor, DriftState, FindingKind, GitRef, PrTarget, RepoKind, ServiceRef, Severity,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

/// The most bytes of file content an argument may carry: the engine's input limit (S11). Counted in bytes, not characters.
pub const MAX_CONTENT_BYTES: usize = 2 * 1024 * 1024;
/// The most bytes of a PR body.
pub const MAX_BODY_BYTES: usize = 65_536;
/// The most bytes of a search or grep pattern.
pub const MAX_PATTERN_BYTES: usize = 512;
/// The most swimlanes one argument may list.
pub const MAX_SWIMLANES: usize = 64;
/// The most paths a PR or a notification may list.
pub const MAX_PATHS: usize = 100;

// ---------------------------------------------------------------------------------------------------------------------
// Tool names
// ---------------------------------------------------------------------------------------------------------------------

macro_rules! tool_names {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        /// A tool of the contract (`docs/mcp-tools.md`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum ToolName { $($variant),+ }

        impl ToolName {
            /// Every tool, in the order of the document.
            pub const ALL: &'static [ToolName] = &[$(ToolName::$variant),+];

            /// The name on the wire.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $name),+ }
            }

            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                match name { $($name => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

tool_names! {
    ListSwimlanes => "list_swimlanes",
    GetSwimlane => "get_swimlane",
    ListFiles => "list_files",
    ReadFile => "read_file",
    GetEffectiveConfig => "get_effective_config",
    GetServedConfig => "get_served_config",
    Compare => "compare",
    CompareTree => "compare_tree",
    CompareGrid => "compare_grid",
    SearchSettings => "search_settings",
    SearchText => "search_text",
    FindInconsistencies => "find_inconsistencies",
    GetDrift => "get_drift",
    GetPendingRestarts => "get_pending_restarts",
    GetHistory => "get_history",
    SearchDocs => "search_docs",
    ReadDoc => "read_doc",
    GrepDocs => "grep_docs",
    GetDocsForFile => "get_docs_for_file",
    GetFeatureStatus => "get_feature_status",
    ProposeChange => "propose_change",
    ApplyChange => "apply_change",
    UploadFile => "upload_file",
    DeleteFile => "delete_file",
    RaisePr => "raise_pr",
    RestartService => "restart_service",
    NotifyServices => "notify_services",
}

// ---------------------------------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------------------------------

/// Why arguments were refused. Fixed text; no variant carries any of the input (S21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArgError {
    /// An argument is missing, unknown, of the wrong type or out of range.
    #[error("an argument is not valid; see the input schema of the tool")]
    Invalid,
    /// A required argument is missing. The name is one of the tool's own.
    #[error("a required argument is missing")]
    Missing(&'static str),
    /// File content is larger than the 2 MiB the server accepts.
    #[error("the content is larger than 2 MiB")]
    TooLarge,
}

fn de_error<E: serde::de::Error>(what: &'static str) -> E {
    E::custom(what)
}

// ---------------------------------------------------------------------------------------------------------------------
// Strict field types
// ---------------------------------------------------------------------------------------------------------------------

/// `Some(value)` for a field that is present. A present `null` is an error for every type that is not itself an option, which is
/// what the schemas say (no field of them accepts `null` unless it says so).
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

/// A struct that must arrive as a JSON object. serde's derived struct deserializer also accepts a JSON array (`[1, 2]` for a
/// struct of two fields), which the schemas do not, so each object-shaped argument is read as a map first.
fn object<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let map = Map::<String, Value>::deserialize(d)?;
    T::deserialize(Value::Object(map)).map_err(|_| de_error("not valid"))
}

/// A string no longer than `max` bytes.
fn bounded_string<'de, D: Deserializer<'de>>(d: D, max: usize) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.len() > max {
        return Err(de_error("too long"));
    }
    Ok(s)
}

/// Text from the caller that may hold anything, a secret included: file content for a write, a PR body, a search pattern. Its
/// `Debug` prints the length and never the text, so that no log line, panic message or test failure can show it (S10, S21).
#[derive(Clone, PartialEq, Eq)]
pub struct Opaque(String);

impl Opaque {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Opaque {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Opaque({} bytes)", self.0.len())
    }
}

macro_rules! string_arg {
    ($(#[$meta:meta])* $name:ident($inner:ty), max $max:expr, $parse:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(pub $inner);

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = bounded_string(d, $max)?;
                #[allow(clippy::redundant_closure_call)]
                ($parse)(s.as_str()).map(Self).ok_or_else(|| de_error("not valid"))
            }
        }
    };
}

/// The first byte of a name is a letter, a digit or `_` (the schemas' pattern), where the domain parser also lets `-` start one.
fn name_start_ok(s: &str) -> bool {
    s.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
}

string_arg!(
    /// A swimlane id.
    Swimlane(domain::SwimlaneId), max 63, |s: &str| domain::SwimlaneId::parse(s).ok()
);
string_arg!(
    /// A tenant id.
    Tenant(domain::TenantId), max 63, |s: &str| domain::TenantId::parse(s).ok()
);
string_arg!(
    /// A config-server application name.
    App(domain::AppName), max 128, |s: &str| name_start_ok(s).then(|| domain::AppName::parse(s).ok()).flatten()
);
string_arg!(
    /// A channel name.
    Channel(domain::ChannelName), max 128, |s: &str| name_start_ok(s).then(|| domain::ChannelName::parse(s).ok()).flatten()
);
string_arg!(
    /// A relative path below a swimlane's NFS root.
    Nfs(domain::NfsPath), max 1024, |s: &str| domain::NfsPath::parse(s).ok()
);
string_arg!(
    /// A base-relative path without the tenant suffix.
    Logical(domain::LogicalFile), max 1024, |s: &str| domain::LogicalFile::parse(s).ok()
);
string_arg!(
    /// A flattened setting path.
    Setting(domain::SettingPath), max 1024, |s: &str| domain::SettingPath::parse(s).ok()
);
string_arg!(
    /// A path in the virtual docs tree.
    Doc(domain::DocPath), max 1024, |s: &str| domain::DocPath::parse(s).ok()
);
string_arg!(
    /// A draft id.
    Draft(domain::DraftId), max 128, |s: &str| domain::DraftId::parse(s).ok()
);
string_arg!(
    /// Short text: at most 256 bytes, no control characters.
    Short(domain::ShortText), max 256, |s: &str| domain::ShortText::parse(s).ok()
);
string_arg!(
    /// Short text that is not empty.
    ShortNonEmpty(domain::ShortText), max 256, |s: &str| (!s.is_empty()).then(|| domain::ShortText::parse(s).ok()).flatten()
);
string_arg!(
    /// A keyset cursor, as the server handed it out.
    CursorArg(Cursor), max 512, |s: &str| Cursor::parse(s).ok()
);
string_arg!(
    /// A branch, tag or commit id.
    GitRefArg(GitRef), max 255, |s: &str| GitRef::parse(s).ok()
);
string_arg!(
    /// A hash: lower case hexadecimal only (the schema's pattern; the domain parser also reads upper case).
    HashArg(ContentHash), max 64, |s: &str| {
        (s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| ContentHash::parse(s).ok())
            .flatten()
    }
);
string_arg!(
    /// One side of a comparison. The schema's pattern is an ECMA expression whose `.` does not match a line terminator.
    CompareRefArg(domain::CompareRef), max 300, |s: &str| {
        (!s.contains(['\n', '\r', '\u{2028}', '\u{2029}'])).then(|| s.parse::<domain::CompareRef>().ok()).flatten()
    }
);
string_arg!(
    /// A pattern for a search: 1 to 512 bytes of anything.
    PatternArg(Opaque), max MAX_PATTERN_BYTES, |s: &str| (!s.is_empty()).then(|| Opaque(s.to_owned()))
);

/// An integer in `MIN..=MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounded<const MIN: u32, const MAX: u32>(pub u32);

impl<'de, const MIN: u32, const MAX: u32> Deserialize<'de> for Bounded<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = u32::deserialize(d)?;
        if (MIN..=MAX).contains(&n) {
            Ok(Self(n))
        } else {
            Err(de_error("out of range"))
        }
    }
}

/// A page size of a list tool.
pub type Limit200 = Bounded<1, 200>;

/// A 1-based inclusive line range. Strict: no other key, and at most ten million (the schema's `maximum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinesArg(pub domain::LineRange);

impl<'de> Deserialize<'de> for LinesArg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            start: Bounded<1, 10_000_000>,
            end: Bounded<1, 10_000_000>,
        }
        let raw: Raw = object(d)?;
        domain::LineRange::new(raw.start.0, raw.end.0)
            .map(Self)
            .map_err(|_| de_error("not valid"))
    }
}

/// A Kubernetes workload. Strict: no other key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceArg(pub ServiceRef);

impl<'de> Deserialize<'de> for ServiceArg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            namespace: String,
            name: String,
        }
        let raw: Raw = object(d)?;
        if raw.namespace.len() > 253 || raw.name.len() > 253 {
            return Err(de_error("too long"));
        }
        ServiceRef::new(&raw.namespace, &raw.name)
            .map(Self)
            .map_err(|_| de_error("not valid"))
    }
}

/// What the caller expects to find (rule 11). Strict: no other key, and a hash exactly when `state` is `hash`.
///
/// Written by hand: serde's `deny_unknown_fields` does not apply to the unit variant of an internally tagged enum, so
/// `{"state": "absent", "hash": ...}` would be accepted, and the schema refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedArg {
    Absent,
    Hash { hash: ContentHash },
}

impl<'de> Deserialize<'de> for ExpectedArg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            state: String,
            #[serde(default, deserialize_with = "present")]
            hash: Option<HashArg>,
        }
        let raw: Raw = object(d)?;
        match (raw.state.as_str(), raw.hash) {
            ("absent", None) => Ok(Self::Absent),
            ("hash", Some(hash)) => Ok(Self::Hash { hash: hash.0 }),
            _ => Err(de_error("not valid")),
        }
    }
}

impl ExpectedArg {
    #[must_use]
    pub fn to_domain(self) -> domain::Expected {
        match self {
            Self::Absent => domain::Expected::Absent,
            Self::Hash { hash } => domain::Expected::Hash { hash },
        }
    }
}

/// The branch a PR goes to. Strict for the same reason as [`ExpectedArg`]: `{"kind": "base", "branch": ...}` is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrTargetArg(pub PrTarget);

impl<'de> Deserialize<'de> for PrTargetArg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            kind: String,
            #[serde(default, deserialize_with = "present")]
            branch: Option<Tenant>,
        }
        let raw: Raw = object(d)?;
        match (raw.kind.as_str(), raw.branch) {
            ("base", None) => Ok(Self(PrTarget::Base)),
            ("tenant", Some(branch)) => Ok(Self(PrTarget::Tenant { branch: branch.0 })),
            _ => Err(de_error("not valid")),
        }
    }
}

/// Where a file is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceArg {
    Nfs,
    Baseline,
    Git,
}

/// How much of a comparison to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailArg {
    Auto,
    Settings,
    Lines,
}

/// A unique list of 1 to `MAX` items, unique as parsed.
fn unique_list<'de, D, T>(d: D, min: usize, max: usize) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Ord + Clone,
{
    let items = Vec::<T>::deserialize(d)?;
    if items.len() < min || items.len() > max {
        return Err(de_error("wrong number of items"));
    }
    let distinct: BTreeSet<&T> = items.iter().collect();
    if distinct.len() != items.len() {
        return Err(de_error("items are not unique"));
    }
    Ok(items)
}

impl PartialOrd for Swimlane {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Swimlane {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}
impl PartialOrd for Nfs {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Nfs {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

/// Zero to 64 swimlanes, unique.
fn swimlanes_0_64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<Swimlane>>, D::Error> {
    unique_list(d, 0, MAX_SWIMLANES).map(Some)
}

/// One to 64 swimlanes, unique.
fn swimlanes_1_64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Swimlane>, D::Error> {
    unique_list(d, 1, MAX_SWIMLANES)
}

/// One to 100 paths, unique.
fn paths_1_100<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Nfs>, D::Error> {
    unique_list(d, 1, MAX_PATHS)
}

/// Zero to 12 finding kinds, unique.
fn kinds_0_12<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<FindingKind>>, D::Error> {
    unique_list(d, 0, 12).map(Some)
}

/// File content: at most 2 MiB of bytes. The error is `TooLarge` through [`ArgError`].
fn content_text<'de, D: Deserializer<'de>>(d: D) -> Result<Opaque, D::Error> {
    let s = String::deserialize(d)?;
    if s.len() > MAX_CONTENT_BYTES {
        return Err(de_error("too large"));
    }
    Ok(Opaque(s))
}

/// A PR body: a string of at most 64 KiB, or `null`.
fn pr_body<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Opaque>, D::Error> {
    let body = Option::<String>::deserialize(d)?;
    match body {
        Some(b) if b.len() > MAX_BODY_BYTES => Err(de_error("too long")),
        other => Ok(other.map(Opaque)),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The arguments of each tool
// ---------------------------------------------------------------------------------------------------------------------

macro_rules! args {
    ($(#[$meta:meta])* $name:ident { $( $(#[$fmeta:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $( $(#[$fmeta])* pub $field: $ty ),* }
    };
}

args!(ListSwimlanes {
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(GetSwimlane { swimlane: Swimlane });
args!(ListFiles {
    swimlane: Swimlane,
    #[serde(default, deserialize_with = "present")] prefix: Option<Nfs>,
    #[serde(default, deserialize_with = "present")] drift: Option<DriftState>,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(ReadFile {
    swimlane: Swimlane,
    path: Nfs,
    #[serde(default, deserialize_with = "present")] source: Option<SourceArg>,
    #[serde(default, deserialize_with = "present")] repo: Option<RepoKind>,
    #[serde(default, rename = "ref", deserialize_with = "present")] git_ref: Option<GitRefArg>,
    #[serde(default, deserialize_with = "present")] lines: Option<LinesArg>,
    #[serde(default, deserialize_with = "present")] setting_path: Option<Setting>,
});
args!(GetEffectiveConfig {
    swimlane: Swimlane,
    file: Logical,
    tenant: Tenant,
    #[serde(default, deserialize_with = "present")] channel: Option<Channel>,
    #[serde(default, deserialize_with = "present")] setting_prefix: Option<Setting>,
});
args!(GetServedConfig {
    swimlane: Swimlane,
    application: App,
    tenant: Tenant,
    #[serde(default, deserialize_with = "present")] channel: Option<Channel>,
    file: Nfs,
});
args!(Compare {
    left: CompareRefArg,
    right: CompareRefArg,
    file: Logical,
    #[serde(default, deserialize_with = "present")] channel: Option<Channel>,
    #[serde(default, deserialize_with = "present")] detail: Option<DetailArg>,
});
args!(CompareTree {
    left: CompareRefArg,
    right: CompareRefArg,
    #[serde(default, deserialize_with = "present")] prefix: Option<Nfs>,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(CompareGrid {
    file: Logical,
    #[serde(deserialize_with = "swimlanes_1_64")] swimlanes: Vec<Swimlane>,
    #[serde(default, deserialize_with = "present")] channel: Option<Channel>,
    #[serde(default, deserialize_with = "present")] only_differing: Option<bool>,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(SearchSettings {
    q: ShortNonEmpty,
    #[serde(default, deserialize_with = "swimlanes_0_64")] swimlanes: Option<Vec<Swimlane>>,
    #[serde(default, deserialize_with = "present")] file: Option<Logical>,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(SearchText {
    pattern: PatternArg,
    #[serde(default, deserialize_with = "present")] regex: Option<bool>,
    #[serde(default, deserialize_with = "present")] case_sensitive: Option<bool>,
    #[serde(default, deserialize_with = "swimlanes_0_64")] swimlanes: Option<Vec<Swimlane>>,
    #[serde(default, deserialize_with = "present")] path_glob: Option<Short>,
    #[serde(default, deserialize_with = "present")] max_total: Option<Bounded<1, 500>>,
    #[serde(default, deserialize_with = "present")] max_per_file: Option<Bounded<1, 100>>,
});
args!(FindInconsistencies {
    #[serde(default, deserialize_with = "present")] swimlane: Option<Swimlane>,
    #[serde(default, deserialize_with = "kinds_0_12")] kinds: Option<Vec<FindingKind>>,
    #[serde(default, deserialize_with = "present")] min_severity: Option<Severity>,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(GetDrift {
    swimlane: Swimlane,
    #[serde(default, deserialize_with = "present")] state: Option<DriftState>,
    #[serde(default, deserialize_with = "present")] prefix: Option<Nfs>,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(GetPendingRestarts {
    swimlane: Swimlane,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(GetHistory {
    swimlane: Swimlane,
    path: Nfs,
    #[serde(default, deserialize_with = "present")] cursor: Option<CursorArg>,
    #[serde(default, deserialize_with = "present")] limit: Option<Limit200>,
});
args!(SearchDocs {
    q: PatternArgQuery,
    #[serde(default, deserialize_with = "present")] limit: Option<Bounded<1, 50>>,
});
args!(ReadDoc {
    path: Doc,
    #[serde(default, deserialize_with = "present")] lines: Option<LinesArg>,
});
args!(GrepDocs {
    pattern: PatternArg,
    #[serde(default, deserialize_with = "present")] regex: Option<bool>,
    #[serde(default, deserialize_with = "present")] context: Option<Bounded<0, 5>>,
    #[serde(default, deserialize_with = "present")] path_prefix: Option<Doc>,
    #[serde(default, deserialize_with = "present")] limit: Option<Bounded<1, 100>>,
});
args!(GetDocsForFile {
    file: Logical,
    #[serde(default, deserialize_with = "present")] setting: Option<Setting>,
    #[serde(default, deserialize_with = "present")] limit: Option<Bounded<1, 50>>,
});
args!(GetFeatureStatus {
    #[serde(default, deserialize_with = "present")] flag: Option<Short>,
    #[serde(default, deserialize_with = "present")] file: Option<Logical>,
    #[serde(default, deserialize_with = "present")] channel: Option<Channel>,
    #[serde(default, deserialize_with = "swimlanes_0_64")] swimlanes: Option<Vec<Swimlane>>,
});
args!(ProposeChange {
    swimlane: Swimlane,
    path: Nfs,
    expected: ExpectedArg,
    #[serde(deserialize_with = "content_text")] content: Opaque,
    #[serde(default, deserialize_with = "present")] change_eol: Option<ChangeEol>,
});
args!(ApplyChange { draft_id: Draft });
args!(UploadFile {
    swimlane: Swimlane,
    path: Nfs,
    expected: ExpectedArg,
    #[serde(deserialize_with = "content_text")]
    content: Opaque,
});
args!(DeleteFile {
    swimlane: Swimlane,
    path: Nfs,
    expected_hash: HashArg,
});
args!(RaisePr {
    swimlane: Swimlane,
    #[serde(deserialize_with = "paths_1_100")] paths: Vec<Nfs>,
    title: Short,
    #[serde(default, deserialize_with = "pr_body")] body: Option<Opaque>,
    #[serde(default, deserialize_with = "present")] target: Option<PrTargetArg>,
});
args!(RestartService {
    swimlane: Swimlane,
    service: ServiceArg,
});
args!(NotifyServices {
    swimlane: Swimlane,
    #[serde(deserialize_with = "paths_1_100")] paths: Vec<Nfs>,
});

string_arg!(
    /// The query of a docs search: 1 to 512 bytes.
    PatternArgQuery(Opaque), max MAX_PATTERN_BYTES, |s: &str| (!s.is_empty()).then(|| Opaque(s.to_owned()))
);

/// The parsed arguments of a call, one variant per tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolArgs {
    ListSwimlanes(ListSwimlanes),
    GetSwimlane(GetSwimlane),
    ListFiles(ListFiles),
    ReadFile(ReadFile),
    GetEffectiveConfig(GetEffectiveConfig),
    GetServedConfig(GetServedConfig),
    Compare(Compare),
    CompareTree(CompareTree),
    CompareGrid(CompareGrid),
    SearchSettings(SearchSettings),
    SearchText(SearchText),
    FindInconsistencies(FindInconsistencies),
    GetDrift(GetDrift),
    GetPendingRestarts(GetPendingRestarts),
    GetHistory(GetHistory),
    SearchDocs(SearchDocs),
    ReadDoc(ReadDoc),
    GrepDocs(GrepDocs),
    GetDocsForFile(GetDocsForFile),
    GetFeatureStatus(GetFeatureStatus),
    ProposeChange(ProposeChange),
    ApplyChange(ApplyChange),
    UploadFile(UploadFile),
    DeleteFile(DeleteFile),
    RaisePr(RaisePr),
    RestartService(RestartService),
    NotifyServices(NotifyServices),
}

/// The names of the properties a tool declares, for refusing an unknown one before any parser runs, and for telling a missing
/// required argument by name. A closed set of our own strings: a name the caller sent is never echoed.
#[must_use]
pub fn declared(tool: ToolName) -> (&'static [&'static str], &'static [&'static str]) {
    use ToolName as T;
    match tool {
        T::ListSwimlanes => (&["cursor", "limit"], &[]),
        T::GetSwimlane => (&["swimlane"], &["swimlane"]),
        T::ListFiles => (&["swimlane", "prefix", "drift", "cursor", "limit"], &["swimlane"]),
        T::ReadFile => (
            &[
                "swimlane",
                "path",
                "source",
                "repo",
                "ref",
                "lines",
                "setting_path",
            ],
            &["swimlane", "path"],
        ),
        T::GetEffectiveConfig => (
            &["swimlane", "file", "tenant", "channel", "setting_prefix"],
            &["swimlane", "file", "tenant"],
        ),
        T::GetServedConfig => (
            &["swimlane", "application", "tenant", "channel", "file"],
            &["swimlane", "application", "tenant", "file"],
        ),
        T::Compare => (
            &["left", "right", "file", "channel", "detail"],
            &["left", "right", "file"],
        ),
        T::CompareTree => (
            &["left", "right", "prefix", "cursor", "limit"],
            &["left", "right"],
        ),
        T::CompareGrid => (
            &[
                "file",
                "swimlanes",
                "channel",
                "only_differing",
                "cursor",
                "limit",
            ],
            &["file", "swimlanes"],
        ),
        T::SearchSettings => (&["q", "swimlanes", "file", "cursor", "limit"], &["q"]),
        T::SearchText => (
            &[
                "pattern",
                "regex",
                "case_sensitive",
                "swimlanes",
                "path_glob",
                "max_total",
                "max_per_file",
            ],
            &["pattern"],
        ),
        T::FindInconsistencies => (&["swimlane", "kinds", "min_severity", "cursor", "limit"], &[]),
        T::GetDrift => (&["swimlane", "state", "prefix", "cursor", "limit"], &["swimlane"]),
        T::GetPendingRestarts => (&["swimlane", "cursor", "limit"], &["swimlane"]),
        T::GetHistory => (&["swimlane", "path", "cursor", "limit"], &["swimlane", "path"]),
        T::SearchDocs => (&["q", "limit"], &["q"]),
        T::ReadDoc => (&["path", "lines"], &["path"]),
        T::GrepDocs => (
            &["pattern", "regex", "context", "path_prefix", "limit"],
            &["pattern"],
        ),
        T::GetDocsForFile => (&["file", "setting", "limit"], &["file"]),
        T::GetFeatureStatus => (&["flag", "file", "channel", "swimlanes"], &[]),
        T::ProposeChange => (
            &["swimlane", "path", "expected", "content", "change_eol"],
            &["swimlane", "path", "expected", "content"],
        ),
        T::ApplyChange => (&["draft_id"], &["draft_id"]),
        T::UploadFile => (
            &["swimlane", "path", "expected", "content"],
            &["swimlane", "path", "expected", "content"],
        ),
        T::DeleteFile => (
            &["swimlane", "path", "expected_hash"],
            &["swimlane", "path", "expected_hash"],
        ),
        T::RaisePr => (
            &["swimlane", "paths", "title", "body", "target"],
            &["swimlane", "paths", "title"],
        ),
        T::RestartService => (&["swimlane", "service"], &["swimlane", "service"]),
        T::NotifyServices => (&["swimlane", "paths"], &["swimlane", "paths"]),
    }
}

fn from_map<T: DeserializeOwned>(arguments: Map<String, Value>) -> Result<T, serde_json::Error> {
    serde_json::from_value(Value::Object(arguments))
}

/// Parse the arguments of a call.
///
/// # Errors
///
/// [`ArgError::Missing`] naming one of the tool's own required arguments; [`ArgError::TooLarge`] for content over 2 MiB;
/// [`ArgError::Invalid`] for everything else (an unknown argument, a wrong type, a value out of range or not valid).
pub fn parse_args(tool: ToolName, arguments: Option<Map<String, Value>>) -> Result<ToolArgs, ArgError> {
    use ToolName as T;
    let arguments = arguments.unwrap_or_default();
    let (known, required) = declared(tool);
    if arguments.keys().any(|k| !known.contains(&k.as_str())) {
        return Err(ArgError::Invalid);
    }
    if let Some(missing) = required.iter().find(|r| !arguments.contains_key(**r)) {
        return Err(ArgError::Missing(missing));
    }
    // The size of content is checked on the value, before any parser sees it, so the answer is the same whatever else is wrong.
    if let Some(Value::String(content)) = arguments.get("content")
        && content.len() > MAX_CONTENT_BYTES
    {
        return Err(ArgError::TooLarge);
    }
    macro_rules! parse {
        ($variant:ident) => {
            from_map(arguments)
                .map(ToolArgs::$variant)
                .map_err(|_| ArgError::Invalid)
        };
    }
    match tool {
        T::ListSwimlanes => parse!(ListSwimlanes),
        T::GetSwimlane => parse!(GetSwimlane),
        T::ListFiles => parse!(ListFiles),
        T::ReadFile => parse!(ReadFile),
        T::GetEffectiveConfig => parse!(GetEffectiveConfig),
        T::GetServedConfig => parse!(GetServedConfig),
        T::Compare => parse!(Compare),
        T::CompareTree => parse!(CompareTree),
        T::CompareGrid => parse!(CompareGrid),
        T::SearchSettings => parse!(SearchSettings),
        T::SearchText => parse!(SearchText),
        T::FindInconsistencies => parse!(FindInconsistencies),
        T::GetDrift => parse!(GetDrift),
        T::GetPendingRestarts => parse!(GetPendingRestarts),
        T::GetHistory => parse!(GetHistory),
        T::SearchDocs => parse!(SearchDocs),
        T::ReadDoc => parse!(ReadDoc),
        T::GrepDocs => parse!(GrepDocs),
        T::GetDocsForFile => parse!(GetDocsForFile),
        T::GetFeatureStatus => parse!(GetFeatureStatus),
        T::ProposeChange => parse!(ProposeChange),
        T::ApplyChange => parse!(ApplyChange),
        T::UploadFile => parse!(UploadFile),
        T::DeleteFile => parse!(DeleteFile),
        T::RaisePr => parse!(RaisePr),
        T::RestartService => parse!(RestartService),
        T::NotifyServices => parse!(NotifyServices),
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ToolArgs {
    /// The tool these arguments are for.
    #[must_use]
    pub fn tool(&self) -> ToolName {
        match self {
            Self::ListSwimlanes(_) => ToolName::ListSwimlanes,
            Self::GetSwimlane(_) => ToolName::GetSwimlane,
            Self::ListFiles(_) => ToolName::ListFiles,
            Self::ReadFile(_) => ToolName::ReadFile,
            Self::GetEffectiveConfig(_) => ToolName::GetEffectiveConfig,
            Self::GetServedConfig(_) => ToolName::GetServedConfig,
            Self::Compare(_) => ToolName::Compare,
            Self::CompareTree(_) => ToolName::CompareTree,
            Self::CompareGrid(_) => ToolName::CompareGrid,
            Self::SearchSettings(_) => ToolName::SearchSettings,
            Self::SearchText(_) => ToolName::SearchText,
            Self::FindInconsistencies(_) => ToolName::FindInconsistencies,
            Self::GetDrift(_) => ToolName::GetDrift,
            Self::GetPendingRestarts(_) => ToolName::GetPendingRestarts,
            Self::GetHistory(_) => ToolName::GetHistory,
            Self::SearchDocs(_) => ToolName::SearchDocs,
            Self::ReadDoc(_) => ToolName::ReadDoc,
            Self::GrepDocs(_) => ToolName::GrepDocs,
            Self::GetDocsForFile(_) => ToolName::GetDocsForFile,
            Self::GetFeatureStatus(_) => ToolName::GetFeatureStatus,
            Self::ProposeChange(_) => ToolName::ProposeChange,
            Self::ApplyChange(_) => ToolName::ApplyChange,
            Self::UploadFile(_) => ToolName::UploadFile,
            Self::DeleteFile(_) => ToolName::DeleteFile,
            Self::RaisePr(_) => ToolName::RaisePr,
            Self::RestartService(_) => ToolName::RestartService,
            Self::NotifyServices(_) => ToolName::NotifyServices,
        }
    }
}
