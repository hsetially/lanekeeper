//! C8 `duplicate_keys` (domain-model, "Consistency checks"): a YAML mapping that has the same key more than once. The last
//! definition wins, and a reader who sees the first one is wrong. The parser reports each repeat as a warning; this check
//! turns them into one finding per file, with the lines of every definition.

use std::collections::{BTreeMap, BTreeSet};

use domain::{FindingKind, LineRange, SettingLocation, SettingPath};

use super::{CheckDetail, CheckFile, CheckResult, DuplicatedKey, count, message};
use crate::limits::Limits;
use crate::parse::ParseWarning;

/// C8 for one structured file. `None` for a file that is not structured or has no repeated key.
pub fn c8_duplicate_keys(file: &CheckFile<'_>, limits: &Limits) -> Option<CheckResult> {
    let structured = file.structured.as_ref()?;
    let mut keys: BTreeMap<Option<&SettingPath>, BTreeSet<u32>> = BTreeMap::new();
    for warning in structured.parse_warnings {
        if let ParseWarning::DuplicateKey { path, first, second } = warning {
            let lines = keys.entry(path.as_ref()).or_default();
            lines.insert(first.line);
            lines.insert(second.line);
        }
    }
    if keys.is_empty() {
        return None;
    }

    let total = keys.len();
    let listed: Vec<DuplicatedKey> = keys
        .into_iter()
        .take(limits.locations_per_finding)
        .map(|(path, lines)| DuplicatedKey {
            path: path.cloned(),
            lines: lines.into_iter().collect(),
        })
        .collect();
    // The definition that wins is the last one: that is the line to point at.
    let locations = listed
        .iter()
        .filter_map(|key| {
            let path = key.path.clone()?;
            let last = *key.lines.last()?;
            Some(SettingLocation {
                path,
                lines: LineRange::new(last, last).ok()?,
            })
        })
        .collect();

    let more_not_listed = structured.parse_warnings_dropped > 0;
    let mut text = format!(
        "{} defined more than once; the last definition wins",
        count(total, "key is", "keys are")
    );
    if more_not_listed {
        text.push_str(" (there may be more than listed)");
    }
    text.push('.');
    Some(CheckResult {
        kind: FindingKind::DuplicateKeys,
        path: Some(file.path.clone()),
        message: message(&text)?,
        locations,
        detail: CheckDetail::DuplicateKeys {
            keys: listed,
            total,
            more_not_listed,
        },
    })
}
