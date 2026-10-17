//! The `registry-pipeline` job (T3, D62, D66, D80, P13): events through the real bus into the loop, the coalescing window, signals,
//! the reconcile, a takeover, and what a lost lease or a failing batch leaves. Over a real Postgres 16 and the real `PgEventBus`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::{Duration, Instant};

use domain::{DomainEvent, GitRef, RepoKind};
use engine::drift_hash;
use hub_platform::jobs::JobError;
use hub_registry::metrics::LaneResult;
use hub_registry::{RegistryConfig, Work};
use ports::EventBus;
use support::rig::path;
use support::world::{RunningJob, World, state_of, wait_until};

const WAIT: Duration = Duration::from_secs(20);

fn drift(text: &str) -> domain::ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

/// Git, an inventory and a swimlane computed whole (so the job has something to keep up to date).
async fn world_with(config: RegistryConfig) -> Option<World> {
    let mut w = World::with_config(&["sit1"], config).await?;
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/drifted.yml", Some("d: 0\n")),
    ]);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("a: 2\n"))]);
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "a: 2\n"),
        ("app/drifted.yml", "d: 1\n"),
    ])
    .await;
    w.whole().await;
    Some(w)
}

async fn world() -> Option<World> {
    world_with(RegistryConfig::default()).await
}

/// Start the job and wait until its takeover has loaded the swimlane, so that what a test then measures is a change and not the load.
async fn started(w: &World) -> RunningJob {
    let wholes = w.rig.metrics.pipeline_lane_count(LaneResult::Whole);
    let job = RunningJob::start(&w.pipeline);
    wait_until("the takeover to load the swimlane", WAIT, || async {
        w.rig.metrics.pipeline_lane_count(LaneResult::Whole) > wholes
            && !w.pipeline.lanes_in_memory().await.is_empty()
    })
    .await;
    job
}

// ------------------------------------------------------------------------------------------------ the loop

/// P13 (live update to subscribers within 1 s): the registry's share is the coalescing window and the recompute, and it leaves the rest of
/// the second to the event delivery (03b's `sse_delivery`). Measured over the real bus and the real clock, as the median of several
/// changes so that one slow scheduler tick on a shared machine is not the verdict; the bench `delta_to_drift_changed` holds the p95.
#[tokio::test]
async fn drift_changed_is_in_the_outbox_within_the_window_plus_compute() {
    const SAMPLES: usize = 5;
    let config = RegistryConfig::default();
    let window = config.coalesce_window;
    assert_eq!(
        window,
        Duration::from_millis(200),
        "the window P13 is budgeted with"
    );
    let Some(mut w) = world_with(config).await else {
        return;
    };
    let job = started(&w).await;

    let mut took = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let seen = w.drift_events().await.len();
        let at = Instant::now();
        w.nfs_change(&[("app/core.yml", format!("a: {}\n", 100 + i).as_str())])
            .await;
        wait_until("DriftChanged", WAIT, || async {
            w.drift_named_after(seen, "app/core.yml").await
        })
        .await;
        took.push(at.elapsed());
    }
    took.sort();
    let median = took[SAMPLES / 2];
    assert!(
        median < Duration::from_secs(1),
        "the window ({window:?}) plus the recompute must fit in the 1 s of P13, took {took:?}"
    );
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("unknown"));
    assert!(matches!(job.lose_lease().await, Err(JobError::LeaseLost)));
}

/// P1 (the registry's share): a change on NFS reaches `DriftChanged` within the 2 s the performance document gives the hub.
#[tokio::test]
async fn a_change_reaches_drift_changed_through_the_bus_and_the_window() {
    let Some(mut w) = world().await else { return };
    let job = started(&w).await;

    let seen = w.drift_events().await.len();
    let started = Instant::now();
    w.nfs_change(&[("app/core.yml", "a: 9\n")]).await;
    wait_until("DriftChanged", WAIT, || async {
        w.drift_named_after(seen, "app/core.yml").await
    })
    .await;
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "the registry's share of P1 (2 s from a delta to DriftChanged), took {took:?}"
    );
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("unknown"));
    assert!(matches!(job.lose_lease().await, Err(JobError::LeaseLost)));
}

#[tokio::test]
async fn a_burst_of_changes_to_one_swimlane_is_one_batch() {
    let Some(mut w) = world().await else { return };
    let job = started(&w).await;
    let before = w.rig.metrics.pipeline_lane_count(LaneResult::Incremental);
    let paths_before = w.rig.metrics.pipeline_paths_count();
    let seen = w.drift_events().await.len();

    // One scan delta with many files: many FileObserved events at once.
    let files: Vec<(String, String)> = (0..30)
        .map(|i| (format!("app/many-{i}.yml"), format!("n: {i}\n")))
        .collect();
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    w.nfs_change(&borrowed).await;
    wait_until("the last file", WAIT, || async {
        w.drift_named_after(seen, "app/many-29.yml").await
    })
    .await;
    let batches = w.rig.metrics.pipeline_lane_count(LaneResult::Incremental) - before;
    assert_eq!(
        batches, 1,
        "thirty events in one window are one recompute, not thirty"
    );
    assert_eq!(
        w.rig.metrics.pipeline_paths_count() - paths_before,
        30,
        "the thirty paths, once each"
    );
    let t = w.tables().await;
    assert!(t.rows.contains_key("app/many-0.yml") && t.rows.contains_key("app/many-29.yml"));
    let _ = job.lose_lease().await;
}

#[tokio::test]
async fn git_head_moved_recomputes_only_changed_paths() {
    let Some(w) = world().await else { return };
    let job = started(&w).await;
    let seen = w.drift_events().await.len();
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[
        ("config/app/drifted.yml", Some("d: 1\n")),
        ("README.md", Some("x")),
    ]);
    // The mirror publishes the event; the pipeline hears it from the bus.
    w.rig
        .events
        .as_ref()
        .unwrap()
        .publish(DomainEvent::GitHeadMoved {
            repo: RepoKind::Base,
            branch: GitRef::parse("master").unwrap(),
            from: Some(from),
            to,
        })
        .await
        .unwrap();
    wait_until("DriftChanged", WAIT, || async {
        w.drift_named_after(seen, "app/drifted.yml").await
    })
    .await;
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("in_sync"));
    assert_eq!(
        w.drift_events().await[seen..],
        [vec!["app/drifted.yml".to_owned()]],
        "that one path was named, and nothing else"
    );
    let _ = job.lose_lease().await;
}

#[tokio::test]
async fn a_signal_from_a_decision_recomputes_its_path() {
    let Some(w) = world().await else { return };
    let job = started(&w).await;
    // A person marks the drift of the file as intended; nothing is published for it, so the code that wrote the mark says so.
    w.mark("app/drifted.yml", Some(drift("d: 1\n")), Some(drift("d: 0\n")))
        .await;
    assert!(w.handle.paths_changed(w.lane(), vec![path("app/drifted.yml")]));
    wait_until("the mark to count", WAIT, || async {
        state_of(&w, "app/drifted.yml").await.as_deref() == Some("intentional_divergence")
    })
    .await;
    // And a whole-swimlane signal recomputes the swimlane whole.
    let wholes = w.rig.metrics.pipeline_lane_count(LaneResult::Whole);
    assert!(w.handle.lane_changed(w.lane()));
    wait_until("a whole recompute", WAIT, || async {
        w.rig.metrics.pipeline_lane_count(LaneResult::Whole) > wholes
    })
    .await;
    let _ = job.lose_lease().await;
}

// ------------------------------------------------------------------------------------------------ the reconcile

#[tokio::test]
async fn the_reconcile_finds_what_no_event_told_the_pipeline() {
    let Some(mut w) = world().await else { return };
    w.pipeline.remember_heads().await.unwrap();
    assert!(
        w.pipeline.reconcile().await.unwrap().work.is_empty(),
        "nothing differs yet: memory, the database and the mirror agree"
    );

    // An NFS change whose event was lost, a baseline written on another replica, and a Git push nobody announced.
    w.nfs_change(&[("app/core.yml", "a: 5\n")]).await;
    w.baseline("app/drifted.yml", drift("d: 0\n")).await;
    w.base_commit(&[("config/app/drifted.yml", Some("d: 7\n"))]);

    let found = w.pipeline.reconcile().await.unwrap();
    let named: std::collections::BTreeSet<String> = found
        .work
        .iter()
        .flat_map(|(_, work)| match work {
            Work::Paths(paths) => paths.iter().map(|p| p.as_str().to_owned()).collect::<Vec<_>>(),
            Work::Whole => vec!["<whole>".to_owned()],
        })
        .collect();
    assert!(named.contains("app/core.yml"), "the NFS change: {named:?}");
    assert!(
        named.contains("app/drifted.yml"),
        "the baseline and the push: {named:?}"
    );
    assert!(found.divergent_paths >= 2);

    // Doing that work brings the swimlane to what a full recompute gives.
    w.pipeline.process(found.work).await.unwrap();
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("unknown"));
    assert_eq!(
        state_of(&w, "app/drifted.yml").await.as_deref(),
        Some("conflict"),
        "N, G and the baseline all differ"
    );
    let tables = w.tables().await;
    let (fresh, _) = w
        .rig
        .pipeline(std::sync::Arc::new(w.git.clone()), RegistryConfig::default());
    fresh.refresh_lane(&w.lane()).await.unwrap();
    assert_eq!(
        w.tables().await,
        tables,
        "a full recompute has nothing left to repair"
    );
    assert!(
        w.pipeline.reconcile().await.unwrap().work.is_empty(),
        "and the next reconcile finds nothing"
    );
}

#[tokio::test]
async fn a_swimlane_that_was_never_computed_is_cold_until_it_is() {
    let Some(w) = World::new(&["sit1"]).await else {
        return;
    };
    let mut w = w;
    w.base_commit(&[("config/a.yml", Some("a: 1\n"))]);
    w.inventory(&[("a.yml", "a: 1\n")]).await;
    assert_eq!(w.pipeline.reconcile().await.unwrap().cold, vec![w.lane()]);
    w.whole().await;
    assert!(w.pipeline.reconcile().await.unwrap().cold.is_empty());
}

#[tokio::test]
async fn a_lost_lease_ends_the_run_and_the_next_one_starts_from_the_database() {
    let Some(w) = world().await else { return };
    let job = started(&w).await;
    assert!(matches!(job.lose_lease().await, Err(JobError::LeaseLost)));
    // What the replica knew may have been overtaken: the next run forgets it and loads again.
    let wholes = w.rig.metrics.pipeline_lane_count(LaneResult::Whole);
    let job = RunningJob::start(&w.pipeline);
    wait_until("a fresh load", WAIT, || async {
        w.rig.metrics.pipeline_lane_count(LaneResult::Whole) > wholes
    })
    .await;
    let _ = job.lose_lease().await;
}

#[tokio::test]
async fn a_run_that_ends_on_its_own_keeps_the_memory_for_the_next() {
    let config = RegistryConfig {
        pipeline_run: Duration::from_millis(1500),
        ..RegistryConfig::default()
    };
    let Some(w) = world_with(config).await else { return };
    let first = RunningJob::start(&w.pipeline);
    assert!(
        first.finished(Duration::from_secs(20)).await.is_ok(),
        "a run ends by itself after pipeline_run"
    );
    assert!(!w.pipeline.lanes_in_memory().await.is_empty());
    let wholes = w.rig.metrics.pipeline_lane_count(LaneResult::Whole);
    let second = RunningJob::start(&w.pipeline);
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        w.rig.metrics.pipeline_lane_count(LaneResult::Whole),
        wholes,
        "the state is kept between runs of one holder: no swimlane is loaded again"
    );
    let _ = second.finished(Duration::from_secs(20)).await;
}

// ------------------------------------------------------------------------------------------------ failures

#[tokio::test]
async fn a_batch_that_fails_is_tried_again_and_nothing_half_written_is_left() {
    let Some(mut w) = world().await else { return };
    w.adopt().await;
    w.whole().await;
    let job = started(&w).await;
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("unknown"));

    // NFS now has what Git has: they converge, the swimlane is adopted, so a baseline is set, and that needs an audit event. It fails.
    let seen = w.drift_events().await.len();
    w.pipeline_audit.fail_after(Some(0));
    w.nfs_change(&[("app/drifted.yml", "d: 0\n")]).await;
    wait_until("the failure", WAIT, || async {
        w.rig.metrics.pipeline_lane_count(LaneResult::Failed) >= 1
    })
    .await;
    assert_eq!(
        state_of(&w, "app/drifted.yml").await.as_deref(),
        Some("unknown"),
        "the transaction rolled back: no row moved without its audit event"
    );
    assert!(!w.tables().await.baselines.contains_key("app/drifted.yml"));
    assert!(
        !w.drift_named_after(seen, "app/drifted.yml").await,
        "and no event for it"
    );

    // The audit log is back: the batch that failed is tried again, from the database, and goes through.
    w.pipeline_audit.fail_after(None);
    wait_until("the retry", WAIT, || async {
        w.tables().await.baselines.contains_key("app/drifted.yml")
    })
    .await;
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("in_sync"));
    wait_until("the event", WAIT, || async {
        w.drift_named_after(seen, "app/drifted.yml").await
    })
    .await;
    let _ = job.lose_lease().await;
}
