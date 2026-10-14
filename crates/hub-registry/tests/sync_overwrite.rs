//! What a sync replaced (T4, C7, D73): the context that makes `overwritten_nfs_change` possible is written on the observation of the
//! sync's version, when it arrives, because drift is recomputed on every change and the earlier state is gone by then.
//!
//! This file is the ingestion half (a version the agent tagged with a Job, or that falls in a sync window). The other half, a sync that
//! nobody tagged and that is reported with `record_sync`, is in `sync_record.rs` and `sequence_adoption_oob_sync_c7.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{ContentHash, JobRef};
use engine::drift_hash;
use support::world::{World, state_of};

fn drift(text: &str) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

/// An adopted swimlane `sit1` whose `app/core.yml` is in sync with Git, then edited on NFS and recomputed: `nfs_ahead`.
async fn ahead_on_nfs() -> Option<World> {
    let mut w = World::new(&["sit1"]).await?;
    w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    w.inventory(&[("app/core.yml", "a: 1\n")]).await;
    w.adopt().await;
    w.baseline("app/core.yml", drift("a: 1\n")).await;
    w.whole().await;
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("in_sync"));
    w.nfs_change(&[("app/core.yml", "a: 99\n")]).await;
    w.paths(&["app/core.yml"]).await;
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("nfs_ahead"));
    Some(w)
}

async fn open_window(w: &World, from_ms: i64) {
    sqlx::query(
        "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at) VALUES ('sit1', 'sync-sit1', 'uid-1', \
         timestamptz 'epoch' + ($1::bigint * interval '1 millisecond'))",
    )
    .bind(from_ms)
    .execute(&w.rig.pool)
    .await
    .unwrap();
}

async fn open_findings(w: &World, kind: &str) -> Vec<(String, String)> {
    w.tables()
        .await
        .findings
        .into_iter()
        .filter(|f| f.0 == kind)
        .map(|f| (f.2.unwrap_or_default(), f.3))
        .collect()
}

#[tokio::test]
async fn a_change_in_a_sync_window_over_an_nfs_only_change_carries_the_context_and_raises_c7() {
    let Some(mut w) = ahead_on_nfs().await else { return };
    open_window(&w, support::rig::T).await;
    // The sync puts Git's content back over the edit.
    w.nfs_change(&[("app/core.yml", "a: 1\n")]).await;

    let obs = w.rig.observations("app/core.yml").await;
    let sync = obs.last().unwrap();
    assert_eq!(sync.attribution_source, "sync_job");
    assert_eq!(
        sync.previous_state.as_deref(),
        Some("nfs_ahead"),
        "the state the pipeline had computed for the file the sync replaced"
    );
    assert_eq!(
        sync.previous_hash,
        Some(drift("a: 99\n")),
        "N of the replaced version: the hash for comparing"
    );
    assert!(
        obs[..obs.len() - 1].iter().all(|o| o.previous_state.is_none()),
        "only a sync's version carries it"
    );

    w.paths(&["app/core.yml"]).await;
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("in_sync"));
    let c7 = open_findings(&w, "overwritten_nfs_change").await;
    assert_eq!(c7.len(), 1, "{c7:?}");
    assert_eq!(c7[0].0, "app/core.yml");
    assert_eq!(c7[0].1, "critical", "C7 is critical by default (D76)");
}

#[tokio::test]
async fn a_change_the_agent_tags_with_a_job_does_the_same() {
    let Some(w) = ahead_on_nfs().await else { return };
    // The delta names the Job that was running, as the agent does (D75); the world's helper sends none, so the delta is built here.
    let root = w.rig.root().await.unwrap();
    let mut d = support::rig::delta(
        3,
        Some(root),
        support::rig::root(9),
        vec![support::rig::entry(
            "app/core.yml",
            b"a: 1\n",
            support::rig::T + 100_000,
        )],
    );
    d.during_job = Some(JobRef::new("sync-sit1", "uid-5").unwrap());
    w.rig.delta(d).await.unwrap();
    let obs = w.rig.observations("app/core.yml").await;
    let sync = obs.last().unwrap();
    assert_eq!(sync.attribution_source, "sync_job");
    assert_eq!(sync.previous_state.as_deref(), Some("nfs_ahead"));
    assert_eq!(sync.previous_hash, Some(drift("a: 99\n")));
}

#[tokio::test]
async fn a_change_nobody_attributes_to_a_sync_carries_no_context_and_raises_no_c7() {
    let Some(mut w) = ahead_on_nfs().await else { return };
    // No window, no job: the content goes back to Git's, and nothing says why.
    w.nfs_change(&[("app/core.yml", "a: 1\n")]).await;
    let obs = w.rig.observations("app/core.yml").await;
    let last = obs.last().unwrap();
    assert_eq!(last.attribution_source, "unknown");
    assert_eq!((last.previous_state.clone(), last.previous_hash), (None, None));
    w.paths(&["app/core.yml"]).await;
    assert!(open_findings(&w, "overwritten_nfs_change").await.is_empty());
}

#[tokio::test]
async fn a_sync_over_a_file_that_was_in_sync_is_not_c7() {
    let Some(mut w) = ahead_on_nfs().await else { return };
    // Put the file back in sync first (the person undoes the edit; no sync involved) and let the pipeline see it.
    w.nfs_change(&[("app/core.yml", "a: 1\n")]).await;
    w.paths(&["app/core.yml"]).await;
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("in_sync"));
    // A sync then changes it to something else, in a window.
    open_window(&w, support::rig::T).await;
    w.nfs_change(&[("app/core.yml", "a: 2\n")]).await;
    let obs = w.rig.observations("app/core.yml").await;
    let last = obs.last().unwrap();
    assert_eq!(last.attribution_source, "sync_job");
    assert_eq!(last.previous_state.as_deref(), Some("in_sync"));
    w.paths(&["app/core.yml"]).await;
    assert!(
        open_findings(&w, "overwritten_nfs_change").await.is_empty(),
        "the engine decides: only ahead or conflict makes C7"
    );
}
