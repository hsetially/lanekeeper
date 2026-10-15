//! P12: no steady-state query takes longer than 50 ms (`P12.hub_writes_queries`, `hub-writes/write_queries`).
//!
//! The statements the write path runs on every request, at target-scale row counts: the idempotency claim (an insert that
//! checks the key), the read of a retry that finds a finished claim, the completion of a claim, the editing-lock read, the
//! reconcile job's scan for unfinished claims, one page of the expiry sweep, and (from T4) a page of the list of proposals, the
//! expiry job's scan and the read of one proposal. Each sample is one statement, taken in turn, so the registered p95 is over the
//! whole set. Later tasks add the open-PR lookup and the credential load to the set.
//!
//! The tables hold 60,000 idempotency rows (a busy day's writes and some days of unswept rows), 40 swimlanes and 10,000 proposals,
//! over a real Postgres (a container, or `LK_TEST_PG_ADMIN_URL`). Loopback: this bounds the statements and the database, not the
//! network; prompt 16 measures staging.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use domain::Role;
use support::pg::TestDb;
use support::rig::Rig;

fn bench(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let Some(db) = rt.block_on(TestDb::ready()) else {
        panic!("the P12 benchmark needs a database: start Docker or set LK_TEST_PG_ADMIN_URL");
    };
    let rig = rt.block_on(Rig::with_live_registry(db, 4));
    let editor = rt.block_on(rig.user(1, Role::Editor));
    let user = editor.id.clone();
    let mut swimlanes = Vec::new();
    for i in 0..40 {
        swimlanes.push(rt.block_on(rig.swimlane(&format!("sit{i}"), true)));
    }
    rt.block_on(async {
        // 60,000 idempotency rows: finished, spread over three days; the oldest day has expired and is for the sweep.
        sqlx::query(
            "INSERT INTO idempotency_keys (user_tid, user_oid, key, request_hash, response_status, response_body, created_at, expires_at) \
             SELECT $1, $2, 'filler-' || g, sha256(g::text::bytea), 200, '{}'::bytea, \
                    to_timestamp($3::bigint / 1000.0) - (g % 3) * interval '1 day', \
                    to_timestamp($3::bigint / 1000.0) - (g % 3) * interval '1 day' + interval '1 day' \
             FROM generate_series(1, 60000) g",
        )
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .bind(support::rig::START)
        .execute(&rig.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, status, via, expires_at) \
             SELECT $1, $2, 'edit', 'sit' || (g % 40), ARRAY['a/' || g || '.yml'], 'pending', 'ui', now() + interval '3 days' \
             FROM generate_series(1, 10000) g",
        )
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .execute(&rig.pool)
        .await
        .unwrap();
    });
    // Planner statistics, as autovacuum would have made them in service. The app role does not own the tables, so it cannot.
    rt.block_on(async {
        let mut admin = rig.db.admin().await;
        sqlx::query("ANALYZE").execute(&mut admin).await.unwrap();
    });

    let probe = rig.hub.probe();
    let mut group = c.benchmark_group("hub-writes");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(500)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_millis(100));
    // A multiple of ten less one, so that the first step is a claim and the steps that read a claim follow one.
    let mut n = 6_999_999_u64;
    group.bench_function("write_queries", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    n += 1;
                    let started = Instant::now();
                    let ok = match n % 10 {
                        // A claim, the read of the row a step later while it is unfinished, and its completion a step after.
                        0 => probe.claim(&user, &format!("probe-{n}"), n).await,
                        1 => probe.replay(&user, &format!("probe-{}", n - 1), n - 1).await,
                        2 => probe.finish(&user, &format!("probe-{}", n - 2)).await,
                        3 => probe.lock(&swimlanes[(n % 40) as usize]).await,
                        4 => probe.stale_scan().await < usize::MAX,
                        5 => probe.purge_page().await,
                        // A retry that finds a finished row, among the 60,000. The sweep deletes the expired ones (a third of them),
                        // so the rows read are the ones that stay.
                        6 => {
                            let key = format!("filler-{}", 3 * (n % 20_000) + 3);
                            probe.replay(&user, &key, n).await
                        }
                        // The list of proposals, the expiry job's scan, and one proposal, among the 10,000.
                        7 => probe.proposal_page().await == 25,
                        8 => probe.proposal_expiry_scan().await == 0,
                        _ => {
                            probe
                                .proposal_get(i64::try_from(1 + n % 10_000).unwrap_or(1))
                                .await
                        }
                    };
                    total += started.elapsed();
                    assert!(ok, "statement {} failed at step {n}", n % 10);
                }
                total.max(Duration::from_nanos(1))
            })
        });
    });
    group.finish();
    let Rig { db, pool, hub, .. } = rig;
    drop(hub);
    rt.block_on(pool.close());
    rt.block_on(db.shutdown());
}

criterion_group!(benches, bench);
criterion_main!(benches);
