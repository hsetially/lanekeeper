//! Retention GC (T9, D78, D116, A34): what the job deletes, what it never deletes, and what a dry run is.
//!
//! Over a real Postgres 16 as `lanekeeper_app`. The one property that matters is `gc_never_deletes_a_referenced_blob`: a proptest over
//! random reference graphs that put every reference class and every look-alike on random blobs of random age and random history, and
//! hold the job to an oracle written in Rust from the rules, in both directions (nothing referenced goes, everything collectable does).
//! The races are in `retention_races.rs`, the columns in `retention_columns.rs`, the prune of windows and staging in `retention_prune.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod support;

use std::collections::BTreeSet;
use std::time::Duration;

use domain::ContentHash;
use hub_platform::blobs::endpoint::BlobPolicy;
use hub_platform::jobs::{JobContext, JobError};
use hub_registry::metrics::{RetentionBlobs, RetentionRows, RetentionTable};
use hub_registry::{RegistryBlobPolicy, RegistryConfig, RegistryMetrics};
use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use support::retention::{Attach, DAY, FRESH, Gc, OLD, retention_of};

const HOUR: f64 = 3600.0;

/// A configuration whose run is not cut and whose transactions are small, so that several of them make a run.
fn config() -> RegistryConfig {
    RegistryConfig {
        gc_batch: 10_000,
        gc_chunk: 7,
        ..RegistryConfig::default()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The property.

#[derive(Debug, Clone)]
struct ObsSpec {
    /// How old the version is, in days, by the registry's clock.
    days: u32,
    /// How many later versions of the same file there are (they are deletions, which pin nothing).
    newer: u32,
}

#[derive(Debug, Clone)]
struct BlobSpec {
    /// Seconds since the blob was last stored.
    age_secs: u64,
    attach: Vec<Attach>,
    obs: Vec<ObsSpec>,
}

#[derive(Debug, Clone)]
struct WorldSpec {
    keep_versions: u32,
    blobs: Vec<BlobSpec>,
}

const GRACE_SECS: u64 = 48 * 3600;

fn blob_spec() -> impl Strategy<Value = BlobSpec> {
    let age = prop::sample::select(vec![FRESH as u64, 47 * 3600, 49 * 3600, OLD as u64]);
    let attach = proptest::collection::btree_set(prop::sample::select(Attach::all()), 0..=2)
        .prop_map(|set| set.into_iter().collect::<Vec<_>>());
    let obs = proptest::collection::vec(
        (prop::sample::select(vec![10_u32, 100, 260, 400]), 0_u32..=3)
            .prop_map(|(days, newer)| ObsSpec { days, newer }),
        0..=2,
    );
    (age, attach, obs).prop_map(|(age_secs, attach, obs)| BlobSpec {
        age_secs,
        attach,
        obs,
    })
}

fn world_spec() -> impl Strategy<Value = WorldSpec> {
    (1_u32..=3, proptest::collection::vec(blob_spec(), 1..=9))
        .prop_map(|(keep_versions, blobs)| WorldSpec { keep_versions, blobs })
}

impl BlobSpec {
    fn pinned(&self) -> bool {
        self.attach.iter().any(|a| a.pins())
    }

    fn kept(&self, keep_versions: u32) -> bool {
        self.obs.iter().any(|o| o.days <= 180 || o.newer < keep_versions)
    }

    fn collectable(&self, keep_versions: u32) -> bool {
        self.age_secs > GRACE_SECS && !self.pinned() && !self.kept(keep_versions)
    }
}

/// Build the world, run the job, and compare what is left with the oracle.
async fn case(gc: &Gc, spec: WorldSpec, salt: usize) -> Result<(), TestCaseError> {
    gc.clear().await;
    gc.keep(i32::try_from(spec.keep_versions).unwrap(), 180).await;
    let mut hashes: Vec<ContentHash> = Vec::new();
    let mut n = 0_usize;
    for (i, blob) in spec.blobs.iter().enumerate() {
        let hash = gc.put(&format!("case {salt} blob {i}\n")).await;
        gc.age(&hash, blob.age_secs as f64).await;
        for how in &blob.attach {
            n += 1;
            gc.attach(*how, &hash, n).await;
        }
        for (j, o) in blob.obs.iter().enumerate() {
            let path = format!("hist/{i}-{j}.yml");
            gc.observe(&path, Some(&hash), f64::from(o.days)).await;
            for k in 0..o.newer {
                // A later version of the same file, 15 minutes apart.
                gc.observe(&path, None, f64::from(o.days) - 0.01 * f64::from(k + 1))
                    .await;
            }
        }
        hashes.push(hash);
    }
    let expected_gone: BTreeSet<ContentHash> = spec
        .blobs
        .iter()
        .zip(&hashes)
        .filter(|(b, _)| b.collectable(spec.keep_versions))
        .map(|(_, h)| *h)
        .collect();
    let before = gc.all_blobs().await;

    let report = gc.retention.run_once().await.expect("the run");

    let after = gc.all_blobs().await;
    let gone: BTreeSet<ContentHash> = before.difference(&after).copied().collect();
    prop_assert_eq!(
        gone.len(),
        expected_gone.len(),
        "the number of blobs collected (spec {:?})",
        spec
    );
    for (b, h) in spec.blobs.iter().zip(&hashes) {
        if b.pinned() {
            prop_assert!(after.contains(h), "a referenced blob was deleted: {:?}", b);
        }
        prop_assert_eq!(
            expected_gone.contains(h),
            gone.contains(h),
            "the blob {:?} (keep versions {})",
            b,
            spec.keep_versions
        );
    }
    prop_assert_eq!(report.deleted_blobs, expected_gone.len());
    prop_assert_eq!(
        report.contended_blobs,
        0,
        "no transaction was stopped by a foreign key or a lock"
    );
    Ok(())
}

#[test]
fn gc_never_deletes_a_referenced_blob() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let Some(gc) = rt.block_on(Gc::with(config())) else {
        return;
    };
    // 24 cases run in the gate; `PROPTEST_CASES=300` runs more. Every case is a world in the same database, emptied before it.
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);
    let mut runner = TestRunner::new(ProptestConfig {
        cases,
        max_shrink_iters: 40,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let counter = std::cell::Cell::new(0_usize);
    let result = runner.run(&world_spec(), |spec| {
        counter.set(counter.get() + 1);
        rt.block_on(case(&gc, spec, counter.get()))
    });
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// The property reaches every class by chance; this makes sure by listing them: each class that pins keeps a blob that is otherwise
/// collectable, and each look-alike does not.
#[tokio::test]
async fn every_reference_class_pins_and_every_look_alike_does_not() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let mut classes = Vec::new();
    for (n, how) in Attach::all().into_iter().enumerate() {
        let hash = gc.old(&format!("class {how:?}\n")).await;
        gc.attach(how, &hash, n + 1).await;
        classes.push((how, hash));
    }
    let control = gc.old("no reference at all\n").await;

    let report = gc.retention.run_once().await.unwrap();

    assert!(!gc.exists(&control).await, "the control is collected");
    for (how, hash) in &classes {
        assert_eq!(
            gc.exists(hash).await,
            how.pins(),
            "{how:?}: a reference class keeps its blob, a look-alike does not"
        );
    }
    let look_alikes = classes.iter().filter(|(h, _)| !h.pins()).count();
    assert_eq!(report.deleted_blobs, look_alikes + 1);
    assert_eq!(report.contended_blobs, 0);
}

// ---------------------------------------------------------------------------------------------------------------
// The keep rules.

/// A file's versions as observations, oldest first, each `days` before the registry's clock; the content of version `k` is
/// `{name} v{k}`. Returns the hashes in the same order.
async fn history(gc: &Gc, name: &str, versions: &[f64]) -> Vec<ContentHash> {
    let mut out = Vec::new();
    for (k, days) in versions.iter().enumerate() {
        let hash = gc.old(&format!("{name} v{}\n", k + 1)).await;
        gc.observe(name, Some(&hash), *days).await;
        out.push(hash);
    }
    out
}

#[tokio::test]
async fn keeps_the_last_50_versions_and_180_days() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };

    // File A: 60 versions, all older than 180 days. The newest 50 stay, the oldest 10 go.
    let a_days: Vec<f64> = (0..60).map(|k| 400.0 - f64::from(k)).collect();
    let a = history(&gc, "a.yml", &a_days).await;

    // File B: three versions inside 180 days: they stay for their age.
    let b = history(&gc, "b.yml", &[100.0, 90.0, 80.0]).await;

    // File C: 52 versions. The newest 50 are at most 4 days old. The 51st is 1 minute inside 180 days, the 52nd one minute outside:
    // the 51st is kept for its age, the 52nd goes (it is beyond 50 versions and older than 180 days).
    let one_minute = 1.0 / (24.0 * 60.0);
    let mut c_days = vec![180.0 + one_minute, 180.0 - one_minute];
    c_days.extend((0..50).map(|k| 4.0 - 0.01 * f64::from(k)));
    let c = history(&gc, "c.yml", &c_days).await;

    // File D: 55 versions inside ten days: the five beyond the last 50 stay for their age.
    let d_days: Vec<f64> = (0..55).map(|k| 10.0 - 0.1 * f64::from(k)).collect();
    let d = history(&gc, "d.yml", &d_days).await;

    // A version that two files have: the one that is buried and old in E1 is the newest in E2, so it stays.
    let shared = gc.old("shared by two files\n").await;
    gc.observe("e1.yml", Some(&shared), 300.0).await;
    for k in 0..50 {
        gc.observe("e1.yml", None, 299.0 - f64::from(k)).await;
    }
    gc.observe("e2.yml", Some(&shared), 5.0).await;
    // And one that is buried and old in both.
    let buried_twice = gc.old("buried in two files\n").await;
    gc.observe("e1.yml", Some(&buried_twice), 301.0).await;
    gc.observe("e3.yml", Some(&buried_twice), 302.0).await;
    for k in 0..50 {
        gc.observe("e3.yml", None, 301.0 - f64::from(k)).await;
    }
    // And a blob that no version names at all (a part that was stored and never applied).
    let never_applied = gc.old("never applied\n").await;

    let report = gc.retention.run_once().await.unwrap();
    assert_eq!(
        (report.keep_versions, report.keep_days),
        (50, 180),
        "Q33's defaults, from the settings row"
    );

    let gone: BTreeSet<ContentHash> = a[..10]
        .iter()
        .chain([&c[0], &buried_twice, &never_applied])
        .copied()
        .collect();
    let kept: BTreeSet<ContentHash> = a[10..]
        .iter()
        .chain(&b)
        .chain(&c[1..])
        .chain(&d)
        .chain([&shared])
        .copied()
        .collect();
    for h in &gone {
        assert!(
            !gc.exists(h).await,
            "collectable: beyond the last 50 versions and older than 180 days"
        );
    }
    for h in &kept {
        assert!(
            gc.exists(h).await,
            "kept: among the last 50 versions or newer than 180 days"
        );
    }
    assert_eq!(report.deleted_blobs, gone.len());
    assert_eq!(
        gc.all_blobs().await.len(),
        kept.len(),
        "nothing else was in the store, and nothing else was deleted"
    );
}

#[tokio::test]
async fn the_keep_rules_are_the_settings_row_and_q33_when_there_is_none() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let b = history(&gc, "b.yml", &[100.0, 90.0, 80.0]).await;

    // Keep the last 3 versions: all three stay (nothing is older than 180 days anyway).
    gc.keep(3, 10).await;
    let report = gc.retention.run_once().await.unwrap();
    assert_eq!((report.keep_versions, report.keep_days), (3, 10));
    assert_eq!(report.deleted_blobs, 0, "three versions, the last three are kept");

    // Keep the last 2 versions and 10 days: the oldest is beyond both.
    gc.keep(2, 10).await;
    let report = gc.retention.run_once().await.unwrap();
    assert_eq!((report.keep_versions, report.keep_days), (2, 10));
    assert_eq!(report.deleted_blobs, 1);
    assert!(!gc.exists(&b[0]).await);
    assert!(gc.exists(&b[1]).await && gc.exists(&b[2]).await);

    // Keep the last version and 85 days: the version of 90 days ago is beyond both, the one of 80 days ago is the last.
    gc.keep(1, 85).await;
    let report = gc.retention.run_once().await.unwrap();
    assert_eq!((report.keep_versions, report.keep_days), (1, 85));
    assert_eq!(report.deleted_blobs, 1);
    assert!(!gc.exists(&b[1]).await);
    assert!(
        gc.exists(&b[2]).await,
        "the last version stays, and it is inside 85 days"
    );

    // No settings row at all: Q33's defaults, 50 versions and 180 days, so a version of 80 days ago stays whatever else is true.
    gc.admin("DELETE FROM retention_settings").await;
    let orphan = gc.old("an orphan that no version names\n").await;
    let report = gc.retention.run_once().await.unwrap();
    assert_eq!((report.keep_versions, report.keep_days), (50, 180));
    assert_eq!(report.deleted_blobs, 1);
    assert!(!gc.exists(&orphan).await);
    assert!(gc.exists(&b[2]).await);
}

// ---------------------------------------------------------------------------------------------------------------
// The grace period.

#[tokio::test]
async fn a_blob_is_not_collected_before_its_grace_period_has_passed() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let inside = gc.aged("inside the grace period\n", 47.0 * HOUR).await;
    let outside = gc.aged("outside the grace period\n", 49.0 * HOUR).await;
    let fresh = gc.put("just stored\n").await;

    let report = gc.retention.run_once().await.unwrap();

    assert!(
        gc.exists(&inside).await,
        "47 hours: inside the 48 hour grace period"
    );
    assert!(gc.exists(&fresh).await);
    assert!(
        !gc.exists(&outside).await,
        "49 hours, unreferenced and unkept: collected"
    );
    assert_eq!(report.deleted_blobs, 1);
}

#[tokio::test]
async fn a_longer_grace_period_is_the_configurations() {
    let config = RegistryConfig {
        gc_grace: Duration::from_secs(10 * 24 * 3600),
        ..config()
    };
    let Some(gc) = Gc::with(config).await else { return };
    let nine_days = gc.aged("nine days\n", 9.0 * DAY).await;
    let eleven_days = gc.aged("eleven days\n", 11.0 * DAY).await;
    gc.retention.run_once().await.unwrap();
    assert!(gc.exists(&nine_days).await);
    assert!(!gc.exists(&eleven_days).await);
}

// ---------------------------------------------------------------------------------------------------------------
// The bounds of a run (A34).

#[tokio::test]
async fn a_run_takes_at_most_gc_batch_blobs_and_the_rest_waits_for_the_next() {
    let config = RegistryConfig {
        gc_batch: 3,
        gc_chunk: 2,
        ..RegistryConfig::default()
    };
    let Some(gc) = Gc::with(config).await else { return };
    for i in 0..7 {
        gc.old(&format!("orphan {i}\n")).await;
    }
    let mut taken = Vec::new();
    for _ in 0..5 {
        let report = gc.retention.run_once().await.unwrap();
        taken.push((report.deleted_blobs, report.truncated));
    }
    assert_eq!(
        taken,
        vec![(3, true), (3, true), (1, false), (0, false), (0, false)],
        "3, 3 and 1, and then nothing: the last run was not cut, and a run with nothing to do deletes nothing"
    );
    assert!(gc.all_blobs().await.is_empty());
}

#[tokio::test]
async fn a_run_takes_at_most_gc_max_bytes() {
    // The smallest limit the configuration allows is the largest blob (4 MiB): blobs of 2 MiB, two to a run.
    let config = RegistryConfig {
        gc_max_bytes: 4 * 1024 * 1024,
        ..RegistryConfig::default()
    };
    let Some(gc) = Gc::with(config).await else { return };
    for i in 0..3 {
        let text = format!("{i}").repeat(2 * 1024 * 1024);
        gc.old(&text).await;
    }
    let first = gc.retention.run_once().await.unwrap();
    assert_eq!(
        first.deleted_blobs, 2,
        "two blobs of 2 MiB are 4 MiB, the third does not fit"
    );
    assert_eq!(first.deleted_bytes, 4 * 1024 * 1024);
    assert_eq!(first.planned_bytes, 4 * 1024 * 1024);
    assert!(first.truncated);
    let second = gc.retention.run_once().await.unwrap();
    assert_eq!((second.deleted_blobs, second.truncated), (1, false));
    assert_eq!(second.deleted_bytes, 2 * 1024 * 1024);
    assert!(gc.all_blobs().await.is_empty());
}

#[tokio::test]
async fn a_run_that_deleted_everything_has_nothing_to_do_the_second_time() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    for i in 0..4 {
        gc.old(&format!("old orphan {i}\n")).await;
    }
    let kept = gc.old("referenced\n").await;
    gc.attach(Attach::FileStateNfs, &kept, 1).await;
    gc.put("fresh\n").await;

    let first = gc.retention.run_once().await.unwrap();
    assert_eq!(first.deleted_blobs, 4);
    let after_first = gc.dump().await;

    let second = gc.retention.run_once().await.unwrap();

    assert_eq!(
        (
            second.planned_blobs,
            second.deleted_blobs,
            second.skipped_blobs,
            second.contended_blobs
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        gc.dump().await,
        after_first,
        "a second run changes nothing: not a row, not even a tuple version"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The dry run (A34): it reports and changes nothing.

/// A database with something for every step of the job to do: blobs to collect, a processed window and a stale staged part.
async fn with_something_to_do(gc: &Gc) -> Vec<ContentHash> {
    let mut orphans = Vec::new();
    for i in 0..3 {
        let h = gc.old(&format!("collectable {i}\n")).await;
        gc.index_rows(&h, 5).await;
        orphans.push(h);
    }
    let kept = gc.old("referenced\n").await;
    gc.attach(Attach::BaselineBlob, &kept, 1).await;
    let long_ago = Gc::days_ago_ms(30.0);
    gc.admin(&format!(
        "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at, closed_at, processed_at) \
         VALUES ('sit1', 'sync-sit1', 'uid-old', to_timestamp({ms}::double precision / 1000.0), to_timestamp({ms}::double precision / 1000.0), \
                 to_timestamp({ms}::double precision / 1000.0)); \
         INSERT INTO delta_staging (swimlane_id, seq, part, base_root, new_root, metadata, staged_at) \
         VALUES ('sit1', 1, 0, NULL, '\\x{root}', '{{\"v\":1,\"entries\":[],\"removed\":[],\"skipped\":[],\"bytes\":0}}', \
                 to_timestamp({ms}::double precision / 1000.0))",
        ms = long_ago,
        root = hex::encode(ports::content_hash(b"root").as_bytes())
    ))
    .await;
    orphans
}

#[tokio::test]
async fn dry_run_deletes_nothing() {
    let dry = RegistryConfig {
        retention_dry_run: true,
        ..config()
    };
    let Some(gc) = Gc::with(dry.clone()).await else {
        return;
    };
    let orphans = with_something_to_do(&gc).await;
    let before = gc.dump().await;
    let metrics = RegistryMetrics::default();
    let job = retention_of(&gc.rig, dry, metrics.clone());

    let report = job.run_once().await.unwrap();

    assert!(report.dry_run);
    assert_eq!(report.planned_blobs, 3, "it says what it would delete");
    assert!(report.planned_bytes > 0);
    assert_eq!(
        (report.windows, report.staging),
        (1, 1),
        "and which rows it would prune and sweep"
    );
    assert_eq!(
        (report.deleted_blobs, report.skipped_blobs, report.contended_blobs),
        (0, 0, 0),
        "and deletes nothing"
    );
    assert_eq!(
        gc.dump().await,
        before,
        "every table is as it was: not a row less, not a tuple version more (the digest includes xmin)"
    );
    for h in &orphans {
        assert!(gc.exists(h).await);
    }
    // The only metric-visible effects are the counters of its own.
    for (result, expected) in [
        (RetentionBlobs::Deleted, 0),
        (RetentionBlobs::Skipped, 0),
        (RetentionBlobs::Contended, 0),
    ] {
        assert_eq!(metrics.retention_blob_count(result), expected, "{result:?}");
    }
    assert_eq!(metrics.retention_blob_count(RetentionBlobs::WouldDelete), 3);
    assert_eq!(
        metrics.retention_row_count(RetentionTable::SyncWindows, RetentionRows::Deleted),
        0
    );
    assert_eq!(
        metrics.retention_row_count(RetentionTable::DeltaStaging, RetentionRows::Deleted),
        0
    );
    assert_eq!(
        metrics.retention_row_count(RetentionTable::SyncWindows, RetentionRows::WouldDelete),
        1
    );
    assert_eq!(
        metrics.retention_row_count(RetentionTable::DeltaStaging, RetentionRows::WouldDelete),
        1
    );
    assert_eq!(
        metrics.retention_run_counts(),
        (0, 1, 0),
        "a dry run is counted as one"
    );

    // The control: the same database, the same job, not a dry run: it does all of it.
    let real = retention_of(&gc.rig, config(), RegistryMetrics::default());
    let report = real.run_once().await.unwrap();
    assert_eq!(report.deleted_blobs, 3);
    assert_eq!((report.windows, report.staging), (1, 1));
    assert_ne!(gc.dump().await, before);
}

#[tokio::test]
async fn a_dry_run_takes_no_lock() {
    let dry = RegistryConfig {
        retention_dry_run: true,
        ..config()
    };
    let Some(gc) = Gc::with(dry).await else { return };
    let h = gc.old("collectable\n").await;
    // A transaction holds the row, as a put refreshing it would. A job that locked the row would skip it or wait; a dry run does neither: it
    // reads, and says the blob is collectable.
    let mut holder = gc.rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT 1 FROM blobs WHERE hash = $1::bytea FOR UPDATE")
        .bind(h.as_bytes().as_slice())
        .execute(&mut holder)
        .await
        .unwrap();
    let report = tokio::time::timeout(Duration::from_secs(5), gc.retention.run_once())
        .await
        .expect("a dry run does not wait for a lock")
        .unwrap();
    assert_eq!(report.planned_blobs, 1);
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
}

// ---------------------------------------------------------------------------------------------------------------
// The lease.

#[tokio::test]
async fn the_job_runs_through_the_runners_context() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let h = gc.old("collectable\n").await;
    let spec = hub_registry::Retention::spec(&config()).unwrap();
    assert_eq!(spec.name(), "registry-retention");
    let (ctx, _expiry) = JobContext::held_until_told_otherwise(spec.name());
    gc.retention.job().run(&ctx).await.unwrap();
    assert!(!gc.exists(&h).await);
}

#[tokio::test]
async fn a_run_stops_at_the_next_chunk_when_the_lease_is_lost() {
    let config = RegistryConfig {
        gc_chunk: 1,
        ..config()
    };
    let Some(gc) = Gc::with(config).await else { return };
    for i in 0..5 {
        gc.old(&format!("orphan {i}\n")).await;
    }
    // The lease is asked before the run, before the staging sweep, before the prune of windows, and then before each chunk. It is held for
    // those three and for the first two chunks, and lost at the third.
    let asked = std::sync::atomic::AtomicUsize::new(0);
    let result = gc
        .retention
        .sweep(|| {
            if asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 5 {
                Ok(())
            } else {
                Err(JobError::LeaseLost)
            }
        })
        .await;
    assert_eq!(result, Err(JobError::LeaseLost));
    assert_eq!(
        gc.all_blobs().await.len(),
        3,
        "two chunks of one blob were deleted, each whole, and the third never began"
    );
    // A job whose lease expired through the runner's context ends the same way, before it writes anything.
    let (ctx, expiry) = JobContext::held_until_told_otherwise("registry-retention");
    expiry
        .send(tokio::time::Instant::now() - Duration::from_secs(1))
        .unwrap();
    assert_eq!(gc.retention.job().run(&ctx).await, Err(JobError::LeaseLost));
    assert_eq!(gc.all_blobs().await.len(), 3);
}

#[tokio::test]
async fn only_the_lease_holder_runs_the_job() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use hub_platform::jobs::{Job, JobMetrics, JobRunner, JobSpec};
    use ports::fakes::FakeLeases;
    use ports::{LeaseError, LeaseGuard, Leases};
    use support::world::wait_until;

    /// How long the test waits for what the replicas do. It is a deadline for a broken job, not a delay: the test goes on as soon as the
    /// replicas have done what it waits for, which takes a few hundred milliseconds on a machine with nothing else to do.
    const DEADLINE: Duration = Duration::from_secs(20);
    /// How many times each replica must have had its say before "the other one never ran" is read as being about the lease: it has run
    /// the job this many times, or it has been told this many times that another replica holds the lease.
    const CHANCES: usize = 3;

    /// Counts the runs of the real job.
    struct Counting {
        inner: Arc<dyn Job>,
        runs: AtomicUsize,
    }
    #[async_trait]
    impl Job for Counting {
        async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.inner.run(ctx).await
        }
    }

    /// One replica's way to the lease store the replicas share: it asks the store and counts the answers "someone else holds it".
    struct Asking {
        store: FakeLeases,
        refused: AtomicUsize,
    }
    #[async_trait]
    impl Leases for Asking {
        async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError> {
            let answer = self.store.try_acquire(name, ttl).await;
            if matches!(answer, Ok(None)) {
                self.refused.fetch_add(1, Ordering::SeqCst);
            }
            answer
        }
    }

    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let h = gc.old("collectable\n").await;
    // Two replicas: their own job over the same database, and one lease between them.
    let replicas: Vec<Arc<Counting>> = (0..2)
        .map(|_| {
            Arc::new(Counting {
                inner: retention_of(&gc.rig, config(), RegistryMetrics::default()).job(),
                runs: AtomicUsize::new(0),
            })
        })
        .collect();
    // The name is the real job's; the timing is shortened so that the test does not wait a minute.
    let name = hub_registry::Retention::spec(&config())
        .unwrap()
        .name()
        .to_owned();
    let leases = FakeLeases::new();
    let asking: Vec<Arc<Asking>> = replicas
        .iter()
        .map(|_| {
            Arc::new(Asking {
                store: leases.clone(),
                refused: AtomicUsize::new(0),
            })
        })
        .collect();
    let mut handles = Vec::new();
    for (replica, asked) in replicas.iter().zip(&asking) {
        let mut runner = JobRunner::new(asked.clone(), JobMetrics::default());
        runner
            .register(
                JobSpec::new(&name, Duration::from_millis(150), Duration::from_secs(3)).unwrap(),
                replica.clone(),
            )
            .unwrap();
        handles.push(runner.start());
    }
    let runs_of = |replicas: &[Arc<Counting>]| -> Vec<usize> {
        replicas.iter().map(|r| r.runs.load(Ordering::SeqCst)).collect()
    };
    // Some replica has run the job, and the run collected.
    wait_until(
        "a replica to run the job and collect the blob",
        DEADLINE,
        || async { runs_of(&replicas).iter().any(|r| *r > 0) && !gc.exists(&h).await },
    )
    .await;
    // Every replica has had its chances: the one that holds the lease has run the job again and again (every 150 ms), and the one that
    // does not has asked for the lease as often (every 150 ms) and been told that another holds it. Counted, not slept: however slow the
    // machine is, "the other one never ran" is only read after the other one has been refused the lease `CHANCES` times.
    wait_until(
        "every replica to have run the job, or been refused the lease, three times",
        DEADLINE,
        || async {
            replicas.iter().zip(&asking).all(|(r, a)| {
                r.runs.load(Ordering::SeqCst) >= CHANCES || a.refused.load(Ordering::SeqCst) >= CHANCES
            })
        },
    )
    .await;
    for handle in handles {
        handle.shutdown().await;
    }
    let runs = runs_of(&replicas);
    assert!(
        (runs[0] > 0) != (runs[1] > 0),
        "exactly one replica ran the job: {runs:?}"
    );
    assert!(!gc.exists(&h).await, "and it collected");
}

// ---------------------------------------------------------------------------------------------------------------
// What a denied file's old versions are.

#[tokio::test]
async fn a_denied_files_old_blob_is_collected_like_any_other_once_beyond_the_keep_rules() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    gc.keep(2, 10).await;
    // A file that was an ordinary config file, whose versions are in the store, and that its agent has since flagged as denied.
    let old_version = gc.old("secret = hunter2\n").await;
    gc.observe("secrets/app.key", Some(&old_version), 300.0).await;
    for k in 0..3 {
        gc.observe("secrets/app.key", None, 200.0 - f64::from(k)).await;
    }
    gc.admin(
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state, denied) \
         VALUES ('sit1', 'secrets/app.key', 'secrets/app.key', 'base', 'denied', 'in_sync', true)",
    )
    .await;
    let policy = RegistryBlobPolicy::new(gc.pool().clone(), &gc.config, RegistryMetrics::default());
    assert!(
        !policy.allowed(&old_version).await,
        "meanwhile the blob endpoint refuses every version a denied file has had (T9, D104)"
    );
    assert!(gc.exists(&old_version).await);

    let report = gc.retention.run_once().await.unwrap();

    assert_eq!(report.deleted_blobs, 1);
    assert!(
        !gc.exists(&old_version).await,
        "beyond the last 2 versions and 10 days: collected like any other"
    );
    assert!(
        !policy.allowed(&old_version).await,
        "and the endpoint still refuses the hash (the history names it), so a blob that comes back is not served either"
    );
}

#[tokio::test]
async fn an_audit_events_hashes_do_not_pin_a_blob_only_a_blob_refs_row_does() {
    use domain::{Guid, UserId};
    use ports::{AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, Tx};

    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let named_only_by_the_chain = gc.old("named by an audit event\n").await;
    let pinned_by_a_row = gc.old("pinned by an audit_event blob_refs row\n").await;
    let user = UserId::new(
        Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
        Guid::parse("00000000-0000-4000-8000-000000000001").unwrap(),
    );
    let mut tx = gc.pool().begin().await.unwrap();
    for hash in [named_only_by_the_chain, pinned_by_a_row] {
        let mut event = AuditEvent::new(
            hub_registry::Clock::now(&*gc.rig.clock),
            AuditActor::User { user: user.clone() },
            AuditAction::FileEdited,
            AuditVia::Ui,
        );
        event.swimlane = Some(gc.rig.swimlane.clone());
        event.path = Some(support::rig::path("svc/a.yml"));
        event.hash_before = Some(hash);
        event.hash_after = Some(hash);
        gc.rig
            .audit
            .record(&mut Tx::from_pg(&mut tx), event)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    // The audit event's writer chose to pin the second blob: an explicit row of kind audit_event.
    gc.attach(Attach::BlobRef(1), &pinned_by_a_row, 1).await;
    let chain_before: i64 = gc.rig.count("SELECT count(*) FROM audit_events").await;
    assert_eq!(chain_before, 2);

    let report = gc.retention.run_once().await.unwrap();

    assert!(
        !gc.exists(&named_only_by_the_chain).await,
        "D116: an audit event keeps its own diff text and its hashes; the keep rules decide which versions survive"
    );
    assert!(
        gc.exists(&pinned_by_a_row).await,
        "an explicit blob_refs row of kind audit_event pins"
    );
    assert_eq!(report.deleted_blobs, 1);
    assert_eq!(
        gc.rig.count("SELECT count(*) FROM audit_events").await,
        chain_before,
        "the chain is untouched: the audit events are kept for ever, and a collection is not audited (A12)"
    );
}
