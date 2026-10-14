//! The seam with prompt 06 (A10): a tool write is registered in the caller's transaction before the write is sent.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use domain::{DomainEvent, Guid, ObservationSource, UserId};
use hub_registry::ToolWriteRecord;
use ports::{Tx, content_hash};
use support::rig::{Rig, path};

fn user() -> UserId {
    UserId::new(
        Guid::parse("11111111-1111-1111-1111-111111111111").unwrap(),
        Guid::parse("22222222-2222-2222-2222-222222222222").unwrap(),
    )
}

#[tokio::test]
async fn a_tool_write_is_an_observation_with_a_certain_attribution_and_an_event() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let hash = content_hash(b"edited: true\n");
    let mut tx = rig.pool.begin().await.unwrap();
    let fresh = rig
        .ingest
        .tool_writes()
        .record(
            &mut Tx::from_pg(&mut tx),
            ToolWriteRecord {
                swimlane: &rig.swimlane,
                path: &path("svc/a.yml"),
                hash: Some(hash),
                size: Some(13),
                actor: &user(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(fresh);

    let obs = rig.observations("svc/a.yml").await;
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].source, "tool_write");
    assert_eq!(
        (
            obs[0].attribution_source.as_str(),
            obs[0].attribution_confidence.as_str()
        ),
        ("tool_write", "certain")
    );
    let actor = obs[0].attribution_actor.as_ref().unwrap();
    assert_eq!(actor["kind"], "user");
    assert_eq!(actor["user"], user().to_string());
    assert!(
        rig.state("svc/a.yml").await.is_none(),
        "a registered write is not a state: the file is current when a scan sees it (a failed write must leave no trace)"
    );
    let events = rig.bus.published().await;
    assert!(matches!(
        events.as_slice(),
        [DomainEvent::FileObserved { source: ObservationSource::ToolWrite, hash: h, .. }] if *h == hash
    ));
}

#[tokio::test]
async fn registering_twice_is_one_observation_and_one_event() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let u = user();
    let p = path("a.yml");
    let record = || ToolWriteRecord {
        swimlane: &rig.swimlane,
        path: &p,
        hash: Some(content_hash(b"x")),
        size: Some(1),
        actor: &u,
    };
    let mut tx = rig.pool.begin().await.unwrap();
    assert!(
        rig.ingest
            .tool_writes()
            .record(&mut Tx::from_pg(&mut tx), record())
            .await
            .unwrap()
    );
    assert!(
        !rig.ingest
            .tool_writes()
            .record(&mut Tx::from_pg(&mut tx), record())
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(rig.observations("a.yml").await.len(), 1);
    assert_eq!(rig.bus.count().await, 1);
}

#[tokio::test]
async fn a_rolled_back_transaction_leaves_no_trace() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let u = user();
    let p = path("a.yml");
    let mut tx = rig.pool.begin().await.unwrap();
    rig.ingest
        .tool_writes()
        .record(
            &mut Tx::from_pg(&mut tx),
            ToolWriteRecord {
                swimlane: &rig.swimlane,
                path: &p,
                hash: Some(content_hash(b"x")),
                size: Some(1),
                actor: &u,
            },
        )
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(rig.observations("a.yml").await.is_empty());
    assert_eq!(
        rig.bus.count().await,
        0,
        "D80: nothing is published for a change that did not commit"
    );
}

#[tokio::test]
async fn a_delete_is_registered_without_an_event() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let u = user();
    let p = path("a.yml");
    let mut tx = rig.pool.begin().await.unwrap();
    rig.ingest
        .tool_writes()
        .record(
            &mut Tx::from_pg(&mut tx),
            ToolWriteRecord {
                swimlane: &rig.swimlane,
                path: &p,
                hash: None,
                size: None,
                actor: &u,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rig.observations("a.yml").await[0].hash, None);
    assert_eq!(
        rig.bus.count().await,
        0,
        "FileObserved names a hash, and a delete has none"
    );
}

#[tokio::test]
async fn a_scan_that_sees_a_deletion_the_tool_made_is_not_out_of_band() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.clock.advance_secs(10);
    let u = user();
    let p = path("a.yml");
    let mut tx = rig.pool.begin().await.unwrap();
    rig.ingest
        .tool_writes()
        .record(
            &mut Tx::from_pg(&mut tx),
            ToolWriteRecord {
                swimlane: &rig.swimlane,
                path: &p,
                hash: None,
                size: None,
                actor: &u,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut gone = support::rig::delta(2, Some(support::rig::root(1)), support::rig::root(2), vec![]);
    gone.removed = vec![path("a.yml")];
    rig.delta(gone).await.unwrap();
    assert_eq!(rig.state("a.yml").await.unwrap().hash, None);
    assert!(
        support::pg_audit::audit_rows(&rig.pool).await.is_empty(),
        "the tool audits its own delete"
    );
}
