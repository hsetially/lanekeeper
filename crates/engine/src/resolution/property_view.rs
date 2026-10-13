//! The property view of an application and tenant (D82): the property sources merged setting by setting.
//!
//! Precedence, strongest first:
//!
//! 1. `<app>-<tenant>.*`
//! 2. `application-<tenant>.*`
//! 3. `<app>.*`
//! 4. `application.*`
//!
//! At equal rank `.properties` beats `.yml` and `.yaml`, and a later search location beats an earlier one (the order of
//! [`SearchLocations`]); two files that tie on all of this are ordered by path, so the answer never depends on the order of
//! the input. A file outside the search locations (a channel folder, say) is not read. The settings come in already parsed
//! and flattened: the caller parses each blob once.

use std::collections::BTreeMap;

use domain::{AppName, NfsPath, SettingValue, TenantId};

use super::locations::SearchLocations;
use super::role::{PROPERTY_EXTENSIONS, split_name};
use crate::error::{LimitKind, ResolveError};
use crate::limits::Limits;
use crate::parse::FlatSetting;

/// One property source: where it is and its settings.
#[derive(Debug, Clone, Copy)]
pub struct PropertyFile<'a> {
    pub path: &'a NfsPath,
    /// `flatten` of the parsed file. Empty for a file that did not parse.
    pub settings: &'a [FlatSetting],
}

/// A setting of the view and the file that supplied it.
#[derive(Clone, PartialEq, Eq)]
pub struct ViewSetting {
    /// Spring's key: dotted, `[n]` for list items, no `#<doc>/` prefix.
    pub key: String,
    pub value: SettingValue,
    pub source: NfsPath,
}

impl std::fmt::Debug for ViewSetting {
    // A value may be a secret: `Debug` shows its size, never its text (S13, S21).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewSetting")
            .field("key", &self.key)
            .field("value_bytes", &self.value.text.len())
            .field("source", &self.source)
            .finish()
    }
}

/// The merged settings of the property sources an application and tenant read, sorted by key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PropertyView {
    settings: BTreeMap<String, ViewSetting>,
}

impl PropertyView {
    /// The setting with this key.
    pub fn get(&self, key: &str) -> Option<&ViewSetting> {
        self.settings.get(key)
    }

    /// Every setting, sorted by key.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ViewSetting> {
        self.settings.values()
    }

    pub fn len(&self) -> usize {
        self.settings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.settings.is_empty()
    }
}

/// The rank of a file name for `(application, tenant)`: 1 is the strongest. `None` for a file the request does not read.
fn rank(stem: &str, application: &AppName, tenant: Option<&TenantId>) -> Option<u8> {
    let app = application.as_str();
    if let Some(tenant) = tenant {
        let t = tenant.as_str();
        let is_suffixed = |name: &str| {
            stem.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|rest| rest == t)
        };
        if is_suffixed(app) {
            return Some(1);
        }
        if is_suffixed("application") {
            return Some(2);
        }
    }
    if stem == app {
        return Some(3);
    }
    (stem == "application").then_some(4)
}

/// A key without the `#<doc>/` prefix that multi-document files get.
fn view_key(path: &str) -> &str {
    if let Some(rest) = path.strip_prefix('#') {
        if let Some((doc, key)) = rest.split_once('/') {
            if !doc.is_empty() && doc.bytes().all(|b| b.is_ascii_digit()) {
                return key;
            }
        }
    }
    path
}

/// The part of a key before its first `[`: the list a list item belongs to.
fn list_prefix(key: &str) -> Option<&str> {
    key.find('[').map(|i| &key[..i])
}

/// The view for `application` and `tenant` (`None`: a request with no tenant, which reads no tenant file).
///
/// # Errors
///
/// `Limit(Settings)` when the merged view would hold more than `Limits::settings` settings.
pub fn property_view(
    application: &AppName,
    tenant: Option<&TenantId>,
    locations: &SearchLocations,
    files: &[PropertyFile<'_>],
    limits: &Limits,
) -> Result<PropertyView, ResolveError> {
    // The files this request reads, weakest first: the last one wins.
    struct Candidate<'a> {
        // Sorts the weakest first: a larger rank number is weaker, `.yml` is weaker than `.properties`, an earlier location
        // is weaker than a later one.
        order: (std::cmp::Reverse<u8>, u8, usize),
        file: &'a PropertyFile<'a>,
    }
    let mut candidates: Vec<Candidate<'_>> = files
        .iter()
        .filter_map(|file| {
            let (stem, Some(ext)) = split_name(file.path.file_name()) else {
                return None;
            };
            if !PROPERTY_EXTENSIONS.contains(&ext) {
                return None;
            }
            let rank = rank(stem, application, tenant)?;
            let position = locations.position(&file.path.parent())?;
            Some(Candidate {
                order: (std::cmp::Reverse(rank), u8::from(ext == "properties"), position),
                file,
            })
        })
        .collect();
    candidates.sort_by(|a, b| (a.order, a.file.path).cmp(&(b.order, b.file.path)));

    let mut settings: BTreeMap<String, ViewSetting> = BTreeMap::new();
    for candidate in &candidates {
        let file = candidate.file;
        // A list that this source defines replaces the lower source's list as a unit (the binder's rule).
        let mut lists: Vec<&str> = file
            .settings
            .iter()
            .filter_map(|s| list_prefix(view_key(s.path.as_str())))
            .collect();
        lists.sort_unstable();
        lists.dedup();
        for list in lists {
            let opener = format!("{list}[");
            let doomed: Vec<String> = settings
                .range(opener.clone()..)
                .take_while(|(k, _)| k.starts_with(opener.as_str()))
                .filter(|(_, v)| v.source != *file.path)
                .map(|(k, _)| k.clone())
                .collect();
            for key in doomed {
                settings.remove(&key);
            }
        }
        for setting in file.settings {
            let key = view_key(setting.path.as_str());
            if !settings.contains_key(key) && settings.len() >= limits.settings {
                return Err(ResolveError::Limit(LimitKind::Settings));
            }
            settings.insert(
                key.to_owned(),
                ViewSetting {
                    key: key.to_owned(),
                    value: setting.value.clone(),
                    source: file.path.clone(),
                },
            );
        }
    }
    Ok(PropertyView { settings })
}
