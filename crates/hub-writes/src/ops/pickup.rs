//! What a notification does to the services' pickup states (T11, D85, plan A16).
//!
//! The config-server was told about some paths. Every service that reads one of them is told to refresh: a client that listens
//! (`CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED`) drops its cache and, for a property source, refreshes its context. For each such
//! service that has changes waiting (`service_state.pending_files`) the engine's `pickup_state` is asked again, with the
//! notification as the time the services were last told, and the answer is written back in the same transaction as the audit event.
//!
//! - **Who reads a path.** A file in the root folder is read by every service (D82). A file below the folder of an application is
//!   read by that application's Deployments (their `SPRING_APPLICATION_NAME`, or their name when they declare none). A file below a
//!   folder that is no application's is a search location for every service (D83), so every service reads it.
//! - **What is asked.** The engine decides the state (rule 7); this module only supplies the facts: the kind of change (a property
//!   source or a resource file, by the same rule the preview uses), the client's settings from the allowlisted environment values
//!   (D88), when the oldest waiting change was seen, and when the oldest pod started.
//! - **What is left alone.** A service that waits for the config-server to restart (a new folder, `channels.yml`) stays as it is: no
//!   notification helps it, and the path alone cannot tell a new folder from an old one. A service with nothing waiting stays as it
//!   is. A waiting file the engine says is still pending stays in the list.
//! - **The swimlane** remembers when its services were last told (`swimlanes.last_notified_at`), which the registry's own
//!   recomputation reads. That time is the swimlane's, not a path's; see the threat model.

use std::collections::BTreeSet;

use domain::{DomainEvent, NfsPath, PickupState, ServiceRef, SwimlaneId, Timestamp};
use engine::{ClientSettings, DEFAULT_CACHE_TTL_SECS, PickupChange, PickupInput, file_role, pickup_state};
use ports::Tx;
use sqlx::{Postgres, Row, Transaction};

use crate::error::{Internal, Res};
use crate::preview::effect::Facts;
use crate::service::Inner;

/// The most services one page of a notification's update reads.
pub const MAX_PICKUP_SERVICES: i64 = 2_000;

/// The most pages one notification reads: 200,000 services, far above a swimlane. A swimlane that had more would be left with the
/// rest stale, which the log says.
const MAX_PICKUP_PAGES: usize = 100;

/// The name of the environment value that says a client listens for notifications (D88).
const MONITOR: &str = "CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED";
const CACHE_TTL: &str = "CONFIG_CLIENT_CACHE_TTL";

const TOUCH: &str = "UPDATE swimlanes SET last_notified_at = to_timestamp($2::bigint / 1000.0) WHERE id = $1 \
     RETURNING (EXTRACT(EPOCH FROM config_server_started_at) * 1000)::bigint";

/// The services of a swimlane that read the notified paths and have changes waiting. `$2` is true when every service reads one of
/// the paths; otherwise `$3` names the applications. `$5` is the last service of the page before (0 for the first page): the rows
/// are read a page at a time, by id. The rows are locked: two notifications at once update one after the other.
pub(crate) const PENDING: &str = "SELECT s.id, s.namespace, s.name, s.env_values, st.pickup_state, st.pending_files, \
            (EXTRACT(EPOCH FROM st.pending_since) * 1000)::bigint AS pending_ms, \
            (EXTRACT(EPOCH FROM st.oldest_pod_started_at) * 1000)::bigint AS pod_ms \
     FROM services s JOIN service_state st ON st.service_id = s.id \
     WHERE s.swimlane_id = $1 AND s.removed_at IS NULL AND jsonb_array_length(st.pending_files) > 0 \
       AND ($2 OR s.name = ANY($3) OR s.env_values ->> 'SPRING_APPLICATION_NAME' = ANY($3)) AND s.id > $5 \
     ORDER BY s.id LIMIT $4 FOR UPDATE OF st";

const WRITE: &str = "UPDATE service_state SET pickup_state = $2, pending_files = $3, \
            pending_since = to_timestamp($4::bigint / 1000.0), live_by = to_timestamp($5::bigint / 1000.0), updated_at = now() \
     WHERE service_id = $1";

/// What a notification asks of the pickup states: the facts about the swimlane, the paths and the time.
pub(crate) struct PickupUpdate {
    pub facts: Facts,
    pub paths: Vec<NfsPath>,
    /// When the hub began to send the notification, read before the command went out: what the config-server refreshes can be
    /// no newer than its answer, and a change that the registry saw after this may not be in it. A later time (the answer's, or
    /// the one at which the facts were read) would count such a change as told.
    pub now: Timestamp,
}

/// Who reads the paths: every service, or the applications named.
pub(crate) struct Readers {
    pub everyone: bool,
    pub apps: BTreeSet<String>,
}

pub(crate) fn readers(facts: &Facts, paths: &[NfsPath]) -> Readers {
    let mut out = Readers {
        everyone: false,
        apps: BTreeSet::new(),
    };
    for path in paths {
        match path.components().next() {
            // A file in the root folder has no folder: its first component is its own name.
            _ if path.parent().is_root() => out.everyone = true,
            Some(folder) if facts.apps.iter().any(|a| a.as_str() == folder) => {
                out.apps.insert(folder.to_owned());
            }
            // A folder that is no application's is a search location for all of them.
            _ => out.everyone = true,
        }
    }
    out
}

/// The kind of change a waiting file is, for the engine.
pub(crate) fn change_of(path: &NfsPath, facts: &Facts) -> PickupChange {
    if path.file_name() == "channels.yml" {
        PickupChange::ChannelsYml
    } else if file_role(path, &facts.tenants, &facts.apps) == domain::FileRole::PropertySource {
        PickupChange::PropertySource
    } else {
        PickupChange::ResourceFile
    }
}

/// `CONFIG_CLIENT_CACHE_TTL` in seconds: a number of seconds, or a number with `ms`, `s`, `m`, `h` or `d`, or an ISO duration
/// (`PT20M`, `PT1H30M`). Anything else, and zero, is the default of 20 minutes (D85): a value the hub cannot read must not make a
/// change look live sooner than it is.
pub(crate) fn cache_ttl_secs(text: &str) -> u32 {
    let text = text.trim();
    let parsed = if let Some(rest) = text.strip_prefix("PT").or_else(|| text.strip_prefix("pt")) {
        iso_seconds(rest)
    } else {
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        let (number, unit) = text.split_at(digits);
        let n: u64 = number.parse().ok().filter(|_| digits <= 9).unwrap_or(0);
        match unit.trim() {
            "" | "s" => Some(n),
            "ms" => Some(n.div_ceil(1000)),
            "m" => Some(n.saturating_mul(60)),
            "h" => Some(n.saturating_mul(3600)),
            "d" => Some(n.saturating_mul(86_400)),
            _ => None,
        }
    };
    match parsed {
        Some(secs) if secs > 0 => u32::try_from(secs).unwrap_or(u32::MAX),
        _ => DEFAULT_CACHE_TTL_SECS,
    }
}

/// The seconds of the part of an ISO duration after `PT`: `1H30M15S`.
fn iso_seconds(rest: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut number = String::new();
    let mut seen = false;
    for c in rest.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: u64 = number.parse().ok().filter(|_| number.len() <= 9)?;
        number.clear();
        let unit = match c.to_ascii_uppercase() {
            'H' => 3600,
            'M' => 60,
            'S' => 1,
            _ => return None,
        };
        total = total.saturating_add(n.saturating_mul(unit));
        seen = true;
    }
    (seen && number.is_empty()).then_some(total)
}

pub(crate) fn client_of(env: &serde_json::Value) -> ClientSettings {
    let text = |key: &str| env.get(key).and_then(serde_json::Value::as_str);
    ClientSettings {
        notifications_enabled: text(MONITOR).is_some_and(|v| v.trim().eq_ignore_ascii_case("true")),
        cache_ttl_secs: text(CACHE_TTL).map_or(DEFAULT_CACHE_TTL_SECS, cache_ttl_secs),
    }
}

/// What a service's waiting files come to after the notification.
struct Outcome {
    state: PickupState,
    waiting: Vec<String>,
    live_by: Option<i64>,
}

fn recompute(
    update: &PickupUpdate,
    client: ClientSettings,
    pending: &[String],
    changed_at: Timestamp,
    pod_starts: &[Timestamp],
    config_server_started: Timestamp,
) -> Outcome {
    let mut out = Outcome {
        state: PickupState::Live,
        waiting: Vec::new(),
        live_by: None,
    };
    for text in pending {
        let Ok(path) = NfsPath::parse(text) else {
            // A file the hub cannot read stays waiting, as the least it can claim.
            out.waiting.push(text.clone());
            out.state = out.state.max(PickupState::NeedsNotifyOrRestart);
            continue;
        };
        let picked = pickup_state(&PickupInput {
            change: change_of(&path, &update.facts),
            client,
            changed_at,
            now: update.now,
            notified_at: Some(update.now),
            pod_starts,
            config_server_started,
        });
        if picked.state != PickupState::Live {
            out.waiting.push(text.clone());
            out.state = out.state.max(picked.state);
            if let Some(by) = picked.live_by {
                out.live_by = out.live_by.max(Some(by.unix_millis()));
            }
        }
    }
    out
}

/// Record the notification in the transaction that records its audit event: the swimlane's `last_notified_at`, and the pickup state
/// of every service that read a notified path and had changes waiting. Publishes `PickupChanged` for the services whose state or
/// waiting files changed.
pub(crate) async fn apply(
    hub: &Inner,
    tx: &mut Transaction<'_, Postgres>,
    swimlane: &SwimlaneId,
    update: &PickupUpdate,
) -> Res<()> {
    let started: Option<Option<i64>> = sqlx::query_scalar(TOUCH)
        .bind(swimlane.as_str())
        .bind(update.now.unix_millis())
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| Internal::db("pickup_touch", &e))?;
    let config_server_started = Timestamp::from_unix_millis(started.flatten().unwrap_or(0));

    let who = readers(&update.facts, &update.paths);
    let apps: Vec<String> = who.apps.into_iter().collect();

    // The services are read a page at a time, by id, until a page is not full: a swimlane with more waiting services than a page
    // holds is updated whole, not left with the higher ids stale after every notification.
    let mut changed: Vec<ServiceRef> = Vec::new();
    let mut after = 0_i64;
    for page in 0..MAX_PICKUP_PAGES {
        let rows = sqlx::query(PENDING)
            .bind(swimlane.as_str())
            .bind(who.everyone)
            .bind(&apps)
            .bind(MAX_PICKUP_SERVICES)
            .bind(after)
            .fetch_all(&mut **tx)
            .await
            .map_err(|e| Internal::db("pickup_pending", &e))?;
        let full = i64::try_from(rows.len()).is_ok_and(|n| n >= MAX_PICKUP_SERVICES);
        if let Some(last) = rows.last() {
            after = last
                .try_get("id")
                .map_err(|e| Internal::db("pickup_pending", &e))?;
        }
        changed.extend(update_page(tx, update, config_server_started, rows).await?);
        if !full {
            break;
        }
        if page + 1 == MAX_PICKUP_PAGES {
            tracing::warn!(
                pages = MAX_PICKUP_PAGES,
                "a notification reached the limit of services it updates; the rest keep the state they had"
            );
        }
    }

    if !changed.is_empty() {
        hub.bus
            .publish_in_tx(
                &mut Tx::from_pg(tx),
                DomainEvent::PickupChanged {
                    swimlane: swimlane.clone(),
                    services: changed,
                },
            )
            .await
            .map_err(|_| Internal::new("commit", "event outbox"))?;
    }
    Ok(())
}

/// Recompute the pickup state of one page of waiting services and write the ones that changed. The services whose state or
/// waiting files changed, in order.
async fn update_page(
    tx: &mut Transaction<'_, Postgres>,
    update: &PickupUpdate,
    config_server_started: Timestamp,
    rows: Vec<sqlx::postgres::PgRow>,
) -> Res<Vec<ServiceRef>> {
    let mut changed: Vec<ServiceRef> = Vec::new();
    for row in rows {
        let get = |e: sqlx::Error| Internal::db("pickup_pending", &e);
        let id: i64 = row.try_get("id").map_err(get)?;
        let before: String = row.try_get("pickup_state").map_err(get)?;
        // Nothing but the config-server's restart helps a service that waits for it.
        if before == PickupState::NeedsConfigServerRestart.as_str() {
            continue;
        }
        let pending: serde_json::Value = row.try_get("pending_files").map_err(get)?;
        let Ok(pending) = serde_json::from_value::<Vec<String>>(pending) else {
            continue;
        };
        let env: serde_json::Value = row.try_get("env_values").map_err(get)?;
        let since: Option<i64> = row.try_get("pending_ms").map_err(get)?;
        let pod: Option<i64> = row.try_get("pod_ms").map_err(get)?;
        let pods: Vec<Timestamp> = pod.into_iter().map(Timestamp::from_unix_millis).collect();
        // The waiting changes were seen at least as early as the oldest of them; a service that does not say is taken to have
        // them from now, which a notification at `now` covers.
        let changed_at = Timestamp::from_unix_millis(since.unwrap_or(update.now.unix_millis()));

        let result = recompute(
            update,
            client_of(&env),
            &pending,
            changed_at,
            &pods,
            config_server_started,
        );
        if result.state.as_str() == before && result.waiting == pending {
            continue;
        }
        let still_waiting = !result.waiting.is_empty();
        sqlx::query(WRITE)
            .bind(id)
            .bind(result.state.as_str())
            .bind(serde_json::json!(result.waiting))
            .bind(if still_waiting { since } else { None })
            .bind(result.live_by)
            .execute(&mut **tx)
            .await
            .map_err(|e| Internal::db("pickup_write", &e))?;
        let namespace: String = row.try_get("namespace").map_err(get)?;
        let name: String = row.try_get("name").map_err(get)?;
        if let Ok(service) = ServiceRef::new(&namespace, &name) {
            changed.push(service);
        }
    }

    Ok(changed)
}

/// The statements of a notification, for the `EXPLAIN` test (P12).
pub(crate) fn queries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("notify_pickup_pending", PENDING),
        ("notify_touch", TOUCH),
        ("notify_pickup_write", WRITE),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_lifetime_is_read_the_way_a_person_writes_it() {
        for (text, secs) in [
            ("20m", 1_200),
            ("5m", 300),
            ("90", 90),
            ("90s", 90),
            ("2h", 7_200),
            ("1d", 86_400),
            ("1500ms", 2),
            (" 3m ", 180),
            ("PT20M", 1_200),
            ("PT1H30M", 5_400),
            ("PT1H30M15S", 5_415),
            ("pt45s", 45),
        ] {
            assert_eq!(cache_ttl_secs(text), secs, "{text}");
        }
    }

    #[test]
    fn a_lifetime_the_hub_cannot_read_is_the_default_and_never_shorter() {
        for text in [
            "",
            "m",
            "0",
            "0s",
            "PT",
            "PT0S",
            "PT5",
            "P1D",
            "-5m",
            "5 minutes",
            "5x",
            "1.5h",
            "PT1X",
            "99999999999m",
            "\u{663}m",
        ] {
            assert_eq!(cache_ttl_secs(text), DEFAULT_CACHE_TTL_SECS, "{text:?}");
        }
        // A long one saturates instead of wrapping.
        assert_eq!(cache_ttl_secs("999999999"), 999_999_999);
        assert_eq!(cache_ttl_secs("99999999d"), u32::MAX);
    }

    #[test]
    fn a_client_listens_only_when_it_says_true() {
        let on = |v: serde_json::Value| client_of(&v).notifications_enabled;
        assert!(on(serde_json::json!({ MONITOR: "true" })));
        assert!(on(serde_json::json!({ MONITOR: " TRUE " })));
        assert!(!on(serde_json::json!({ MONITOR: "false" })));
        assert!(!on(serde_json::json!({ MONITOR: "yes" })));
        assert!(
            !on(serde_json::json!({ MONITOR: true })),
            "a value is text, as the agent reports it"
        );
        assert!(!on(serde_json::json!({})));
        assert_eq!(
            client_of(&serde_json::json!({})).cache_ttl_secs,
            DEFAULT_CACHE_TTL_SECS
        );
        assert_eq!(
            client_of(&serde_json::json!({ CACHE_TTL: "2m" })).cache_ttl_secs,
            120
        );
    }
}
