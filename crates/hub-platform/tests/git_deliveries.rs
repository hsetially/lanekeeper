//! T7 (D64, S18): the replay guard and its prune, on a real Postgres 16 as the app role.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use hub_platform::git::deliveries::{DeliveryPrune, PRUNE_BATCH, PgDeliveryLog, RETENTION_DAYS};
use hub_platform::git::webhook::DeliveryLog;
use hub_platform::jobs::{Job, JobContext};
use support::pg::TestDb;

#[tokio::test(flavor = "multi_thread")]
async fn pg_delivery_log_records_an_id_once() {
    let Some(db) = TestDb::ready().await else { return };
    let log = PgDeliveryLog::new(db.app_pool().await);
    assert!(
        log.first_time("11111111-1111-1111-1111-111111111111", "push")
            .await
            .unwrap()
    );
    assert!(
        !log.first_time("11111111-1111-1111-1111-111111111111", "push")
            .await
            .unwrap()
    );
    assert!(
        log.first_time("22222222-2222-2222-2222-222222222222", "push")
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_delivery_log_lets_exactly_one_of_many_concurrent_replays_through() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let log = PgDeliveryLog::new(pool.clone());
        tasks.push(tokio::spawn(async move {
            log.first_time("racing-delivery", "push").await.unwrap()
        }));
    }
    let mut first = 0;
    for task in tasks {
        if task.await.unwrap() {
            first += 1;
        }
    }
    assert_eq!(first, 1, "the primary key decides, across replicas too");
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_delivery_prune_deletes_ids_older_than_the_retention_in_batches_and_keeps_the_rest() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let mut admin = db.admin().await;
    // 12,000 old ids and 50 recent ones.
    sqlx::query(
        "INSERT INTO webhook_deliveries (delivery_id, event, received_at) \
         SELECT 'old-' || g, 'push', now() - interval '8 days' - g * interval '1 second' FROM generate_series(1, 12000) g",
    )
    .execute(&mut admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO webhook_deliveries (delivery_id, event, received_at) \
         SELECT 'new-' || g, 'push', now() - g * interval '1 minute' FROM generate_series(1, 50) g",
    )
    .execute(&mut admin)
    .await
    .unwrap();

    let log = PgDeliveryLog::new(pool.clone());
    let one = log.prune(RETENTION_DAYS, PRUNE_BATCH).await.unwrap();
    assert_eq!(
        one,
        PRUNE_BATCH.unsigned_abs(),
        "one statement deletes at most one batch"
    );
    assert_eq!(
        db.count(&pool, "SELECT count(*) FROM webhook_deliveries").await,
        12_050 - 5_000
    );

    let (ctx, _hold) = JobContext::held_until_told_otherwise(hub_platform::git::deliveries::PRUNE_JOB);
    DeliveryPrune::new(log).run(&ctx).await.unwrap();
    assert_eq!(
        db.count(&pool, "SELECT count(*) FROM webhook_deliveries").await,
        50,
        "only the recent ones remain"
    );
    assert_eq!(
        db.count(
            &pool,
            "SELECT count(*) FROM webhook_deliveries WHERE delivery_id LIKE 'new-%'"
        )
        .await,
        50
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_delivery_prune_stops_when_the_lease_is_lost() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let mut admin = db.admin().await;
    sqlx::query(
        "INSERT INTO webhook_deliveries (delivery_id, event, received_at) \
         SELECT 'old-' || g, 'push', now() - interval '9 days' FROM generate_series(1, 100) g",
    )
    .execute(&mut admin)
    .await
    .unwrap();
    let (ctx, hold) = JobContext::held_until_told_otherwise(hub_platform::git::deliveries::PRUNE_JOB);
    hold.send_replace(tokio::time::Instant::now() - std::time::Duration::from_secs(1));
    let result = DeliveryPrune::new(PgDeliveryLog::new(pool.clone()))
        .run(&ctx)
        .await;
    assert_eq!(result, Err(hub_platform::jobs::JobError::LeaseLost));
    assert_eq!(
        db.count(&pool, "SELECT count(*) FROM webhook_deliveries").await,
        100,
        "nothing was deleted"
    );
}
