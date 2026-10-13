//! C9 `ambiguous_file_name` (D83, D87): the same file name in more than one folder that the config-server searches.
//!
//! Every folder but a channel's is a search location, so file names share one namespace; which copy a request gets depends on
//! the search order. Real example: `authentication-config.yaml` in 12 core-adapter folders of the base repository.
//!
//! The name is the logical file's: `a/x-sit1.yml` and `b/x.yml` are two files called `x.yml`. A folder that holds a base file
//! and a tenant copy of the same name counts once, and a file in a channel folder (or below one) does not count at all,
//! because the channel's copy is how a channel overrides a file and not an ambiguity.

use std::collections::{BTreeMap, BTreeSet};

use domain::{ChannelName, FindingKind, NfsPath};

use super::{CheckDetail, CheckFile, CheckResult, count, message};
use crate::limits::Limits;
use crate::resolution::is_channel_dir;

/// C9 over the swimlane's files: one result per name that is in two folders or more, sorted by name. The result's path is the
/// first copy in path order; `folders` lists the folders (the root is the empty path), at most
/// `Limits::locations_per_finding`, and `total` counts them all.
pub fn c9_ambiguous_file_names(
    files: &[CheckFile<'_>],
    channels: &[ChannelName],
    limits: &Limits,
) -> Vec<CheckResult> {
    let mut folders_of: BTreeMap<&str, BTreeSet<NfsPath>> = BTreeMap::new();
    for file in files {
        let logical = file.facts.logical.as_path();
        let folder = logical.parent();
        if is_channel_dir(&folder, channels) {
            continue;
        }
        folders_of.entry(logical.file_name()).or_default().insert(folder);
    }

    let mut out = Vec::new();
    for (name, folders) in folders_of {
        if folders.len() < 2 {
            continue;
        }
        let total = folders.len();
        let listed: Vec<NfsPath> = folders.into_iter().take(limits.locations_per_finding).collect();
        let text = format!(
            "{name} is in {}; which copy a service gets depends on the config-server's search order.",
            count(total, "folder", "folders")
        );
        let (Some(message), Some(first)) = (message(&text), listed.first()) else {
            continue;
        };
        let path = NfsPath::parse(&if first.is_root() {
            name.to_owned()
        } else {
            format!("{first}/{name}")
        })
        .ok();
        out.push(CheckResult {
            kind: FindingKind::AmbiguousFileName,
            path,
            message,
            locations: Vec::new(),
            detail: CheckDetail::AmbiguousFileName {
                name: name.to_owned(),
                folders: listed,
                total,
            },
        });
    }
    out
}
