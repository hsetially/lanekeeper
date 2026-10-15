//! The leased job `registry-sentinel-partitions` (T7, S9, S21): the one caller of the `SECURITY DEFINER` function that creates and drops the
//! months of `sentinel_records`. The retention is the admin's, with the function's floor of 90 days in front of it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};

use hub_platform::jobs::{JobContext, JobError};
use hub_registry::{RegistryConfig, SentinelPartitions};
use support::rig::Rig;
use support::sentinel::{batch, record};

/// Rows in the default partition, read as the owner (the app role has the privileges of the parent table only).
async fn in_default_partition(rig: &Rig) -> i64 {
    let mut admin = rig.db.admin().await;
    sqlx::query_scalar("SELECT count(*) FROM ONLY sentinel_records_default")
        .fetch_one(&mut admin)
        .await
        .unwrap()
}

/// The time `days` ago on the **database's** clock (the function prunes by it), in Unix milliseconds.
async fn days_ago(rig: &Rig, days: i32) -> i64 {
    sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM now() - make_interval(days => $1)) * 1000)::bigint")
        .bind(days)
        .fetch_one(&rig.pool)
        .await
        .unwrap()
}

async fn monthly_partitions(rig: &Rig) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid \
         WHERE i.inhparent = 'sentinel_records'::regclass AND c.relname ~ '^sentinel_records_y[0-9]{4}m[0-9]{2}$' ORDER BY 1",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap()
}

async fn set_keep(rig: &Rig, days: Option<i32>) {
    match days {
        Some(d) => {
            sqlx::query("UPDATE retention_settings SET sentinel_keep_days = $1")
                .bind(d)
                .execute(&rig.pool)
                .await
                .unwrap();
        }
        None => {
            sqlx::query("DELETE FROM retention_settings")
                .execute(&rig.pool)
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn the_job_prunes_by_the_admins_retention_and_never_below_the_floor_of_the_function() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    let (old, middling) = (10_i64, days_ago(&rig, 200).await);
    // A record from 1970 and one from 200 days ago go into the default partition: no month of theirs exists.
    rig.records(batch(
        1,
        vec![record("svc/a.yml", old), record("svc/b.yml", middling)],
    ))
    .await
    .unwrap();
    assert_eq!(in_default_partition(&rig).await, 2);
    let job = rig.sentinel.partitions();

    // The admin keeps 365 days: the one from 1970 goes and the one from 200 days ago stays.
    set_keep(&rig, Some(365)).await;
    let report = job.run_once().await.unwrap();
    assert_eq!(report.keep_days, 365);
    assert_eq!(in_default_partition(&rig).await, 1);
    // The admin asks for 30 days. The function refuses less than 90, so the job asks for 90 and does not fail every day: the one from 200
    // days ago goes, and one from 40 days ago (inside the floor) would not have.
    let recent = days_ago(&rig, 40).await;
    rig.records(batch(2, vec![record("svc/c.yml", recent)]))
        .await
        .unwrap();
    set_keep(&rig, Some(30)).await;
    let report = job.run_once().await.unwrap();
    assert_eq!(
        report.keep_days, 90,
        "the floor of the function, which no setting lowers"
    );
    let left: Vec<String> = sqlx::query_scalar("SELECT path FROM sentinel_records ORDER BY path")
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(left, vec!["svc/c.yml".to_owned()]);
}

#[tokio::test]
async fn the_job_creates_the_months_that_are_missing_and_is_idempotent() {
    let Some(rig) = Rig::new().await else { return };
    let before = monthly_partitions(&rig).await;
    assert_eq!(
        before.len(),
        4,
        "last month, this month and the next two exist from the migration: {before:?}"
    );
    let last = before.last().unwrap().clone();
    let mut admin = rig.db.admin().await;
    sqlx::raw_sql(&format!("DROP TABLE {last}"))
        .execute(&mut admin)
        .await
        .unwrap();
    assert_eq!(monthly_partitions(&rig).await.len(), 3);

    let job = rig.sentinel.partitions();
    let first = job.run_once().await.unwrap();
    assert_eq!((first.created, first.dropped), (1, 0));
    assert_eq!(monthly_partitions(&rig).await, before);
    let again = job.run_once().await.unwrap();
    assert_eq!((again.created, again.dropped), (0, 0), "idempotent");
}

#[tokio::test]
async fn a_missing_retention_row_is_the_floor_and_not_a_failure() {
    let Some(rig) = Rig::new().await else { return };
    set_keep(&rig, None).await;
    let report = rig.sentinel.partitions().run_once().await.unwrap();
    assert_eq!(report.keep_days, 90);
}

#[tokio::test]
async fn the_job_runs_through_the_runners_context_and_stops_when_the_lease_is_lost() {
    let Some(rig) = Rig::new().await else { return };
    let spec = SentinelPartitions::spec(&RegistryConfig::default()).unwrap();
    assert_eq!(spec.name(), "registry-sentinel-partitions");
    let (ctx, _expiry) = JobContext::held_until_told_otherwise(spec.name());
    rig.sentinel.partitions().job().run(&ctx).await.unwrap();

    let asked = AtomicUsize::new(0);
    let lost = rig
        .sentinel
        .partitions()
        .sweep(|| {
            if asked.fetch_add(1, Ordering::SeqCst) < 1 {
                Ok(())
            } else {
                Err(JobError::LeaseLost)
            }
        })
        .await;
    assert_eq!(
        lost,
        Err(JobError::LeaseLost),
        "lost before the function is called"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn the_function_the_job_calls_is_the_only_way_the_app_prunes_records() {
    // The app has no DELETE on the table (S9): what the job does it does through the definer function, and a direct delete is refused.
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, vec![record("svc/a.yml", 10)]))
        .await
        .unwrap();
    let refused = sqlx::query("DELETE FROM sentinel_records")
        .execute(&rig.pool)
        .await;
    assert!(
        refused.is_err(),
        "lanekeeper_app has no DELETE on sentinel_records"
    );
    assert_eq!(rig.stored_records().await.len(), 1);
}
