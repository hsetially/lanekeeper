//! C1 `missed_base_changes` (domain-model, "Consistency checks").
//!
//! - **`whole_file`**: the tenant file replaces the base file, so it is a copy, and a copy falls behind. It has missed the
//!   base file's commits after the copy's last commit. The finding lists those commits and the settings that changed in
//!   them (the base file as it was when the copy was last updated, against the newest version). Commits are matched to the
//!   copy by commit time, and the message says so.
//! - **`spring_merge`**: the tenant file is a set of overrides. C1 reports the dead ones: tenant settings whose base
//!   setting no longer exists.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::ops::Bound;

use domain::{LineRange, MergeMode, SettingLocation, SettingPath, TenantSet};

use super::{
    BaseCommit, BaseHistory, BaseVersion, ChangedSetting, CheckDetail, CheckFile, CheckResult, count,
    deployed_tenant, message, settings_of_blob,
};
use crate::diff::semantic_diff;
use crate::limits::Limits;
use crate::parse::{FlatSetting, PathSegment, decode_setting_path};

/// C1 for one tenant file of a tenant that the swimlane deploys. `base` is the base file of the same logical file on the
/// swimlane's NFS (used in `spring_merge` mode). `None` for every other file, and when there is nothing to report.
pub fn c1_missed_base_changes(
    file: &CheckFile<'_>,
    base: Option<&CheckFile<'_>>,
    tenants: &TenantSet,
    limits: &Limits,
) -> Option<CheckResult> {
    let tenant = deployed_tenant(file, tenants)?;
    match file.merge_mode {
        MergeMode::WholeFile => whole_file(file, tenant.as_str(), file.base_history.as_ref()?, limits),
        MergeMode::SpringMerge => dead_overrides(file, tenant.as_str(), base?, limits),
    }
}

fn whole_file(
    file: &CheckFile<'_>,
    tenant: &str,
    history: &BaseHistory<'_>,
    limits: &Limits,
) -> Option<CheckResult> {
    let mut since: Vec<&BaseVersion> = history
        .since
        .iter()
        .filter(|v| v.time > history.tenant_commit_time)
        .collect();
    // By time, then by commit id, so that the order is the same whatever order the caller listed them in.
    since.sort_by(|a, b| (a.time, &a.commit).cmp(&(b.time, &b.commit)));
    let newest = *since.last()?;

    let commits_total = since.len();
    let commits: Vec<BaseCommit> = since
        .iter()
        .take(limits.locations_per_finding)
        .map(|v| BaseCommit {
            commit: v.commit.clone(),
            time: v.time,
        })
        .collect();

    let diff = changed_settings(file, history.at_tenant_commit, newest, limits);
    let compared = diff.is_some();
    let all_changes = diff.unwrap_or_default();
    let changes_total = all_changes.len();
    let locations = locations_in_file(file, all_changes.iter().map(|c| &c.path), limits);
    let changes: Vec<ChangedSetting> = all_changes
        .into_iter()
        .take(limits.locations_per_finding)
        .collect();

    let mut text = format!(
        "The base file has {} since the copy for {tenant} was last updated (matched by commit time)",
        count(commits_total, "commit", "commits")
    );
    if compared {
        let _ = write!(text, "; {} differ", count(changes_total, "setting", "settings"));
    }
    text.push('.');

    Some(CheckResult {
        kind: domain::FindingKind::MissedBaseChanges,
        path: Some(file.path.clone()),
        message: message(&text)?,
        locations,
        detail: CheckDetail::MissedBaseChanges {
            commits,
            commits_total,
            changes,
            changes_total,
            compared,
        },
    })
}

/// The settings that differ between the base file when the copy was made and its newest version, sorted by path. `None`
/// when they cannot be compared: the file is not structured, there is no old version, the newest commit deleted the file,
/// or either version does not parse.
fn changed_settings(
    file: &CheckFile<'_>,
    before: Option<&BaseVersion>,
    newest: &BaseVersion,
    limits: &Limits,
) -> Option<Vec<ChangedSetting>> {
    file.structured?;
    let old = settings_of_blob(file.path, before?.content.as_ref()?, limits)?;
    let new = settings_of_blob(file.path, newest.content.as_ref()?, limits)?;
    Some(
        semantic_diff(&old, &new)
            .into_iter()
            .map(|c| ChangedSetting {
                path: c.path,
                kind: c.kind,
            })
            .collect(),
    )
}

/// Where the settings are written in the tenant file, for those that are there. Sorted by path.
fn locations_in_file<'a>(
    file: &CheckFile<'_>,
    paths: impl Iterator<Item = &'a SettingPath>,
    limits: &Limits,
) -> Vec<SettingLocation> {
    let Some(structured) = &file.structured else {
        return Vec::new();
    };
    let wanted: BTreeSet<&SettingPath> = paths.collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut lines: BTreeMap<&SettingPath, LineRange> = BTreeMap::new();
    for setting in structured.settings {
        if wanted.contains(&setting.path) {
            lines.insert(&setting.path, setting.lines);
        }
    }
    lines
        .into_iter()
        .take(limits.locations_per_finding)
        .map(|(path, lines)| SettingLocation {
            path: path.clone(),
            lines,
        })
        .collect()
}

/// What the lower layers of a property source define, in a form that answers "is this tenant setting an override of
/// something?" quickly.
struct Defined {
    keys: BTreeSet<Vec<PathSegment>>,
    /// Every path that has a list directly below it: for `a[0].b[1]`, `a` and `a[0].b`.
    lists: BTreeSet<Vec<PathSegment>>,
    /// Every path that has a map directly below it, that is, a key: for `a.b[0].c`, the root, `a` and `a.b[0]`.
    maps: BTreeSet<Vec<PathSegment>>,
}

impl Defined {
    fn new<'a>(settings: impl Iterator<Item = &'a FlatSetting>) -> Self {
        let mut keys = BTreeSet::new();
        let mut lists = BTreeSet::new();
        let mut maps = BTreeSet::new();
        for setting in settings {
            let Some(decoded) = decode_setting_path(&setting.path) else {
                continue;
            };
            for (at, segment) in decoded.segments.iter().enumerate() {
                match segment {
                    PathSegment::Index(_) => lists.insert(decoded.segments[..at].to_vec()),
                    PathSegment::Key(_) => maps.insert(decoded.segments[..at].to_vec()),
                };
            }
            keys.insert(decoded.segments);
        }
        Self { keys, lists, maps }
    }

    /// True if a setting at `key` replaces or extends something that is defined. The rules are the merge's (`effective`):
    ///
    /// - the same setting;
    /// - a scalar over a map or a map over a scalar: a prefix of the key is defined, or the key is a prefix of something
    ///   defined;
    /// - an item of a list the lower layers have (the list is replaced as a unit);
    /// - a list over a map: an item of a list at a path where the lower layers have keys (the list replaces them);
    /// - a map over a list: a key at a path where the lower layers have a list (the key replaces its items).
    fn is_override(&self, key: &[PathSegment]) -> bool {
        // A prefix of the key is defined (a map or a list below it is replaced by this scalar, or a scalar by this map).
        if (0..key.len()).any(|n| self.keys.contains(&key[..n])) {
            return true;
        }
        // The key itself, or something below it, is defined.
        if self
            .keys
            .range::<[PathSegment], _>((Bound::Included(key), Bound::Unbounded))
            .next()
            .is_some_and(|k| k.starts_with(key))
        {
            return true;
        }
        // A list where the lower layers have a list (replaced as a unit) or keys (replaced by the list); a key, up to the
        // first index, where they have a list (the merge replaces the items of that list, `no_lists` in `effective`).
        let first_index = key
            .iter()
            .position(|segment| matches!(segment, PathSegment::Index(_)))
            .unwrap_or(key.len());
        key.iter().enumerate().any(|(at, segment)| match segment {
            PathSegment::Index(_) => self.lists.contains(&key[..at]) || self.maps.contains(&key[..at]),
            PathSegment::Key(_) => at < first_index && self.lists.contains(&key[..at]),
        })
    }
}

fn dead_overrides(
    file: &CheckFile<'_>,
    tenant: &str,
    base: &CheckFile<'_>,
    limits: &Limits,
) -> Option<CheckResult> {
    let own = file.structured.as_ref()?;
    let lower = base.structured.as_ref()?;
    let defined = Defined::new(lower.settings.iter().chain(file.root_settings.unwrap_or(&[])));

    let mut dead: Vec<&FlatSetting> = own
        .settings
        .iter()
        .filter(|setting| {
            // A path that cannot be taken apart is one the writer never produced: it is not judged.
            decode_setting_path(&setting.path).is_some_and(|d| !defined.is_override(&d.segments))
        })
        .collect();
    if dead.is_empty() {
        return None;
    }
    dead.sort_by(|a, b| a.path.cmp(&b.path));

    let total = dead.len();
    let settings: Vec<SettingPath> = dead
        .iter()
        .take(limits.locations_per_finding)
        .map(|s| s.path.clone())
        .collect();
    let locations = dead
        .iter()
        .take(limits.locations_per_finding)
        .map(|s| SettingLocation {
            path: s.path.clone(),
            lines: s.lines,
        })
        .collect();
    let text = format!(
        "{} in the file for {tenant} override a base setting that no longer exists.",
        count(total, "setting", "settings")
    );
    Some(CheckResult {
        kind: domain::FindingKind::MissedBaseChanges,
        path: Some(file.path.clone()),
        message: message(&text)?,
        locations,
        detail: CheckDetail::DeadOverrides { settings, total },
    })
}
