//! The tenant-suffix review (T4, D21, A46): when the tenant suffixes on a swimlane's NFS change, the registry re-ranks the deployable
//! branches against what is on NFS and raises an admin confirmation, an `adoptions` row with the reason `tenant_suffix_changed` and the
//! status `review`. It does not change the swimlane's tenants: an admin confirms (T5).
//!
//! Over a real Postgres 16, the real pipeline and the in-memory Git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use domain::ContentHash;
use engine::drift_hash;
use hub_registry::metrics::RematchResult;
use hub_registry::{RegistryConfig, Work};
use sqlx::Row;
use support::pg_audit::audit_rows;
use support::world::World;

fn drift(text: &str) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

/// An adopted swimlane `sit1` that deploys sit1. Git: a base file; branches sit1, sit2 and sit3 with different copies of two tenant
/// files; a template that holds exactly what sit2 holds. NFS: the base file and sit1's two files, in sync.
async fn world() -> Option<World> {
    let mut w = World::new(&["sit1"]).await?;
    w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    for (branch, n) in [("sit1", 1), ("sit2", 2), ("sit3", 3)] {
        w.tenant_commit(
            branch,
            &[
                ("data/config/app/core.yml", Some(&format!("t: {n}\n"))),
                ("data/config/app/extra.yml", Some(&format!("e: {n}\n"))),
            ],
        );
    }
    w.tenant_commit(
        "templates/core",
        &[
            ("data/config/app/core.yml", Some("t: 2\n")),
            ("data/config/app/extra.yml", Some("e: 2\n")),
        ],
    );
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/extra-sit1.yml", "e: 1\n"),
    ])
    .await;
    w.adopt().await;
    for (path, content) in [
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/extra-sit1.yml", "e: 1\n"),
    ] {
        w.baseline(path, drift(content)).await;
    }
    w.whole().await;
    Some(w)
}

struct Review {
    id: i64,
    status: String,
    step: String,
    ranking: serde_json::Value,
    hints: serde_json::Value,
    tenants: Vec<String>,
}

async fn reviews(w: &World) -> Vec<Review> {
    sqlx::query(
        "SELECT id, status, step, ranking, hints, tenants FROM adoptions \
         WHERE swimlane_id = 'sit1' AND reason = 'tenant_suffix_changed' ORDER BY id LIMIT 100",
    )
    .fetch_all(&w.rig.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| Review {
        id: r.get("id"),
        status: r.get("status"),
        step: r.get("step"),
        ranking: r.get("ranking"),
        hints: r.get("hints"),
        tenants: r.get("tenants"),
    })
    .collect()
}

async fn swimlane_tenants(w: &World) -> Vec<String> {
    sqlx::query_scalar("SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = 'sit1' ORDER BY tenant_id")
        .fetch_all(&w.rig.pool)
        .await
        .unwrap()
}

async fn settings_events(w: &World) -> usize {
    audit_rows(&w.rig.pool)
        .await
        .iter()
        .filter(|a| a.action == "settings_changed")
        .count()
}

/// The swimlane is deployed from sit2 now: the sync copied sit2's files and cleaned sit1's.
async fn redeployed_from_sit2(w: &mut World) {
    w.nfs_delete(&["app/core-sit1.yml", "app/extra-sit1.yml"]).await;
    w.nfs_change(&[("app/core-sit2.yml", "t: 2\n"), ("app/extra-sit2.yml", "e: 2\n")])
        .await;
    w.paths(&[
        "app/core-sit1.yml",
        "app/extra-sit1.yml",
        "app/core-sit2.yml",
        "app/extra-sit2.yml",
    ])
    .await;
}

#[tokio::test]
async fn nothing_is_raised_while_the_suffixes_are_the_swimlane_s() {
    let Some(w) = world().await else { return };
    assert!(reviews(&w).await.is_empty());
    assert_eq!(w.rig.metrics.rematch_count(RematchResult::Raised), 0);
}

#[tokio::test]
async fn a_new_suffix_on_nfs_raises_a_review_that_ranks_the_deployable_branches() {
    let Some(mut w) = world().await else { return };
    redeployed_from_sit2(&mut w).await;

    let r = reviews(&w).await;
    assert_eq!(r.len(), 1, "one review");
    assert_eq!((r[0].status.as_str(), r[0].step.as_str()), ("review", "branch"));
    assert_eq!(r[0].hints["foreign_suffixes"], serde_json::json!(["sit2"]));
    assert_eq!(r[0].hints["swimlane_tenants"], serde_json::json!(["sit1"]));
    assert!(
        r[0].tenants.is_empty(),
        "the registry proposes; the admin chooses"
    );

    let ranking = r[0].ranking.as_array().unwrap();
    let names: Vec<&str> = ranking.iter().map(|b| b["branch"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["sit2", "sit1", "sit3"],
        "the branch that looks like NFS first; no template, whatever it holds (D22)"
    );
    assert_eq!(
        (ranking[0]["shared"].as_u64(), ranking[0]["union"].as_u64()),
        (Some(2), Some(2))
    );
    assert_eq!(ranking[0]["tenant"], "sit2");
    assert_eq!(ranking[1]["shared"].as_u64(), Some(0));

    // It only asks: the tenants of the swimlane are what they were.
    assert_eq!(swimlane_tenants(&w).await, vec!["sit1".to_owned()]);
    // The C4 findings the new suffix implies are there, and it was audited (rule 8).
    let orphans = w
        .tables()
        .await
        .findings
        .into_iter()
        .filter(|f| f.0 == "orphan_file")
        .count();
    assert_eq!(orphans, 2);
    assert_eq!(settings_events(&w).await, 1);
    assert_eq!(w.rig.metrics.rematch_count(RematchResult::Raised), 1);
}

#[tokio::test]
async fn the_same_suffixes_raise_nothing_more_on_this_replica_or_another() {
    let Some(mut w) = world().await else { return };
    redeployed_from_sit2(&mut w).await;
    assert_eq!(reviews(&w).await.len(), 1);

    // Another batch on the same replica.
    w.paths(&["app/core-sit2.yml"]).await;
    w.whole().await;
    assert_eq!(reviews(&w).await.len(), 1);

    // Another replica takes over: it has no memory of what was raised, and the table says so.
    let (other, _handle) = w.rig.pipeline(Arc::new(w.git.clone()), RegistryConfig::default());
    other.process(vec![(w.lane(), Work::Whole)]).await.unwrap();
    assert_eq!(
        reviews(&w).await.len(),
        1,
        "the review of the same suffixes is not raised twice"
    );
    assert_eq!(settings_events(&w).await, 1);
    assert_eq!(w.rig.metrics.rematch_count(RematchResult::Raised), 1);
}

#[tokio::test]
async fn another_suffix_refreshes_the_open_review_and_a_closed_one_is_not_reopened() {
    let Some(mut w) = world().await else { return };
    redeployed_from_sit2(&mut w).await;
    let first = reviews(&w).await.remove(0);

    // A third branch's file shows up: the open review is the same review, with both suffixes.
    w.nfs_change(&[("app/core-sit3.yml", "t: 3\n")]).await;
    w.paths(&["app/core-sit3.yml"]).await;
    let r = reviews(&w).await;
    assert_eq!(
        r.len(),
        1,
        "at most one review is open for a swimlane (the table's unique index)"
    );
    assert_eq!(r[0].id, first.id);
    assert_eq!(
        r[0].hints["foreign_suffixes"],
        serde_json::json!(["sit2", "sit3"])
    );
    assert_eq!(w.rig.metrics.rematch_count(RematchResult::Refreshed), 1);
    assert_eq!(settings_events(&w).await, 2);

    // An admin is working on it: the registry does not change it under them.
    sqlx::query("UPDATE adoptions SET status = 'in_progress' WHERE id = $1")
        .bind(first.id)
        .execute(&w.rig.pool)
        .await
        .unwrap();
    w.tenant_commit("sit4", &[("data/config/app/core.yml", Some("t: 4\n"))]);
    w.nfs_change(&[("app/core-sit4.yml", "t: 4\n")]).await;
    w.whole().await;
    let r = reviews(&w).await;
    assert_eq!(r.len(), 1);
    assert_eq!(
        r[0].hints["foreign_suffixes"],
        serde_json::json!(["sit2", "sit3"]),
        "untouched"
    );

    // The admin cancels it. A cancelled review is closed: what has come since (sit4, which arrived while it was open and untouched) is
    // a change, and raises a new one; the same suffixes after that raise nothing more.
    sqlx::query("UPDATE adoptions SET status = 'cancelled' WHERE id = $1")
        .bind(first.id)
        .execute(&w.rig.pool)
        .await
        .unwrap();
    w.whole().await;
    let r = reviews(&w).await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].status, "review");
    assert_eq!(
        r[1].hints["foreign_suffixes"],
        serde_json::json!(["sit2", "sit3", "sit4"])
    );
    w.whole().await;
    assert_eq!(reviews(&w).await.len(), 2, "the same suffixes raise nothing more");
    assert!(
        w.rig.metrics.rematch_count(RematchResult::Deferred) >= 1,
        "while the admin was working it was left alone, and said so"
    );
}

#[tokio::test]
async fn nothing_is_raised_before_the_swimlane_is_adopted() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t: 1\n"))]);
    w.tenant_commit("sit2", &[("data/config/app/core.yml", Some("t: 2\n"))]);
    w.inventory(&[("app/core.yml", "a: 1\n"), ("app/core-sit2.yml", "t: 2\n")])
        .await;
    w.whole().await;
    let orphans = w
        .tables()
        .await
        .findings
        .into_iter()
        .filter(|f| f.0 == "orphan_file")
        .count();
    assert_eq!(orphans, 1, "the suffix is a finding already");
    assert!(
        reviews(&w).await.is_empty(),
        "there is no confirmed tenant set to change: the adoption pass is what asks"
    );
}
