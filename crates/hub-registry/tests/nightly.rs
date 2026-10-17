//! The nightly full recompute (T3, D62, P6): the engine over every swimlane, compared with what is stored, with any divergence repaired
//! and counted. Over a real Postgres 16 and the in-memory Git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use hub_platform::jobs::{JobContext, JobError};
use hub_registry::RegistryConfig;
use support::world::{World, state_of};

async fn world() -> Option<World> {
    let mut w = World::new(&["sit1"]).await?;
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

async fn exec(w: &World, sql: &str) {
    sqlx::query(sql).execute(&w.rig.pool).await.unwrap();
}

#[tokio::test]
async fn a_clean_nightly_run_repairs_nothing_and_writes_nothing() {
    let Some(w) = world().await else { return };
    let tables = w.tables().await;
    let outbox = w.rig.count("SELECT count(*) FROM outbox").await;
    let report = w.pipeline.refresh_all().await.unwrap();
    assert_eq!(report.lanes, 1);
    assert_eq!(report.paths, 3);
    assert_eq!((report.repaired_paths, report.rows_changed), (0, 0), "{report:?}");
    assert!(report.failed.is_empty());
    assert_eq!(w.tables().await, tables);
    assert_eq!(
        w.rig.count("SELECT count(*) FROM outbox").await,
        outbox,
        "no event for a recompute that found nothing"
    );
}

#[tokio::test]
async fn nightly_full_recompute_repairs_a_corrupted_row() {
    let Some(w) = world().await else { return };
    let clean = w.tables().await;
    // A row, a finding, an effective hash and a baseline-free path damaged behind the pipeline's back.
    exec(
        &w,
        "UPDATE file_state SET state = 'conflict' WHERE path = 'app/drifted.yml'",
    )
    .await;
    exec(
        &w,
        "UPDATE file_state SET git_hash = NULL WHERE path = 'app/core.yml'",
    )
    .await;
    exec(
        &w,
        "DELETE FROM effective_files WHERE logical_file = 'app/core.yml'",
    )
    .await;
    exec(
        &w,
        "UPDATE effective_tree_hashes SET hash = sha256('damaged'::bytea) WHERE dir_path = 'app'",
    )
    .await;
    exec(
        &w,
        "UPDATE findings SET resolved_at = now() WHERE kind = 'drift' AND path = 'app/drifted.yml' AND resolved_at IS NULL",
    )
    .await;
    exec(
        &w,
        "INSERT INTO findings (swimlane_id, kind, path, severity, message, fingerprint) \
         VALUES ('sit1', 'drift', 'app/ghost.yml', 'high', 'a finding of a file that is not there', 'ghost')",
    )
    .await;
    assert_ne!(w.tables().await, clean, "the damage is there");
    let events = w.drift_events().await.len();

    let report = w.pipeline.refresh_all().await.unwrap();
    assert_eq!(report.lanes, 1);
    assert!(
        report.rows_changed >= 2,
        "the two damaged rows were rewritten: {report:?}"
    );
    assert_eq!(
        w.tables().await,
        clean,
        "everything is what a full computation says"
    );
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("unknown"));
    assert_eq!(
        report.repaired_paths, 0,
        "memory was right; the database was what was wrong"
    );
    assert!(
        w.drift_events().await.len() > events,
        "a repair of what readers can see is announced like any change"
    );
    let ghost: i64 = w
        .rig
        .count("SELECT count(*) FROM findings WHERE path = 'app/ghost.yml' AND resolved_at IS NULL")
        .await;
    assert_eq!(ghost, 0, "a finding that no check reports any more is resolved");
}

#[tokio::test]
async fn the_nightly_run_counts_what_the_incremental_pipeline_missed() {
    let Some(mut w) = world().await else { return };
    // An NFS change whose event never reached the pipeline.
    w.nfs_change(&[("app/core.yml", "a: 5\n")]).await;
    assert_eq!(
        state_of(&w, "app/core.yml").await.as_deref(),
        Some("in_sync"),
        "the pipeline has not heard of it"
    );
    let before = w.rig.metrics.pipeline_divergences_count();
    let report = w.pipeline.refresh_all().await.unwrap();
    assert_eq!(
        report.repaired_paths, 1,
        "memory said in_sync, the recompute says unknown: {report:?}"
    );
    assert_eq!(w.rig.metrics.pipeline_divergences_count() - before, 1);
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("unknown"));
    // And the next run finds nothing: the swimlane in memory is the new computation.
    let again = w.pipeline.refresh_all().await.unwrap();
    assert_eq!((again.repaired_paths, again.rows_changed), (0, 0));
}

#[tokio::test]
async fn swimlanes_are_computed_in_batches_and_one_that_has_no_inventory_is_not() {
    let config = RegistryConfig {
        nightly_batch_lanes: 1,
        ..RegistryConfig::default()
    };
    let Some(mut w) = World::with_config(&["sit1"], config).await else {
        return;
    };
    w.base_commit(&[("config/a.yml", Some("a: 1\n"))]);
    w.tenant_commit("sit1", &[]);
    w.inventory(&[("a.yml", "a: 1\n")]).await;
    w.second_lane("sit2", &["sit2"], &[("a.yml", "a: 1\n"), ("b.yml", "b: 1\n")])
        .await;
    // A third swimlane that has been created and said hello, but has not delivered its first inventory.
    w.rig.add_swimlane("sit3", &["sit3"], false).await;
    let report = w.pipeline.refresh_all().await.unwrap();
    assert_eq!(report.lanes, 2, "{report:?}");
    assert_eq!(report.paths, 1 + 2);
    assert_eq!(w.pipeline.lanes_in_memory().await.len(), 2);
}

#[tokio::test]
async fn the_nightly_job_runs_the_recompute_and_stops_when_its_lease_is_gone() {
    let Some(mut w) = world().await else { return };
    w.nfs_change(&[("app/core.yml", "a: 7\n")]).await;
    let job = w.pipeline.nightly_job();
    let (ctx, lease): (JobContext, _) =
        JobContext::held_until_told_otherwise(hub_registry::pipeline::NIGHTLY_JOB);
    job.run(&ctx).await.unwrap();
    assert_eq!(
        state_of(&w, "app/core.yml").await.as_deref(),
        Some("unknown"),
        "the job did the recompute"
    );

    lease.send_replace(tokio::time::Instant::now() - std::time::Duration::from_secs(1));
    assert_eq!(job.run(&ctx).await, Err(JobError::LeaseLost));
    let (spec, _) = w.pipeline.jobs().unwrap().remove(1);
    assert_eq!(spec.name(), "registry-nightly");
}
