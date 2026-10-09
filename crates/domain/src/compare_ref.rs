//! `CompareRef`: the reference grammar shared by REST and MCP (`docs/domain-model.md`, "Compare references").
//!
//! ```text
//! nfs:<swimlane>   baseline:<swimlane>   effective:<swimlane>
//! git:base@<ref>   git:tenant@<ref>
//! ```
//! `<ref>` is a branch or tag name following `git check-ref-format`, or a 40/64-hex commit id (Q15).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{IdError, SwimlaneId};

/// Why a Git ref was rejected. Never carries the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GitRefError {
    #[error("ref is empty")]
    Empty,
    #[error("ref is too long")]
    TooLong,
    #[error("ref is not a valid Git ref name")]
    Invalid,
}

/// Maximum ref length in bytes.
pub const MAX_GIT_REF_BYTES: usize = 255;

/// A branch, tag or commit id, validated with the rules of `git check-ref-format`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GitRef(String);

fn is_commit_like(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|c| c.is_ascii_hexdigit())
}

impl GitRef {
    pub fn parse(s: &str) -> Result<Self, GitRefError> {
        if s.is_empty() {
            return Err(GitRefError::Empty);
        }
        if s.len() > MAX_GIT_REF_BYTES {
            return Err(GitRefError::TooLong);
        }
        if is_commit_like(s) {
            return Ok(Self(s.to_ascii_lowercase()));
        }
        let bad_char =
            |c: char| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\');
        if s.chars().any(bad_char)
            || s == "@"
            || s.contains("..")
            || s.contains("@{")
            || s.starts_with('-')
            || s.starts_with('/')
            || s.ends_with('/')
            || s.ends_with('.')
            || s.contains("//")
        {
            return Err(GitRefError::Invalid);
        }
        if s.split('/')
            .any(|c| c.starts_with('.') || c.to_ascii_lowercase().ends_with(".lock"))
        {
            return Err(GitRefError::Invalid);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for GitRef {
    type Err = GitRefError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for GitRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for GitRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Why a compare reference was rejected. Never carries the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CompareRefError {
    #[error("compare reference has an unknown or missing kind")]
    UnknownKind,
    #[error("compare reference has an invalid swimlane")]
    BadSwimlane(#[source] IdError),
    #[error("compare reference has an invalid Git ref")]
    BadGitRef(#[source] GitRefError),
}

/// One side of a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CompareRef {
    /// Current NFS content of a swimlane.
    Nfs(SwimlaneId),
    /// The adopted baseline of a swimlane.
    Baseline(SwimlaneId),
    /// The effective (rendered) config of a swimlane.
    Effective(SwimlaneId),
    /// A ref in the base repo.
    GitBase(GitRef),
    /// A ref in the tenant repo.
    GitTenant(GitRef),
}

impl FromStr for CompareRef {
    type Err = CompareRefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (kind, rest) = s.split_once(':').ok_or(CompareRefError::UnknownKind)?;
        let lane = |r: &str| SwimlaneId::parse(r).map_err(CompareRefError::BadSwimlane);
        match kind {
            "nfs" => Ok(Self::Nfs(lane(rest)?)),
            "baseline" => Ok(Self::Baseline(lane(rest)?)),
            "effective" => Ok(Self::Effective(lane(rest)?)),
            "git" => {
                let (repo, git_ref) = rest.split_once('@').ok_or(CompareRefError::UnknownKind)?;
                let git_ref = GitRef::parse(git_ref).map_err(CompareRefError::BadGitRef)?;
                match repo {
                    "base" => Ok(Self::GitBase(git_ref)),
                    "tenant" => Ok(Self::GitTenant(git_ref)),
                    _ => Err(CompareRefError::UnknownKind),
                }
            }
            _ => Err(CompareRefError::UnknownKind),
        }
    }
}

impl fmt::Display for CompareRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nfs(s) => write!(f, "nfs:{s}"),
            Self::Baseline(s) => write!(f, "baseline:{s}"),
            Self::Effective(s) => write!(f, "effective:{s}"),
            Self::GitBase(r) => write!(f, "git:base@{r}"),
            Self::GitTenant(r) => write!(f, "git:tenant@{r}"),
        }
    }
}

impl Serialize for CompareRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CompareRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
