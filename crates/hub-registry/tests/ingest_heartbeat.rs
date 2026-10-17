//! `heartbeat` (T1, D63, P3, A13): the hub compares the agent's root with the root of the last delta it applied.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use domain::HeartbeatAction;
use ports::ReportSink;
use support::rig::{Rig, T, delta, entry, root};

#[tokio::test]
async fn an_unknown_root_asks_for_a_full_scan_and_a_known_one_for_nothing() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    assert_eq!(
        rig.heartbeat(root(1)).await.unwrap(),
        HeartbeatAction::RequestFullScan
    );
    rig.delta(delta(1, None, root(1), vec![entry("a.yml", b"x: 1", T)]))
        .await
        .unwrap();
    assert_eq!(rig.heartbeat(root(1)).await.unwrap(), HeartbeatAction::None);
    assert_eq!(
        rig.heartbeat(root(2)).await.unwrap(),
        HeartbeatAction::RequestDelta { since_root: root(1) },
        "the delta is asked for since the root the hub holds"
    );
}

#[tokio::test]
async fn a_scan_the_hub_still_needs_is_asked_for_whatever_the_roots_say() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    sqlx::query("UPDATE swimlane_registry SET needs_full_scan = true WHERE swimlane_id = 'sit1'")
        .execute(&rig.pool)
        .await
        .unwrap();
    assert_eq!(
        rig.heartbeat(root(1)).await.unwrap(),
        HeartbeatAction::RequestFullScan
    );
}

#[tokio::test]
async fn a_full_scan_clears_the_need_for_one() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    sqlx::query("UPDATE swimlane_registry SET needs_full_scan = true WHERE swimlane_id = 'sit1'")
        .execute(&rig.pool)
        .await
        .unwrap();
    rig.delta(delta(2, None, root(2), vec![entry("a.yml", b"x: 1", T)]))
        .await
        .unwrap();
    assert!(!rig.needs_full_scan().await);
    assert_eq!(rig.heartbeat(root(2)).await.unwrap(), HeartbeatAction::None);
}

#[tokio::test]
async fn a_heartbeat_is_noted_on_the_agent() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    rig.clock.advance_secs(30);
    rig.ingest
        .heartbeat(
            &rig.identity,
            domain::Heartbeat {
                scan_seq: 41,
                merkle_root: root(1),
                file_count: 2_000,
            },
        )
        .await
        .unwrap();
    let (seq, files, at): (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT scan_seq, file_count, (EXTRACT(EPOCH FROM last_heartbeat_at) * 1000)::bigint FROM agents",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!((seq, files), (41, 2_000));
    assert_eq!(at, Some(hub_registry::ManualClock::T0_MS + 30_000));
}

#[tokio::test]
async fn a_delta_that_does_not_start_at_the_hubs_root_asks_for_a_full_scan() {
    // A13: the hub missed a delta, so the entries of this one may not be all the changes.
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(delta(
        2,
        Some(root(7)),
        root(8),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    ))
    .await
    .unwrap();
    assert!(rig.needs_full_scan().await);
    assert_eq!(
        rig.heartbeat(root(8)).await.unwrap(),
        HeartbeatAction::RequestFullScan
    );
}

#[tokio::test]
async fn a_delta_that_starts_at_the_hubs_root_does_not() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    ))
    .await
    .unwrap();
    assert!(!rig.needs_full_scan().await);
    assert_eq!(rig.root().await, Some(root(2)));
}

#[tokio::test]
async fn a_lost_stretch_of_the_spool_asks_for_a_full_scan() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    let mut d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    );
    d.gap = Some(domain::SpoolGap {
        from: domain::Timestamp::from_unix_millis(T),
        to: domain::Timestamp::from_unix_millis(T + 1_000),
        lost_entries: 12,
    });
    rig.delta(d).await.unwrap();
    assert!(rig.needs_full_scan().await, "D74: history was lost");
    assert_eq!(
        rig.heartbeat(root(2)).await.unwrap(),
        HeartbeatAction::RequestFullScan
    );
}
