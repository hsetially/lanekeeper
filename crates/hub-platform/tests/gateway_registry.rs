//! T4: `agent_connections`, the registry of which replica holds each agent's stream (D62), on a real Postgres.
//!
//! The epoch is what makes a reconnect safe: it only ever grows, a stale replica cannot clear a newer registration, and the
//! row is never deleted (a delete would let the next connect reuse an epoch a stale replica still holds).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::collections::HashSet;
use std::time::Duration;

use domain::SwimlaneId;
use hub_platform::ca::profile::PeerName;
use hub_platform::gateway::forward::PeerAddress;
use hub_platform::gateway::local::Epoch;
use hub_platform::gateway::registry::{ConnectionRegistry, Lookup};
use proptest::prelude::*;
use sqlx::PgPool;
use support::pg::TestDb;

fn swimlane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

fn registry(pool: &PgPool, replica: &str, stale_after: Duration) -> ConnectionRegistry {
    ConnectionRegistry::new(
        pool.clone(),
        PeerName::parse(replica).unwrap(),
        PeerAddress::parse(&format!("{replica}.hub.test:8444")).unwrap(),
        stale_after,
    )
}

async fn row(db: &TestDb, pool: &PgPool, s: &str) -> (String, String, i64) {
    let _ = db;
    sqlx::query_as::<_, (String, String, i64)>(
        "SELECT replica_id, replica_address, epoch FROM agent_connections WHERE swimlane_id = $1",
    )
    .bind(s)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn register_upserts_the_row_and_the_epoch_grows_on_every_connect() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", None).await;
    let a = registry(&pool, "hub-a", Duration::from_secs(30));
    let b = registry(&pool, "hub-b", Duration::from_secs(30));

    assert_eq!(a.register(&swimlane("sit1")).await.unwrap(), Epoch::new(1));
    assert_eq!(
        row(&db, &pool, "sit1").await,
        ("hub-a".into(), "hub-a.hub.test:8444".into(), 1)
    );
    // The agent reconnects to another replica: the same row, the next epoch, the new owner.
    assert_eq!(b.register(&swimlane("sit1")).await.unwrap(), Epoch::new(2));
    assert_eq!(
        row(&db, &pool, "sit1").await,
        ("hub-b".into(), "hub-b.hub.test:8444".into(), 2)
    );
    // And to the same replica again.
    assert_eq!(b.register(&swimlane("sit1")).await.unwrap(), Epoch::new(3));
}

#[tokio::test]
async fn a_swimlane_that_does_not_exist_cannot_be_registered() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let a = registry(&pool, "hub-a", Duration::from_secs(30));
    assert!(a.register(&swimlane("no-such-swimlane")).await.is_err());
}

#[tokio::test]
async fn clear_with_a_stale_epoch_removes_nothing() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", None).await;
    let a = registry(&pool, "hub-a", Duration::from_secs(30));
    let b = registry(&pool, "hub-b", Duration::from_secs(30));
    let old = a.register(&swimlane("sit1")).await.unwrap();
    let new = b.register(&swimlane("sit1")).await.unwrap();

    // Replica A's stream ends late: its clear carries the epoch it registered with.
    assert!(
        !a.clear(&swimlane("sit1"), old).await.unwrap(),
        "a stale clear matches nothing"
    );
    assert!(matches!(
        b.lookup(&swimlane("sit1")).await.unwrap(),
        Lookup::Live(route) if route.epoch == new && route.replica == "hub-b"
    ));
    // Even the right epoch from the wrong replica is not enough.
    assert!(!a.clear(&swimlane("sit1"), new).await.unwrap());
    assert!(b.clear(&swimlane("sit1"), new).await.unwrap());
}

#[tokio::test]
async fn a_late_clear_from_the_same_replica_with_an_older_epoch_removes_nothing() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", None).await;
    let a = registry(&pool, "hub-a", Duration::from_secs(30));
    // The agent reconnects to the same replica; the first stream's handler ends after the second registered.
    let first = a.register(&swimlane("sit1")).await.unwrap();
    let second = a.register(&swimlane("sit1")).await.unwrap();
    assert_eq!((first, second), (Epoch::new(1), Epoch::new(2)));
    assert!(
        !a.clear(&swimlane("sit1"), first).await.unwrap(),
        "the replaced stream's release matches nothing"
    );
    assert!(
        matches!(a.lookup(&swimlane("sit1")).await.unwrap(), Lookup::Live(route) if route.epoch == second)
    );
    assert!(a.clear(&swimlane("sit1"), second).await.unwrap());
    assert!(
        !a.clear(&swimlane("sit1"), second).await.unwrap(),
        "releasing twice is a no-op"
    );
}

#[tokio::test]
async fn clear_keeps_the_row_so_the_epoch_stays_monotonic() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", None).await;
    let a = registry(&pool, "hub-a", Duration::from_secs(30));
    let epoch = a.register(&swimlane("sit1")).await.unwrap();
    assert!(a.clear(&swimlane("sit1"), epoch).await.unwrap());

    assert_eq!(
        db.count(&pool, "SELECT count(*) FROM agent_connections").await,
        1,
        "the row stays"
    );
    let (_, address, kept) = row(&db, &pool, "sit1").await;
    assert_eq!((address.as_str(), kept), ("", 1));
    assert!(matches!(
        a.lookup(&swimlane("sit1")).await.unwrap(),
        Lookup::Stale
    ));
    assert_eq!(
        a.register(&swimlane("sit1")).await.unwrap(),
        Epoch::new(2),
        "the next connect does not reuse epoch 1"
    );
}

#[tokio::test]
async fn lookup_distinguishes_live_stale_and_absent() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", None).await;
    db.seed_swimlane(&pool, "sit2", None).await;
    let a = registry(&pool, "hub-a", Duration::from_secs(30));

    assert!(matches!(
        a.lookup(&swimlane("sit1")).await.unwrap(),
        Lookup::Absent
    ));
    a.register(&swimlane("sit1")).await.unwrap();
    assert!(matches!(
        a.lookup(&swimlane("sit1")).await.unwrap(),
        Lookup::Live(_)
    ));

    // The owner stopped refreshing 31 seconds ago (by the database's clock): the row no longer counts.
    let mut admin = db.admin().await;
    sqlx::query("UPDATE agent_connections SET last_seen_at = now() - interval '31 seconds' WHERE swimlane_id = 'sit1'")
        .execute(&mut admin)
        .await
        .unwrap();
    assert!(matches!(
        a.lookup(&swimlane("sit1")).await.unwrap(),
        Lookup::Stale
    ));
    assert!(matches!(
        a.lookup(&swimlane("sit2")).await.unwrap(),
        Lookup::Absent
    ));
}

#[tokio::test]
async fn refresh_renews_the_epochs_this_replica_holds_and_reports_the_rest_as_superseded() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    for s in ["sit1", "sit2", "sit3"] {
        db.seed_swimlane(&pool, s, None).await;
    }
    let a = registry(&pool, "hub-a", Duration::from_secs(30));
    let b = registry(&pool, "hub-b", Duration::from_secs(30));
    let e1 = a.register(&swimlane("sit1")).await.unwrap();
    let e2 = a.register(&swimlane("sit2")).await.unwrap();
    let e3 = a.register(&swimlane("sit3")).await.unwrap();
    // sit2 moved to replica B; sit3 was cleared.
    b.register(&swimlane("sit2")).await.unwrap();
    assert!(a.clear(&swimlane("sit3"), e3).await.unwrap());

    let mut admin = db.admin().await;
    sqlx::query("UPDATE agent_connections SET last_seen_at = now() - interval '20 seconds'")
        .execute(&mut admin)
        .await
        .unwrap();
    let held = [
        (swimlane("sit1"), e1),
        (swimlane("sit2"), e2),
        (swimlane("sit3"), e3),
    ];
    let renewed = a.refresh(&held).await.unwrap();
    assert_eq!(
        renewed,
        HashSet::from([swimlane("sit1")]),
        "only the registration A still owns is renewed"
    );
    let age: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - last_seen_at)::float8 FROM agent_connections WHERE swimlane_id = 'sit1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(age < 5.0, "sit1 was renewed, age {age}");
    let age2: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - last_seen_at)::float8 FROM agent_connections WHERE swimlane_id = 'sit3'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(age2 > 15.0, "a cleared row is not renewed, age {age2}");
    assert!(
        a.refresh(&[]).await.unwrap().is_empty(),
        "nothing held, nothing to do"
    );
}

#[tokio::test]
async fn has_joined_reads_agents_joined_at() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    db.seed_swimlane(&pool, "sit1", None).await;
    db.seed_swimlane(&pool, "sit2", None).await;
    let a = registry(&pool, "hub-a", Duration::from_secs(30));
    sqlx::query("INSERT INTO agents (swimlane_id, joined_at) VALUES ('sit1', now())")
        .execute(&pool)
        .await
        .unwrap();
    assert!(a.has_joined(&swimlane("sit1")).await.unwrap());
    assert!(!a.has_joined(&swimlane("sit2")).await.unwrap());
    assert!(!a.has_joined(&swimlane("unknown")).await.unwrap());
}

// ------------------------------------------------------------------------------------- the epoch, as a property

#[derive(Debug, Clone)]
enum Step {
    /// Replica `0` or `1` takes the agent's stream.
    Connect(u8),
    /// Replica `0` or `1` clears with one of the epochs issued so far, picked by `n`: the newest, or one long superseded.
    ClearAt(u8, u8),
    /// Replica `0` or `1` clears with the newest epoch, whoever holds it.
    ClearNewest(u8),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0u8..2).prop_map(Step::Connect),
        (0u8..2, any::<u8>()).prop_map(|(r, n)| Step::ClearAt(r, n)),
        (0u8..2).prop_map(Step::ClearNewest),
    ]
}

/// Over any interleaving of connects and clears from two replicas: the epoch never decreases, a registration is only ever
/// removed by the replica and epoch that made it, and a connect always gets the epoch after the newest.
#[test]
fn epoch_never_decreases_and_a_stale_clear_never_removes_a_newer_registration() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    // The container is removed when `db` drops, which needs a runtime context.
    let _inside = runtime.enter();
    let Some(db) = runtime.block_on(TestDb::ready()) else {
        return;
    };
    let pool = runtime.block_on(db.app_pool());
    let registries = [
        registry(&pool, "hub-a", Duration::from_secs(30)),
        registry(&pool, "hub-b", Duration::from_secs(30)),
    ];
    let case = std::sync::atomic::AtomicU32::new(0);

    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 24,
        ..ProptestConfig::default()
    });
    runner
        .run(&proptest::collection::vec(step(), 1..14), |steps| {
            let id = format!("prop-{}", case.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
            runtime.block_on(async {
                db.seed_swimlane(&pool, &id, None).await;
                let s = swimlane(&id);
                // The model: the newest epoch, who holds it, and whether it is still registered.
                let mut newest = 0i64;
                let mut holder: Option<u8> = None;
                let mut registered = false;
                for step in &steps {
                    match *step {
                        Step::Connect(r) => {
                            let epoch = registries[usize::from(r)].register(&s).await.unwrap();
                            prop_assert_eq!(epoch.get(), newest + 1);
                            newest += 1;
                            holder = Some(r);
                            registered = true;
                        }
                        Step::ClearAt(r, n) => {
                            if newest == 0 {
                                continue;
                            }
                            // Any epoch issued so far, 1 to the newest.
                            let epoch = i64::from(n) % newest + 1;
                            let cleared = registries[usize::from(r)]
                                .clear(&s, Epoch::new(epoch))
                                .await
                                .unwrap();
                            // Only the replica that holds the newest registration, naming that epoch, releases it. A late
                            // clear from the same replica with an older epoch (it reconnected) removes nothing.
                            let expect = registered && holder == Some(r) && epoch == newest;
                            prop_assert_eq!(
                                cleared,
                                expect,
                                "clear by {} at epoch {} (newest {})",
                                r,
                                epoch,
                                newest
                            );
                            if expect {
                                registered = false;
                            }
                        }
                        Step::ClearNewest(r) => {
                            if newest == 0 {
                                continue;
                            }
                            let cleared = registries[usize::from(r)]
                                .clear(&s, Epoch::new(newest))
                                .await
                                .unwrap();
                            let expect = registered && holder == Some(r);
                            prop_assert_eq!(cleared, expect);
                            if expect {
                                registered = false;
                            }
                        }
                    }
                    // Whatever happened, the row says what the model says.
                    if newest > 0 {
                        let (replica, address, epoch) = row(&db, &pool, &id).await;
                        prop_assert_eq!(epoch, newest);
                        prop_assert_eq!(address.is_empty(), !registered);
                        if registered {
                            let want = if holder == Some(0) { "hub-a" } else { "hub-b" };
                            prop_assert_eq!(replica, want);
                        }
                    }
                }
                Ok(())
            })
        })
        .unwrap();
}
