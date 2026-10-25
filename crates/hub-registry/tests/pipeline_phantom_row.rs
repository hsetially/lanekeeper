//! Regression (T3): a file created and removed on NFS before the pipeline looked at it leaves no row behind.
//!
//! Ingestion writes a row for every file an agent reports, with the state `unknown`, and a removal only clears the hash of the file on
//! NFS: the row stays, so that the pipeline can see what happened to it. When a create and the removal of the same file are both in the
//! database before the pipeline is told about the path, the pipeline never held the file (it has nothing in memory for it), Git does not
//! have it, and the file is on neither side: it is not part of the swimlane. A whole computation deletes the row of such a path
//! (`lane_loader`: "a path on neither is not part of the swimlane and its row is deleted"); an incremental one must too, or the swimlane
//! lists a drift row (`unknown`, no file on NFS) that no computation from scratch would produce, that the engine's counts and findings do
//! not know, and that only a whole computation repairs.
//!
//! The counterexample proptest found in `pipeline_prop` (seed in `pipeline_prop.proptest-regressions`) is
//! `[Nfs { file: 2, text: 0 }, NfsDelete { file: 2 }]`, both steps unflushed: the first test below is that sequence by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use domain::{ContentHash, FileClass, RepoKind, SwimlaneId};
use engine::drift_hash;
use hub_registry::{RegistryConfig, Work};
use support::rig::path;
use support::world::{World, state_of};

const CORE: &str = "a: 1\n";
const TENANT_CORE: &str = "a: 2\n";
const NEW: &str = "n: 1\n";
const NEW_AGAIN: &str = "n: 2\n";
const FILE: &str = "app/new.yml";

/// A base file and a tenant file in Git and the same on NFS, computed whole: the starting point of the property test.
async fn start() -> Option<World> {
    let mut w = World::new(&["sit1"]).await?;
    w.base_commit(&[
        ("config/app/core.yml", Some(CORE)),
        ("config/application.yml", Some(CORE)),
    ]);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some(TENANT_CORE))]);
    w.inventory(&[
        ("app/core.yml", CORE),
        ("app/core-sit1.yml", TENANT_CORE),
        ("application.yml", CORE),
    ])
    .await;
    w.whole().await;
    Some(w)
}

/// The work the job would hold for a change of `FILE`: one piece per event, merged by the batch.
fn change_of_file(lane: &SwimlaneId) -> (SwimlaneId, Work) {
    (lane.clone(), Work::Paths([path(FILE)].into()))
}

/// What a swimlane computed from scratch is: a fresh pipeline, that knows nothing, finds nothing to repair and ends with the tables and
/// the engine state of the one that was brought up to date by events.
async fn assert_a_fresh_computation_agrees(w: &World) {
    let lane = w.lane();
    let tables = w.tables().await;
    let state = w.pipeline.lane_state(&lane).await;
    let (fresh, _) = w.rig.pipeline(Arc::new(w.git.clone()), RegistryConfig::default());
    let report = fresh.refresh_lane(&lane).await.unwrap();
    assert_eq!(
        report.lanes[0].rows_written + report.lanes[0].rows_deleted,
        0,
        "rows a computation from scratch would repair: {report:?}"
    );
    assert_eq!(w.tables().await, tables, "the tables");
    assert_eq!(fresh.lane_state(&lane).await, state, "the engine state");
}

fn drift_of(text: &str) -> ContentHash {
    drift_hash(Some(&bytes::Bytes::from(text.to_owned())), FileClass::Structured).unwrap()
}

/// The counterexample: a file is created and deleted on NFS, and the pipeline is told about the path once both are in the database.
#[tokio::test]
async fn a_file_created_and_removed_before_the_pipeline_looked_leaves_no_row() {
    let Some(mut w) = start().await else { return };
    let lane = w.lane();
    w.nfs_change(&[(FILE, NEW)]).await;
    w.nfs_delete(&[FILE]).await;
    // The precondition: ingestion left the row of a file that is gone (state `unknown`, no hash), and nothing has looked at it.
    let left = w
        .rig
        .state(FILE)
        .await
        .expect("ingestion keeps the row of a removed file");
    assert_eq!(left.hash, None);
    assert_eq!(left.state, "unknown");
    assert!(
        w.pipeline
            .lane_state(&lane)
            .await
            .is_some_and(|s| s.file(&path(FILE)).is_none())
    );
    let drift_events_before = w.drift_events().await.len();

    let report = w
        .pipeline
        .process(vec![change_of_file(&lane), change_of_file(&lane)])
        .await
        .unwrap();

    assert_eq!(report.lanes.len(), 1, "{report:?}");
    assert_eq!(
        report.lanes[0].rows_deleted, 1,
        "the row of the removed file: {report:?}"
    );
    assert!(
        w.rig.state(FILE).await.is_none(),
        "no row for a file that is on neither side"
    );
    assert!(!w.tables().await.rows.contains_key(FILE));
    assert!(
        w.drift_named_after(drift_events_before, FILE).await,
        "the row that was listed is gone: watchers are told, as for any other removal"
    );
    assert_a_fresh_computation_agrees(&w).await;
}

/// The same with a baseline recorded for the path: the baseline stays (it is the record of an earlier adoption), the row goes.
#[tokio::test]
async fn a_file_with_a_baseline_created_and_removed_before_the_pipeline_looked_leaves_no_row() {
    let Some(mut w) = start().await else { return };
    let lane = w.lane();
    w.baseline(FILE, drift_of(NEW)).await;
    w.nfs_change(&[(FILE, NEW)]).await;
    w.nfs_delete(&[FILE]).await;

    let report = w.pipeline.process(vec![change_of_file(&lane)]).await.unwrap();

    assert_eq!(report.lanes[0].rows_deleted, 1, "{report:?}");
    assert!(
        w.rig.state(FILE).await.is_none(),
        "no row for a file that is on neither side"
    );
    assert!(
        w.tables().await.baselines.contains_key(FILE),
        "a baseline is not the pipeline's to delete"
    );
    assert_a_fresh_computation_agrees(&w).await;
}

/// The control: the pipeline looks between the create and the removal, so it holds the file when it is removed. This was never wrong.
#[tokio::test]
async fn a_file_created_and_removed_with_a_pass_between_leaves_no_row() {
    let Some(mut w) = start().await else { return };
    let lane = w.lane();
    w.nfs_change(&[(FILE, NEW)]).await;
    w.pipeline.process(vec![change_of_file(&lane)]).await.unwrap();
    assert!(
        w.rig.state(FILE).await.is_some_and(|r| r.hash.is_some()),
        "the file is a row while it exists"
    );
    w.nfs_delete(&[FILE]).await;

    let report = w.pipeline.process(vec![change_of_file(&lane)]).await.unwrap();

    assert_eq!(report.lanes[0].rows_deleted, 1, "{report:?}");
    assert!(w.rig.state(FILE).await.is_none());
    assert_a_fresh_computation_agrees(&w).await;
}

/// The other side of the fix: a file that is created, removed and created again before the pipeline looks is on NFS, and its row stays,
/// with the hash of the last version. (Green before the fix as well: the row has a hash, so it is not the trace of a removed file.)
#[tokio::test]
async fn a_file_created_removed_and_created_again_in_one_batch_keeps_its_row() {
    let Some(mut w) = start().await else { return };
    let lane = w.lane();
    w.nfs_change(&[(FILE, NEW)]).await;
    w.nfs_delete(&[FILE]).await;
    w.nfs_change(&[(FILE, NEW_AGAIN)]).await;

    let report = w
        .pipeline
        .process(vec![
            change_of_file(&lane),
            change_of_file(&lane),
            change_of_file(&lane),
        ])
        .await
        .unwrap();

    assert_eq!(report.lanes[0].rows_deleted, 0, "{report:?}");
    let row = w
        .rig
        .state(FILE)
        .await
        .expect("the file is on NFS, so it has a row");
    assert_eq!(row.hash, Some(ports::content_hash(NEW_AGAIN.as_bytes())));
    assert_eq!(state_of(&w, FILE).await.as_deref(), Some(row.state.as_str()));
    assert_a_fresh_computation_agrees(&w).await;
}

/// The same with Git in it: the file is added to the base branch, created on NFS, deleted on NFS and removed from Git again, all
/// before the pipeline is told anything. Git's answer is the one of the batch (the file is not there), so it is on neither side.
#[tokio::test]
async fn a_file_added_and_removed_in_git_and_on_nfs_in_one_batch_leaves_no_row() {
    let Some(mut w) = start().await else { return };
    let lane = w.lane();
    let from = w.head(RepoKind::Base, "master");
    let added = w.base_commit(&[("config/app/new.yml", Some(NEW))]);
    let mut work = w.moved(RepoKind::Base, "master", Some(&from), &added).await;
    w.nfs_change(&[(FILE, NEW)]).await;
    w.nfs_delete(&[FILE]).await;
    let removed = w.base_commit(&[("config/app/new.yml", None)]);
    work.extend(w.moved(RepoKind::Base, "master", Some(&added), &removed).await);
    work.push(change_of_file(&lane));

    w.pipeline.process(work).await.unwrap();

    assert!(
        w.rig.state(FILE).await.is_none(),
        "no row for a file that is on neither side"
    );
    assert_a_fresh_computation_agrees(&w).await;
}

/// A database that already holds such a row (the pipeline left it before this fix; here ingestion's own, with no pass since): the
/// reconcile finds the path, and the work it queues is a pass over that path alone, which now removes the row. Before the fix the same
/// pass found nothing to delete, so the reconcile would queue the path again at every interval.
#[tokio::test]
async fn the_reconcile_repairs_a_row_left_for_a_file_that_is_on_neither_side() {
    let Some(mut w) = start().await else { return };
    let lane = w.lane();
    w.nfs_change(&[(FILE, NEW)]).await;
    w.nfs_delete(&[FILE]).await;
    assert!(w.rig.state(FILE).await.is_some_and(|r| r.hash.is_none()));
    // What the job does first: the heads it has handled are the ones there are, so the reconcile has no moved head to recompute.
    w.pipeline.remember_heads().await.unwrap();

    let found = w.pipeline.reconcile().await.unwrap();

    assert_eq!(found.divergent_paths, 1, "{found:?}");
    assert_eq!(
        found.work,
        vec![change_of_file(&lane)],
        "only the path that differs, not the whole swimlane"
    );
    let report = w.pipeline.process(found.work).await.unwrap();
    assert_eq!(report.lanes[0].rows_deleted, 1, "{report:?}");
    assert!(w.rig.state(FILE).await.is_none());
    // And it stays repaired: nothing differs the next time.
    let again = w.pipeline.reconcile().await.unwrap();
    assert_eq!((again.divergent_paths, again.work.len()), (0, 0), "{again:?}");
    assert_a_fresh_computation_agrees(&w).await;
}
