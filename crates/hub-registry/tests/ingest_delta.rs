//! `delta` (T1, D47, D63, D74, D75, D79, D80, P1, P15, S11, S17): what the hub keeps of a scan, how it keeps it, and what a
//! failure, a replay or a crash leaves behind.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::Duration;

use domain::{DomainEvent, HeartbeatAction, JobRef, ObservationSource, ScanDelta, SwimlaneId};
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::{MessageKind, Outcome, VersionResult};
use ports::{ReportSink, SinkError, content_hash};
use support::pg_audit::audit_rows;
use support::rig::{Rig, T, delta, entry, part, path, root};

async fn file_observed(rig: &Rig) -> Vec<(String, String)> {
    rig.bus
        .published()
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::FileObserved { path, hash, .. } => {
                Some((path.as_str().to_owned(), hash.to_string()))
            }
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ D80: one transaction

#[tokio::test]
async fn delta_publishes_file_observed_in_the_same_transaction() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1"), ("svc/b.yml", "y: 1")]).await;
    let hash = |c: &str| content_hash(c.as_bytes()).to_string();
    assert_eq!(
        file_observed(&rig).await,
        vec![
            ("a.yml".to_owned(), hash("x: 1")),
            ("svc/b.yml".to_owned(), hash("y: 1"))
        ],
        "a version that was seen is published (T1: delta to FileObserved in the outbox)"
    );

    // An audit write that fails rolls back the change, its observation and its event.
    let before = (
        rig.count("SELECT count(*) FROM file_observations").await,
        rig.count("SELECT count(*) FROM outbox").await,
    );
    rig.audit.fail_after(Some(0));
    let d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    );
    assert_eq!(rig.delta(d.clone()).await, Err(SinkError::Unavailable));
    assert_eq!(
        (
            rig.count("SELECT count(*) FROM file_observations").await,
            rig.count("SELECT count(*) FROM outbox").await
        ),
        before
    );
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 1"))
    );
    assert_eq!(
        rig.root().await,
        Some(root(1)),
        "the root moves only when the delta is applied"
    );

    // So does an event that cannot be written.
    rig.audit.fail_after(None);
    rig.bus.set_failing(true);
    assert_eq!(rig.delta(d.clone()).await, Err(SinkError::Unavailable));
    assert_eq!(
        rig.count("SELECT count(*) FROM audit_events").await,
        0,
        "the audit event rolled back too"
    );
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 1"))
    );

    // Both healthy: the same delta, sent again as the agent does, applies.
    rig.bus.set_failing(false);
    rig.delta(d).await.unwrap();
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 2"))
    );
    assert_eq!(
        file_observed(&rig).await.last(),
        Some(&("a.yml".to_owned(), hash("x: 2")))
    );
    assert_eq!(rig.count("SELECT count(*) FROM audit_events").await, 1);
    assert_eq!(rig.root().await, Some(root(2)));
}

// ------------------------------------------------------------------------------------------------ what a scan records

#[tokio::test]
async fn the_first_scan_is_an_inventory_and_not_a_change() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[
        ("application.yml", "a: 1"),
        ("svc/api/app.yml", "b: 2"),
        ("svc/api/x/y.txt", "c"),
    ])
    .await;
    assert!(
        audit_rows(&rig.pool).await.is_empty(),
        "A11: the first inventory writes no audit event"
    );
    assert_eq!(rig.metrics.out_of_band_count(), 0);
    for p in ["application.yml", "svc/api/app.yml", "svc/api/x/y.txt"] {
        let obs = rig.observations(p).await;
        assert_eq!(obs.len(), 1, "{p}");
        assert!(obs[0].initial, "{p} is part of the first inventory");
        assert_eq!(obs[0].source, "scan");
    }
    assert_eq!(rig.count("SELECT count(*) FROM file_state").await, 3);
    // Folders are noted, as first seen in the inventory (C11 takes them as seen at the epoch).
    let folders: Vec<(String, bool)> = sqlx::query_as(
        "SELECT dir_path, initial FROM swimlane_folders WHERE swimlane_id = 'sit1' ORDER BY dir_path",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        folders,
        vec![
            ("svc".to_owned(), true),
            ("svc/api".to_owned(), true),
            ("svc/api/x".to_owned(), true)
        ]
    );
    let inventory: Option<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM first_inventory_at) * 1000)::bigint FROM swimlane_registry",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(inventory, Some(hub_registry::ManualClock::T0_MS));
    assert_eq!(rig.root().await, Some(root(1)));
    assert_eq!(rig.heartbeat(root(1)).await.unwrap(), HeartbeatAction::None);
    let acked: i64 = sqlx::query_scalar("SELECT acked_seq FROM agent_acks")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(acked, 1);
}

#[tokio::test]
async fn an_increment_before_any_full_scan_does_not_end_the_inventory() {
    // The hub has no picture of the swimlane (a new database, a lost row) and the first message to reach it is an increment from
    // the agent's spool. The inventory is the full scan that follows. Its files are not changes either, or a swimlane of 2,000
    // files raises 2,000 alarms (A11).
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    rig.delta(delta(1, Some(root(7)), root(8), vec![entry("a.yml", b"x: 2", T)]))
        .await
        .unwrap();
    assert!(
        rig.needs_full_scan().await,
        "A13: the hub cannot vouch for an increment on top of nothing"
    );
    let inventory = || async {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT (EXTRACT(EPOCH FROM first_inventory_at) * 1000)::bigint FROM swimlane_registry",
        )
        .fetch_one(&rig.pool)
        .await
        .unwrap()
    };
    assert_eq!(inventory().await, None, "an increment is not an inventory");

    rig.delta(delta(
        2,
        None,
        root(9),
        vec![
            entry("a.yml", b"x: 2", T + 1_000),
            entry("b.yml", b"y: 1", T + 1_000),
            entry("c/d.yml", b"z: 1", T + 1_000),
        ],
    ))
    .await
    .unwrap();
    assert!(
        audit_rows(&rig.pool).await.is_empty(),
        "the full scan that follows is the inventory"
    );
    assert_eq!(rig.metrics.out_of_band_count(), 0);
    assert!(!rig.needs_full_scan().await);
    assert_eq!(inventory().await, Some(hub_registry::ManualClock::T0_MS));
    assert!(rig.observations("b.yml").await[0].initial);

    // From now on a file nobody expected is a change.
    rig.delta(delta(
        3,
        Some(root(9)),
        root(10),
        vec![entry("e.yml", b"e: 1", T + 9_000)],
    ))
    .await
    .unwrap();
    assert_eq!(audit_rows(&rig.pool).await.len(), 1);
}

#[tokio::test]
async fn a_new_file_after_the_inventory_is_a_change_in_a_new_folder() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("fresh/dir/new.yml", b"n: 1", T + 9_000)],
    ))
    .await
    .unwrap();
    assert_eq!(
        audit_rows(&rig.pool).await.len(),
        1,
        "a file nobody expected is an out-of-band change"
    );
    let folders: Vec<(String, bool, i64)> = sqlx::query_as(
        "SELECT dir_path, initial, (EXTRACT(EPOCH FROM first_seen_at) * 1000)::bigint \
         FROM swimlane_folders ORDER BY dir_path",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        folders,
        vec![
            ("fresh".to_owned(), false, T + 9_000),
            ("fresh/dir".to_owned(), false, T + 9_000)
        ],
        "C11: a folder created after the first inventory is dated by its first file"
    );
    assert!(!rig.observations("fresh/dir/new.yml").await[0].initial);
}

#[tokio::test]
async fn an_out_of_band_change_is_audited_and_its_author_is_unknown() {
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
    let audit = audit_rows(&rig.pool).await;
    assert_eq!(audit.len(), 1);
    let a = &audit[0];
    assert_eq!(
        (a.action.as_str(), a.via.as_str()),
        ("file_change_detected", "agent_detected")
    );
    assert_eq!(a.swimlane.as_deref(), Some("sit1"));
    assert_eq!(a.path.as_deref(), Some("a.yml"));
    assert_eq!(a.hash_before, Some(content_hash(b"x: 1")));
    assert_eq!(a.hash_after, Some(content_hash(b"x: 2")));
    assert_eq!(a.attribution_source.as_deref(), Some("unknown"), "D39");
    let actor: (String, Option<String>) = sqlx::query_as("SELECT actor_kind, actor_ref FROM audit_events")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(actor, ("agent".to_owned(), Some("sit1".to_owned())));

    let obs = rig.observations("a.yml").await;
    assert_eq!(obs.len(), 2);
    assert_eq!(
        (
            obs[1].attribution_source.as_str(),
            obs[1].attribution_confidence.as_str()
        ),
        ("unknown", "low")
    );
    assert_eq!(obs[1].observed_at_ms, T + 5_000);
    assert_eq!(rig.metrics.out_of_band_count(), 1);
    let s = rig.state("a.yml").await.unwrap();
    assert_eq!(s.observed_at_ms, Some(T + 5_000));
}

#[tokio::test]
async fn a_registered_tool_write_is_not_out_of_band() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    let user = domain::UserId::new(
        domain::Guid::parse("11111111-1111-1111-1111-111111111111").unwrap(),
        domain::Guid::parse("22222222-2222-2222-2222-222222222222").unwrap(),
    );
    // Prompt 06 registers the write in its own transaction before it sends WriteFile (A10).
    let mut tx = rig.pool.begin().await.unwrap();
    let new_hash = content_hash(b"x: edited");
    let fresh = rig
        .ingest
        .tool_writes()
        .record(
            &mut ports::Tx::from_pg(&mut tx),
            hub_registry::ToolWriteRecord {
                swimlane: &rig.swimlane,
                path: &path("a.yml"),
                hash: Some(new_hash),
                size: Some(9),
                actor: &user,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(fresh);

    rig.clock.advance_secs(5);
    rig.delta(delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: edited", T + 700_000)],
    ))
    .await
    .unwrap();
    assert!(
        audit_rows(&rig.pool).await.is_empty(),
        "the tool audits its own writes; the scan that sees one is not a second change"
    );
    assert_eq!(rig.metrics.out_of_band_count(), 0);
    let scan = rig
        .observations("a.yml")
        .await
        .into_iter()
        .find(|o| o.source == "scan" && !o.initial)
        .unwrap();
    assert_eq!(
        (
            scan.attribution_source.as_str(),
            scan.attribution_confidence.as_str()
        ),
        ("tool_write", "certain")
    );
    assert_eq!(scan.attribution_actor.as_ref().unwrap()["kind"], "user");
    assert_eq!(rig.state("a.yml").await.unwrap().hash, Some(new_hash));
}

#[tokio::test]
async fn a_change_during_a_sync_job_is_attributed_to_the_job_and_audited_as_a_sync() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1"), ("b.yml", "y: 1")]).await;
    let mut d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    );
    d.during_job = Some(JobRef::new("sync-sit1", "uid-77").unwrap());
    rig.delta(d).await.unwrap();
    let obs = rig.observations("a.yml").await;
    assert_eq!(obs[1].source, "sync");
    assert_eq!(
        (
            obs[1].attribution_source.as_str(),
            obs[1].attribution_confidence.as_str()
        ),
        ("sync_job", "high")
    );
    assert_eq!(obs[1].during_job.as_deref(), Some("sync-sit1/uid-77"));
    let audit = audit_rows(&rig.pool).await;
    assert_eq!(
        (audit[0].via.as_str(), audit[0].action.as_str()),
        ("sync", "file_change_detected")
    );
    assert_eq!(rig.metrics.out_of_band_count(), 0, "a sync is explained");
    assert!(matches!(
        file_observed_sources(&rig).await.last(),
        Some(ObservationSource::Sync)
    ));

    // A window from the cluster report explains a change the delta does not tag.
    sqlx::query(
        "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at, closed_at) VALUES ('sit1', 'sync-sit1', 'uid-78', \
         timestamptz 'epoch' + ($1::bigint * interval '1 millisecond'), timestamptz 'epoch' + ($2::bigint * interval '1 millisecond'))",
    )
    .bind(T + 10_000)
    .bind(T + 20_000)
    .execute(&rig.pool)
    .await
    .unwrap();
    rig.delta(delta(
        3,
        Some(root(2)),
        root(3),
        vec![entry("b.yml", b"y: 2", T + 15_000)],
    ))
    .await
    .unwrap();
    assert_eq!(rig.observations("b.yml").await[1].attribution_source, "sync_job");
    rig.delta(delta(
        4,
        Some(root(3)),
        root(4),
        vec![entry("b.yml", b"y: 3", T + 25_000)],
    ))
    .await
    .unwrap();
    assert_eq!(
        rig.observations("b.yml").await[2].attribution_source,
        "unknown",
        "after the window closed the change is nobody's"
    );
}

async fn file_observed_sources(rig: &Rig) -> Vec<ObservationSource> {
    rig.bus
        .published()
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::FileObserved { source, .. } => Some(source),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ idempotence and replay

#[tokio::test]
async fn a_second_delivery_of_the_same_delta_changes_nothing() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    let d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    );
    rig.delta(d.clone()).await.unwrap();
    let count = |rig: &Rig| {
        let rig_pool = rig.pool.clone();
        async move {
            let n = |sql: &'static str| {
                let pool = rig_pool.clone();
                async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&pool).await.unwrap() }
            };
            (
                n("SELECT count(*) FROM file_observations").await,
                n("SELECT count(*) FROM audit_events").await,
                n("SELECT count(*) FROM outbox").await,
                n("SELECT version FROM swimlane_registry").await,
            )
        }
    };
    let before = count(&rig).await;
    rig.delta(d).await.unwrap();
    assert_eq!(
        count(&rig).await,
        before,
        "the same delta twice is the same tables (T1)"
    );
}

#[tokio::test]
async fn a_replayed_spool_loses_no_acknowledged_version() {
    // P15: after an outage the agent sends its spool again, and not one version that was acknowledged may be missing.
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let files = 12_usize;
    let deltas = 30_u64;
    let mut acknowledged: Vec<(String, String, i64)> = Vec::new();
    let mut spool: Vec<ScanDelta> = Vec::new();
    for k in 1..=deltas {
        let base = if k == 1 {
            None
        } else {
            Some(root(u8::try_from(k - 1).unwrap()))
        };
        let k_index = usize::try_from(k).unwrap();
        let entries = (0..5)
            .map(|j| {
                let p = format!("svc{}/file{}.yml", (k_index + j) % 3, (k_index * 5 + j) % files);
                let content = format!("version: {k}\nslot: {j}\n");
                let at = T + i64::try_from(k).unwrap() * 1_000 + i64::try_from(j).unwrap();
                acknowledged.push((p.clone(), content_hash(content.as_bytes()).to_string(), at));
                entry(&p, content.as_bytes(), at)
            })
            .collect();
        spool.push(delta(k, base, root(u8::try_from(k).unwrap()), entries));
    }
    // The first delta is a full scan, then increments, each acknowledged when `delta` returns Ok.
    for d in &spool {
        rig.delta(d.clone()).await.unwrap();
    }
    let recorded = |rig: &Rig| {
        let pool = rig.pool.clone();
        async move {
            sqlx::query_as::<_, (String, Vec<u8>, i64)>(
                "SELECT path, hash::bytea, (EXTRACT(EPOCH FROM observed_at) * 1000)::bigint FROM file_observations ORDER BY id",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    let first_pass = recorded(&rig).await;
    for (p, h, at) in &acknowledged {
        assert!(
            first_pass.iter().any(|(rp, rh, rat)| rp == p
                && domain::ContentHash::from_bytes(rh.clone().try_into().unwrap()).to_string() == *h
                && rat == at),
            "acknowledged version {p} {at} is missing"
        );
    }
    let state_before: Vec<_> = {
        let mut v = Vec::new();
        for i in 0..files {
            for s in 0..3 {
                let p = format!("svc{s}/file{i}.yml");
                if let Some(row) = rig.state(&p).await {
                    v.push((p, row.hash, row.observed_at_ms));
                }
            }
        }
        v
    };
    let (audit_before, outbox_before) = (
        rig.count("SELECT count(*) FROM audit_events").await,
        rig.count("SELECT count(*) FROM outbox").await,
    );

    // The hub restarts and the agent, which never saw the acknowledgements, sends everything again, in order.
    rig.hello().await;
    for d in &spool {
        rig.delta(d.clone()).await.unwrap();
    }
    assert_eq!(
        recorded(&rig).await,
        first_pass,
        "a replay records nothing twice and loses nothing"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM audit_events").await,
        audit_before,
        "no second audit event"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM outbox").await,
        outbox_before,
        "no second event"
    );
    for (p, hash, at) in state_before {
        let row = rig.state(&p).await.unwrap();
        assert_eq!(
            (row.hash, row.observed_at_ms),
            (hash, at),
            "{p}: the newest version stays current"
        );
    }
    assert!(
        rig.metrics.version_count(VersionResult::Replayed) > 0,
        "the replay was recognised as one"
    );
    assert!(
        !rig.needs_full_scan().await,
        "a replay in order is a continuous chain"
    );
}

#[tokio::test]
async fn a_version_older_than_the_stored_one_is_history_and_does_not_move_the_state_back() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 3", T + 9_000)],
    ))
    .await
    .unwrap();
    let audits = rig.count("SELECT count(*) FROM audit_events").await;
    let events = rig.count("SELECT count(*) FROM outbox").await;
    // A scan that saw the file before it changed arrives late.
    rig.delta(delta(
        3,
        Some(root(2)),
        root(3),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    ))
    .await
    .unwrap();
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 3"))
    );
    assert_eq!(
        rig.observations("a.yml").await.len(),
        3,
        "recorded: the history keeps every version (D47)"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM audit_events").await,
        audits,
        "the current state did not change"
    );
    assert_eq!(rig.count("SELECT count(*) FROM outbox").await, events);
    assert_eq!(rig.metrics.version_count(VersionResult::HistoryOnly), 1);
}

#[tokio::test]
async fn the_intermediate_versions_of_a_file_are_all_in_the_history() {
    // D74: the spool keeps every version, so a file that went a -> b -> c while the hub was away has three observations.
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "v: 0")]).await;
    let d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![
            entry("a.yml", b"v: 2", T + 8_000),
            entry("a.yml", b"v: 1", T + 4_000),
            entry("a.yml", b"v: 3", T + 12_000),
        ],
    );
    rig.delta(d).await.unwrap();
    let hashes: Vec<_> = rig
        .observations("a.yml")
        .await
        .into_iter()
        .map(|o| o.hash.unwrap())
        .collect();
    assert_eq!(
        hashes,
        ["v: 0", "v: 1", "v: 2", "v: 3"].map(|c| content_hash(c.as_bytes())),
        "oldest first"
    );
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"v: 3"))
    );
    let audit = audit_rows(&rig.pool).await;
    assert_eq!(
        audit.len(),
        1,
        "A12: one audit event for the path, not one for every version"
    );
    assert_eq!(audit[0].hash_before, Some(content_hash(b"v: 0")));
    assert_eq!(audit[0].hash_after, Some(content_hash(b"v: 3")));
    assert_eq!(
        file_observed(&rig)
            .await
            .iter()
            .filter(|(p, _)| p == "a.yml")
            .count(),
        2,
        "initial, then one for the path"
    );
}

// ------------------------------------------------------------------------------------------------ removals

#[tokio::test]
async fn a_removal_moves_the_state_and_is_published() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1"), ("b.yml", "y: 1")]).await;
    let mut d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("b.yml", b"y: 2", T + 5_000)],
    );
    d.removed = vec![path("a.yml")];
    rig.delta(d).await.unwrap();
    let s = rig.state("a.yml").await.unwrap();
    assert_eq!(
        (s.hash, s.drift_hash, s.size),
        (None, None, None),
        "the row stays, as the file is gone (nfs_ahead, N = empty)"
    );
    let obs = rig.observations("a.yml").await;
    assert_eq!(obs.len(), 2);
    assert_eq!(
        obs[1].hash, None,
        "a removal is an observation with no hash (D47)"
    );
    let audit = audit_rows(&rig.pool).await;
    let removal = audit.iter().find(|a| a.path.as_deref() == Some("a.yml")).unwrap();
    assert_eq!(removal.hash_before, Some(content_hash(b"x: 1")));
    assert_eq!(removal.hash_after, None);
    // The event names the version that went away: FileObserved has no empty hash.
    assert!(
        file_observed(&rig)
            .await
            .contains(&("a.yml".to_owned(), content_hash(b"x: 1").to_string()))
    );
    // A removal of a file that is not there changes nothing.
    let mut again = delta(3, Some(root(2)), root(3), vec![]);
    again.removed = vec![path("a.yml"), path("never/was.yml")];
    let events = rig.count("SELECT count(*) FROM outbox").await;
    rig.delta(again).await.unwrap();
    assert_eq!(rig.count("SELECT count(*) FROM outbox").await, events);
}

#[tokio::test]
async fn a_file_that_comes_back_is_a_new_version() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    let mut gone = delta(2, Some(root(1)), root(2), vec![]);
    gone.removed = vec![path("a.yml")];
    rig.delta(gone).await.unwrap();
    rig.delta(delta(
        3,
        Some(root(2)),
        root(3),
        vec![entry("a.yml", b"x: 1", T + 60_000)],
    ))
    .await
    .unwrap();
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 1"))
    );
    assert_eq!(rig.observations("a.yml").await.len(), 3);
}

#[tokio::test]
async fn a_full_scan_removes_the_files_it_no_longer_lists() {
    // D63: a full scan names every file there is. A file the hub holds that it does not name was deleted while nobody watched.
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1"), ("gone.yml", "g: 1"), ("c/d.yml", "d: 1")])
        .await;
    rig.delta(delta(
        2,
        None,
        root(2),
        vec![
            entry("a.yml", b"x: 1", T + 60_000),
            entry("c/d.yml", b"d: 1", T + 60_000),
        ],
    ))
    .await
    .unwrap();
    assert_eq!(rig.state("gone.yml").await.unwrap().hash, None);
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 1"))
    );
    assert_eq!(rig.metrics.version_count(VersionResult::Removed), 1);
    // A file the scan skipped (too large to read) is not gone.
    let mut with_skip = delta(3, None, root(3), vec![entry("a.yml", b"x: 1", T + 90_000)]);
    with_skip.skipped = vec![domain::SkippedEntry {
        path: path("c/d.yml"),
        reason: domain::ShortText::parse("too large").unwrap(),
    }];
    rig.delta(with_skip).await.unwrap();
    assert_eq!(
        rig.state("c/d.yml").await.unwrap().hash,
        Some(content_hash(b"d: 1"))
    );
}

// ------------------------------------------------------------------------------------------------ classification (D79, D17)

#[tokio::test]
async fn files_are_classified_and_their_format_and_drift_hash_are_recorded() {
    let Some(rig) = Rig::new().await else { return };
    rig.db.seed_tenant(&rig.pool, "sit1", "sit1").await;
    rig.hello().await;
    let crlf = b"a: 1\r\nb: 2\r\n";
    let bom = b"\xEF\xBB\xBFkey=value\n";
    rig.delta(delta(
        1,
        None,
        root(1),
        vec![
            entry("tx-api/tx-api-core.yml", crlf, T),
            entry("tx-api/tx-api-core-sit1.yml", b"a: 9\n", T),
            entry("tx-api/application.properties", bom, T),
            entry("report/layout.xsl", b"<xsl/>", T),
            entry("img/logo.png", b"\x89PNG\r\n\x1a\n", T),
            entry("README", b"plain text\n", T),
        ],
    ))
    .await
    .unwrap();

    let base = rig.state("tx-api/tx-api-core.yml").await.unwrap();
    assert_eq!((base.class.as_str(), base.kind.as_str()), ("structured", "base"));
    assert_eq!(base.eol.as_deref(), Some("crlf"));
    assert_eq!(base.encoding.as_deref(), Some("utf8"));
    assert_eq!(base.bom, Some(false));
    assert_eq!(
        base.hash,
        Some(content_hash(crlf)),
        "the raw hash is what blob URLs need"
    );
    assert_eq!(
        base.drift_hash,
        Some(content_hash(b"a: 1\nb: 2\n")),
        "D19: drift compares after line endings are normalised"
    );
    assert_eq!(
        base.state, "unknown",
        "no baseline and no Git yet: drift is the pipeline's"
    );
    assert_eq!(base.size, Some(i64::try_from(crlf.len()).unwrap()));
    assert_eq!(base.logical_file, "tx-api/tx-api-core.yml");

    let tenant = rig.state("tx-api/tx-api-core-sit1.yml").await.unwrap();
    assert_eq!(tenant.kind, "tenant", "the suffix is the swimlane's tenant");
    assert_eq!(
        tenant.logical_file, "tx-api/tx-api-core.yml",
        "D13: the same logical file as the base"
    );

    let props = rig.state("tx-api/application.properties").await.unwrap();
    assert_eq!(props.bom, Some(true));
    assert_eq!(props.role.as_deref(), Some("property_source"));
    assert_eq!(rig.state("report/layout.xsl").await.unwrap().class, "text");
    let png = rig.state("img/logo.png").await.unwrap();
    assert_eq!((png.class.as_str(), png.eol), ("binary", None));
    assert_eq!(
        png.drift_hash, png.hash,
        "binary files are compared by their hash"
    );
    assert_eq!(
        rig.state("README").await.unwrap().class,
        "text",
        "an extension-less UTF-8 file is text"
    );
    assert_eq!(
        rig.state("report/layout.xsl").await.unwrap().role.as_deref(),
        Some("resource")
    );
}

#[tokio::test]
async fn denied_files_are_recorded_by_name_size_and_hash_only() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let key = b"-----BEGIN PRIVATE KEY-----";
    // The agent applied its glob and sent no bytes.
    let mut flagged = entry("tls/app.pem", key, T);
    flagged.denied = true;
    flagged.bytes = None;
    // The agent's list was out of date: it sent a key the hub's globs deny.
    let leaked = entry("tls/server.key", b"SECRET-KEY-BYTES", T);
    rig.delta(delta(
        1,
        None,
        root(1),
        vec![flagged, leaked, entry("ok.yml", b"x: 1", T)],
    ))
    .await
    .unwrap();
    for p in ["tls/app.pem", "tls/server.key"] {
        let s = rig.state(p).await.unwrap();
        assert!(s.denied, "{p}");
        assert_eq!(s.class, "denied");
        assert!(s.hash.is_some(), "the hash is recorded");
    }
    assert!(!rig.blobs.has(&content_hash(key)).await);
    assert!(
        !rig.blobs.has(&content_hash(b"SECRET-KEY-BYTES")).await,
        "S17: a key never reaches blobs"
    );
    assert!(rig.blobs.has(&content_hash(b"x: 1")).await);
    assert_eq!(rig.blobs.stored().await, 1);
    assert!(rig.state("tls/app.pem").await.unwrap().eol.is_none());
}

#[tokio::test]
async fn a_denied_file_changing_is_audited() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let mut first = entry("tls/app.pem", b"one", T);
    first.denied = true;
    first.bytes = None;
    rig.delta(delta(1, None, root(1), vec![first])).await.unwrap();
    let mut second = entry("tls/app.pem", b"two", T + 5_000);
    second.denied = true;
    second.bytes = None;
    rig.delta(delta(2, Some(root(1)), root(2), vec![second]))
        .await
        .unwrap();
    let audit = audit_rows(&rig.pool).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].hash_after, Some(content_hash(b"two")));
}

#[tokio::test]
async fn content_is_stored_before_the_message_is_acknowledged() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let content = b"stored: yes\n";
    rig.delta(part(1, 0, true, None, root(1), vec![entry("a.yml", content, T)]))
        .await
        .unwrap();
    assert!(
        rig.blobs.has(&content_hash(content)).await,
        "D47: the version is in blobs when the part with `more` is acknowledged, before the delta is applied"
    );
    assert!(rig.state("a.yml").await.is_none());
    // A blob store that cannot take the content refuses the message, so no acknowledgement is sent.
    rig.blobs.set_failing(true);
    assert_eq!(
        rig.delta(part(2, 1, true, None, root(1), vec![entry("b.yml", b"b: 1", T)]))
            .await,
        Err(SinkError::Unavailable)
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM delta_staging").await,
        1,
        "the refused part was not staged"
    );
}

#[tokio::test]
async fn a_blob_store_that_never_answers_fails_the_message_and_frees_the_swimlane() {
    // Rule 5: every external call has a timeout. A blob store that hangs must not hold the swimlane's lock for ever.
    let config = RegistryConfig {
        blob_timeout: Duration::from_millis(200),
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.hello().await;
    rig.blobs.set_hanging(true);
    let d = delta(1, None, root(1), vec![entry("a.yml", b"x: 1", T)]);
    let answer = tokio::time::timeout(Duration::from_secs(10), rig.delta(d.clone()))
        .await
        .expect("the call must not wait for ever");
    assert_eq!(answer, Err(SinkError::Unavailable));
    assert_eq!(
        rig.metrics
            .message_count(MessageKind::Delta, Outcome::Unavailable),
        1
    );
    assert!(rig.state("a.yml").await.is_none());
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 0);

    // The lock was given back: once the store answers, the same delta goes through.
    rig.blobs.set_hanging(false);
    rig.delta(d).await.unwrap();
    assert!(rig.state("a.yml").await.is_some());
}

#[tokio::test]
async fn identical_content_is_stored_once() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let same = b"shared: 1\n";
    rig.delta(delta(
        1,
        None,
        root(1),
        vec![
            entry("a/x.yml", same, T),
            entry("b/x.yml", same, T),
            entry("c/x.yml", same, T),
        ],
    ))
    .await
    .unwrap();
    assert_eq!(rig.blobs.stored().await, 1);
    assert_eq!(rig.blobs.puts(), 1, "one put for the three files of one message");
    assert_eq!(rig.count("SELECT count(*) FROM file_state").await, 3);
}

// ------------------------------------------------------------------------------------------------ S11: bounds

#[tokio::test]
async fn size_and_entry_limits_are_enforced() {
    let config = RegistryConfig {
        max_file_bytes: 64,
        max_delta_parts: 3,
        max_delta_entries: 10,
        max_delta_bytes: 100,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.hello().await;

    // A file over the size the agent was told.
    let big = vec![b'x'; 65];
    assert_eq!(
        rig.delta(delta(1, None, root(1), vec![entry("big.yml", &big, T)]))
            .await,
        Err(SinkError::TooLarge)
    );
    // More entries than a message may carry.
    let many: Vec<_> = (0..=ScanDelta::MAX_ENTRIES)
        .map(|i| entry(&format!("f{i}.yml"), b"x", T))
        .collect();
    assert_eq!(
        rig.delta(delta(2, None, root(1), many)).await,
        Err(SinkError::TooLarge)
    );
    // Parts of one delta over the bound on parts.
    for (seq, n) in [(10, 0), (11, 1), (12, 2)] {
        rig.delta(part(
            seq,
            n,
            true,
            None,
            root(2),
            vec![entry(&format!("p{n}.yml"), b"x", T)],
        ))
        .await
        .unwrap();
    }
    assert_eq!(
        rig.delta(part(13, 3, true, None, root(2), vec![entry("p3.yml", b"x", T)]))
            .await,
        Err(SinkError::TooLarge),
        "a fourth part of a delta bounded to three"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM delta_staging").await,
        0,
        "an over-limit delta is dropped whole"
    );
    // Over the bound on entries and on bytes of a whole logical delta.
    for (seq, n) in [(20, 0), (21, 1)] {
        let entries = (0..5).map(|i| entry(&format!("e{n}_{i}.yml"), b"x", T)).collect();
        rig.delta(part(seq, n, true, None, root(3), entries))
            .await
            .unwrap();
    }
    let too_many = (0..2).map(|i| entry(&format!("e2_{i}.yml"), b"x", T)).collect();
    assert_eq!(
        rig.delta(part(22, 2, false, None, root(3), too_many)).await,
        Err(SinkError::TooLarge),
        "12 entries in a delta bounded to 10"
    );
    for (seq, n) in [(30, 0), (31, 1)] {
        rig.delta(part(
            seq,
            n,
            true,
            None,
            root(4),
            vec![entry(&format!("g{n}.yml"), &[b'y'; 40], T)],
        ))
        .await
        .unwrap();
    }
    assert_eq!(
        rig.delta(part(
            32,
            2,
            false,
            None,
            root(4),
            vec![entry("g2.yml", &[b'y'; 40], T)]
        ))
        .await,
        Err(SinkError::TooLarge),
        "120 bytes in a delta bounded to 100"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM file_state").await,
        0,
        "nothing of a refused delta is applied"
    );
}

#[tokio::test]
async fn invalid_messages_store_nothing() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let mut forged = entry("d.yml", b"z: 1", T);
    forged.hash = content_hash(b"something else");
    // One bad entry sinks the message: the good one beside it is not kept either.
    let good = entry("good.yml", b"g: 1", T);
    assert_eq!(
        rig.delta(delta(1, None, root(1), vec![good, forged])).await,
        Err(SinkError::Invalid)
    );
    assert_eq!(
        rig.blobs.stored().await,
        0,
        "validation happens before anything is stored"
    );
    assert_eq!(rig.count("SELECT count(*) FROM file_state").await, 0);

    let mut wrong_size = entry("s.yml", b"s: 1", T);
    wrong_size.size = 1;
    assert_eq!(
        rig.delta(delta(2, None, root(1), vec![wrong_size])).await,
        Err(SinkError::Invalid)
    );
    let mut before_the_epoch = entry("t.yml", b"t: 1", T);
    before_the_epoch.observed_at = domain::Timestamp::from_unix_millis(-1);
    assert_eq!(
        rig.delta(delta(3, None, root(1), vec![before_the_epoch])).await,
        Err(SinkError::Invalid)
    );
    let mut beyond_year_9999 = entry("u.yml", b"u: 1", T);
    beyond_year_9999.observed_at = domain::Timestamp::from_unix_millis(i64::MAX);
    assert_eq!(
        rig.delta(delta(4, None, root(1), vec![beyond_year_9999])).await,
        Err(SinkError::Invalid)
    );
    assert_eq!(rig.blobs.stored().await, 0);
}

#[tokio::test]
async fn a_time_in_the_future_is_clamped_to_the_hubs_clock() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    let a_year_ahead = hub_registry::ManualClock::T0_MS + 365 * 24 * 3600 * 1000;
    rig.delta(delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", a_year_ahead)],
    ))
    .await
    .unwrap();
    assert_eq!(
        rig.state("a.yml").await.unwrap().observed_at_ms,
        Some(hub_registry::ManualClock::T0_MS + 5 * 60 * 1000),
        "now plus the skew the config allows"
    );
    // So the next real change is still newer than the stored one.
    rig.clock.advance_secs(3600);
    rig.delta(delta(
        3,
        Some(root(2)),
        root(3),
        vec![entry(
            "a.yml",
            b"x: 3",
            hub_registry::ManualClock::T0_MS + 3_600_000,
        )],
    ))
    .await
    .unwrap();
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 3"))
    );
}

// ------------------------------------------------------------------------------------------------ multi-part deltas (A14)

#[tokio::test]
async fn a_multi_part_delta_applies_only_with_its_last_part() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(part(
        2,
        0,
        true,
        Some(root(1)),
        root(2),
        vec![entry("p0.yml", b"p0", T + 5_000)],
    ))
    .await
    .unwrap();
    rig.delta(part(
        3,
        1,
        true,
        Some(root(1)),
        root(2),
        vec![entry("p1.yml", b"p1", T + 5_001)],
    ))
    .await
    .unwrap();
    assert!(
        rig.state("p0.yml").await.is_none(),
        "nothing is applied before the last part"
    );
    assert_eq!(rig.root().await, Some(root(1)));
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 2);
    assert_eq!(
        rig.heartbeat(root(2)).await.unwrap(),
        HeartbeatAction::RequestDelta { since_root: root(1) }
    );
    rig.delta(part(
        4,
        2,
        false,
        Some(root(1)),
        root(2),
        vec![entry("p2.yml", b"p2", T + 5_002)],
    ))
    .await
    .unwrap();
    for p in ["p0.yml", "p1.yml", "p2.yml"] {
        assert!(rig.state(p).await.is_some(), "{p}");
    }
    assert_eq!(rig.root().await, Some(root(2)));
    assert_eq!(
        rig.count("SELECT count(*) FROM delta_staging").await,
        0,
        "the staging is cleared with the apply"
    );
    let acked: i64 = sqlx::query_scalar("SELECT acked_seq FROM agent_acks")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(acked, 4);
}

#[tokio::test]
async fn stream_end_before_the_last_part_applies_nothing() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(part(
        2,
        0,
        true,
        Some(root(1)),
        root(2),
        vec![entry("p0.yml", b"p0", T + 5_000)],
    ))
    .await
    .unwrap();
    // The stream ends here. Whatever else happens, the part is not applied.
    assert!(rig.state("p0.yml").await.is_none());
    assert_eq!(rig.root().await, Some(root(1)));
    // The agent reconnects and sends the delta again from its first part.
    rig.hello().await;
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 0);
    rig.delta(part(
        2,
        0,
        true,
        Some(root(1)),
        root(2),
        vec![entry("p0.yml", b"p0", T + 5_000)],
    ))
    .await
    .unwrap();
    rig.delta(part(
        3,
        1,
        false,
        Some(root(1)),
        root(2),
        vec![entry("p1.yml", b"p1", T + 5_001)],
    ))
    .await
    .unwrap();
    assert!(rig.state("p0.yml").await.is_some() && rig.state("p1.yml").await.is_some());
}

#[tokio::test]
async fn a_delta_missing_a_part_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(part(
        2,
        0,
        true,
        Some(root(1)),
        root(2),
        vec![entry("p0.yml", b"p0", T + 5_000)],
    ))
    .await
    .unwrap();
    // Part 1 never arrived: a last part numbered 2 cannot complete the delta.
    assert_eq!(
        rig.delta(part(
            4,
            2,
            false,
            Some(root(1)),
            root(2),
            vec![entry("p2.yml", b"p2", T + 5_002)]
        ))
        .await,
        Err(SinkError::Invalid)
    );
    assert!(rig.state("p0.yml").await.is_none() && rig.state("p2.yml").await.is_none());
    assert_eq!(rig.root().await, Some(root(1)));
    // A last part numbered above 0 with nothing staged is refused in the same way.
    assert_eq!(
        rig.delta(part(9, 3, false, Some(root(5)), root(6), vec![])).await,
        Err(SinkError::Invalid)
    );
}

#[tokio::test]
async fn a_part_sent_again_replaces_itself() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    for _ in 0..2 {
        rig.delta(part(1, 0, true, None, root(1), vec![entry("p0.yml", b"p0", T)]))
            .await
            .unwrap();
    }
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 1);
    rig.delta(part(2, 1, false, None, root(1), vec![entry("p1.yml", b"p1", T)]))
        .await
        .unwrap();
    assert_eq!(rig.count("SELECT count(*) FROM file_state").await, 2);
}

#[tokio::test]
async fn parts_of_different_deltas_are_not_mixed() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    // Parts of a delta that was abandoned (another new root) stay out of the one that completes.
    rig.delta(part(
        1,
        0,
        true,
        None,
        root(1),
        vec![entry("abandoned.yml", b"a", T)],
    ))
    .await
    .unwrap();
    rig.delta(part(2, 0, false, None, root(2), vec![entry("kept.yml", b"k", T)]))
        .await
        .unwrap();
    assert!(rig.state("kept.yml").await.is_some());
    assert!(rig.state("abandoned.yml").await.is_none());
    assert_eq!(
        rig.count("SELECT count(*) FROM delta_staging").await,
        1,
        "the abandoned part waits for its sweep"
    );
}

// ------------------------------------------------------------------------------------------------ crash safety

#[tokio::test]
async fn a_crash_between_chunks_is_repaired_by_the_retry() {
    // Two chunks of three paths. The audit log fails in the second, so the first chunk has committed and the second has not.
    let config = RegistryConfig {
        chunk_paths: 3,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    let names: Vec<String> = (0..6).map(|i| format!("f{i}.yml")).collect();
    let first: Vec<_> = names.iter().map(|n| entry(n, b"v: 1", T)).collect();
    rig.hello().await;
    rig.delta(delta(1, None, root(1), first)).await.unwrap();

    let changed: Vec<_> = names.iter().map(|n| entry(n, b"v: 2", T + 5_000)).collect();
    let d = delta(2, Some(root(1)), root(2), changed);
    rig.audit.fail_after(Some(3));
    assert_eq!(rig.delta(d.clone()).await, Err(SinkError::Unavailable));
    let moved = |rig: &Rig| {
        let names = names.clone();
        let pool = rig.pool.clone();
        async move {
            let mut n = 0;
            for name in &names {
                let h: Option<Vec<u8>> =
                    sqlx::query_scalar("SELECT nfs_hash::bytea FROM file_state WHERE path = $1")
                        .bind(name)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                if h.as_deref() == Some(content_hash(b"v: 2").as_bytes().as_slice()) {
                    n += 1;
                }
            }
            n
        }
    };
    assert_eq!(
        moved(&rig).await,
        3,
        "the first chunk committed, the second rolled back"
    );
    assert_eq!(
        rig.root().await,
        Some(root(1)),
        "the root moves last, so the agent still owes the delta"
    );
    assert!(
        rig.needs_full_scan().await,
        "A13: a failed apply that left chunks behind asks for a full scan"
    );
    assert_eq!(rig.count("SELECT count(*) FROM audit_events").await, 3);

    // The agent sends it again, unacknowledged as it was.
    rig.audit.fail_after(None);
    rig.delta(d.clone()).await.unwrap();
    assert_eq!(moved(&rig).await, 6);
    assert_eq!(rig.root().await, Some(root(2)));
    assert_eq!(
        rig.count("SELECT count(*) FROM audit_events").await,
        6,
        "one audit event for each path, none twice: the chunk that had committed found its paths equal"
    );
    let events = file_observed(&rig).await;
    for n in &names {
        assert_eq!(
            events
                .iter()
                .filter(|(p, h)| p == n && *h == content_hash(b"v: 2").to_string())
                .count(),
            1,
            "{n} is published once"
        );
    }
    // A full scan then clears the flag.
    let full: Vec<_> = names.iter().map(|n| entry(n, b"v: 2", T + 5_000)).collect();
    rig.delta(delta(3, None, root(3), full)).await.unwrap();
    assert!(!rig.needs_full_scan().await);
}

#[tokio::test]
async fn a_failure_before_any_chunk_committed_does_not_ask_for_a_full_scan() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.audit.fail_after(Some(0));
    let d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    );
    assert_eq!(rig.delta(d).await, Err(SinkError::Unavailable));
    assert!(
        !rig.needs_full_scan().await,
        "nothing was left behind, so the agent's retry is enough"
    );
}

#[tokio::test]
async fn one_swimlanes_deltas_do_not_touch_another() {
    let Some(rig) = Rig::new().await else { return };
    rig.db.seed_swimlane(&rig.pool, "sit2").await;
    let other = ports::AgentIdentity::new(SwimlaneId::parse("sit2").unwrap());
    rig.ingest
        .hello(&other, support::rig::hello("sit2"))
        .await
        .unwrap();
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.ingest
        .delta(
            &other,
            delta(1, None, root(9), vec![entry("a.yml", b"other: 1", T)]),
        )
        .await
        .unwrap();
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 1"))
    );
    let theirs: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT nfs_hash::bytea FROM file_state WHERE swimlane_id = 'sit2' AND path = 'a.yml'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(theirs.unwrap(), content_hash(b"other: 1").as_bytes().to_vec());
    assert_eq!(rig.root().await, Some(root(1)));
}
