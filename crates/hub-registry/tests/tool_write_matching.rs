//! Which observation a tool-write registration explains, over a real Postgres, with the clock moved by hand (never a sleep).
//!
//! Two places decide it and both are held to the port's rules here:
//!
//! - **on arrival** (ingestion, under the swimlane's lock): an observation applied while a registration is visible is explained by the
//!   oldest one that matches, which it consumes, and it drops the registrations it contradicts, all in its own transaction;
//! - **the late commit** (the leased matcher): an observation applied before the registration committed is explained by it afterwards,
//!   at most one observation for each registration, and only an observation that is eligible.
//!
//! `tests/tool_write_register.rs` holds the registration itself (the savepoint, the bound, the stamp).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::Duration;

use domain::DomainEvent;
use hub_registry::RegistryConfig;
use hub_registry::metrics::ToolWriteResult;
use support::pg_audit::audit_rows;
use support::rig::{Rig, T, path};
use support::tool_writes::{Script, hash_of, user};

const SECOND: i64 = 1_000;
const TTL: i64 = 15 * 60 * SECOND;

async fn registrations(rig: &Rig) -> i64 {
    rig.count("SELECT count(*) FROM tool_write_registrations").await
}

// ================================================================================================ on arrival

#[tokio::test]
async fn an_observation_applied_while_the_registration_is_visible_is_the_tools() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.registered("svc/a.yml", "x: 2", "req-1", 5).await;
    rig.clock.advance_secs(3);
    s.change(&[("svc/a.yml", "x: 2")]).await;

    let history = rig.history("svc/a.yml").await;
    assert_eq!(history.len(), 2);
    assert!(
        history[1].is_tool_write_of(5),
        "source tool_write, certain, the registration's user: {:?}",
        history[1]
    );
    assert!(history[0].is_unattributed(), "the first inventory is nobody's");
    assert!(
        audit_rows(&rig.pool).await.is_empty(),
        "the tool audits its own write: no FileChangeDetected"
    );
    assert_eq!(rig.metrics.out_of_band_count(), 0);
    assert_eq!(registrations(&rig).await, 0, "consumed, in the same transaction");
    assert_eq!(rig.metrics.tool_write_count(ToolWriteResult::MatchedOnArrival), 1);
    let events = rig.bus.published().await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            DomainEvent::FileObserved { source: domain::ObservationSource::ToolWrite, hash, .. } if *hash == hash_of("x: 2")
        )),
        "the observation's own source is published: {events:?}"
    );
    assert_eq!(rig.state("svc/a.yml").await.unwrap().hash, Some(hash_of("x: 2")));
}

#[tokio::test]
async fn the_same_hash_at_another_path_or_in_another_swimlane_does_not_match() {
    let Some(rig) = Rig::new().await else { return };
    rig.add_swimlane("sit2", &[], false).await;
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1"), ("svc/b.yml", "x: 1")]).await;
    // A registration for svc/a.yml of sit1 and one for svc/b.yml of sit2, both for the content "x: 2".
    rig.registered("svc/a.yml", "x: 2", "req-1", 5).await;
    {
        let mut tx = rig.caller().await;
        ports::ToolWrites::register(
            &rig.tool_writes,
            &mut ports::Tx::from_pg(&mut tx),
            &domain::SwimlaneId::parse("sit2").unwrap(),
            &path("svc/b.yml"),
            &hash_of("x: 2"),
            &user(6),
            &support::tool_writes::request("req-2"),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    rig.clock.advance_secs(3);
    // svc/b.yml of sit1 becomes "x: 2": the registrations are for svc/a.yml of sit1 and svc/b.yml of sit2.
    s.change(&[("svc/b.yml", "x: 2")]).await;
    let history = rig.history("svc/b.yml").await;
    assert!(
        history[1].is_unattributed(),
        "another path, another swimlane: nobody registered this: {:?}",
        history[1]
    );
    assert_eq!(rig.metrics.out_of_band_count(), 1, "it is an out-of-band change");
    let sit1: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tool_write_registrations WHERE swimlane_id = 'sit1'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(
        sit1, 1,
        "the registration of svc/a.yml was not touched (the path of the observation is another): the observation of x: 2 at svc/b.yml \
         is not an observation of svc/a.yml, so it neither explains nor contradicts it"
    );
    // The registration of sit2 is not touched either.
    let sit2: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tool_write_registrations WHERE swimlane_id = 'sit2'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(sit2, 1);
}

#[tokio::test]
async fn the_oldest_registration_by_id_explains_the_observation_and_the_other_stays() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    // Two requests registered the same content; the first one inserted is stamped LATER than the second (two replicas, two clocks).
    rig.clock.advance_secs(100);
    rig.registered("svc/a.yml", "x: 2", "req-first", 1).await;
    rig.clock.set_ms(hub_registry::ManualClock::T0_MS);
    rig.registered("svc/a.yml", "x: 2", "req-second", 2).await;
    rig.clock.advance_secs(200);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    let history = rig.history("svc/a.yml").await;
    assert!(
        history[1].is_tool_write_of(1),
        "the lowest id, whatever the stamps say: {:?}",
        history[1]
    );
    assert_eq!(
        rig.pending().await,
        vec![("svc/a.yml".to_owned(), false, "req-second".to_owned())],
        "the other waits for its own observation, the TTL or an invalidation"
    );
    // The file goes back and forth: x: 1, then x: 2 again, and the second registration explains that one.
    s.change(&[("svc/a.yml", "x: 1")]).await;
    // ... x: 1 contradicts it (another hash replaced the state it was for).
    assert_eq!(registrations(&rig).await, 0);
}

#[tokio::test]
async fn another_hash_invalidates_a_pending_hash_registration_at_once() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.registered("svc/a.yml", "x: 3", "req-1", 5).await;
    rig.clock.advance_secs(3);
    // Someone else writes x: 2: the state the registration was for has been replaced.
    s.change(&[("svc/a.yml", "x: 2")]).await;
    assert_eq!(
        registrations(&rig).await,
        0,
        "dropped at once, in the transaction of the observation"
    );
    assert_eq!(rig.metrics.tool_write_count(ToolWriteResult::Invalidated), 1);
    // A later write of the registered content is not the tool's.
    rig.clock.advance_secs(3);
    s.change(&[("svc/a.yml", "x: 3")]).await;
    let history = rig.history("svc/a.yml").await;
    assert!(
        history[1].is_unattributed() && history[2].is_unattributed(),
        "{history:?}"
    );
    // Dropping is not audited; the two changes are out of band, and that is all the audit log has.
    let audit = audit_rows(&rig.pool).await;
    assert_eq!(audit.len(), 2);
    assert!(audit.iter().all(|a| a.action == "file_change_detected"));
}

#[tokio::test]
async fn a_deletion_invalidates_a_pending_hash_registration() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.registered("svc/a.yml", "x: 2", "req-1", 5).await;
    rig.clock.advance_secs(3);
    s.delete(&["svc/a.yml"]).await;
    assert_eq!(
        registrations(&rig).await,
        0,
        "the file is gone: a write of x: 2 is not what the path holds"
    );
    // The file comes back with the registered content: not the tool's.
    rig.clock.advance_secs(3);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    assert!(rig.history("svc/a.yml").await.last().unwrap().is_unattributed());
}

#[tokio::test]
async fn a_later_hash_observation_invalidates_a_delete_registration() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.registered_delete("svc/a.yml", "x: 1", "req-1", 5).await;
    rig.clock.advance_secs(3);
    // Before the deletion is reported, someone writes the file: the delete registration was for a disappearance of x: 1.
    s.change(&[("svc/a.yml", "x: 2")]).await;
    assert_eq!(registrations(&rig).await, 0);
    rig.clock.advance_secs(3);
    s.delete(&["svc/a.yml"]).await;
    let history = rig.history("svc/a.yml").await;
    assert!(
        history.last().unwrap().is_unattributed(),
        "a later disappearance is not the tool's delete: {history:?}"
    );
}

#[tokio::test]
async fn a_deletion_is_explained_only_if_it_deleted_the_version_the_registration_names() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    // The tool deleted a version the agent has not reported (it is "x: 9" for the tool, "x: 1" for the registry's last known state).
    rig.registered_delete("svc/a.yml", "x: 9", "req-other", 5).await;
    rig.registered_delete("svc/a.yml", "x: 1", "req-right", 6).await;
    rig.clock.advance_secs(3);
    s.delete(&["svc/a.yml"]).await;
    let history = rig.history("svc/a.yml").await;
    assert!(
        history[1].is_tool_write_of(6),
        "the delete of the version that was deleted: {:?}",
        history[1]
    );
    assert_eq!(
        rig.pending().await,
        vec![("svc/a.yml".to_owned(), true, "req-other".to_owned())],
        "a delete of another version does not match, and a deletion does not contradict it"
    );
    assert!(
        audit_rows(&rig.pool).await.is_empty(),
        "the tool's own delete is not a change detected"
    );
    assert_eq!(rig.metrics.out_of_band_count(), 0);
}

#[tokio::test]
async fn a_registration_ages_out_by_the_registry_clock_and_not_a_millisecond_early() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1"), ("svc/b.yml", "y: 1")]).await;
    rig.registered("svc/a.yml", "x: 2", "req-a", 5).await;
    rig.registered("svc/b.yml", "y: 2", "req-b", 5).await;
    // 1 ms short of the TTL: still the tool's.
    rig.clock.advance_ms(TTL - 1);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    assert!(rig.history("svc/a.yml").await[1].is_tool_write_of(5));
    // At the TTL: expired. The agent's own clock is no part of it (the entries are dated minutes before the registry's clock).
    rig.clock.advance_ms(1);
    s.change(&[("svc/b.yml", "y: 2")]).await;
    assert!(
        rig.history("svc/b.yml").await[1].is_unattributed(),
        "a registration that expired explains nothing"
    );
    assert_eq!(
        registrations(&rig).await,
        1,
        "ingestion ignores it; the sweep of the matcher deletes it"
    );
}

#[tokio::test]
async fn a_registration_that_was_rolled_back_explains_nothing() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    let mut tx = rig.caller().await;
    rig.register_in(&mut tx, "svc/a.yml", "x: 2", "req-1", 5).await;
    tx.rollback().await.unwrap();
    rig.clock.advance_secs(3);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    assert!(rig.history("svc/a.yml").await[1].is_unattributed());
    assert_eq!(registrations(&rig).await, 0);
}

#[tokio::test]
async fn the_state_before_a_deletion_is_read_from_the_observations_in_the_order_they_were_applied() {
    // The path's observation order is the order the registry applied them (`id`), and the state before a deletion is the hash of the
    // closest earlier observation that has one. An older version that is replayed after a newer one is the later observation, whatever
    // its time, so it is what a deletion that follows deleted. `file_state` (the newest by time) says "x: 2" here, and is not asked.
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(1);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    // A replay of an older version: seen before the state it follows, so it is history only, and applied last.
    rig.clock.advance_secs(1);
    s.change_seen_at("svc/a.yml", "x: 0", T - 5_000).await;
    let history = rig.history("svc/a.yml").await;
    assert_eq!(history.len(), 3);
    assert_eq!(
        rig.state("svc/a.yml").await.unwrap().hash,
        Some(hash_of("x: 2")),
        "the state did not move back"
    );
    // A delete registration of the version the state has (x: 2) and one of the version the observations end with (x: 0).
    rig.registered_delete("svc/a.yml", "x: 2", "req-state", 5).await;
    rig.registered_delete("svc/a.yml", "x: 0", "req-observed", 6)
        .await;
    rig.clock.advance_secs(1);
    s.delete(&["svc/a.yml"]).await;
    let history = rig.history("svc/a.yml").await;
    assert!(
        history[3].is_tool_write_of(6),
        "the closest earlier observation with a hash is x: 0: {:?}",
        history[3]
    );
    assert_eq!(
        rig.pending().await,
        vec![("svc/a.yml".to_owned(), true, "req-state".to_owned())],
        "a safe false negative for the other: its version is not what the observations end with"
    );
}

// ================================================================================================ the late commit

/// The lost-wakeup window: the registration is made (stamped) before the file is written, the agent reports the file before the caller's
/// transaction commits, so the observation is applied while the registration is not visible. Returns the rig and the script.
async fn lost_wakeup(rig: &Rig) -> Script<'_> {
    let mut s = Script::start(rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_in(&mut tx, "svc/a.yml", "x: 2", "req-late", 5).await;
    rig.clock.advance_secs(2);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    // Nothing explains it yet: it is an out-of-band change.
    assert!(rig.history("svc/a.yml").await[1].is_unattributed());
    assert_eq!(rig.metrics.out_of_band_count(), 1);
    tx.commit().await.unwrap();
    s
}

#[tokio::test]
async fn an_observation_applied_before_the_registration_committed_is_attributed_by_the_matcher() {
    let Some(rig) = Rig::new().await else { return };
    let _s = lost_wakeup(&rig).await;
    rig.clock.advance_secs(1);

    let report = rig.matcher().run_once().await.unwrap();
    assert_eq!((report.matched, report.lanes, report.failed), (1, 1, 0));

    let history = rig.history("svc/a.yml").await;
    assert!(
        history[1].is_tool_write_of(5),
        "upgraded to the tool's, with the registration's user: {:?}",
        history[1]
    );
    assert_eq!(
        registrations(&rig).await,
        0,
        "the registration is consumed in the same transaction"
    );
    // One AttributionUpgraded audit event, written after the FileChangeDetected the observation had when it came first.
    let audit = audit_rows(&rig.pool).await;
    assert_eq!(
        audit.iter().map(|a| a.action.as_str()).collect::<Vec<_>>(),
        ["file_change_detected", "attribution_upgraded"]
    );
    let upgrade = &audit[1];
    assert_eq!(upgrade.via, "system");
    assert_eq!(upgrade.path.as_deref(), Some("svc/a.yml"));
    assert_eq!(upgrade.hash_after, Some(hash_of("x: 2")));
    assert_eq!(upgrade.attribution_source.as_deref(), Some("tool_write"));
    let request: Option<String> = sqlx::query_scalar(
        "SELECT request_id FROM audit_events WHERE action = 'attribution_upgraded' LIMIT 1",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        request.as_deref(),
        Some("req-late"),
        "the upgrade names the request of the write"
    );
    // One AttributionUpdated outbox event.
    let events = rig.bus.published().await;
    let updates: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, DomainEvent::AttributionUpdated { hash, .. } if *hash == hash_of("x: 2")))
        .collect();
    assert_eq!(updates.len(), 1, "{events:?}");
    assert_eq!(rig.metrics.tool_write_count(ToolWriteResult::MatchedLate), 1);

    // A second run finds nothing to do: no second upgrade, no second event.
    let again = rig.matcher().run_once().await.unwrap();
    assert_eq!((again.matched, again.lanes), (0, 0));
    assert_eq!(audit_rows(&rig.pool).await.len(), 2);
}

#[tokio::test]
async fn the_late_commit_works_in_both_orders() {
    // Order one: the registration is visible when the observation is applied (on arrival). Order two: it commits after (the matcher).
    // Either way the observation ends up the tool's, with the same attribution, and the registration is gone.
    let Some(early) = Rig::new().await else { return };
    let mut s = Script::start(&early, &[("svc/a.yml", "x: 1")]).await;
    early.clock.advance_secs(10);
    early.registered("svc/a.yml", "x: 2", "req-late", 5).await;
    early.clock.advance_secs(2);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    let first = early.history("svc/a.yml").await[1].clone();
    assert_eq!(
        early.matcher().run_once().await.unwrap().matched,
        0,
        "already explained: nothing for the matcher"
    );

    let Some(late) = Rig::new().await else { return };
    let _s = lost_wakeup(&late).await;
    late.clock.advance_secs(1);
    late.matcher().run_once().await.unwrap();
    let second = late.history("svc/a.yml").await[1].clone();

    assert!(first.is_tool_write_of(5) && second.is_tool_write_of(5));
    assert_eq!(
        (
            first.hash,
            first.source,
            first.attribution,
            first.confidence,
            first.actor
        ),
        (
            second.hash,
            second.source,
            second.attribution,
            second.confidence,
            second.actor
        )
    );
    assert_eq!(registrations(&early).await + registrations(&late).await, 0);
}

#[tokio::test]
async fn an_observation_applied_before_the_registration_was_stamped_is_not_the_tools_write() {
    // Someone else writes x: 2, then x: 3. Ten minutes later an editor reverts to x: 2 through the tool: the registration is stamped, the
    // matcher runs, and the editor's own write has not been reported yet. The old observation of x: 2 is not the editor's.
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(1);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    rig.clock.advance_secs(1);
    s.change(&[("svc/a.yml", "x: 3")]).await;
    rig.clock.advance_secs(600);
    rig.registered("svc/a.yml", "x: 2", "req-revert", 5).await;
    rig.clock.advance_secs(1);

    let report = rig.matcher().run_once().await.unwrap();
    assert_eq!(report.matched, 0);
    let history = rig.history("svc/a.yml").await;
    assert!(
        history[1].is_unattributed(),
        "the older x: 2 stays nobody's: {:?}",
        history[1]
    );
    assert_eq!(
        registrations(&rig).await,
        1,
        "and the registration waits for the editor's own write"
    );

    // The editor's write is then reported: it is explained on arrival, and is the one the registration is credited with.
    s.change(&[("svc/a.yml", "x: 2")]).await;
    let history = rig.history("svc/a.yml").await;
    assert!(history[3].is_tool_write_of(5));
    assert!(history[1].is_unattributed());
}

#[tokio::test]
async fn a_registration_that_commits_late_attributes_one_observation_the_newest_of_its_hash() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_in(&mut tx, "svc/a.yml", "x: 2", "req-late", 5).await;
    // x: 2, x: 3, x: 2 again, x: 3 again: two observations of the registered content, both applied after the stamp.
    for content in ["x: 2", "x: 3", "x: 2", "x: 3"] {
        rig.clock.advance_secs(2);
        s.change(&[("svc/a.yml", content)]).await;
    }
    tx.commit().await.unwrap();
    rig.matcher().run_once().await.unwrap();
    let history = rig.history("svc/a.yml").await;
    // [x: 1 inventory, x: 2, x: 3, x: 2, x: 3]
    assert!(
        history[3].is_tool_write_of(5),
        "the newest observation of x: 2: {:?}",
        history[3]
    );
    assert!(
        history[1].is_unattributed(),
        "older observations of the same hash stay as they were: {:?}",
        history[1]
    );
    assert!(history[2].is_unattributed() && history[4].is_unattributed());
    assert_eq!(registrations(&rig).await, 0);
}

#[tokio::test]
async fn the_matcher_upgrades_what_is_weaker_and_never_replaces_a_tool_write() {
    let Some(rig) = Rig::new().await else { return };
    let files: Vec<(String, String)> = (0..6)
        .map(|i| (format!("svc/f{i}.yml"), "x: 1".to_owned()))
        .collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let mut s = Script::start(&rig, &refs).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    for i in 0..6 {
        rig.register_in(&mut tx, &format!("svc/f{i}.yml"), "x: 2", &format!("req-{i}"), 5)
            .await;
    }
    rig.clock.advance_secs(2);
    let changes: Vec<(&str, &str)> = refs.iter().map(|(p, _)| (*p, "x: 2")).collect();
    s.change(&changes).await;
    // Each observation had a different attribution when the registration committed (set by hand, as the sentinel or a sync would).
    let weaker = [
        ("unknown", "low"),
        ("fs_owner_hint", "low"),
        ("nfs_client", "medium"),
        ("sentinel_login", "high"),
        ("sync_job", "high"),
    ];
    for (i, (source, confidence)) in weaker.iter().enumerate() {
        rig.attribute_by_hand(&format!("svc/f{i}.yml"), 1, source, confidence)
            .await;
    }
    // The sixth is a tool write already, by another user (an earlier registration explained it).
    sqlx::query(
        "UPDATE file_observations SET source = 'tool_write', attribution_source = 'tool_write', attribution_confidence = 'certain', \
                attribution_actor = $2::jsonb WHERE swimlane_id = $1 AND path = 'svc/f5.yml' AND hash IS NOT NULL AND NOT initial",
    )
    .bind(rig.swimlane.as_str())
    .bind(format!("{{\"kind\":\"user\",\"user\":\"{}\"}}", user(9)))
    .execute(&rig.pool)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let report = rig.matcher().run_once().await.unwrap();
    assert_eq!(report.matched, 5);
    for (i, had) in weaker.iter().enumerate() {
        let history = rig.history(&format!("svc/f{i}.yml")).await;
        assert!(
            history[1].is_tool_write_of(5),
            "f{i} had {had:?} and is upgraded: {:?}",
            history[1]
        );
    }
    let history = rig.history("svc/f5.yml").await;
    assert!(
        history[1].is_tool_write_of(9),
        "a tool write is never replaced by another: {:?}",
        history[1]
    );
    assert_eq!(
        rig.pending().await,
        vec![("svc/f5.yml".to_owned(), false, "req-5".to_owned())],
        "its registration finds nothing to explain and stays"
    );
}

#[tokio::test]
async fn the_oldest_registration_takes_the_observation_in_the_matcher_too() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(100);
    let mut tx = rig.caller().await;
    rig.register_in(&mut tx, "svc/a.yml", "x: 2", "req-first", 1)
        .await;
    // The second registration is inserted later and stamped earlier.
    rig.clock.set_ms(hub_registry::ManualClock::T0_MS + 99_500);
    rig.register_in(&mut tx, "svc/a.yml", "x: 2", "req-second", 2)
        .await;
    rig.clock.set_ms(hub_registry::ManualClock::T0_MS + 105_000);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    tx.commit().await.unwrap();
    rig.matcher().run_once().await.unwrap();
    assert!(
        rig.history("svc/a.yml").await[1].is_tool_write_of(1),
        "the lowest id"
    );
    assert_eq!(
        rig.pending().await,
        vec![("svc/a.yml".to_owned(), false, "req-second".to_owned())],
        "an observation never has two actors; the other registration stays"
    );
}

#[tokio::test]
async fn the_agents_clock_is_no_part_of_the_matchers_time_to_live() {
    // The agent saw the file two hours ago by its own clock, and the registry applied it a moment ago: it is a candidate.
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_in(&mut tx, "svc/a.yml", "x: 2", "req-a", 5).await;
    rig.clock.advance_secs(1);
    s.change_seen_at("svc/a.yml", "x: 2", T - 7_200_000).await;
    tx.commit().await.unwrap();
    rig.clock.advance_secs(1);
    assert_eq!(rig.matcher().run_once().await.unwrap().matched, 1);
    assert!(rig.history("svc/a.yml").await[1].is_tool_write_of(5));
}

/// An observation applied 500 ms before the registration was stamped (inside the skew allowed between two replicas' clocks), and the
/// matcher run `offset_ms` after the observation's time to live is over: how many observations it attributed.
async fn matched_when_the_matcher_runs_at_the_end_of_the_observations_ttl(offset_ms: i64) -> usize {
    let Some(rig) = Rig::new().await else { return 0 };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.set_ms(hub_registry::ManualClock::T0_MS + 9_500);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    rig.clock.set_ms(hub_registry::ManualClock::T0_MS + 10_000);
    rig.registered("svc/a.yml", "x: 2", "req-a", 5).await;
    rig.clock
        .set_ms(hub_registry::ManualClock::T0_MS + 9_500 + TTL + offset_ms);
    rig.matcher().run_once().await.unwrap().matched
}

#[tokio::test]
async fn an_observation_is_kept_for_the_ttl_from_when_the_registry_applied_it_and_not_a_millisecond_longer() {
    // The registration (stamped at +10 s) is live for 500 ms after the observation (applied at +9.5 s) has run out of time.
    assert_eq!(
        matched_when_the_matcher_runs_at_the_end_of_the_observations_ttl(-1).await,
        1
    );
    assert_eq!(
        matched_when_the_matcher_runs_at_the_end_of_the_observations_ttl(0).await,
        0
    );
}

#[tokio::test]
async fn the_contracts_example_of_a_late_delete_attributes_the_tools_deletion_and_nothing_else() {
    // A person deletes svc/a.yml over NFS (O0), someone recreates it out of band (O1, a hash), and an editor deletes it through the tool
    // (O2), which the agent reports before the editor's transaction commits.
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    s.delete(&["svc/a.yml"]).await;
    rig.clock.advance_secs(10);
    s.change(&[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_delete_in(&mut tx, "svc/a.yml", "x: 1", "req-delete", 7)
        .await;
    rig.clock.advance_secs(2);
    s.delete(&["svc/a.yml"]).await;
    tx.commit().await.unwrap();
    rig.clock.advance_secs(1);

    let report = rig.matcher().run_once().await.unwrap();
    assert_eq!(report.matched, 1);
    let history = rig.history("svc/a.yml").await;
    // [inventory x: 1, O0 deletion, O1 x: 1, O2 deletion]
    assert_eq!(history.len(), 4);
    assert!(
        history[3].is_tool_write_of(7),
        "the editor's deletion is the tool's: {:?}",
        history[3]
    );
    assert_eq!(history[3].hash, None);
    assert!(
        history[1].is_unattributed(),
        "the person's older deletion is not the tool's, whatever its age: {:?}",
        history[1]
    );
    assert!(history[2].is_unattributed() && history[0].is_unattributed());
    assert_eq!(registrations(&rig).await, 0);
    let audit = audit_rows(&rig.pool).await;
    let upgrade = audit
        .iter()
        .find(|a| a.action == "attribution_upgraded")
        .expect("one upgrade is audited");
    assert_eq!(
        upgrade.hash_before,
        Some(hash_of("x: 1")),
        "the version that went away"
    );
    assert_eq!(upgrade.hash_after, None);
    assert_eq!(
        audit
            .iter()
            .filter(|a| a.action == "attribution_upgraded")
            .count(),
        1
    );
    // Its outbox event names the version that went away, as FileObserved does for a removal.
    let events = rig.bus.published().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DomainEvent::AttributionUpdated { hash, .. } if *hash == hash_of("x: 1")))
    );
}

#[tokio::test]
async fn a_late_delete_of_another_version_never_matches() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    // The editor's expected hash was another version (the file had changed under it, and the agent had not yet reported it).
    rig.register_delete_in(&mut tx, "svc/a.yml", "x: 9", "req-delete", 7)
        .await;
    rig.clock.advance_secs(2);
    s.delete(&["svc/a.yml"]).await;
    tx.commit().await.unwrap();
    rig.matcher().run_once().await.unwrap();
    assert!(rig.history("svc/a.yml").await[1].is_unattributed());
    assert_eq!(
        registrations(&rig).await,
        1,
        "nothing was attributed, and the registration waits for its TTL"
    );
}

#[tokio::test]
async fn a_late_delete_does_not_take_a_deletion_that_a_hash_observation_follows() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_delete_in(&mut tx, "svc/a.yml", "x: 1", "req-delete", 7)
        .await;
    rig.clock.advance_secs(2);
    s.delete(&["svc/a.yml"]).await;
    rig.clock.advance_secs(2);
    // The file came back before the registration committed: the deletion is not the newest observation of the path.
    s.change(&[("svc/a.yml", "x: 5")]).await;
    tx.commit().await.unwrap();
    rig.matcher().run_once().await.unwrap();
    let history = rig.history("svc/a.yml").await;
    assert!(history[1].is_unattributed(), "{history:?}");
    assert_eq!(registrations(&rig).await, 1);
}

#[tokio::test]
async fn ingestion_and_the_matcher_read_the_state_before_a_deletion_from_the_same_place() {
    // The scenario of `the_state_before_a_deletion_is_read_from_the_observations...`, with the registrations committing after the
    // deletion: the matcher decides as ingestion did.
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(1);
    s.change(&[("svc/a.yml", "x: 2")]).await;
    rig.clock.advance_secs(1);
    s.change_seen_at("svc/a.yml", "x: 0", T - 5_000).await;
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_delete_in(&mut tx, "svc/a.yml", "x: 2", "req-state", 5)
        .await;
    rig.register_delete_in(&mut tx, "svc/a.yml", "x: 0", "req-observed", 6)
        .await;
    rig.clock.advance_secs(2);
    s.delete(&["svc/a.yml"]).await;
    tx.commit().await.unwrap();
    rig.matcher().run_once().await.unwrap();
    let history = rig.history("svc/a.yml").await;
    assert!(
        history[3].is_tool_write_of(6),
        "x: 0, as on arrival: {:?}",
        history[3]
    );
    assert_eq!(
        rig.pending().await,
        vec![("svc/a.yml".to_owned(), true, "req-state".to_owned())]
    );
}

#[tokio::test]
async fn the_state_before_a_deletion_skips_earlier_deletions_to_the_closest_hash() {
    // The closest earlier observation that HAS a hash: two deletions in a row (which ingestion does not write, one removal follows the
    // state it removes, but the history may hold, for instance after a restore) do not make the state before the second one unknown.
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    rig.clock.advance_secs(5);
    s.delete(&["svc/a.yml"]).await;
    rig.clock.advance_secs(5);
    let mut tx = rig.caller().await;
    rig.register_delete_in(&mut tx, "svc/a.yml", "x: 1", "req-delete", 7)
        .await;
    rig.clock.advance_secs(2);
    // A second deletion of the path, written as a restored history would hold it (the clock is +12 s).
    sqlx::query(
        "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, recorded_at) \
         VALUES ('sit1', 'svc/a.yml', NULL, 'scan', to_timestamp(1800000012), to_timestamp(1800000012))",
    )
    .execute(&rig.pool)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    rig.matcher().run_once().await.unwrap();
    let history = rig.history("svc/a.yml").await;
    assert_eq!(history.len(), 3);
    assert!(
        history[2].is_tool_write_of(7),
        "the state before it is x: 1, past the earlier deletion: {:?}",
        history[2]
    );
    assert!(history[1].is_unattributed());
}

#[tokio::test]
async fn the_attribution_statement_never_touches_a_tool_write_or_an_observation_nothing_is_above() {
    // Defence in depth under the matcher's own check: what the statement itself does when it is handed an observation that became
    // ineligible between the look and the write (another component upgraded it).
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(
        &rig,
        &[
            ("svc/a.yml", "x: 1"),
            ("svc/b.yml", "y: 1"),
            ("svc/c.yml", "z: 1"),
        ],
    )
    .await;
    rig.clock.advance_secs(2);
    s.change(&[
        ("svc/a.yml", "x: 2"),
        ("svc/b.yml", "y: 2"),
        ("svc/c.yml", "z: 2"),
    ])
    .await;
    rig.attribute_by_hand("svc/b.yml", 1, "tool_write", "certain")
        .await;
    rig.attribute_by_hand("svc/c.yml", 1, "sentinel_login", "certain")
        .await;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM file_observations WHERE hash IS NOT NULL AND NOT initial ORDER BY path LIMIT 10",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap();
    assert_eq!(ids.len(), 3);
    let actor = format!("{{\"kind\":\"user\",\"user\":\"{}\"}}", user(5));
    let upgraded: Vec<i64> = sqlx::query_scalar(hub_registry::tool_writes::ATTRIBUTE_SQL)
        .bind("sit1")
        .bind(&ids)
        .bind(vec![actor; 3])
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(
        upgraded,
        vec![ids[0]],
        "only the one that was weaker than certain and not a tool write"
    );
}

// ================================================================================================ the job

#[tokio::test]
async fn the_matcher_deletes_expired_registrations_in_bounded_batches() {
    let config = RegistryConfig {
        tool_write_sweep_batch: 3,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    let mut tx = rig.caller().await;
    for i in 0..35 {
        rig.register_in(
            &mut tx,
            &format!("svc/f{i}.yml"),
            &format!("v{i}"),
            &format!("req-{i}"),
            5,
        )
        .await;
    }
    tx.commit().await.unwrap();
    rig.clock.advance_ms(TTL - 1);
    assert_eq!(
        rig.matcher().run_once().await.unwrap().expired,
        0,
        "not a millisecond early"
    );
    rig.clock.advance_ms(1);
    let first = rig.matcher().run_once().await.unwrap();
    assert_eq!(first.expired, 30, "ten batches of three: a run is bounded");
    assert_eq!(registrations(&rig).await, 5);
    let second = rig.matcher().run_once().await.unwrap();
    assert_eq!(second.expired, 5);
    assert_eq!(registrations(&rig).await, 0);
    assert_eq!(rig.metrics.tool_write_count(ToolWriteResult::Expired), 35);
    // Registrations that are not expired are never touched by the sweep.
    rig.registered("svc/live.yml", "live", "req-live", 5).await;
    assert_eq!(rig.matcher().run_once().await.unwrap().expired, 0);
    assert_eq!(registrations(&rig).await, 1);
}

#[tokio::test]
async fn the_matcher_is_bounded_by_swimlanes_and_says_when_it_stopped() {
    let config = RegistryConfig {
        tool_write_match_lanes: 1,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.add_swimlane("sit2", &[], false).await;
    for lane in ["sit1", "sit2"] {
        let mut tx = rig.caller().await;
        ports::ToolWrites::register(
            &rig.tool_writes,
            &mut ports::Tx::from_pg(&mut tx),
            &domain::SwimlaneId::parse(lane).unwrap(),
            &path("svc/a.yml"),
            &hash_of("x"),
            &user(1),
            &support::tool_writes::request("req-1"),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let report = rig.matcher().run_once().await.unwrap();
    assert_eq!(
        (report.lanes, report.truncated),
        (1, true),
        "one swimlane, and it says there were more"
    );
}

#[tokio::test]
async fn the_matcher_stops_when_the_lease_is_lost_before_it_writes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Some(rig) = Rig::new().await else { return };
    let _s = lost_wakeup(&rig).await;
    // The lease is held for the start of the run and the first sweep, and lost before the swimlane is looked at.
    let asked = AtomicUsize::new(0);
    let lost = rig
        .matcher()
        .sweep(|| {
            if asked.fetch_add(1, Ordering::SeqCst) < 2 {
                Ok(())
            } else {
                Err(hub_platform::jobs::JobError::LeaseLost)
            }
        })
        .await;
    assert_eq!(lost, Err(hub_platform::jobs::JobError::LeaseLost));
    assert_eq!(asked.load(Ordering::SeqCst), 3);
    assert!(
        rig.history("svc/a.yml").await[1].is_unattributed(),
        "nothing was written"
    );
    assert_eq!(registrations(&rig).await, 1);
    assert_eq!(
        audit_rows(&rig.pool).await.len(),
        1,
        "only the change the apply detected"
    );
}

#[tokio::test]
async fn the_matcher_takes_the_swimlanes_lock_only_when_it_has_something_to_attribute() {
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(&rig, &[("svc/a.yml", "x: 1")]).await;
    // A registration that nothing explains.
    rig.registered("svc/a.yml", "x: 9", "req-quiet", 5).await;
    // Someone holds the swimlane's lock, as an apply does for the length of its chunk.
    let mut holder = rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query(hub_registry_lock_sql())
        .bind("sit1")
        .execute(&mut holder)
        .await
        .unwrap();

    let quiet = tokio::time::timeout(Duration::from_secs(5), rig.matcher().run_once()).await;
    assert!(
        quiet
            .expect("with nothing to attribute the matcher must not wait for the lock")
            .is_ok(),
        "the usual state must not queue behind every apply of the swimlane"
    );

    // With something to attribute it does wait for the lock, and goes on when the lock is released: it runs under it.
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_in(&mut tx, "svc/a.yml", "x: 2", "req-late", 6).await;
    rig.clock.advance_secs(2);
    // Applying needs the lock too, so the apply is made before the lock is taken: release, apply, take it again.
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
    s.change(&[("svc/a.yml", "x: 2")]).await;
    tx.commit().await.unwrap();
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query(hub_registry_lock_sql())
        .bind("sit1")
        .execute(&mut holder)
        .await
        .unwrap();
    let matcher = rig.matcher();
    let run = tokio::spawn(async move { matcher.run_once().await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!run.is_finished(), "it is waiting for the swimlane's lock");
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
    let report = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(report.matched, 1);
}

fn hub_registry_lock_sql() -> &'static str {
    // The advisory lock that serialises the applies of one swimlane (`ingest::apply::LOCK_SQL`), written out so that this test fails if the
    // matcher were to take another one.
    "SELECT pg_advisory_xact_lock(hashtextextended('lanekeeper.registry.apply.' || $1, 0))"
}

#[tokio::test]
async fn the_job_runs_through_the_runners_context() {
    let Some(rig) = Rig::new().await else { return };
    let _s = lost_wakeup(&rig).await;
    let spec = hub_registry::ToolWriteMatcher::spec(&RegistryConfig::default()).unwrap();
    assert_eq!(spec.name(), "registry-tool-write-matcher");
    let (ctx, _expiry) = hub_platform::jobs::JobContext::held_until_told_otherwise(spec.name());
    rig.matcher().job().run(&ctx).await.unwrap();
    assert!(rig.history("svc/a.yml").await[1].is_tool_write_of(5));
}

// ================================================================================================ S10

#[tokio::test]
async fn no_path_hash_or_user_is_in_a_log_line_of_matching_or_of_the_matcher() {
    use support::log_capture::LogCapture;
    let Some(rig) = Rig::new().await else { return };
    let mut s = Script::start(
        &rig,
        &[("svc/secret-path.yml", "x: 1"), ("svc/other-secret.yml", "y: 1")],
    )
    .await;
    let (capture, _guard) = LogCapture::install();

    // On arrival: explained, and another one contradicted.
    rig.registered(
        "svc/secret-path.yml",
        "x: 2 token-value",
        "req-secret-arrival",
        4242,
    )
    .await;
    rig.clock.advance_secs(3);
    s.change(&[("svc/secret-path.yml", "x: 2 token-value")]).await;

    // Late, with a statement that fails (a trigger on the observations' update), and then with one that works.
    rig.clock.advance_secs(10);
    let mut tx = rig.caller().await;
    rig.register_in(
        &mut tx,
        "svc/other-secret.yml",
        "y: 2 token-value",
        "req-secret-late",
        4243,
    )
    .await;
    rig.clock.advance_secs(2);
    s.change(&[("svc/other-secret.yml", "y: 2 token-value")]).await;
    tx.commit().await.unwrap();
    // A registration that cannot be read back (a user id of a shape this code does not know) is skipped, not an error.
    let mut admin = rig.db.admin().await;
    sqlx::raw_sql(
        "INSERT INTO tool_write_registrations (swimlane_id, path, hash, is_delete, user_tid, user_oid, request_id, registered_at) \
         VALUES ('sit1', 'svc/secret-path.yml', sha256('unreadable'), false, 'not-a-guid', 'secret-oid', 'req-secret-bad', to_timestamp(1800000016))",
    )
    .execute(&mut admin)
    .await
    .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION lk_fail_attribution() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'forced secret-path failure'; END $$; \
         CREATE TRIGGER lk_fail BEFORE UPDATE ON file_observations FOR EACH ROW EXECUTE FUNCTION lk_fail_attribution();",
    )
    .execute(&mut admin)
    .await
    .unwrap();
    rig.clock.advance_secs(1);
    let failed = rig.matcher().run_once().await.unwrap();
    assert_eq!(
        (failed.failed, failed.matched),
        (1, 0),
        "the statement failed and the lane is left for the next run"
    );
    sqlx::raw_sql("DROP TRIGGER lk_fail ON file_observations")
        .execute(&mut admin)
        .await
        .unwrap();
    assert_eq!(rig.matcher().run_once().await.unwrap().matched, 1);
    assert!(
        rig.metrics.tool_write_count(ToolWriteResult::Unreadable) >= 1,
        "the unreadable registration was met, skipped and counted"
    );

    let text = capture.text();
    assert!(
        text.contains("tool_write_attribute"),
        "the failed statement is logged as an operation: {text}"
    );
    for secret in [
        "secret-path".to_owned(),
        "other-secret".to_owned(),
        "svc/".to_owned(),
        hex::encode(hash_of("x: 2 token-value").as_bytes()),
        hex::encode(hash_of("y: 2 token-value").as_bytes()),
        "token-value".to_owned(),
        user(4242).oid().to_string(),
        user(4243).oid().to_string(),
        "req-secret".to_owned(),
        "forced".to_owned(),
        "secret-oid".to_owned(),
    ] {
        // sqlx logs the text of each statement (never its binds), and the trigger's text is a statement of this test's own.
        let leaked: Vec<&str> = text
            .lines()
            .filter(|l| l.contains(&secret) && !l.contains(" sqlx::query "))
            .collect();
        assert!(leaked.is_empty(), "S10: `{secret}` is in a log line: {leaked:?}");
    }
}
