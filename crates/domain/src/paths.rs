//! Path newtypes. A path is untrusted until it has been through one of these parsers (S11).
//!
//! Rules for [`NfsPath`] and [`RepoPath`] (plan Q13):
//! - relative: a leading `/` is rejected;
//! - `//` collapses, `.` components and a trailing `/` are dropped;
//! - any `..` component, NUL or other control character, and backslash are rejected;
//! - at most 1,024 bytes after normalisation, and at most 255 bytes per component;
//! - names are opaque UTF-8, with no Unicode normalisation.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum length of a normalised path in bytes.
pub const MAX_PATH_BYTES: usize = 1024;
/// Maximum length of one path component in bytes (Linux `NAME_MAX`).
pub const MAX_COMPONENT_BYTES: usize = 255;
/// Raw inputs longer than this are rejected before any work is done on them.
const MAX_RAW_BYTES: usize = 8192;

/// Why a path was rejected. Never carries the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path is absolute")]
    Absolute,
    #[error("path contains a parent-directory component")]
    Traversal,
    #[error("path contains a control character")]
    ControlChar,
    #[error("path contains a backslash")]
    Backslash,
    #[error("path is too long")]
    TooLong,
    #[error("path has a component that is too long")]
    ComponentTooLong,
    #[error("path points at Git metadata")]
    GitMetadata,
    #[error("path has the wrong prefix")]
    BadPrefix,
    #[error("line range is invalid")]
    BadRange,
}

fn normalise(input: &str, allow_empty: bool, forbid_git: bool) -> Result<String, PathError> {
    if input.len() > MAX_RAW_BYTES {
        return Err(PathError::TooLong);
    }
    if input.chars().any(char::is_control) {
        return Err(PathError::ControlChar);
    }
    if input.contains('\\') {
        return Err(PathError::Backslash);
    }
    if input.starts_with('/') {
        return Err(PathError::Absolute);
    }
    let mut out = String::with_capacity(input.len());
    for comp in input.split('/') {
        match comp {
            "" | "." => {}
            ".." => return Err(PathError::Traversal),
            c => {
                if c.len() > MAX_COMPONENT_BYTES {
                    return Err(PathError::ComponentTooLong);
                }
                if forbid_git && c.eq_ignore_ascii_case(".git") {
                    return Err(PathError::GitMetadata);
                }
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(c);
            }
        }
    }
    if out.is_empty() && !allow_empty {
        return Err(PathError::Empty);
    }
    if out.len() > MAX_PATH_BYTES {
        return Err(PathError::TooLong);
    }
    Ok(out)
}

/// Implements the accessors and conversions shared by the relative path types.
macro_rules! rel_path {
    ($name:ident) => {
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// The path components, outermost first.
            pub fn components(&self) -> impl Iterator<Item = &str> {
                self.0.split('/').filter(|c| !c.is_empty())
            }

            /// The last component (empty for the root).
            pub fn file_name(&self) -> &str {
                self.0.rsplit('/').next().unwrap_or("")
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = PathError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

/// A path relative to a swimlane's NFS root. The only path type that reaches the agent or sentinel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NfsPath(String);

impl NfsPath {
    /// Everything before the last component, as a prefix string (empty at the top level).
    pub(crate) fn dir_str(&self) -> &str {
        self.0.rsplit_once('/').map_or("", |(d, _)| d)
    }

    /// Parse a file or directory path. The result is never empty; use [`NfsPath::parse_prefix`] for a tree prefix.
    pub fn parse(s: &str) -> Result<Self, PathError> {
        normalise(s, false, false).map(Self)
    }

    /// Parse a tree prefix, which may be the root (`""`, `.` or `./`).
    pub fn parse_prefix(s: &str) -> Result<Self, PathError> {
        normalise(s, true, false).map(Self)
    }

    /// The NFS root, used as the prefix of a whole-tree query.
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The containing directory. The parent of a top-level file is the root.
    #[must_use]
    pub fn parent(&self) -> NfsPath {
        Self(self.dir_str().to_owned())
    }

    /// Append a relative path.
    pub fn join(&self, rel: &NfsPath) -> Result<NfsPath, PathError> {
        if self.is_root() {
            return Ok(rel.clone());
        }
        if rel.is_root() {
            return Ok(self.clone());
        }
        let joined = format!("{}/{}", self.0, rel.0);
        if joined.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong);
        }
        Ok(Self(joined))
    }

    /// True when `self` is `prefix` or lies under it.
    pub fn starts_with(&self, prefix: &NfsPath) -> bool {
        prefix.is_root()
            || self.0 == prefix.0
            || self
                .0
                .strip_prefix(prefix.0.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}
rel_path!(NfsPath);

/// A path inside a Git repository (`config/...` or `data/config/...`). `.git` components are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoPath(String);

impl RepoPath {
    pub fn parse(s: &str) -> Result<Self, PathError> {
        normalise(s, false, true).map(Self)
    }

    /// Wrap a compile-time constant that is known to be valid. Not for runtime input.
    pub(crate) fn trusted(s: &'static str) -> Self {
        Self(s.to_owned())
    }

    /// The part of this path below `root`, or `None` when the path is not under it (or is the root itself).
    pub(crate) fn below(&self, root: &RepoPath) -> Option<&str> {
        let rest = self.0.strip_prefix(root.0.as_str())?.strip_prefix('/')?;
        (!rest.is_empty()).then_some(rest)
    }

    /// `root/rel`, validated again so the combined length is checked.
    pub(crate) fn under(root: &RepoPath, rel: &str) -> Result<RepoPath, PathError> {
        RepoPath::parse(&format!("{}/{}", root.0, rel))
    }
}
rel_path!(RepoPath);

/// An NFS path with the tenant suffix removed: `tx-infinity-core-sit1.yml` and `tx-infinity-core.yml`
/// are the same logical file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LogicalFile(NfsPath);

impl LogicalFile {
    pub fn parse(s: &str) -> Result<Self, PathError> {
        NfsPath::parse(s).map(Self)
    }

    /// Wrap a path that the caller knows is already suffix-free (the mapping code does).
    pub fn from_nfs(p: NfsPath) -> Result<Self, PathError> {
        if p.is_root() {
            return Err(PathError::Empty);
        }
        Ok(Self(p))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn as_path(&self) -> &NfsPath {
        &self.0
    }
}

impl fmt::Display for LogicalFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogicalFile {
    type Err = PathError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for LogicalFile {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LogicalFile {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A flattened setting path such as `a.b.c`, `a.b[0]` or `a["x.y"]`, optionally prefixed `#<doc>/`.
/// Bounded and printable; its structure is the engine's concern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SettingPath(String);

impl SettingPath {
    pub fn parse(s: &str) -> Result<Self, PathError> {
        if s.is_empty() {
            return Err(PathError::Empty);
        }
        if s.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong);
        }
        if s.chars().any(char::is_control) {
            return Err(PathError::ControlChar);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SettingPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SettingPath {
    type Err = PathError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for SettingPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SettingPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A path in the virtual docs filesystem: `/git/<branch>/<path>` or `/uploads/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocPath(String);

impl DocPath {
    pub fn parse(s: &str) -> Result<Self, PathError> {
        for prefix in ["/git/", "/uploads/"] {
            if let Some(rest) = s.strip_prefix(prefix) {
                let rel = normalise(rest.trim_start_matches('/'), false, true)?;
                return Ok(Self(format!("{prefix}{rel}")));
            }
        }
        Err(PathError::BadPrefix)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DocPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for DocPath {
    type Err = PathError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for DocPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DocPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A 1-based inclusive line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct LineRange {
    start: u32,
    end: u32,
}

impl LineRange {
    pub fn new(start: u32, end: u32) -> Result<Self, PathError> {
        if start == 0 || end < start {
            return Err(PathError::BadRange);
        }
        Ok(Self { start, end })
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn end(&self) -> u32 {
        self.end
    }
}

impl<'de> Deserialize<'de> for LineRange {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            start: u32,
            end: u32,
        }
        let raw = Raw::deserialize(d)?;
        Self::new(raw.start, raw.end).map_err(serde::de::Error::custom)
    }
}
