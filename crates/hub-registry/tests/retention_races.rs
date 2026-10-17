//! Retention against concurrent writers (T9, D78, D116): the interleavings that decide whether a blob that a writer is about to need can
//! be lost.
//!
//! Every interleaving here is made by a lock or a condition, never by a delay: a transaction that holds a row, a statement that
//! `pg_stat_activity` shows waiting on a lock (polled), and the job's own two steps called apart (`plan`, then `collect`). The first half
//! is the contract of `PgBlobStore::put` (a re-put of an old blob refreshes its age, and the collector tests the age in the statement that
//! locks and deletes); the second is the reference classes (a reference that appears after the plan is seen by the delete).
//!
//! The way a delete is made to wait without a gate in the production SQL: the blob has settings rows, `settings_index.blob_hash` is
//! `ON DELETE CASCADE`, and a test transaction that holds one of those rows keeps the collector's `DELETE` waiting inside the cascade,
//! after it took its snapshot and while it holds the blob's row lock.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::ContentHash;
use hub_registry::{RegistryConfig, RegistryMetrics, Retention, RetentionDeps};
use ports::BlobStore;
use sqlx::Row;
use support::retention::{Attach, Gc};

fn config() -> RegistryConfig {
    RegistryConfig {
        gc_batch: 1_000,
        gc_chunk: 10,
        // Long enough that nothing in a test that waits for a lock is cut short by it.
        gc_lock_timeout: Duration::from_secs(30),
        ..RegistryConfig::default()
    }
}

const NO_LEASE_CHECK: fn() -> Result<(), hub_platform::jobs::JobError> = || Ok(());

/// A transaction of a second connection that holds the settings rows of `hash`, so that a `DELETE` of the blob waits in its cascade.
async fn hold_the_settings_rows(gc: &Gc, hash: &ContentHash) -> sqlx::PgConnection {
    let mut holder = gc.rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    let held = sqlx::query("SELECT 1 FROM settings_index WHERE blob_hash = $1::bytea FOR UPDATE")
        .bind(hash.as_bytes().as_slice())
        .fetch_all(&mut holder)
        .await
        .unwrap();
    assert!(!held.is_empty(), "the blob has settings rows to hold");
    holder
}

fn collector(gc: &Gc, plan: hub_registry::Plan) -> tokio::task::JoinHandle<hub_registry::Collected> {
    let job = gc.retention.clone();
    tokio::spawn(async move { job.collect(&plan, &NO_LEASE_CHECK).await.unwrap() })
}

// ---------------------------------------------------------------------------------------------------------------
// A put and a delete.

/// The content comes back (a file reverted to a version from months ago) after the plan was made and before the delete: the put refreshes
/// the blob's age, and the delete, which tests the age in its own statement, leaves it. The control is the same run without the put.
#[tokio::test]
async fn a_put_between_the_plan_and_the_delete_protects_the_blob() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let control = gc.old("control: not stored again\n").await;
    let reverted = gc.old("reverted to an old version\n").await;
    let plan = gc.retention.plan().await.unwrap();
    assert!(
        plan.contains(&control) && plan.contains(&reverted),
        "both are collectable when the plan is made"
    );

    // The content is stored again, as an ingest does before the transaction that references it: committed at once.
    let again = gc.put("reverted to an old version\n").await;
    assert_eq!(again, reverted);
    assert!(
        gc.touched_secs_ago(&reverted).await < 60.0,
        "the real put refreshed the age"
    );

    let done = gc.retention.collect(&plan, &NO_LEASE_CHECK).await.unwrap();

    assert!(
        !gc.exists(&control).await,
        "with no put in between the blob is collected"
    );
    assert!(gc.exists(&reverted).await, "a put after the plan wins");
    assert_eq!(
        (done.deleted_blobs, done.skipped_blobs, done.contended_blobs),
        (1, 1, 0)
    );
}

/// The writer that matters, the real ingestion: a delta brings an old version's content back. Ingestion stores the blob (`put`, committed
/// at once, on its own connection) and only then takes the swimlane's apply lock and commits the rows that reference it. The test holds
/// that lock, so the ingestion stands in the gap between the two, and the collector runs its whole delete there, from a plan that was made
/// before the put. The blob must survive (the put refreshed its age), and when the ingestion commits its reference the blob is there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ingest_that_stored_an_old_blob_and_has_not_yet_committed_its_reference_keeps_it() {
    use support::rig::{T, delta, entry, root};

    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let reverted = gc.old("x: 1\n").await;
    let control = gc.old("a blob that nobody stores again\n").await;
    gc.rig.hello().await;
    let plan = gc.retention.plan().await.unwrap();
    assert!(
        plan.contains(&reverted) && plan.contains(&control),
        "both are collectable when the plan is made"
    );

    // The swimlane's apply lock (`ingest::apply::LOCK_SQL`), held by a transaction of the test.
    let mut holder = gc.rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('lanekeeper.registry.apply.sit1', 0))")
        .execute(&mut holder)
        .await
        .unwrap();
    let ingesting = {
        let ingest = gc.rig.ingest.clone();
        let identity = gc.rig.identity.clone();
        tokio::spawn(async move {
            ports::ReportSink::delta(
                &ingest,
                &identity,
                delta(1, None, root(1), vec![entry("a.yml", b"x: 1\n", T)]),
            )
            .await
        })
    };
    gc.wait_for_lock_wait("SELECT pg_advisory_xact_lock").await;
    assert!(
        !ingesting.is_finished(),
        "the ingestion is waiting for the apply lock, after its put"
    );
    assert!(
        gc.touched_secs_ago(&reverted).await < 60.0,
        "its put committed and refreshed the age before the reference exists"
    );
    assert!(
        gc.rig.state("a.yml").await.is_none(),
        "and no row references the blob yet"
    );

    let done = gc.retention.collect(&plan, &NO_LEASE_CHECK).await.unwrap();

    assert_eq!(
        (done.deleted_blobs, done.skipped_blobs, done.contended_blobs),
        (1, 1, 0)
    );
    assert!(
        !gc.exists(&control).await,
        "what nobody stored again is collected"
    );
    assert!(
        gc.exists(&reverted).await,
        "the blob of the ingestion that has not committed yet is not"
    );

    sqlx::raw_sql("COMMIT").execute(&mut holder).await.unwrap();
    ingesting.await.unwrap().expect("the ingestion commits");
    let state = gc.rig.state("a.yml").await.expect("the file is recorded");
    assert_eq!(state.hash, Some(reverted), "the reference names the blob");
    assert!(gc.exists(&reverted).await, "and the blob is there");
    assert_eq!(gc.content(&reverted).await.as_deref(), Some(b"x: 1\n".as_slice()));
}

/// A put that is refreshing the blob when the collector looks (its UPDATE is in flight, not committed): the collector's `SKIP LOCKED` leaves
/// the row, and nothing waits.
#[tokio::test]
async fn a_blob_that_a_put_is_refreshing_is_skipped_and_nobody_waits() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let x = gc.old("being refreshed\n").await;
    let mut put = gc.rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut put).await.unwrap();
    sqlx::query("UPDATE blobs SET touched_at = now() WHERE hash = $1::bytea")
        .bind(x.as_bytes().as_slice())
        .execute(&mut put)
        .await
        .unwrap();

    let report = tokio::time::timeout(Duration::from_secs(10), gc.retention.run_once())
        .await
        .expect("the collector does not wait for the refresh")
        .unwrap();

    assert_eq!(
        (report.planned_blobs, report.deleted_blobs, report.skipped_blobs),
        (1, 0, 1)
    );
    sqlx::raw_sql("COMMIT").execute(&mut put).await.unwrap();
    assert!(gc.exists(&x).await);
    assert!(gc.touched_secs_ago(&x).await < 60.0, "the refresh committed");
    let next = gc.retention.run_once().await.unwrap();
    assert_eq!(next.planned_blobs, 0, "and the next run does not even plan it");
}

/// The collector is deleting an old blob and has not committed when the content is stored again. The put finds the row locked and waits;
/// when the collector commits, the put stores the blob again. Whatever the collector did, a put that returned Ok leaves a stored blob with
/// the bytes that were put, so the transaction that references it commits against a blob that is there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_that_finds_the_delete_locked_waits_and_the_blob_is_stored_again() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let x = gc.old("collected, then needed again\n").await;
    gc.index_rows(&x, 3).await;
    let plan = gc.retention.plan().await.unwrap();
    assert!(plan.contains(&x));
    let holder = hold_the_settings_rows(&gc, &x).await;
    let mut holder = holder;

    let collecting = collector(&gc, plan);
    gc.wait_for_lock_wait("DELETE FROM blobs").await;
    // The collector holds the blob's row (it locked it first) and waits in the cascade. A put of the same content arrives.
    let putting = {
        let blobs = Arc::clone(&gc.rig.blobs);
        tokio::spawn(async move {
            blobs
                .put(Bytes::from_static(b"collected, then needed again\n"))
                .await
        })
    };
    gc.wait_for_lock_wait("INSERT INTO blobs").await;
    assert!(!putting.is_finished(), "the put waits for the collector's lock");
    assert!(!collecting.is_finished());

    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();

    let done = collecting.await.unwrap();
    assert_eq!(
        done.deleted_blobs, 1,
        "the collector deleted it: nothing referenced it when it looked"
    );
    assert_eq!(putting.await.unwrap().unwrap(), x);
    assert!(
        gc.exists(&x).await,
        "and the put stored it again after the delete committed"
    );
    assert_eq!(
        gc.content(&x).await.as_deref(),
        Some(b"collected, then needed again\n".as_slice())
    );
    assert!(gc.touched_secs_ago(&x).await < 60.0);
    // The writer's reference now commits against a blob that is there.
    gc.attach(Attach::FileStateNfs, &x, 1).await;
    let next = gc.retention.run_once().await.unwrap();
    assert_eq!(next.planned_blobs, 0, "referenced and fresh: not collectable");
}

// ---------------------------------------------------------------------------------------------------------------
// A reference and a delete.

/// A reference of every class that appears after the plan and before the delete: the delete tests every class again, on a snapshot that is
/// newer than its lock, and leaves the blob. `contended` stays zero: it is the check that stopped it, not a foreign key (three of the
/// classes have one, and a missed check would show as a failed transaction, not a lost row).
#[tokio::test]
async fn a_reference_that_appears_between_the_plan_and_the_delete_keeps_the_blob() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    for (n, how) in Attach::all().into_iter().filter(|a| a.pins()).enumerate() {
        gc.clear().await;
        let x = gc.old(&format!("referenced after the plan: {how:?}\n")).await;
        let plan = gc.retention.plan().await.unwrap();
        assert!(plan.contains(&x), "{how:?}: collectable when the plan is made");

        gc.attach(how, &x, n + 1).await;
        let done = gc.retention.collect(&plan, &NO_LEASE_CHECK).await.unwrap();

        assert!(
            gc.exists(&x).await,
            "{how:?}: a reference that appeared after the plan keeps the blob"
        );
        assert_eq!(
            (done.deleted_blobs, done.skipped_blobs, done.contended_blobs),
            (0, 1, 0),
            "{how:?}: skipped by the check, not stopped by a foreign key"
        );
    }
    // The control: no reference appears.
    gc.clear().await;
    let x = gc.old("never referenced\n").await;
    let plan = gc.retention.plan().await.unwrap();
    let done = gc.retention.collect(&plan, &NO_LEASE_CHECK).await.unwrap();
    assert!(!gc.exists(&x).await);
    assert_eq!(done.deleted_blobs, 1);
}

/// A new observation of the same content appears after the plan: the keep rules are tested again in the delete, so the blob is a version
/// that is kept (a version newer than 180 days), with no put at all.
#[tokio::test]
async fn a_new_observation_of_the_same_content_after_the_plan_keeps_the_blob() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let x = gc.old("seen again\n").await;
    let plan = gc.retention.plan().await.unwrap();
    assert!(plan.contains(&x));

    gc.observe("app/core.yml", Some(&x), 0.5).await;
    let done = gc.retention.collect(&plan, &NO_LEASE_CHECK).await.unwrap();

    assert!(gc.exists(&x).await);
    assert_eq!((done.deleted_blobs, done.skipped_blobs), (0, 1));
}

/// A writer is referencing the blob through a foreign key: its insert is in flight, and holds a `FOR KEY SHARE` lock on the blob's row.
/// The collector's `SKIP LOCKED` leaves the row, and the next run sees the reference.
#[tokio::test]
async fn a_writer_that_is_referencing_a_blob_makes_the_collector_skip_it() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    gc.user().await;
    let x = gc.old("a draft is being written\n").await;
    let mut writer = gc.rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut writer).await.unwrap();
    sqlx::query(
        "INSERT INTO drafts (id, author_tid, author_oid, swimlane_id, path, expected, new_hash, expires_at) \
         VALUES ('draft-1', 't1', 'o1', 'sit1', 'a.yml', '{}', $1::bytea, now() + interval '3 days')",
    )
    .bind(x.as_bytes().as_slice())
    .execute(&mut writer)
    .await
    .unwrap();

    let report = tokio::time::timeout(Duration::from_secs(10), gc.retention.run_once())
        .await
        .expect("the collector does not wait for the writer")
        .unwrap();

    assert_eq!(
        (
            report.planned_blobs,
            report.deleted_blobs,
            report.skipped_blobs,
            report.contended_blobs
        ),
        (1, 0, 1, 0),
        "skipped: locked by the writer, not stopped by the foreign key and not waited for"
    );
    sqlx::raw_sql("COMMIT").execute(&mut writer).await.unwrap();
    assert!(gc.exists(&x).await);
    let next = gc.retention.run_once().await.unwrap();
    assert_eq!(next.planned_blobs, 0, "the committed draft is a reference");
    assert!(gc.exists(&x).await);
}

/// The other order: the collector holds the blob (it is deleting it) when a writer tries to reference it. The writer's insert waits for the
/// collector, and when the collector commits the foreign key refuses it: a clean error, and no row that points at a blob that is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_foreign_key_refuses_a_reference_that_arrives_while_the_blob_is_being_deleted() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    gc.user().await;
    let x = gc.old("deleted under a late writer\n").await;
    gc.index_rows(&x, 2).await;
    let plan = gc.retention.plan().await.unwrap();
    let mut holder = hold_the_settings_rows(&gc, &x).await;
    let collecting = collector(&gc, plan);
    gc.wait_for_lock_wait("DELETE FROM blobs").await;

    let writing = {
        let pool = gc.pool().clone();
        let hash = x;
        tokio::spawn(async move {
            sqlx::query(
                "INSERT INTO drafts (id, author_tid, author_oid, swimlane_id, path, expected, new_hash, expires_at) \
                 VALUES ('late', 't1', 'o1', 'sit1', 'a.yml', '{}', $1::bytea, now() + interval '3 days')",
            )
            .bind(hash.as_bytes().as_slice())
            .execute(&pool)
            .await
        })
    };
    gc.wait_for_lock_wait("INSERT INTO drafts").await;
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();

    assert_eq!(collecting.await.unwrap().deleted_blobs, 1);
    let refused = writing
        .await
        .unwrap()
        .expect_err("the foreign key refuses the late draft");
    let code = refused
        .as_database_error()
        .and_then(|e| e.code().map(std::borrow::Cow::into_owned));
    assert_eq!(code.as_deref(), Some("23503"), "a foreign key violation");
    let drafts: i64 = sqlx::query_scalar("SELECT count(*) FROM drafts")
        .fetch_one(gc.pool())
        .await
        .unwrap();
    assert_eq!(drafts, 0, "no draft points at a blob that is gone");
}

// ---------------------------------------------------------------------------------------------------------------
// The lock timeout.

/// A delete that has to wait gives up after `gc_lock_timeout`, the chunk is left for the next run (counted as contended, the run is not a
/// failure), and the connection goes back to the pool with the session's own settings as it found them.
#[tokio::test]
async fn lock_timeout_leaves_the_sessions_setting_as_it_found_it_and_a_blocked_chunk_waits_for_the_next_run()
{
    let config = RegistryConfig {
        gc_lock_timeout: Duration::from_millis(300),
        ..config()
    };
    let Some(gc) = Gc::with(config.clone()).await else {
        return;
    };
    // One connection, so that every statement of the job and of the checks below runs in the same session.
    let single = gc.rig.db.app_pool_of(1).await;
    let metrics = RegistryMetrics::default();
    let job = Retention::new(RetentionDeps {
        pool: single.clone(),
        ingest: gc.rig.ingest.clone(),
        clock: Arc::clone(&gc.rig.clock) as _,
        config,
        metrics: metrics.clone(),
    })
    .unwrap();
    let setting = |name: &'static str| {
        let pool = single.clone();
        async move {
            sqlx::query(&format!("SHOW {name}"))
                .fetch_one(&pool)
                .await
                .unwrap()
                .get::<String, _>(0)
        }
    };
    let (lock_before, statement_before) = (setting("lock_timeout").await, setting("statement_timeout").await);

    let x = gc.old("blocked in the cascade\n").await;
    gc.index_rows(&x, 2).await;
    let mut holder = hold_the_settings_rows(&gc, &x).await;
    let started = std::time::Instant::now();
    let blocked = job
        .run_once()
        .await
        .expect("a chunk that a lock stopped is not a failed run");
    let waited = started.elapsed();
    assert_eq!(
        (blocked.deleted_blobs, blocked.contended_blobs),
        (0, 1),
        "the delete gave up at its lock timeout"
    );
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_secs(5),
        "it waited for its own timeout (300 ms) and not for the lock: {waited:?}"
    );
    assert!(gc.exists(&x).await, "the blob is where it was");
    assert_eq!(
        setting("lock_timeout").await,
        lock_before,
        "the session's lock_timeout is as it was"
    );
    assert_eq!(
        setting("statement_timeout").await,
        statement_before,
        "and its statement_timeout"
    );
    assert_eq!(
        metrics.retention_blob_count(hub_registry::metrics::RetentionBlobs::Contended),
        1
    );

    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
    let next = job.run_once().await.unwrap();
    assert_eq!(
        next.deleted_blobs, 1,
        "with the lock gone the next run deletes it"
    );
    assert_eq!(setting("lock_timeout").await, lock_before);
    assert_eq!(setting("statement_timeout").await, statement_before);
}
