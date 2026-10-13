//! C10 `unresolved_placeholder` (D82, D87): a `${KEY}` that is in neither the property view nor the names of the environment
//! variables of the Deployments that read the file.
//!
//! The config-server leaves a placeholder it cannot fill as written, and the service then looks for it in its own
//! environment (Spring's relaxed binding: `server.port` is `SERVER_PORT`). If the key is in neither place the service gets the
//! literal `${KEY}`. The check takes environment variable **names** only ([`EnvNames`]): the engine has no way to read a
//! value (D88), so no value can end up in a finding.
//!
//! A key is unresolved for a tenant when that tenant's property view has no such key. With several tenants (Q34) the finding
//! names each tenant it is unresolved for; with none, only the environment can fill a key. Forms that are not plain
//! `${KEY}` (`${KEY:default}`, nested, `$${KEY}`) are not placeholders (plan decision A10).

use std::collections::{BTreeMap, BTreeSet};

use domain::{FindingKind, NfsPath, SettingLocation, TenantId};

use super::{CheckDetail, CheckResult, count, message};
use crate::limits::Limits;
use crate::parse::FlatSetting;
use crate::resolution::{PropertyView, scan_placeholders};

/// The names of the environment variables of the Deployments that read a file (D88). Names only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnvNames(BTreeSet<String>);

/// The form in which Spring binds a key from the environment: upper case, with `.`, `-`, `[` and `]` as `_`.
fn relaxed(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '.' | '-' | '[' | ']' => '_',
            c => c.to_ascii_uppercase(),
        })
        .collect()
}

impl EnvNames {
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self(names.into_iter().filter(|n| !n.is_empty()).map(relaxed).collect())
    }

    /// True when a variable of this name, or of the name Spring would bind this key to, exists.
    pub fn contains(&self, key: &str) -> bool {
        !key.is_empty() && self.0.contains(&relaxed(key))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A resource file's text, for the placeholders in it.
#[derive(Debug, Clone, Copy)]
pub struct PlaceholderFile<'a> {
    pub path: &'a NfsPath,
    pub text: &'a str,
    /// The file's settings, when it is structured and parsed: the finding then points at the settings that hold a
    /// placeholder.
    pub settings: Option<&'a [FlatSetting]>,
}

/// The property view that a tenant of the swimlane reads.
#[derive(Debug, Clone, Copy)]
pub struct TenantView<'a> {
    pub tenant: &'a TenantId,
    pub view: &'a PropertyView,
}

/// An unresolved key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedKey {
    pub key: String,
    /// The tenants whose view lacks it, in tenant order. Empty when there are no views.
    pub tenants: Vec<TenantId>,
    /// The lines it is on, in order, at most `Limits::locations_per_finding`.
    pub lines: Vec<u32>,
}

/// C10 over resource files: one result per file that has an unresolved key, sorted by path. `keys` is sorted by key and holds
/// at most `Limits::locations_per_finding`; `total` counts all.
pub fn c10_unresolved_placeholders(
    files: &[PlaceholderFile<'_>],
    views: &[TenantView<'_>],
    env: &EnvNames,
    limits: &Limits,
) -> Vec<CheckResult> {
    let mut out = Vec::new();
    for file in files {
        // The lines of each key, and the keys that no view and no environment variable fills.
        let mut lines_of: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for placeholder in scan_placeholders(file.text) {
            let lines = lines_of.entry(placeholder.key).or_default();
            if lines.last() != Some(&placeholder.line) {
                lines.push(placeholder.line);
            }
        }
        let mut unresolved: Vec<UnresolvedKey> = Vec::new();
        for (key, mut lines) in lines_of {
            if env.contains(&key) {
                continue;
            }
            let tenants: Vec<TenantId> = views
                .iter()
                .filter(|v| v.view.get(&key).is_none())
                .map(|v| v.tenant.clone())
                .collect();
            if !views.is_empty() && tenants.is_empty() {
                continue;
            }
            lines.truncate(limits.locations_per_finding);
            unresolved.push(UnresolvedKey { key, tenants, lines });
        }
        if unresolved.is_empty() {
            continue;
        }

        let total = unresolved.len();
        let locations = locations(file, &unresolved, limits);
        unresolved.truncate(limits.locations_per_finding);
        let names: Vec<&str> = unresolved.iter().take(5).map(|k| k.key.as_str()).collect();
        let more = if total > names.len() { ", ..." } else { "" };
        let text = format!(
            "{} filled by no property source and no environment variable: {}{more}.",
            count(total, "placeholder is", "placeholders are"),
            names.join(", ")
        );
        let Some(message) = message(&text) else { continue };
        out.push(CheckResult {
            kind: FindingKind::UnresolvedPlaceholder,
            path: Some(file.path.clone()),
            message,
            locations,
            detail: CheckDetail::UnresolvedPlaceholders {
                keys: unresolved,
                total,
            },
        });
    }
    out.sort_by(|a, b| (&a.path, &a.message).cmp(&(&b.path, &b.message)));
    out
}

/// The settings of the file whose value holds an unresolved placeholder, sorted by path.
fn locations(
    file: &PlaceholderFile<'_>,
    unresolved: &[UnresolvedKey],
    limits: &Limits,
) -> Vec<SettingLocation> {
    let Some(settings) = file.settings else {
        return Vec::new();
    };
    let wanted: BTreeSet<&str> = unresolved.iter().map(|k| k.key.as_str()).collect();
    let mut found: Vec<SettingLocation> = settings
        .iter()
        .filter(|s| s.value.text.contains("${"))
        .filter(|s| {
            scan_placeholders(&s.value.text)
                .iter()
                .any(|p| wanted.contains(p.key.as_str()))
        })
        .map(|s| SettingLocation {
            path: s.path.clone(),
            lines: s.lines,
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.truncate(limits.locations_per_finding);
    found
}
