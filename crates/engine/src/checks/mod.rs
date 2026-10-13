//! Consistency checks C1 to C8 (T6, `docs/domain-model.md` "Consistency checks", D19, D20, D84), and C9 to C11 (T10).
//!
//! A check looks at the files of one swimlane and finds a fact: a tenant copy that has fallen behind (C1), a copy that is
//! no copy at all (C2), tenants that are not alike (C3), a file nobody reads (C4), drift (C5), a host of the wrong
//! environment (C6), a sync that replaced a change (C7), a key written twice (C8). It returns a [`CheckResult`]: what
//! the finding is about, a plain sentence written here and not by an AI layer (D33), the lines to point at, and the facts
//! behind the sentence in [`CheckDetail`]. Severity (T9), the finding id, the swimlane and the time belong to the caller,
//! who turns a result into a `domain::Finding`.
//!
//! C9 to C11 belong to the config-server's resolution model (D82 to D87) and take their own inputs, because they are about the
//! tree and the config-server and not about one logical file: C9 ([`c9_ambiguous_file_names`]) reads the swimlane's files and
//! the channel list, C10 ([`c10_unresolved_placeholders`]) the resource files' text, the tenants' property views and the
//! environment variable names, C11 ([`c11_config_server_restart_required`]) the folders' first-seen times, `channels.yml` and the
//! config-server's start time.
//!
//! # Inputs
//!
//! The engine reads no repository and no file. Everything a check needs comes in [`CheckInput`]:
//!
//! - the swimlane's files, each with its path, what the path is ([`FileFacts`]), its class, its hash and, when the file is
//!   structured and parsed, its settings and the warnings of the parse ([`Structured`]);
//! - for C1 in `whole_file` mode, the history the engine cannot read: the time of the tenant file's last commit and the
//!   versions of the base file from then on ([`BaseHistory`], plan decision A13). The base commits are matched to the
//!   tenant commit by commit time, and the finding says so;
//! - for C5 and C7, the drift result and the observation of the last sync ([`Overwrite`]);
//! - for C6, the swimlane's environment tier and the admin's host rules ([`HostRules`], decision A14).
//!
//! # Output
//!
//! Sorted by check, path and message, so the list does not depend on the order of the input. Every list inside a result
//! (commits, settings, hosts, keys, locations) is capped at [`Limits::locations_per_finding`] and carries the full count.
//!
//! No result holds a setting value: messages and details name files, tenants, setting paths, lines, hashes and host names,
//! never the text of a value (S13, S21).

mod c1;
mod c10;
mod c11;
mod c2;
mod c3;
mod c4;
mod c5;
mod c6;
mod c7;
mod c8;
mod c9;

use std::collections::BTreeMap;

use bytes::Bytes;
use domain::{
    AttributionSource, ChangeKind, CommitId, ContentHash, DriftState, FileClass, FileKind, FindingKind,
    LogicalFile, MergeMode, NfsPath, SettingLocation, SettingPath, ShortText, TenantId, TenantSet, Timestamp,
};

pub use self::c1::c1_missed_base_changes;
pub use self::c2::c2_redundant_tenant_file;
pub use self::c3::c3_uneven_tenant_files;
pub use self::c4::c4_orphan_file;
pub use self::c5::c5_drift;
pub use self::c6::{HostMatcher, HostRule, HostRules, c6_environment_host_mismatch};
pub use self::c7::c7_overwritten_nfs_change;
pub use self::c8::c8_duplicate_keys;
pub use self::c9::c9_ambiguous_file_names;
pub use self::c10::{EnvNames, PlaceholderFile, TenantView, UnresolvedKey, c10_unresolved_placeholders};
pub use self::c11::{
    ChannelsYml, ConfigServerFacts, FolderSeen, RestartReason, c11_config_server_restart_required,
};
use crate::bytes::decode_text;
use crate::drift::Drift;
use crate::limits::Limits;
use crate::parse::{FlatSetting, ParseWarning, Syntax, flatten, parse_decoded};
use crate::paths::FileFacts;

/// The longest message the checks write: a `ShortText` is at most 256 bytes.
const MESSAGE_MAX_BYTES: usize = 256;

/// The settings of a structured file that parsed, and the warnings of the parse.
#[derive(Debug, Clone, Copy)]
pub struct Structured<'a> {
    /// `flatten` of the parsed file.
    pub settings: &'a [FlatSetting],
    /// `Parsed::warnings`: C8 reads the duplicate keys from them.
    pub parse_warnings: &'a [ParseWarning],
    /// `Parsed::warnings_dropped`: warnings the parser counted and did not keep.
    pub parse_warnings_dropped: usize,
}

/// A version of the base file in the base repository's default branch.
#[derive(Clone, PartialEq, Eq)]
pub struct BaseVersion {
    pub commit: CommitId,
    /// The commit time.
    pub time: Timestamp,
    /// The content at that commit; `None` if the commit deleted the file.
    pub content: Option<Bytes>,
}

impl std::fmt::Debug for BaseVersion {
    // The content may hold secrets: only its size is printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BaseVersion")
            .field("commit", &self.commit)
            .field("time", &self.time)
            .field("content_bytes", &self.content.as_ref().map(Bytes::len))
            .finish()
    }
}

/// What C1 needs to know about the base file's history (plan decision A13).
#[derive(Debug, Clone, Copy)]
pub struct BaseHistory<'a> {
    /// The time of the tenant file's last commit, on the tenant's branch.
    pub tenant_commit_time: Timestamp,
    /// The base file as it was when the tenant file was last updated: the newest version at or before that time. `None`
    /// if there is none, or the history was not available.
    pub at_tenant_commit: Option<&'a BaseVersion>,
    /// The versions of the base file after that time, in any order. A version not after `tenant_commit_time` is ignored.
    pub since: &'a [BaseVersion],
}

/// The observation behind C7: what was on NFS before the last change, and what made the change (plan decision A13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overwrite {
    /// The drift state before the change.
    pub previous_state: DriftState,
    /// The hash of the file on NFS before the change; `None` if it was missing. This is the version the finding links to.
    pub previous_nfs: Option<ContentHash>,
    /// How the change was attributed (D73). Only a sync job overwrites.
    pub source: AttributionSource,
}

/// One file of the swimlane, as the checks see it.
#[derive(Debug, Clone, Copy)]
pub struct CheckFile<'a> {
    /// Where the file is on NFS.
    pub path: &'a NfsPath,
    /// What the path is: kind, logical file, tenant. Made by `classify_path` against *all* deployable branches, so that a
    /// suffix of a branch that the swimlane does not deploy is still read as a suffix (C4).
    pub facts: &'a FileFacts,
    pub class: FileClass,
    /// The hash for comparing (`normalised_hash` of the content on NFS).
    pub hash: ContentHash,
    /// How this logical file is merged: C1 reads a tenant file as a copy (`whole_file`) or as overrides (`spring_merge`).
    pub merge_mode: MergeMode,
    /// The parsed file, if it is structured and parsed.
    pub structured: Option<Structured<'a>>,
    /// `spring_merge` only: the settings of the root `application.*` file that is the lowest layer of this file (C1).
    pub root_settings: Option<&'a [FlatSetting]>,
    /// The result of `drift_state` for this path (C5).
    pub drift: Option<Drift>,
    /// The last change, if a sync job made it (C7).
    pub overwrite: Option<Overwrite>,
    /// A tenant file in `whole_file` mode: the base file's history (C1).
    pub base_history: Option<BaseHistory<'a>>,
}

/// Everything `checks_c1_to_c8` needs for one swimlane.
#[derive(Debug, Clone, Copy)]
pub struct CheckInput<'a> {
    /// The tenants the swimlane deploys (Q34): a swimlane has a set of them.
    pub tenants: &'a TenantSet,
    /// The swimlane's environment tier (C6). `None` turns C6 off.
    pub tier: Option<&'a ShortText>,
    pub host_rules: &'a HostRules,
    pub files: &'a [CheckFile<'a>],
    pub limits: &'a Limits,
}

/// A base commit that a tenant copy has not seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseCommit {
    pub commit: CommitId,
    pub time: Timestamp,
}

/// A setting that differs between the base file when the copy was made and the base file now. The values are not part of
/// it: `compare` gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedSetting {
    pub path: SettingPath,
    pub kind: ChangeKind,
}

/// How C2 found the tenant file equal to the base file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqualBy {
    /// The same bytes (line endings aside).
    Bytes,
    /// The same settings, written differently (order, comments, quoting).
    Settings,
}

/// Why C4 calls a file an orphan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanReason {
    /// The name carries the suffix of a branch that the swimlane does not deploy, so no service of it reads the file.
    TenantNotDeployed(TenantId),
    /// Git has no such file.
    Untracked,
}

/// A host of another environment, in a setting (C6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MismatchedHost {
    pub path: SettingPath,
    /// The host name in lower case, without port, user or scheme.
    pub host: String,
    /// The tier the rules give the host.
    pub host_tier: ShortText,
}

/// A key defined more than once (C8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicatedKey {
    /// `None` for keys that cannot be written as a setting path; they are counted as one.
    pub path: Option<SettingPath>,
    /// The lines of every definition, in order. The last one wins.
    pub lines: Vec<u32>,
}

/// The facts behind a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckDetail {
    /// C1, `whole_file`: the base commits since the copy and the settings that changed in them.
    MissedBaseChanges {
        /// Oldest first, at most `Limits::locations_per_finding`.
        commits: Vec<BaseCommit>,
        commits_total: usize,
        /// Sorted by path, at most `Limits::locations_per_finding`. Empty if `compared` is false.
        changes: Vec<ChangedSetting>,
        changes_total: usize,
        /// The old and the new base file were read and compared. False for a text or binary file, a file that does not
        /// parse, a missing old version, or a base file that was deleted.
        compared: bool,
    },
    /// C1, `spring_merge`: tenant settings that override a base setting that no longer exists.
    DeadOverrides {
        /// Sorted, at most `Limits::locations_per_finding`.
        settings: Vec<SettingPath>,
        total: usize,
    },
    /// C2.
    RedundantTenantFile { tenant: TenantId, equal_by: EqualBy },
    /// C3.
    UnevenTenantFiles {
        with_file: Vec<TenantId>,
        without_file: Vec<TenantId>,
    },
    /// C4.
    OrphanFile { reason: OrphanReason },
    /// C5.
    Drift {
        state: DriftState,
        label: Option<crate::drift::DriftLabel>,
    },
    /// C6.
    EnvironmentHostMismatch {
        swimlane_tier: ShortText,
        /// Sorted by path, at most `Limits::locations_per_finding`.
        hosts: Vec<MismatchedHost>,
        /// Every mismatch, listed or not.
        total: usize,
    },
    /// C7.
    OverwrittenNfsChange {
        previous_state: DriftState,
        /// The version that was replaced; `None` if the file had been deleted on NFS.
        lost_version: Option<ContentHash>,
        source: AttributionSource,
    },
    /// C8.
    DuplicateKeys {
        /// Sorted by path, at most `Limits::locations_per_finding`.
        keys: Vec<DuplicatedKey>,
        total: usize,
        /// The parser dropped warnings (too many): there may be more keys than listed.
        more_not_listed: bool,
    },
    /// C9: one name, the folders it is in (the root is the empty path), sorted, at most `Limits::locations_per_finding`.
    AmbiguousFileName {
        name: String,
        folders: Vec<NfsPath>,
        /// Every folder, listed or not.
        total: usize,
    },
    /// C10: the keys no view and no environment variable fills, sorted by key, at most `Limits::locations_per_finding`.
    UnresolvedPlaceholders {
        keys: Vec<UnresolvedKey>,
        /// Every such key, listed or not.
        total: usize,
    },
    /// C11.
    ConfigServerRestart {
        reason: RestartReason,
        /// The folders behind the reason, sorted, at most `Limits::locations_per_finding`.
        folders: Vec<NfsPath>,
        /// The channels behind the reason, sorted.
        channels: Vec<domain::ChannelName>,
        /// Every folder or channel behind the reason, listed or not.
        total: usize,
    },
}

/// A finding in the making.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub kind: FindingKind,
    /// The file the finding is about. For C3 it is the logical file (the path without a tenant suffix).
    pub path: Option<NfsPath>,
    /// A sentence for people, at most 256 bytes.
    pub message: ShortText,
    /// Where to point in `path`, sorted by path, at most `Limits::locations_per_finding`.
    pub locations: Vec<SettingLocation>,
    pub detail: CheckDetail,
}

/// Runs C1 to C8 on a swimlane's files. See the module documentation for the inputs and the order of the result.
pub fn checks_c1_to_c8(input: &CheckInput<'_>) -> Vec<CheckResult> {
    // The base file of each logical file: a file without a tenant suffix is its own logical file.
    let mut base_of: BTreeMap<&LogicalFile, &CheckFile<'_>> = BTreeMap::new();
    for file in input.files {
        if file.facts.tenant.is_none() {
            base_of.entry(&file.facts.logical).or_insert(file);
        }
    }

    let mut out = Vec::new();
    for file in input.files {
        let base = base_of.get(&file.facts.logical).copied();
        out.extend(c1_missed_base_changes(file, base, input.tenants, input.limits));
        out.extend(c2_redundant_tenant_file(file, base, input.tenants));
        out.extend(c4_orphan_file(file, input.tenants));
        out.extend(c5_drift(file));
        out.extend(c6_environment_host_mismatch(
            file,
            input.tier,
            input.host_rules,
            input.limits,
        ));
        out.extend(c7_overwritten_nfs_change(file));
        out.extend(c8_duplicate_keys(file, input.limits));
    }
    out.extend(c3_uneven_tenant_files(input.files, input.tenants));

    out.sort_by(|a, b| (a.kind, &a.path, &a.message).cmp(&(b.kind, &b.path, &b.message)));
    out
}

/// The tenant of a tenant file that the swimlane deploys, or `None` for any other file. C1 and C2 are about these.
pub(crate) fn deployed_tenant<'a>(file: &'a CheckFile<'_>, tenants: &TenantSet) -> Option<&'a TenantId> {
    if file.facts.kind != FileKind::Tenant {
        return None;
    }
    file.facts.tenant.as_ref().filter(|t| tenants.contains(t))
}

/// A message as a `ShortText`: control characters become spaces and what is over 256 bytes is cut on a character
/// boundary and ends in `...`. `None` only if `ShortText` refuses the result, which sanitising makes impossible.
pub(crate) fn message(text: &str) -> Option<ShortText> {
    let mut clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.len() > MESSAGE_MAX_BYTES {
        let mut end = MESSAGE_MAX_BYTES - 3;
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        clean.truncate(end);
        clean.push_str("...");
    }
    ShortText::parse(&clean).ok()
}

/// The settings of a blob of the base file (a past version), or `None` when it is not a structured file the engine can
/// read. Past versions are untrusted bytes like any other, so the same limits apply.
pub(crate) fn settings_of_blob(path: &NfsPath, data: &[u8], limits: &Limits) -> Option<Vec<FlatSetting>> {
    let syntax = Syntax::of_path(path)?;
    let decoded = decode_text(data, limits).ok()?;
    let parsed = parse_decoded(&decoded.text, syntax, limits).ok()?;
    flatten(&parsed, limits).ok().map(|flat| flat.settings)
}

/// `n` things in words: `1 commit`, `2 commits`.
pub(crate) fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
