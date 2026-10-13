//! C11 `config_server_restart_required` (D83, D87): what the config-server has not seen, because it computes its search
//! locations once, when it starts.
//!
//! - **New folders:** a folder first seen after the config-server started is not a search location until it restarts.
//! - **`channels.yml`:** a version of the list newer than the start. The channel folders it names are excluded from the
//!   locations, so a change moves folders in or out of them.
//! - **A listed channel with no folder:** `channels.yml` names a channel and no folder of that name exists anywhere.
//! - **A channel folder the server did not know:** a folder named like a channel that the list has now and did not have at
//!   the start. The server still searches it, and now it should not.
//!
//! The engine reads no clock and no cluster: the caller (05) supplies the times (plan decision A12). It does not guess a
//! channel folder from its name when `channels.yml` does not list it. The results come in the order of the list above, one
//! per reason, so that the order does not depend on the input.

use domain::{ChannelName, FindingKind, NfsPath, Timestamp};

use super::{CheckDetail, CheckResult, count, message};
use crate::limits::Limits;
use crate::resolution::is_channel_dir;

/// Why the config-server has to restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RestartReason {
    NewFolders,
    ChannelsYmlChanged,
    ChannelListedWithoutFolder,
    ChannelFolderNotListedAtStart,
}

/// A folder of the config tree and when the hub first saw it.
#[derive(Debug, Clone, Copy)]
pub struct FolderSeen<'a> {
    pub dir: &'a NfsPath,
    pub first_seen: Timestamp,
}

/// `channels.yml` and the time of its newest version.
#[derive(Debug, Clone, Copy)]
pub struct ChannelsYml<'a> {
    pub path: &'a NfsPath,
    pub changed_at: Timestamp,
}

/// Everything C11 compares.
#[derive(Debug, Clone, Copy)]
pub struct ConfigServerFacts<'a> {
    /// When the config-server's pod started.
    pub started: Timestamp,
    /// The channels `channels.yml` lists now.
    pub channels_now: &'a [ChannelName],
    /// The channels it listed when the server started, if the hub knows (the version current at that time).
    pub channels_at_start: Option<&'a [ChannelName]>,
    pub channels_yml: Option<ChannelsYml<'a>>,
    /// Every folder of the config tree, with the time it was first seen.
    pub folders: &'a [FolderSeen<'a>],
}

/// C11: zero to four results, in the order of [`RestartReason`].
pub fn c11_config_server_restart_required(
    facts: &ConfigServerFacts<'_>,
    limits: &Limits,
) -> Vec<CheckResult> {
    let mut out = Vec::new();
    new_folders(facts, limits, &mut out);
    channels_yml_changed(facts, &mut out);
    channels_without_folders(facts, limits, &mut out);
    folders_not_listed_at_start(facts, limits, &mut out);
    out
}

/// A folder first seen after the start, which is not a channel folder (a channel folder is not a search location either way).
fn new_folders(facts: &ConfigServerFacts<'_>, limits: &Limits, out: &mut Vec<CheckResult>) {
    let mut new: Vec<NfsPath> = facts
        .folders
        .iter()
        .filter(|f| f.first_seen > facts.started && !is_channel_dir(f.dir, facts.channels_now))
        .map(|f| f.dir.clone())
        .collect();
    new.sort();
    new.dedup();
    let Some(path) = new.first().cloned() else {
        return;
    };
    let total = new.len();
    new.truncate(limits.locations_per_finding);
    let text = format!(
        "{} created after the config-server started; it does not search {} until it restarts.",
        count(total, "folder was", "folders were"),
        if total == 1 { "it" } else { "them" }
    );
    push(
        out,
        Some(path),
        &text,
        RestartReason::NewFolders,
        new,
        Vec::new(),
        total,
    );
}

/// A version of `channels.yml` newer than the start.
fn channels_yml_changed(facts: &ConfigServerFacts<'_>, out: &mut Vec<CheckResult>) {
    if let Some(yml) = facts.channels_yml.filter(|y| y.changed_at > facts.started) {
        push(
            out,
            Some(yml.path.clone()),
            "channels.yml changed after the config-server started; it reads the list once, when it starts.",
            RestartReason::ChannelsYmlChanged,
            Vec::new(),
            Vec::new(),
            1,
        );
    }
}

/// A channel in the list that no folder is called.
fn channels_without_folders(facts: &ConfigServerFacts<'_>, limits: &Limits, out: &mut Vec<CheckResult>) {
    let mut headless: Vec<ChannelName> = facts
        .channels_now
        .iter()
        .filter(|c| {
            !facts
                .folders
                .iter()
                .any(|f| f.dir.components().any(|p| p == c.as_str()))
        })
        .cloned()
        .collect();
    headless.sort();
    headless.dedup();
    if headless.is_empty() {
        return;
    }
    let total = headless.len();
    headless.truncate(limits.locations_per_finding);
    let text = format!(
        "{} listed in channels.yml but no folder has {} name.",
        count(total, "channel is", "channels are"),
        if total == 1 { "its" } else { "their" }
    );
    push(
        out,
        facts.channels_yml.map(|y| y.path.clone()),
        &text,
        RestartReason::ChannelListedWithoutFolder,
        Vec::new(),
        headless,
        total,
    );
}

/// A folder called like a channel that the list has now and did not have at the start.
fn folders_not_listed_at_start(facts: &ConfigServerFacts<'_>, limits: &Limits, out: &mut Vec<CheckResult>) {
    let Some(at_start) = facts.channels_at_start else {
        return;
    };
    let added: Vec<&ChannelName> = facts
        .channels_now
        .iter()
        .filter(|c| !at_start.contains(c))
        .collect();
    let mut folders: Vec<NfsPath> = facts
        .folders
        .iter()
        .filter(|f| added.iter().any(|c| c.as_str() == f.dir.file_name()))
        .map(|f| f.dir.clone())
        .collect();
    folders.sort();
    folders.dedup();
    let Some(first) = folders.first().cloned() else {
        return;
    };
    let total = folders.len();
    let mut channels: Vec<ChannelName> = added.into_iter().cloned().collect();
    channels.sort();
    channels.dedup();
    folders.truncate(limits.locations_per_finding);
    let text = format!(
        "{} named like a channel that channels.yml lists now and did not when the config-server started; it still searches {}.",
        count(total, "folder is", "folders are"),
        if total == 1 { "it" } else { "them" }
    );
    push(
        out,
        Some(first),
        &text,
        RestartReason::ChannelFolderNotListedAtStart,
        folders,
        channels,
        total,
    );
}

fn push(
    out: &mut Vec<CheckResult>,
    path: Option<NfsPath>,
    text: &str,
    reason: RestartReason,
    folders: Vec<NfsPath>,
    channels: Vec<ChannelName>,
    total: usize,
) {
    let Some(message) = message(text) else { return };
    out.push(CheckResult {
        kind: FindingKind::ConfigServerRestartRequired,
        path,
        message,
        locations: Vec::new(),
        detail: CheckDetail::ConfigServerRestart {
            reason,
            folders,
            channels,
            total,
        },
    });
}
