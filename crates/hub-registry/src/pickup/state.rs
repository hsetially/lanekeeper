//! The pickup state of one service from the changes it reads (T10, D85): pure, and the same function for every caller.
//!
//! The engine decides what a **change** means for a **client** ([`engine::pickup_state`]: a resource file is live within the cache's time to
//! live, a property source needs a notification or a restart, a new folder or `channels.yml` needs the config-server to restart). A service
//! has many changes pending at once and the contract has one state for it, so this module folds them:
//!
//! - **`pickup`** is the **worst** state among its pending changes (a restart of the config-server is worse than a notification, which is worse
//!   than waiting for a cache);
//! - **`pending_since`** is the time of the **oldest** change that is not live yet;
//! - **`live_by`** is set only when the worst state is `live_within_ttl`: the **latest** time any of those changes goes live, which is when
//!   the service has everything;
//! - **`pending_files`** are the files of the changes that are not live yet, in path order, at most the contract's 500, **without the files
//!   that are denied**: a denied file's change counts toward the state (a service that reads a rotated keystore is stale like any other) and
//!   its name is never listed (S10, S14, T9).
//!
//! A service with no running pod has nothing to be stale: it asks the config-server for everything when a pod starts (the engine reads an
//! empty list of start times as "not known" and calls nothing live, which would show a scaled-to-zero Deployment pending for ever).
//!
//! Nothing here reads a clock or a database: [`Context`] holds the times that are compared.

use std::collections::BTreeSet;

use domain::{PickupState, Timestamp};
use engine::{ClientSettings, DEFAULT_CACHE_TTL_SECS, PickupChange, PickupInput, pickup_state};

/// The contract's bound on the files of one service (`ServiceState.pending_files`, `maxItems: 500`).
pub const MAX_PENDING_FILES: usize = 500;

/// A Deployment's config client from the two allowlisted variables (D88). `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED` is on only when it
/// says `true` (any case); `CONFIG_CLIENT_CACHE_TTL` is a number of seconds, and anything else is the engine's default of 20 minutes.
///
/// One parse for what the read side shows (`notifications_enabled`, `cache_ttl_secs`) and for what the state was computed with.
pub fn client_settings(notifications: Option<&str>, ttl: Option<&str>) -> ClientSettings {
    ClientSettings {
        notifications_enabled: notifications.is_some_and(|v| v.trim().eq_ignore_ascii_case("true")),
        cache_ttl_secs: ttl
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_CACHE_TTL_SECS),
    }
}

/// One change, as the engine's state function takes it, with the file it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    /// The file, as written to `pending_files`.
    pub path: String,
    pub kind: PickupChange,
    /// When it was detected (Unix milliseconds): the file's newest version, or the time a new folder was first seen.
    pub changed_at: i64,
    /// The file may be named in `pending_files`. False for a denied file.
    pub listed: bool,
}

/// The times that are compared with a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Context {
    pub now: i64,
    /// When the services were last told (`swimlanes.last_notified_at`, written by prompt 06).
    pub notified_at: Option<i64>,
    /// When the config-server's pod started. Only the kinds that need its restart read it, and those are not produced when it is not known.
    pub config_server_started: i64,
}

/// What a service shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assessment {
    pub state: PickupState,
    pub pending_since: Option<i64>,
    pub live_by: Option<i64>,
    pub files: Vec<String>,
}

impl Assessment {
    /// Nothing pending.
    pub(crate) const fn live() -> Self {
        Self {
            state: PickupState::Live,
            pending_since: None,
            live_by: None,
            files: Vec::new(),
        }
    }

    pub(crate) fn is_live(&self) -> bool {
        self.state == PickupState::Live
    }
}

/// How bad a state is: the order the worst one is chosen in.
pub(crate) const fn rank(state: PickupState) -> u8 {
    match state {
        PickupState::Live => 0,
        PickupState::LiveWithinTtl => 1,
        PickupState::NeedsNotifyOrRestart => 2,
        PickupState::NeedsConfigServerRestart => 3,
    }
}

/// The state of a service that runs pods whose oldest started at `oldest_pod` (`None`: no pod runs), given the changes it reads.
pub(crate) fn assess<'a>(
    client: ClientSettings,
    oldest_pod: Option<i64>,
    changes: impl IntoIterator<Item = &'a Change>,
    ctx: &Context,
    max_files: usize,
) -> Assessment {
    let Some(oldest_pod) = oldest_pod else {
        return Assessment::live();
    };
    // "Every pod started after the change" is "the oldest pod started after it".
    let pods = [Timestamp::from_unix_millis(oldest_pod)];
    let at = Timestamp::from_unix_millis;
    let mut worst = PickupState::Live;
    let mut pending_since: Option<i64> = None;
    let mut ttl_live_by: Option<i64> = None;
    let mut files: BTreeSet<&str> = BTreeSet::new();
    for change in changes {
        let picked = pickup_state(&PickupInput {
            change: change.kind,
            client,
            changed_at: at(change.changed_at),
            now: at(ctx.now),
            notified_at: ctx.notified_at.map(at),
            pod_starts: &pods,
            config_server_started: at(ctx.config_server_started),
        });
        if picked.state == PickupState::Live {
            continue;
        }
        if rank(picked.state) > rank(worst) {
            worst = picked.state;
        }
        pending_since = Some(pending_since.map_or(change.changed_at, |s| s.min(change.changed_at)));
        if picked.state == PickupState::LiveWithinTtl {
            if let Some(by) = picked.live_by {
                let by = by.unix_millis();
                ttl_live_by = Some(ttl_live_by.map_or(by, |b| b.max(by)));
            }
        }
        if change.listed {
            files.insert(change.path.as_str());
        }
    }
    if worst == PickupState::Live {
        return Assessment::live();
    }
    Assessment {
        state: worst,
        pending_since,
        live_by: if worst == PickupState::LiveWithinTtl {
            ttl_live_by
        } else {
            None
        },
        files: files.into_iter().take(max_files).map(str::to_owned).collect(),
    }
}

/// What a changed file is, for the engine: `channels.yml` and a file in a folder the config-server has not seen need its restart, but only when
/// its start time is known; otherwise the role decides. `new_folder_at` is the newest first-seen time among the file's new folders.
pub(crate) fn kind_of(
    property_source: bool,
    is_channels_yml: bool,
    new_folder_at: Option<i64>,
    observed_at: i64,
    config_server_known: bool,
) -> (PickupChange, i64) {
    if config_server_known && is_channels_yml {
        return (PickupChange::ChannelsYml, observed_at);
    }
    if let (true, Some(created)) = (config_server_known, new_folder_at) {
        return (PickupChange::NewFolder, created);
    }
    if property_source {
        (PickupChange::PropertySource, observed_at)
    } else {
        (PickupChange::ResourceFile, observed_at)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const T0: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;

    fn client(notify: bool, ttl: u32) -> ClientSettings {
        ClientSettings {
            notifications_enabled: notify,
            cache_ttl_secs: ttl,
        }
    }

    fn change(path: &str, kind: PickupChange, at: i64) -> Change {
        Change {
            path: path.to_owned(),
            kind,
            changed_at: at,
            listed: true,
        }
    }

    fn ctx(now: i64, notified: Option<i64>, cs: i64) -> Context {
        Context {
            now,
            notified_at: notified,
            config_server_started: cs,
        }
    }

    #[test]
    fn the_client_settings_are_the_two_variables_with_the_engines_default() {
        assert_eq!(client_settings(None, None), client(false, DEFAULT_CACHE_TTL_SECS));
        assert_eq!(client_settings(Some(" TRUE "), Some(" 60 ")), client(true, 60));
        assert_eq!(
            client_settings(Some("yes"), Some("20m")),
            client(false, DEFAULT_CACHE_TTL_SECS)
        );
        assert_eq!(
            client_settings(Some("false"), Some("-1")),
            client(false, DEFAULT_CACHE_TTL_SECS)
        );
        assert_eq!(
            client_settings(Some("1"), Some("")),
            client(false, DEFAULT_CACHE_TTL_SECS)
        );
    }

    #[test]
    fn a_resource_waits_for_the_cache_and_says_by_when() {
        let changes = [change("a/x.xsl", PickupChange::ResourceFile, T0)];
        let got = assess(
            client(false, 1200),
            Some(T0 - MIN),
            &changes,
            &ctx(T0 + MIN, None, 0),
            500,
        );
        assert_eq!(got.state, PickupState::LiveWithinTtl);
        assert_eq!(got.live_by, Some(T0 + 1_200_000));
        assert_eq!(got.pending_since, Some(T0));
        assert_eq!(got.files, ["a/x.xsl"]);
        // The cache expired: live, and nothing is listed.
        let later = assess(
            client(false, 1200),
            Some(T0 - MIN),
            &changes,
            &ctx(T0 + 1_200_000, None, 0),
            500,
        );
        assert_eq!(later, Assessment::live());
    }

    #[test]
    fn a_property_source_needs_a_notification_or_a_restart() {
        let changes = [change("a/app.yml", PickupChange::PropertySource, T0)];
        let waits = assess(
            client(true, 1200),
            Some(T0 - MIN),
            &changes,
            &ctx(T0 + 99 * MIN, None, 0),
            500,
        );
        assert_eq!(
            waits.state,
            PickupState::NeedsNotifyOrRestart,
            "time does not help a property source"
        );
        assert_eq!(waits.live_by, None);
        let notified = assess(
            client(true, 1200),
            Some(T0 - MIN),
            &changes,
            &ctx(T0 + MIN, Some(T0 + 1), 0),
            500,
        );
        assert_eq!(notified, Assessment::live(), "a client that listens refreshes");
        let deaf = assess(
            client(false, 1200),
            Some(T0 - MIN),
            &changes,
            &ctx(T0 + MIN, Some(T0 + 1), 0),
            500,
        );
        assert_eq!(
            deaf.state,
            PickupState::NeedsNotifyOrRestart,
            "a client that does not listen ignores it"
        );
        let restarted = assess(
            client(false, 1200),
            Some(T0 + 5),
            &changes,
            &ctx(T0 + MIN, None, 0),
            500,
        );
        assert_eq!(restarted, Assessment::live(), "the pod started after the change");
    }

    #[test]
    fn a_new_folder_and_channels_yml_wait_for_the_config_server() {
        let changes = [
            change("new/x.yml", PickupChange::NewFolder, T0),
            change("channels.yml", PickupChange::ChannelsYml, T0 + 1),
        ];
        let c = |cs| ctx(T0 + MIN, Some(T0 + 10), cs);
        let waits = assess(client(true, 1200), Some(T0 + 5), &changes, &c(T0 - 1), 500);
        assert_eq!(
            waits.state,
            PickupState::NeedsConfigServerRestart,
            "neither a restart of the service nor a notification helps"
        );
        assert_eq!(waits.files, ["channels.yml", "new/x.yml"]);
        let started = assess(client(true, 1200), Some(T0 - MIN), &changes, &c(T0 + 2), 500);
        assert_eq!(
            started,
            Assessment::live(),
            "the config-server started after both"
        );
        // Only the later change is not covered by a start between them.
        let between = assess(client(true, 1200), Some(T0 - MIN), &changes, &c(T0), 500);
        assert_eq!(between.state, PickupState::NeedsConfigServerRestart);
        assert_eq!(between.pending_since, Some(T0 + 1));
        assert_eq!(between.files, ["channels.yml"]);
    }

    #[test]
    fn the_service_shows_the_worst_state_the_oldest_change_and_the_latest_expiry() {
        // The caches last 30 minutes: `a.xsl` (changed at +10) is live at +40, `b.xsl` (changed at +20) at +50.
        let changes = [
            change("a.xsl", PickupChange::ResourceFile, T0 + 10 * MIN),
            change("b.xsl", PickupChange::ResourceFile, T0 + 20 * MIN),
        ];
        let pod = Some(T0 - MIN);
        let got = assess(
            client(false, 1800),
            pod,
            &changes,
            &ctx(T0 + 21 * MIN, None, 0),
            500,
        );
        assert_eq!(got.state, PickupState::LiveWithinTtl);
        assert_eq!(got.pending_since, Some(T0 + 10 * MIN));
        assert_eq!(
            got.live_by,
            Some(T0 + 50 * MIN),
            "everything is live when the last cache expires"
        );
        assert_eq!(got.files, ["a.xsl", "b.xsl"]);
        // At +41 the first is live: it is not pending, not listed, and not the oldest.
        let later = assess(
            client(false, 1800),
            pod,
            &changes,
            &ctx(T0 + 41 * MIN, None, 0),
            500,
        );
        assert_eq!(later.pending_since, Some(T0 + 20 * MIN));
        assert_eq!(later.files, ["b.xsl"]);
        // A property source in the mix makes the service need a notification, and then only that one has no time.
        let mut more = changes.to_vec();
        more.push(change("c.yml", PickupChange::PropertySource, T0 + 15 * MIN));
        let worst = assess(client(false, 1800), pod, &more, &ctx(T0 + 41 * MIN, None, 0), 500);
        assert_eq!(worst.state, PickupState::NeedsNotifyOrRestart);
        assert_eq!(worst.live_by, None, "live_by is for live_within_ttl only");
        assert_eq!(worst.pending_since, Some(T0 + 15 * MIN), "a.xsl is live already");
        assert_eq!(
            worst.files,
            ["b.xsl", "c.yml"],
            "every change that is not live yet, whatever its state"
        );
    }

    #[test]
    fn a_denied_file_counts_toward_the_state_and_is_never_listed() {
        let mut denied = change("keys/store.p12", PickupChange::ResourceFile, T0);
        denied.listed = false;
        let only = assess(
            client(false, 1200),
            Some(T0 - MIN),
            &[denied.clone()],
            &ctx(T0 + MIN, None, 0),
            500,
        );
        assert_eq!(
            only.state,
            PickupState::LiveWithinTtl,
            "the service is stale like for any file"
        );
        assert!(only.files.is_empty(), "and nothing names the file");
        let both = assess(
            client(false, 1200),
            Some(T0 - MIN),
            &[denied, change("a.xsl", PickupChange::ResourceFile, T0)],
            &ctx(T0 + MIN, None, 0),
            500,
        );
        assert_eq!(both.files, ["a.xsl"]);
    }

    #[test]
    fn a_service_with_no_pod_has_nothing_to_pick_up() {
        let changes = [change("a/app.yml", PickupChange::PropertySource, T0)];
        assert_eq!(
            assess(client(true, 1200), None, &changes, &ctx(T0 + MIN, None, 0), 500),
            Assessment::live()
        );
    }

    #[test]
    fn the_files_are_in_path_order_without_repeats_and_cut_at_the_bound() {
        let changes: Vec<Change> = ["c", "a", "b", "a"]
            .iter()
            .map(|p| change(p, PickupChange::ResourceFile, T0))
            .collect();
        let got = assess(
            client(false, 1200),
            Some(T0 - MIN),
            &changes,
            &ctx(T0 + MIN, None, 0),
            2,
        );
        assert_eq!(got.files, ["a", "b"]);
        assert_eq!(
            got.state,
            PickupState::LiveWithinTtl,
            "the cut is of the list, not of what counts"
        );
    }

    #[test]
    fn the_kind_of_a_change_follows_what_is_known_of_the_config_server() {
        // Known: the files that need its restart say so.
        assert_eq!(
            kind_of(false, true, None, 7, true),
            (PickupChange::ChannelsYml, 7)
        );
        assert_eq!(
            kind_of(true, false, Some(5), 7, true),
            (PickupChange::NewFolder, 5),
            "a new folder beats the role"
        );
        assert_eq!(
            kind_of(true, false, None, 7, true),
            (PickupChange::PropertySource, 7)
        );
        assert_eq!(
            kind_of(false, false, None, 7, true),
            (PickupChange::ResourceFile, 7)
        );
        // Not known: nothing can be said about a restart, so the role decides and the change heals with time or a notification.
        assert_eq!(
            kind_of(false, true, Some(5), 7, false),
            (PickupChange::ResourceFile, 7)
        );
        assert_eq!(
            kind_of(true, true, Some(5), 7, false),
            (PickupChange::PropertySource, 7)
        );
    }

    fn kind() -> impl Strategy<Value = PickupChange> {
        prop_oneof![
            Just(PickupChange::ResourceFile),
            Just(PickupChange::PropertySource),
            Just(PickupChange::NewFolder),
            Just(PickupChange::ChannelsYml),
        ]
    }

    fn changes() -> impl Strategy<Value = Vec<Change>> {
        prop::collection::vec(
            (0_u8..12, kind(), 0_i64..4_000, any::<bool>()).prop_map(|(n, kind, at, listed)| Change {
                path: format!("f{n}"),
                kind,
                changed_at: T0 + at * 1_000,
                listed,
            }),
            0..12,
        )
    }

    proptest! {
        /// More changes never make a service better off, and time never makes one worse off: the two facts that let a sweeper that only moves
        /// time forward and a pipeline that only adds changes agree with a recompute from scratch.
        #[test]
        fn more_changes_never_improve_and_time_never_worsens(
            all in changes(),
            keep in prop::collection::vec(any::<bool>(), 12),
            notify in any::<bool>(),
            ttl in 0_u32..3_000,
            pod in prop::option::of(-1_000_i64..4_000),
            notified in prop::option::of(0_i64..4_000),
            cs in 0_i64..4_000,
            now in 0_i64..8_000,
            later in 0_i64..4_000,
        ) {
            let subset: Vec<Change> = all.iter().zip(keep.iter().cycle()).filter(|(_, k)| **k).map(|(c, _)| c.clone()).collect();
            let c = client(notify, ttl);
            let pod = pod.map(|p| T0 + p * 1_000);
            let at = |now: i64| ctx(T0 + now * 1_000, notified.map(|n| T0 + n * 1_000), T0 + cs * 1_000);
            let few = assess(c, pod, &subset, &at(now), 500);
            let many = assess(c, pod, &all, &at(now), 500);
            prop_assert!(rank(many.state) >= rank(few.state));
            let after = assess(c, pod, &all, &at(now + later), 500);
            prop_assert!(rank(after.state) <= rank(many.state));
            // Folding is order-free and repeat-free.
            let mut shuffled = all.clone();
            shuffled.reverse();
            shuffled.extend(all.iter().cloned());
            prop_assert_eq!(assess(c, pod, &shuffled, &at(now), 500), many.clone());
            // A live service has nothing listed and no time; a pending one has a time; only live_within_ttl has a live_by.
            prop_assert_eq!(many.is_live(), many.pending_since.is_none());
            prop_assert!(many.live_by.is_none() || many.state == PickupState::LiveWithinTtl);
            if many.is_live() { prop_assert!(many.files.is_empty()); }
        }
    }
}
