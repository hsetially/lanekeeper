//! T6: leases on a real Postgres, three replicas (D62, D63).
//!
//! The table's `expires_at` is the only clock that counts, so the tests that need time to pass rewrite the row's expiry (what
//! the database's clock would have done) instead of sleeping, except the shared conformance suite, whose sleeps are 100 ms.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use hub_platform::ca::profile::PeerName;
use hub_platform::leases::PgLeases;
use ports::{LeaseError, LeaseGuard, Leases};
use sqlx::{PgPool, Row};
use support::pg::TestDb;
use tokio::sync::Barrier;

const LONG: Duration = Duration::from_secs(60);

fn replica(name: &str) -> PeerName {
    PeerName::parse(name).unwrap()
}

/// What the database clock would have done had the holder stopped renewing and `ttl` passed.
async fn let_the_lease_run_out(pool: &PgPool, name: &str) {
    sqlx::query("UPDATE leases SET expires_at = now() - interval '1 second' WHERE name = $1")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
}

async fn row(pool: &PgPool, name: &str) -> Option<(String, i64, bool)> {
    sqlx::query("SELECT holder, fence, expires_at > now() AS live FROM leases WHERE name = $1")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
        .map(|r| (r.get("holder"), r.get("fence"), r.get("live")))
}

struct Replicas {
    db: TestDb,
    /// An admin-side view for assertions; the replicas below use their own pools.
    pool: PgPool,
    leases: Vec<(String, PgLeases)>,
}

async fn replicas(n: usize) -> Option<Replicas> {
    let db = TestDb::ready().await?;
    let pool = db.app_pool().await;
    let mut leases = Vec::new();
    for i in 0..n {
        let holder = format!("hub-{i}");
        leases.push((
            holder.clone(),
            PgLeases::new(db.app_pool().await, &replica(&holder)),
        ));
    }
    Some(Replicas { db, pool, leases })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pg_leases_conformance() {
    let Some(r) = replicas(1).await else { return };
    // The shared suite: one holder per name, validation, expiry and loss, renewal, a dropped guard keeps the lease. It runs in
    // real time here because the expiry is the database's.
    ports::conformance::leases(&r.leases[0].1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_hubs_race_for_a_lease_exactly_one_holds_it() {
    let Some(r) = replicas(3).await else { return };
    for round in 0..20 {
        let name = format!("job.race-{round}");
        let barrier = Arc::new(Barrier::new(3));
        let mut tasks = Vec::new();
        for (holder, leases) in &r.leases {
            let (holder, leases, barrier, name) =
                (holder.clone(), leases.clone(), barrier.clone(), name.clone());
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                (holder, leases.try_acquire(&name, LONG).await.unwrap())
            }));
        }
        let mut winners: Vec<(String, LeaseGuard)> = Vec::new();
        for t in tasks {
            let (holder, guard) = t.await.unwrap();
            if let Some(g) = guard {
                winners.push((holder, g));
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "round {round}: exactly one replica holds {name}"
        );
        let (holder, fence, live) = row(&r.pool, &name).await.expect("the lease row exists");
        assert_eq!(holder, winners[0].0, "the row names the winner");
        assert_eq!(fence, 1, "a first acquire is fence 1");
        assert!(live);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_taken_over_after_holder_dies() {
    let Some(r) = replicas(2).await else { return };
    let (a, b) = (&r.leases[0].1, &r.leases[1].1);

    let mut dead = a.try_acquire("job.takeover", LONG).await.unwrap().expect("free");
    assert!(
        b.try_acquire("job.takeover", LONG).await.unwrap().is_none(),
        "held while the holder is alive"
    );

    // The holder dies: it stops renewing, and its ttl passes (the database's clock).
    let_the_lease_run_out(&r.pool, "job.takeover").await;

    let mut new = b
        .try_acquire("job.takeover", LONG)
        .await
        .unwrap()
        .expect("taken over after expiry");
    let (holder, fence, live) = row(&r.pool, "job.takeover").await.unwrap();
    assert_eq!((holder.as_str(), fence, live), ("hub-1", 2, true));

    // The dead holder wakes up: it cannot renew or release what is no longer its own.
    assert_eq!(dead.renew(LONG).await, Err(LeaseError::Lost));
    assert_eq!(dead.release().await, Err(LeaseError::Lost));
    assert_eq!(
        row(&r.pool, "job.takeover").await.unwrap(),
        ("hub-1".to_owned(), 2, true),
        "the new holder's lease is untouched"
    );
    new.renew(LONG).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_replica_taking_over_its_own_expired_lease_invalidates_the_old_guard() {
    // The holder's name is the replica, so the name alone cannot tell two generations apart: the fence does.
    let Some(r) = replicas(1).await else { return };
    let a = &r.leases[0].1;
    let mut old = a.try_acquire("job.same", LONG).await.unwrap().expect("free");
    let_the_lease_run_out(&r.pool, "job.same").await;
    let mut new = a
        .try_acquire("job.same", LONG)
        .await
        .unwrap()
        .expect("expired, so free");
    assert_eq!(old.renew(LONG).await, Err(LeaseError::Lost));
    assert_eq!(old.release().await, Err(LeaseError::Lost));
    new.renew(LONG).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fence_grows_on_each_takeover_and_release_and_never_resets() {
    let Some(r) = replicas(2).await else { return };
    let (a, b) = (&r.leases[0].1, &r.leases[1].1);

    let g1 = a.try_acquire("job.fence", LONG).await.unwrap().unwrap();
    assert_eq!(row(&r.pool, "job.fence").await.unwrap().1, 1);
    g1.release().await.unwrap();
    // Release keeps the row: deleting it would let the next acquire start at 1 again while a stale guard still holds 1.
    let (_, fence, live) = row(&r.pool, "job.fence").await.expect("the row stays");
    assert_eq!((fence, live), (1, false));

    let mut g2 = b.try_acquire("job.fence", LONG).await.unwrap().unwrap();
    assert_eq!(row(&r.pool, "job.fence").await.unwrap().1, 2);
    g2.renew(LONG).await.unwrap();
    g2.renew(LONG).await.unwrap();
    assert_eq!(
        row(&r.pool, "job.fence").await.unwrap().1,
        2,
        "renewal never changes the fence"
    );

    let_the_lease_run_out(&r.pool, "job.fence").await;
    let _g3 = a.try_acquire("job.fence", LONG).await.unwrap().unwrap();
    assert_eq!(row(&r.pool, "job.fence").await.unwrap().1, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renew_extends_the_expiry_the_database_holds() {
    let Some(r) = replicas(1).await else { return };
    let a = &r.leases[0].1;
    let mut g = a
        .try_acquire("job.extend", Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    let before: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - now())::float8 FROM leases WHERE name = 'job.extend'",
    )
    .fetch_one(&r.pool)
    .await
    .unwrap();
    assert!(before <= 5.0 && before > 3.0, "ttl of 5 s, got {before}");
    g.renew(LONG).await.unwrap();
    let after: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - now())::float8 FROM leases WHERE name = 'job.extend'",
    )
    .fetch_one(&r.pool)
    .await
    .unwrap();
    assert!(
        after > 55.0 && after <= 60.0,
        "renewed to 60 s from now, got {after}"
    );
    assert!(g.is_valid());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_local_guard_never_outlives_the_database_lease() {
    // The guard's expiry is measured from before the request was sent, so it is never later than the row's.
    let Some(r) = replicas(1).await else { return };
    let a = &r.leases[0].1;
    let g = a
        .try_acquire("job.early", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    let remaining_local = g
        .expires_at()
        .saturating_duration_since(tokio::time::Instant::now());
    let remaining_db: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - now())::float8 FROM leases WHERE name = 'job.early'",
    )
    .fetch_one(&r.pool)
    .await
    .unwrap();
    assert!(
        remaining_local.as_secs_f64() <= remaining_db + 0.001,
        "local {remaining_local:?} must not exceed database {remaining_db}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_store_is_unavailable_not_a_lease() {
    let Some(r) = replicas(0).await else { return };
    let pool = r.db.app_pool().await;
    let broken = PgLeases::new(pool.clone(), &replica("hub-broken"));
    pool.close().await;
    assert_eq!(
        broken.try_acquire("job.x", LONG).await.unwrap_err(),
        LeaseError::Unavailable
    );
    // A guard from a healthy store whose pool later closes cannot renew either.
    let healthy_pool = r.db.app_pool().await;
    let l = PgLeases::new(healthy_pool.clone(), &replica("hub-closing"));
    let mut g = l.try_acquire("job.y", LONG).await.unwrap().unwrap();
    healthy_pool.close().await;
    assert_eq!(g.renew(LONG).await, Err(LeaseError::Unavailable));
}

mod model {
    //! Lease state machine against the real table: any order of acquire, renew, release and the passing of time keeps at most
    //! one live holder, and the fence only grows.

    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::{Config, TestRunner};

    #[derive(Debug, Clone)]
    enum Op {
        Acquire(usize),
        Renew(usize),
        Release(usize),
        /// The ttl passes: the lease's expiry is now in the past.
        Expire,
    }

    fn ops() -> impl Strategy<Value = Vec<Op>> {
        let op = prop_oneof![
            (0..3usize).prop_map(Op::Acquire),
            (0..3usize).prop_map(Op::Renew),
            (0..3usize).prop_map(Op::Release),
            Just(Op::Expire),
        ];
        proptest::collection::vec(op, 1..14)
    }

    #[test]
    fn lease_state_machine_has_at_most_one_live_holder_and_a_growing_fence() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        // The container's drop, on success and when a case fails, needs the runtime it was started on.
        let _in_runtime = rt.enter();
        let Some(r) = rt.block_on(replicas(3)) else { return };
        let mut runner = TestRunner::new(Config {
            cases: 16,
            failure_persistence: None,
            ..Config::default()
        });
        let case_no = std::sync::atomic::AtomicUsize::new(0);
        runner
            .run(&ops(), |ops| {
                let n = case_no.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let name = format!("job.model-{n}");
                rt.block_on(async {
                    let mut guards: Vec<Option<LeaseGuard>> = vec![None, None, None];
                    // The model: the live holder (replica, fence), the last fence, and whether time has run it out.
                    let mut live: Option<(usize, i64)> = None;
                    let mut fence = 0_i64;
                    for op in ops {
                        match op {
                            Op::Acquire(i) => {
                                let got = r.leases[i].1.try_acquire(&name, LONG).await.unwrap();
                                if live.is_some() {
                                    prop_assert!(
                                        got.is_none(),
                                        "a live lease is not given to a second holder"
                                    );
                                } else {
                                    prop_assert!(got.is_some(), "a free lease is given");
                                    fence += 1;
                                    live = Some((i, fence));
                                    guards[i] = got;
                                }
                            }
                            Op::Renew(i) => {
                                let res = match guards[i].as_mut() {
                                    Some(g) => g.renew(LONG).await,
                                    None => continue,
                                };
                                let ok =
                                    live.is_some_and(|(h, f)| h == i && guards[i].is_some() && f == fence);
                                prop_assert_eq!(res.is_ok(), ok, "renew {:?}", live);
                            }
                            Op::Release(i) => {
                                let Some(g) = guards[i].take() else { continue };
                                let res = g.release().await;
                                let ok = live.is_some_and(|(h, _)| h == i);
                                prop_assert_eq!(res.is_ok(), ok, "release {:?}", live);
                                if ok {
                                    live = None;
                                }
                            }
                            Op::Expire => {
                                let_the_lease_run_out(&r.pool, &name).await;
                                live = None;
                            }
                        }
                        let state = row(&r.pool, &name).await;
                        match state {
                            None => prop_assert_eq!(fence, 0),
                            Some((_, db_fence, db_live)) => {
                                prop_assert_eq!(db_fence, fence, "the fence is the number of takeovers");
                                prop_assert_eq!(db_live, live.is_some());
                            }
                        }
                    }
                    Ok(())
                })
            })
            .unwrap();
    }
}
