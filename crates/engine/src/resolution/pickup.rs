//! When a change takes effect for a consuming service (D85).
//!
//! | Change | State |
//! |---|---|
//! | A resource file | `live_within_ttl` until the client cache expires (`live_by`), live at once after a notification to a client that listens |
//! | A property source | `needs_notify_or_restart`: a notification refreshes a client that listens, otherwise only a restart helps |
//! | A new folder, or a change to `channels.yml` | `needs_config_server_restart`: the config-server computes its search locations once, at start |
//!
//! A change is also picked up when every pod of the service started after it (a pod that starts asks the config-server for
//! what is there), so a service is pending exactly when some pod started before the change. The config-server's own
//! restart is the only thing that picks up a new folder or `channels.yml`.
//!
//! The function reads no clock: it gets `now`, the time of the change, the time of the last notification and the start times.

use domain::{PickupState, Timestamp};

/// `CONFIG_CLIENT_CACHE_TTL` when the Deployment does not set it (D85): 20 minutes.
pub const DEFAULT_CACHE_TTL_SECS: u32 = 20 * 60;

/// What changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickupChange {
    /// A resource file: a file served whole.
    ResourceFile,
    /// A property source.
    PropertySource,
    /// A folder that did not exist when the config-server started.
    NewFolder,
    /// `channels.yml`.
    ChannelsYml,
}

/// What a service's Deployment says about its config client (D88: only the allowlisted names, never other values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientSettings {
    /// `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED`.
    pub notifications_enabled: bool,
    /// `CONFIG_CLIENT_CACHE_TTL`, in seconds.
    pub cache_ttl_secs: u32,
}

/// Everything `pickup_state` compares.
#[derive(Debug, Clone, Copy)]
pub struct PickupInput<'a> {
    pub change: PickupChange,
    pub client: ClientSettings,
    /// When the change was detected.
    pub changed_at: Timestamp,
    pub now: Timestamp,
    /// When the services were last told (the agent's `/update-resources` call), if they were.
    pub notified_at: Option<Timestamp>,
    /// When each currently running pod of the service started. Empty when not known.
    pub pod_starts: &'a [Timestamp],
    /// When the config-server's pod started.
    pub config_server_started: Timestamp,
}

/// The state of one change for one service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pickup {
    pub state: PickupState,
    /// For `live_within_ttl`: the time by which the change is live ("by HH:MM"): `changed_at` plus the cache's time to live.
    /// It is the latest the old file can be served, from a cache that was filled just before the change; a cache filled
    /// earlier expires earlier, and one filled after it already holds the new file, so no client is stale for longer.
    pub live_by: Option<Timestamp>,
    /// For `needs_notify_or_restart`: a notification would do, so "Notify services" helps. False: only a restart does.
    pub refreshable_by_notification: bool,
}

fn state(state: PickupState) -> Pickup {
    Pickup {
        state,
        live_by: None,
        refreshable_by_notification: false,
    }
}

/// The state of a change for a service.
pub fn pickup_state(input: &PickupInput<'_>) -> Pickup {
    let notified =
        input.client.notifications_enabled && input.notified_at.is_some_and(|t| t >= input.changed_at);
    let restarted = !input.pod_starts.is_empty() && input.pod_starts.iter().all(|&t| t >= input.changed_at);
    match input.change {
        PickupChange::NewFolder | PickupChange::ChannelsYml => {
            if input.config_server_started >= input.changed_at {
                state(PickupState::Live)
            } else {
                state(PickupState::NeedsConfigServerRestart)
            }
        }
        PickupChange::PropertySource => {
            if notified || restarted {
                state(PickupState::Live)
            } else {
                Pickup {
                    state: PickupState::NeedsNotifyOrRestart,
                    live_by: None,
                    refreshable_by_notification: input.client.notifications_enabled,
                }
            }
        }
        PickupChange::ResourceFile => {
            // The latest a client can still serve the old file: a cache that was filled at `changed_at` (a fill after it holds
            // the new file, a fill before it expires before this).
            let expires = input
                .changed_at
                .unix_millis()
                .saturating_add(i64::from(input.client.cache_ttl_secs).saturating_mul(1000));
            if notified || restarted || input.now.unix_millis() >= expires {
                state(PickupState::Live)
            } else {
                Pickup {
                    state: PickupState::LiveWithinTtl,
                    live_by: Some(Timestamp::from_unix_millis(expires)),
                    refreshable_by_notification: false,
                }
            }
        }
    }
}
