//! The additions of the swimlane summary (`contract/08-web`, `CG4a`, Q6, D17, D72, D85): the tier, the project's name, the baseline and who
//! confirmed it, the pickup counts and the open sync window, over a real Postgres. The list and the detail say the same, and the person who
//! confirmed a baseline is named by id and display name, never by email.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{BaselineState, PickupState, SwimlaneSummary};
use hub_registry::RegistryApi;
use ports::RegistryRead;
use support::reads::{Reads, lane};

const TID: &str = "11111111-1111-1111-1111-111111111111";
const OID: &str = "22222222-2222-2222-2222-222222222222";

async fn reads() -> Option<Reads> {
    let r = Reads::new(&["sit1"]).await?;
    r.world.rig.add_swimlane("sit2", &["sit2"], false).await;
    Some(r)
}

async fn summary(r: &Reads, id: &str) -> SwimlaneSummary {
    let v = Reads::viewer();
    let listed = r
        .service
        .swimlanes(&v)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id.as_str() == id)
        .unwrap();
    // The detail is the same summary.
    let detail = r.service.swimlane(&v, &lane(id)).await.unwrap();
    assert_eq!(
        detail.summary, listed,
        "{id}: the list and the detail say the same"
    );
    listed
}

#[tokio::test]
async fn the_tier_and_the_project_are_the_swimlanes_own() {
    let Some(r) = reads().await else { return };
    r.sql(
        "INSERT INTO projects (id, display_name) VALUES ('core', 'Core Banking'); \
         UPDATE swimlanes SET tier = 'SIT', project_id = 'core' WHERE id = 'sit1'",
    )
    .await;
    let sit1 = summary(&r, "sit1").await;
    assert_eq!(
        sit1.tier.as_ref().map(ToString::to_string).as_deref(),
        Some("SIT")
    );
    assert_eq!(
        sit1.project.as_ref().map(ToString::to_string).as_deref(),
        Some("Core Banking"),
        "the project's display name, not its id"
    );
    let sit2 = summary(&r, "sit2").await;
    assert_eq!(sit2.tier, None, "no tier is set");
    assert_eq!(
        sit2.project.as_ref().map(ToString::to_string).as_deref(),
        Some("Project 1"),
        "a swimlane always has a project"
    );
    let json = serde_json::to_value(&sit2).unwrap();
    assert!(
        json.get("tier").is_none(),
        "absent, not null, when there is none: {json}"
    );
}

#[tokio::test]
async fn a_baseline_is_unconfirmed_until_editing_is_unlocked_with_a_time_and_names_who_confirmed_it() {
    let Some(r) = reads().await else { return };
    let none = summary(&r, "sit1").await.baseline.unwrap();
    assert_eq!(
        (none.state, none.by.is_none(), none.at.is_none()),
        (BaselineState::Unconfirmed, true, true)
    );

    // A confirmation time without the unlock, and the unlock without a time, are neither a confirmed baseline: the writes' lock is
    // `editing_unlocked` (D2) and the contract says a confirmed baseline says when.
    r.sql("UPDATE swimlanes SET baseline_confirmed_at = now() WHERE id = 'sit1'")
        .await;
    assert_eq!(
        summary(&r, "sit1").await.baseline.unwrap().state,
        BaselineState::Unconfirmed,
        "editing is still locked"
    );
    r.sql("UPDATE swimlanes SET baseline_confirmed_at = NULL, editing_unlocked = true WHERE id = 'sit1'")
        .await;
    assert_eq!(
        summary(&r, "sit1").await.baseline.unwrap().state,
        BaselineState::Unconfirmed,
        "unlocked with no time recorded"
    );

    r.sql(&format!(
        "INSERT INTO users (tid, oid, email, display_name, role, status) \
           VALUES ('{TID}', '{OID}', 'ada@example.com', 'Ada Lovelace', 'admin', 'active'); \
         UPDATE swimlanes SET editing_unlocked = true, baseline_confirmed_at = to_timestamp(1700000000), \
           baseline_confirmed_by_tid = '{TID}', baseline_confirmed_by_oid = '{OID}' WHERE id = 'sit1'"
    ))
    .await;
    let summary = summary(&r, "sit1").await;
    let confirmed = summary.baseline.clone().unwrap();
    assert_eq!(confirmed.state, BaselineState::Confirmed);
    assert_eq!(
        confirmed.at.map(domain::Timestamp::unix_millis),
        Some(1_700_000_000_000)
    );
    let by = confirmed.by.expect("who confirmed it");
    assert_eq!(by.id.to_string(), format!("{TID}:{OID}"));
    assert_eq!(by.display_name.as_str(), "Ada Lovelace");
    let json = serde_json::to_string(&summary).unwrap();
    assert!(
        !json.contains("ada@example.com") && !json.contains("email"),
        "never an email (Q6): {json}"
    );
}

#[tokio::test]
async fn an_open_adoption_is_in_progress_even_on_a_confirmed_swimlane_and_a_closed_one_is_not() {
    let Some(r) = reads().await else { return };
    r.world.adopt().await;
    assert_eq!(
        summary(&r, "sit1").await.baseline.unwrap().state,
        BaselineState::Confirmed
    );
    for status in ["in_progress", "review"] {
        r.sql(&format!(
            "INSERT INTO adoptions (swimlane_id, reason, status) VALUES ('sit1', 'tenant_suffix_changed', '{status}')"
        ))
        .await;
        let baseline = summary(&r, "sit1").await.baseline.unwrap();
        assert_eq!(
            (baseline.state, baseline.by.is_none(), baseline.at.is_none()),
            (BaselineState::InProgress, true, true),
            "{status}: an open adoption is in progress and names nobody, whatever was confirmed before"
        );
        r.sql("DELETE FROM adoptions").await;
    }
    for status in ["confirmed", "cancelled"] {
        r.sql(&format!(
            "INSERT INTO adoptions (swimlane_id, reason, status) VALUES ('sit1', 'initial', '{status}')"
        ))
        .await;
        assert_eq!(
            summary(&r, "sit1").await.baseline.unwrap().state,
            BaselineState::Confirmed,
            "{status}: a closed adoption changes nothing"
        );
    }
    // Another swimlane's adoption is not this one's.
    r.sql("INSERT INTO adoptions (swimlane_id, reason, status) VALUES ('sit2', 'initial', 'in_progress')")
        .await;
    assert_eq!(
        summary(&r, "sit1").await.baseline.unwrap().state,
        BaselineState::Confirmed
    );
    assert_eq!(
        summary(&r, "sit2").await.baseline.unwrap().state,
        BaselineState::InProgress
    );
}

#[tokio::test]
async fn the_pickup_counts_are_the_states_of_the_live_services_of_the_swimlane_that_are_not_live() {
    let Some(r) = reads().await else { return };
    r.sql(
        "INSERT INTO services (swimlane_id, namespace, name) VALUES \
           ('sit1', 'ns', 'a'), ('sit1', 'ns', 'b'), ('sit1', 'ns', 'c'), ('sit1', 'ns', 'd'), ('sit1', 'ns', 'gone'), ('sit2', 'ns', 'e'); \
         UPDATE services SET removed_at = now() WHERE name = 'gone'; \
         INSERT INTO service_state (service_id, pickup_state) SELECT id, CASE name \
           WHEN 'a' THEN 'needs_notify_or_restart' WHEN 'b' THEN 'needs_notify_or_restart' WHEN 'c' THEN 'live_within_ttl' \
           WHEN 'd' THEN 'live' WHEN 'gone' THEN 'needs_config_server_restart' ELSE 'needs_config_server_restart' END FROM services",
    )
    .await;
    let sit1 = summary(&r, "sit1").await;
    assert_eq!(
        sit1.pickup_counts,
        [
            (PickupState::LiveWithinTtl, 1),
            (PickupState::NeedsNotifyOrRestart, 2)
        ],
        "a service that is live, one that was removed and another swimlane's do not count"
    );
    // It is the same set `pending_restarts` lists.
    let v = Reads::viewer();
    let listed = r.service.pending_restarts(&v, &lane("sit1")).await.unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(
        summary(&r, "sit2").await.pickup_counts,
        [(PickupState::NeedsConfigServerRestart, 1)]
    );
    // A swimlane with nothing waiting lists none, and the key is absent on the wire.
    r.sql("UPDATE service_state SET pickup_state = 'live'").await;
    let quiet = summary(&r, "sit1").await;
    assert!(quiet.pickup_counts.is_empty());
    assert!(
        serde_json::to_value(&quiet)
            .unwrap()
            .get("pickup_counts")
            .is_none()
    );
}

#[tokio::test]
async fn the_sync_window_is_the_open_one_that_has_been_open_longest() {
    let Some(r) = reads().await else { return };
    assert!(summary(&r, "sit1").await.syncing.is_none());
    r.sql(
        "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at, closed_at) VALUES \
           ('sit1', 'sync-old', 'uid-old', to_timestamp(1700000000), to_timestamp(1700000100)), \
           ('sit1', 'sync-b', 'uid-b', to_timestamp(1700000300), NULL), \
           ('sit1', 'sync-a', 'uid-a', to_timestamp(1700000200), NULL), \
           ('sit2', 'sync-other', 'uid-other', to_timestamp(1700000050), NULL)",
    )
    .await;
    let syncing = summary(&r, "sit1").await.syncing.expect("a window is open");
    assert_eq!((syncing.job.name(), syncing.job.uid()), ("sync-a", "uid-a"));
    assert_eq!(syncing.since.unix_millis(), 1_700_000_200_000);
    r.sql("UPDATE sync_windows SET closed_at = now() WHERE swimlane_id = 'sit1'")
        .await;
    assert!(
        summary(&r, "sit1").await.syncing.is_none(),
        "closed windows are not listed"
    );
    assert_eq!(
        summary(&r, "sit2").await.syncing.unwrap().job.name(),
        "sync-other",
        "each swimlane has its own"
    );
}
