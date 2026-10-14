//! A baseline holds the raw version behind it, and keeps retention from collecting it (T3 gap found in T5, A24, D78).
//!
//! `baselines.blob_hash` names the raw version of the file the baseline was taken from, and a `blob_refs` row (`ref_kind` `baseline`,
//! `ref_id` `"{swimlane}:{path}"`) keeps the garbage collector from deleting it. `record_sync` (T4) wrote both. The pipeline wrote the
//! baselines that it sets itself, when NFS and Git converge on an adopted swimlane (A24), with the `blob_hash` and no `blob_refs` row, so
//! the one version a baseline stands for could be collected. These tests hold the two writers to the same rule: a baseline set by
//! convergence is referenced, the reference of the version it replaces is dropped, and a denied file has neither (D79).
//!
//! Over a real Postgres 16, the real pipeline and the in-memory Git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{ContentHash, RepoKind};
use ports::content_hash;
use sqlx::Row;
use support::world::World;

fn raw(text: &str) -> ContentHash {
    content_hash(text.as_bytes())
}

/// The hashes that hold a `baseline` reference named `ref_id`.
async fn refs(w: &World, ref_id: &str) -> Vec<ContentHash> {
    sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT hash::bytea FROM blob_refs WHERE ref_kind = 'baseline' AND ref_id = $1 ORDER BY hash",
    )
    .bind(ref_id)
    .fetch_all(&w.rig.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|b| ContentHash::from_bytes(b.try_into().unwrap()))
    .collect()
}

/// The raw version behind the baseline of a path, if it has one.
async fn blob_hash(w: &World, path: &str) -> Option<ContentHash> {
    sqlx::query("SELECT blob_hash::bytea AS b FROM baselines WHERE swimlane_id = 'sit1' AND path = $1")
        .bind(path)
        .fetch_optional(&w.rig.pool)
        .await
        .unwrap()
        .and_then(|r| r.get::<Option<Vec<u8>>, _>("b"))
        .map(|b| ContentHash::from_bytes(b.try_into().unwrap()))
}

async fn all_baseline_refs(w: &World) -> i64 {
    w.rig
        .count("SELECT count(*) FROM blob_refs WHERE ref_kind = 'baseline'")
        .await
}

#[tokio::test]
async fn a_baseline_set_by_convergence_is_referenced_and_the_replaced_version_is_released() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    // NFS has another version than Git: no baseline, `unknown`.
    w.inventory(&[("app/core.yml", "a: 0\n")]).await;
    w.adopt().await;
    w.whole().await;
    assert!(w.tables().await.baselines.is_empty());
    assert_eq!(all_baseline_refs(&w).await, 0);

    // NFS moves to what Git has: they converge, and the pipeline sets the baseline.
    w.nfs_change(&[("app/core.yml", "a: 1\n")]).await;
    let report = w.paths(&["app/core.yml"]).await;
    assert_eq!(report.lanes[0].baselines_set, 1, "{report:?}");
    assert_eq!(
        blob_hash(&w, "app/core.yml").await,
        Some(raw("a: 1\n")),
        "the baseline names the raw version it was taken from"
    );
    assert_eq!(
        refs(&w, "sit1:app/core.yml").await,
        vec![raw("a: 1\n")],
        "and the version is referenced, so that retention keeps it (A24, D78)"
    );

    // Both sides move on to a third version and converge again: the baseline moves, and so does its reference.
    w.nfs_change(&[("app/core.yml", "a: 2\n")]).await;
    w.paths(&["app/core.yml"]).await;
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[("config/app/core.yml", Some("a: 2\n"))]);
    let report = w
        .pipeline
        .process(w.moved(RepoKind::Base, "master", Some(&from), &to).await)
        .await
        .unwrap();
    assert_eq!(report.lanes[0].baselines_set, 1, "{report:?}");
    assert_eq!(blob_hash(&w, "app/core.yml").await, Some(raw("a: 2\n")));
    assert_eq!(
        refs(&w, "sit1:app/core.yml").await,
        vec![raw("a: 2\n")],
        "the version the baseline replaced is no longer held by it"
    );
    assert_eq!(all_baseline_refs(&w).await, 1, "one baseline, one reference");

    // A recompute that changes nothing writes nothing more.
    w.whole().await;
    assert_eq!(refs(&w, "sit1:app/core.yml").await, vec![raw("a: 2\n")]);
}

#[tokio::test]
async fn a_whole_swimlane_that_converges_references_every_baseline_it_sets() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/more.yml", Some("m: 1\n")),
    ]);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t: 1\n"))]);
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/more.yml", "m: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/untracked.yml", "u: 1\n"),
    ])
    .await;
    w.adopt().await;
    w.whole().await;
    let t = w.tables().await;
    assert_eq!(t.baselines.len(), 3, "the three files that agree with Git: {t:?}");
    for (path, text) in [
        ("app/core.yml", "a: 1\n"),
        ("app/more.yml", "m: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
    ] {
        assert_eq!(blob_hash(&w, path).await, Some(raw(text)), "{path}");
        assert_eq!(refs(&w, &format!("sit1:{path}")).await, vec![raw(text)], "{path}");
    }
    assert_eq!(
        all_baseline_refs(&w).await,
        3,
        "the untracked file has no baseline and so no reference"
    );
}

#[tokio::test]
async fn a_denied_file_has_a_baseline_and_no_blob_behind_it() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/server.key", Some("KEY")),
    ]);
    w.inventory(&[("app/core.yml", "a: 1\n")]).await;
    w.nfs_raw(|at| {
        let mut e = support::rig::entry("server.key", b"KEY", at);
        e.denied = true;
        e.bytes = None;
        vec![e]
    })
    .await;
    w.adopt().await;
    w.whole().await;
    let t = w.tables().await;
    assert!(t.baselines.contains_key("server.key"), "it converged: {t:?}");
    assert_eq!(
        blob_hash(&w, "server.key").await,
        None,
        "D79: a denied file's content is never stored, so no baseline points at it"
    );
    assert!(refs(&w, "sit1:server.key").await.is_empty());
    assert_eq!(
        refs(&w, "sit1:app/core.yml").await,
        vec![raw("a: 1\n")],
        "the ordinary file next to it is referenced"
    );
}
