//! P12: recording an audit event adds under 3 ms to a transaction (`P12.audit_record`, `hub-identity/audit_record`).
//!
//! What is measured is the *added* time. Each sample runs the same transaction twice, back to back, over a real Postgres (a
//! container, or the server in `LK_TEST_PG_ADMIN_URL`): once with a small change (`BEGIN`, one `INSERT`, `COMMIT`) and once with
//! `PgAuditLog::record` in it, as its last statement before the commit, which is how callers use it. The sample is the second
//! time minus the first (never below one nanosecond), so the cost of the transaction itself is out of the number and what is
//! left is the chain: the advisory lock, the head read, the hash, the insert, and the longer commit.
//!
//! Contention is in the sample. A second pool, which stands for another hub replica, runs audited transactions of its own all
//! the time: `record`, a random 0 to 1 ms of other work while it still holds the chain lock (which `record`'s own rule, "call
//! it last", says a caller should not do), `COMMIT`, then a random pause of 0 to 6 ms. That keeps the lock held roughly a quarter
//! of the time, far busier than design scale (tens of audited writes a minute), so a measured `record` often waits for the lock.
//! A waiter's added time is then about the other replica's hold time: the registered p95 grows with the work a caller does after
//! `record`, and is about 1.2 ms when the other replica commits at once.
//!
//! Every sample is one iteration (criterion's flat sampling with a measurement time too short for more), so the registered p95 is
//! over single transactions and not over averages. Loopback to a local database: this bounds the code and the database's time,
//! not the network's; prompt 16 measures staging.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use hub_identity::audit::PgAuditLog;
use ports::{AuditLog, Tx};
use sqlx::PgPool;
use support::audit_rig::event;
use support::pg::TestDb;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A small change that stands for the state change an audited transaction makes.
async fn change(tx: &mut sqlx::PgConnection) {
    let id = format!("bench-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    sqlx::query("INSERT INTO webhook_deliveries (delivery_id, event) VALUES ($1, 'push')")
        .bind(id)
        .execute(tx)
        .await
        .unwrap();
}

/// One transaction. With `audited`, `record` is its last statement before the commit.
async fn transaction(pool: &PgPool, audited: bool, n: i64) -> Duration {
    let started = Instant::now();
    let mut tx = pool.begin().await.unwrap();
    change(&mut tx).await;
    if audited {
        PgAuditLog::new()
            .record(&mut Tx::from_pg(&mut tx), event(n))
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    started.elapsed()
}

/// Wait for `d` on this thread. `tokio::time::sleep` has a timer resolution of one millisecond (a request for 0 to 1 ms takes
/// 1 to 2 ms, three times the model's mean), and this stands for work the other replica does, so the wait is precise.
fn work_for(d: Duration) {
    tokio::task::block_in_place(|| {
        let end = Instant::now() + d;
        while Instant::now() < end {
            std::hint::spin_loop();
        }
    });
}

/// The other replica: audited transactions that keep the chain lock for a moment, for as long as the task lives.
fn spawn_contender(rt: &tokio::runtime::Runtime, pool: PgPool) -> tokio::task::JoinHandle<()> {
    rt.spawn(async move {
        let mut n = 1_000_000_i64;
        loop {
            n += 1;
            let mut tx = pool.begin().await.unwrap();
            change(&mut tx).await;
            PgAuditLog::new()
                .record(&mut Tx::from_pg(&mut tx), event(n))
                .await
                .unwrap();
            work_for(Duration::from_micros(fastrand::u64(0..=1_000)));
            tx.commit().await.unwrap();
            tokio::time::sleep(Duration::from_micros(fastrand::u64(0..=6_000))).await;
        }
    })
}

fn audit_record(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let Some(db) = rt.block_on(TestDb::ready()) else {
        panic!("the audit benchmark needs a database: start Docker or set LK_TEST_PG_ADMIN_URL");
    };
    let measured = rt.block_on(db.app_pool(4));
    let other_replica = rt.block_on(db.app_pool(4));
    // A chain with some history, as in service.
    rt.block_on(async {
        for n in 0..200 {
            transaction(&measured, true, n).await;
        }
    });
    let contender = spawn_contender(&rt, other_replica);

    let mut group = c.benchmark_group("hub-identity");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(500)
        .warm_up_time(Duration::from_secs(1))
        // Too short for more than one iteration per sample, which is what makes each sample one transaction.
        .measurement_time(Duration::from_millis(100));
    let mut n = 0_i64;
    group.bench_function("audit_record", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut added = Duration::ZERO;
                for _ in 0..iters {
                    n += 1;
                    // Alternate the order, so a warm connection favours neither side.
                    let (without, with) = if n % 2 == 0 {
                        let without = transaction(&measured, false, n).await;
                        (without, transaction(&measured, true, n).await)
                    } else {
                        let with = transaction(&measured, true, n).await;
                        (transaction(&measured, false, n).await, with)
                    };
                    // Never below one nanosecond: criterion refuses a sample of zero.
                    added += with.saturating_sub(without).max(Duration::from_nanos(1));
                }
                added
            })
        });
    });
    group.finish();
    contender.abort();
}

criterion_group!(benches, audit_record);
criterion_main!(benches);
