//! The folders the config-server searches, in order (D83).
//!
//! `<root>`, `<root>/{application}`, `<root>/{application}/{profile}` and then every other directory except the channel
//! directories (those named in `channels.yml`, and anything below them). The profile is `<tenant>,default`. A later location
//! beats an earlier one when two property sources have the same rank, and the whole list is computed once, when the
//! config-server starts.

use std::collections::{BTreeMap, BTreeSet};

use domain::{AppName, ChannelName, NfsPath, TenantId};

/// The search locations of one request, in order. The root is [`NfsPath::root`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchLocations {
    dirs: Vec<NfsPath>,
    index: BTreeMap<NfsPath, usize>,
}

impl SearchLocations {
    /// The folders in order: earlier ones lose to later ones at equal rank.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &NfsPath> {
        self.dirs.iter()
    }

    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }

    /// The place of a folder in the order, or `None` for a folder the config-server does not search.
    pub fn position(&self, dir: &NfsPath) -> Option<usize> {
        self.index.get(dir).copied()
    }
}

/// True when `dir`, or a folder above it, is named like a channel: the config-server never searches it (a channel's files
/// are reached only by a request that names the channel).
pub fn is_channel_dir(dir: &NfsPath, channels: &[ChannelName]) -> bool {
    dir.components()
        .any(|component| channels.iter().any(|c| c.as_str() == component))
}

/// The search locations for `application` and `tenant` (`None`: a request with no tenant, whose profile is `default`), in
/// a tree with `files`. Only folders that hold something are listed. The order of `files` does not matter.
pub fn search_locations(
    application: &AppName,
    tenant: Option<&TenantId>,
    files: &[NfsPath],
    channels: &[ChannelName],
) -> SearchLocations {
    // Every folder that has a file below it, the root included.
    let mut present: BTreeSet<NfsPath> = BTreeSet::new();
    present.insert(NfsPath::root());
    for file in files {
        let mut dir = file.parent();
        while !dir.is_root() && !present.contains(&dir) {
            let up = dir.parent();
            present.insert(dir);
            dir = up;
        }
    }

    let app_dir = NfsPath::parse(application.as_str()).ok();
    let profile = tenant.map_or_else(|| "default".to_owned(), |t| format!("{t},default"));
    let profile_dir = app_dir
        .as_ref()
        .and_then(|app| NfsPath::parse(&format!("{app}/{profile}")).ok());

    let mut dirs = Vec::with_capacity(present.len());
    let mut push = |dir: &NfsPath| {
        if present.contains(dir) && !is_channel_dir(dir, channels) && !dirs.contains(dir) {
            dirs.push(dir.clone());
        }
    };
    push(&NfsPath::root());
    if let Some(dir) = &app_dir {
        push(dir);
    }
    if let Some(dir) = &profile_dir {
        push(dir);
    }
    // `present` is sorted, so the rest are in path order. Skipping what is already listed keeps the first three in place.
    let listed: BTreeSet<NfsPath> = dirs.iter().cloned().collect();
    for dir in &present {
        if !listed.contains(dir) && !is_channel_dir(dir, channels) {
            dirs.push(dir.clone());
        }
    }

    let index = dirs.iter().cloned().enumerate().map(|(i, d)| (d, i)).collect();
    SearchLocations { dirs, index }
}
