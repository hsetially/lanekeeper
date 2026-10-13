//! The two GitHub payloads the hub reads, `push` and `pull_request` (S11, S22).
//!
//! Only the fields the hub uses are interpreted, and each is typed before anything else sees it: refs are [`GitRef`]s under
//! `refs/heads/` or `refs/tags/`, commit ids are [`CommitId`]s, repository names are [`RepoFullName`]s. Unknown fields are
//! ignored (GitHub adds some), but a missing field, a field of the wrong type and a duplicated key are errors. The body was
//! already limited in size and checked against its HMAC before it gets here; JSON nesting is limited by the parser (128).
//!
//! An error says which field was wrong and never carries the input.

use std::fmt;

use domain::{CommitId, GitRef};
use serde::Deserialize;

/// The longest `owner/name` GitHub allows (an owner has at most 39 characters, a repository at most 100).
const MAX_OWNER: usize = 39;
const MAX_NAME: usize = 100;

/// Why a payload was refused. Names a field, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PayloadError {
    /// Not JSON, not an object, a field of the wrong type, a missing field or a duplicated key.
    #[error("the payload is not valid")]
    Malformed,
    #[error("the payload field {0} is not valid")]
    BadField(&'static str),
}

/// `owner/name` of a GitHub repository, as it appears in `repository.full_name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoFullName(String);

impl RepoFullName {
    /// `owner/name`, each part of `[A-Za-z0-9._-]`, at most 39 and 100 characters.
    pub fn parse(s: &str) -> Result<Self, PayloadError> {
        let bad = || PayloadError::BadField("repository");
        let (owner, name) = s.split_once('/').ok_or_else(bad)?;
        let part = |p: &str, max: usize| {
            !p.is_empty()
                && p.len() <= max
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        };
        if part(owner, MAX_OWNER) && part(name, MAX_NAME) {
            Ok(Self(s.to_owned()))
        } else {
            Err(bad())
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// GitHub names are case-insensitive.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl fmt::Display for RepoFullName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a push moved.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PushedRef {
    Branch(GitRef),
    Tag(GitRef),
}

/// A `push` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushEvent {
    pub repository: RepoFullName,
    pub target: PushedRef,
    /// `None` when the ref is new (all-zero id).
    pub before: Option<CommitId>,
    pub after: CommitId,
    /// The ref was deleted: `deleted` is set or `after` is the all-zero id.
    pub deleted: bool,
}

#[derive(Deserialize)]
struct RawRepository {
    full_name: String,
}

#[derive(Deserialize)]
struct RawPush {
    #[serde(rename = "ref")]
    git_ref: String,
    before: String,
    after: String,
    #[serde(default)]
    deleted: bool,
    repository: RawRepository,
}

fn is_zero(id: &CommitId) -> bool {
    id.as_str().bytes().all(|c| c == b'0')
}

fn commit(field: &'static str, s: &str) -> Result<CommitId, PayloadError> {
    CommitId::parse(s).map_err(|_| PayloadError::BadField(field))
}

impl PushEvent {
    pub fn parse(body: &[u8]) -> Result<Self, PayloadError> {
        let raw: RawPush = serde_json::from_slice(body).map_err(|_| PayloadError::Malformed)?;
        let repository = RepoFullName::parse(&raw.repository.full_name)?;
        let bad_ref = || PayloadError::BadField("ref");
        let target = if let Some(name) = raw.git_ref.strip_prefix("refs/heads/") {
            PushedRef::Branch(GitRef::parse(name).map_err(|_| bad_ref())?)
        } else if let Some(name) = raw.git_ref.strip_prefix("refs/tags/") {
            PushedRef::Tag(GitRef::parse(name).map_err(|_| bad_ref())?)
        } else {
            return Err(bad_ref());
        };
        // A name that is a bare commit id is a valid `GitRef` but never a branch or tag we serve.
        let (PushedRef::Branch(n) | PushedRef::Tag(n)) = &target;
        if n.as_str().len() >= 40 && n.as_str().bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad_ref());
        }
        let before = commit("before", &raw.before)?;
        let after = commit("after", &raw.after)?;
        let deleted = raw.deleted || is_zero(&after);
        Ok(Self {
            repository,
            target,
            before: (!is_zero(&before)).then_some(before),
            after,
            deleted,
        })
    }
}

/// What happened to a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrAction {
    Opened,
    Closed,
    Reopened,
    /// Any other action (edited, labeled, synchronize...). The hub does not act on it.
    Other,
}

/// A `pull_request` event: which pull request of which repository, what happened and whether it was merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestEvent {
    pub repository: RepoFullName,
    pub number: u32,
    pub action: PrAction,
    pub merged: bool,
}

#[derive(Deserialize)]
struct RawPullRequest {
    #[serde(default)]
    merged: bool,
}

#[derive(Deserialize)]
struct RawPrEvent {
    action: String,
    number: i64,
    pull_request: RawPullRequest,
    repository: RawRepository,
}

impl PullRequestEvent {
    pub fn parse(body: &[u8]) -> Result<Self, PayloadError> {
        let raw: RawPrEvent = serde_json::from_slice(body).map_err(|_| PayloadError::Malformed)?;
        let repository = RepoFullName::parse(&raw.repository.full_name)?;
        // `pr_links.pr_number` is a positive `integer`.
        let number = u32::try_from(raw.number)
            .ok()
            .filter(|n| (1..=i32::MAX as u32).contains(n))
            .ok_or(PayloadError::BadField("number"))?;
        let action = match raw.action.as_str() {
            "opened" => PrAction::Opened,
            "closed" => PrAction::Closed,
            "reopened" => PrAction::Reopened,
            _ => PrAction::Other,
        };
        Ok(Self {
            repository,
            number,
            action,
            merged: raw.pull_request.merged,
        })
    }
}
