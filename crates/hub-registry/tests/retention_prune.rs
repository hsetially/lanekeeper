//! What the retention job prunes besides blobs (T9, D117): the processed `sync_windows` rows older than `sync_window_retention`, and the
//! stale `delta_staging` parts. Over a real Postgres 16 as `lanekeeper_app`.
//!
//! The part that matters most is `the_hold_budget_answers_the_same_before_and_after_a_prune`: the hold budget of T8 reads `sync_windows`
//! back, so a prune that took a row the budget still reads would change whether a swimlane's alerts are held. The test compares what
//! the budget says (the rows it read, the time held, whether that is the whole budget, and when the episode of exhaustion began) before and
//! after a prune, over random worlds and at several instants after the prune.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::Duration;

use hub_registry::sync::hold_budget_probe;
use hub_registry::{ManualClock, RegistryConfig, RegistryMetrics};
use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use support::retention::Gc;

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;
/// The registry clock of the tests.
const NOW: i64 = ManualClock::T0_MS;

fn lit(ms: i64) -> String {
    format!("to_timestamp({ms}::double precision / 1000.0)")
}

fn opt(ms: Option<i64>) -> String {
    ms.map_or_else(|| "NULL".to_owned(), lit)
}

/// One row of `sync_windows`, every time a Unix instant in milliseconds.
async fn window(gc: &Gc, lane: &str, uid: &str, opened: i64, closed: Option<i64>, processed: Option<i64>) {
    gc.admin(&format!(
        "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at, closed_at, processed_at) \
         VALUES ('{lane}', 'sync-{lane}', '{uid}', {}, {}, {})",
        lit(opened),
        opt(closed),
        opt(processed)
    ))
    .await;
}

async fn window_uids(gc: &Gc) -> Vec<String> {
    sqlx::query_scalar("SELECT job_uid FROM sync_windows ORDER BY job_uid")
        .fetch_all(gc.pool())
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------------------------------------------
// The prune of `sync_windows`.

#[tokio::test]
async fn only_processed_windows_older_than_the_retention_in_every_time_are_pruned() {
    let Some(gc) = Gc::with(RegistryConfig::default()).await else {
        return;
    };
    gc.rig.add_swimlane("sit2", &["sit2"], false).await;
    let retention = 7 * DAY;
    let cutoff = NOW - retention;
    // Every time older than the cutoff, and processed: these go.
    window(
        &gc,
        "sit1",
        "old",
        NOW - 10 * DAY,
        Some(NOW - 10 * DAY + 5 * MIN),
        Some(NOW - 10 * DAY + 10 * MIN),
    )
    .await;
    window(
        &gc,
        "sit2",
        "old-in-another-lane",
        NOW - 30 * DAY,
        Some(NOW - 30 * DAY + MIN),
        Some(NOW - 30 * DAY + 2 * MIN),
    )
    .await;
    window(
        &gc,
        "sit1",
        "one-minute-beyond",
        cutoff - 2 * HOUR,
        Some(cutoff - HOUR),
        Some(cutoff - MIN),
    )
    .await;
    // One time inside the retention keeps the row.
    window(
        &gc,
        "sit1",
        "processed-one-minute-inside",
        cutoff - 2 * HOUR,
        Some(cutoff - HOUR),
        Some(cutoff + MIN),
    )
    .await;
    window(
        &gc,
        "sit1",
        "closed-one-minute-inside",
        cutoff - 2 * HOUR,
        Some(cutoff + MIN),
        Some(cutoff + 2 * MIN),
    )
    .await;
    window(
        &gc,
        "sit1",
        "opened-one-minute-inside",
        cutoff + MIN,
        Some(cutoff + 2 * MIN),
        Some(cutoff + 3 * MIN),
    )
    .await;
    window(
        &gc,
        "sit1",
        "recent",
        NOW - 2 * HOUR,
        Some(NOW - HOUR),
        Some(NOW - 50 * MIN),
    )
    .await;
    window(
        &gc,
        "sit1",
        "old-open-recent-close",
        NOW - 10 * DAY,
        Some(NOW - DAY),
        Some(NOW - DAY),
    )
    .await;
    // Not processed: the watch job owns the row (it is retried, given up or expired by it), whatever its age.
    window(
        &gc,
        "sit1",
        "closed-never-processed",
        NOW - 20 * DAY,
        Some(NOW - 20 * DAY + HOUR),
        None,
    )
    .await;
    window(&gc, "sit2", "never-closed", NOW - 20 * DAY, None, None).await;

    let report = gc.retention.run_once().await.unwrap();

    assert_eq!(report.windows, 3);
    assert_eq!(
        window_uids(&gc).await,
        vec![
            "closed-never-processed",
            "closed-one-minute-inside",
            "never-closed",
            "old-open-recent-close",
            "opened-one-minute-inside",
            "processed-one-minute-inside",
            "recent",
        ]
    );

    // A second run finds nothing: not a row, not a tuple version.
    let before = gc.dump().await;
    let again = gc.retention.run_once().await.unwrap();
    assert_eq!(again.windows, 0);
    assert_eq!(gc.dump().await, before);
}

#[tokio::test]
async fn a_run_prunes_at_most_batch_times_batches_windows_and_the_rest_waits() {
    let config = RegistryConfig {
        retention_prune_batch: 2,
        retention_prune_batches: 2,
        ..RegistryConfig::default()
    };
    let Some(gc) = Gc::with(config).await else {
        return;
    };
    for i in 0..7 {
        let at = NOW - 20 * DAY - i64::from(i) * HOUR;
        window(
            &gc,
            "sit1",
            &format!("w{i}"),
            at,
            Some(at + MIN),
            Some(at + 2 * MIN),
        )
        .await;
    }
    let mut taken = Vec::new();
    for _ in 0..3 {
        taken.push(gc.retention.run_once().await.unwrap().windows);
    }
    assert_eq!(
        taken,
        vec![4, 3, 0],
        "two batches of two, then the rest, then nothing"
    );
    assert!(window_uids(&gc).await.is_empty());
}

/// A world of windows: the ones the hold budget reads (recent) and the ones that only a prune could touch (old), by their times.
#[derive(Debug, Clone)]
struct W {
    /// Unix ms.
    opened: i64,
    closed: Option<i64>,
    processed: Option<i64>,
}

fn recent_window() -> impl Strategy<Value = W> {
    // Opened up to 56 hours ago, held for up to 6 hours, processed up to 90 minutes after the close, all clamped to the clock.
    (
        0_i64..56 * 60,
        0_i64..6 * 60,
        prop::option::weighted(0.85, 0_i64..90),
        any::<bool>(),
    )
        .prop_map(|(age_min, hold_min, processing_min, closes)| {
            let opened = NOW - age_min * MIN;
            let closed = closes.then(|| (opened + hold_min * MIN).min(NOW));
            let processed = closed
                .zip(processing_min)
                .map(|(closed, p)| (closed + p * MIN).min(NOW));
            W {
                opened,
                closed,
                processed,
            }
        })
}

fn old_window() -> impl Strategy<Value = W> {
    // Beyond the retention in every time: opened at least 7 days and 9 hours ago, closed within 6 hours of that and processed within 2 hours
    // more, so that even the newest time of the window is more than an hour beyond the cutoff.
    (
        7 * 24 * 60 + 9 * 60..40 * 24 * 60_i64,
        0_i64..6 * 60,
        0_i64..120,
        0_u8..4,
    )
        .prop_map(|(age_min, hold_min, processing_min, kind)| {
            let opened = NOW - age_min * MIN;
            let closed = opened + hold_min * MIN;
            let processed = closed + processing_min * MIN;
            match kind {
                0 => W {
                    opened,
                    closed: None,
                    processed: None,
                },
                1 => W {
                    opened,
                    closed: Some(closed),
                    processed: None,
                },
                _ => W {
                    opened,
                    closed: Some(closed),
                    processed: Some(processed),
                },
            }
        })
}

#[derive(Debug, Clone)]
struct World {
    recent: Vec<W>,
    old: Vec<W>,
}

fn world() -> impl Strategy<Value = World> {
    (
        proptest::collection::vec(recent_window(), 0..=8),
        proptest::collection::vec(old_window(), 0..=6),
    )
        .prop_map(|(recent, old)| World { recent, old })
}

async fn build(gc: &Gc, world: &World) {
    gc.clear().await;
    for (i, w) in world.recent.iter().enumerate() {
        window(
            gc,
            "sit1",
            &format!("recent-{i}"),
            w.opened,
            w.closed,
            w.processed,
        )
        .await;
    }
    for (i, w) in world.old.iter().enumerate() {
        window(gc, "sit1", &format!("old-{i}"), w.opened, w.closed, w.processed).await;
    }
}

/// What the budget says at each of several instants: at the prune and after it (the lookback of the budget moves with `now`, and a prune
/// that took a row the budget reads at a later instant would show there).
async fn probes(gc: &Gc, lane: &str) -> Vec<hub_registry::sync::BudgetProbe> {
    let swimlane = domain::SwimlaneId::parse(lane).unwrap();
    let mut out = Vec::new();
    for later in [0, 10 * MIN, HOUR, 5 * HOUR, 26 * HOUR, 3 * DAY, 6 * DAY] {
        out.push(
            hold_budget_probe(gc.pool().clone(), &gc.config, &swimlane, NOW + later)
                .await
                .unwrap(),
        );
    }
    out
}

/// The rule, written again from the brief: processed, and every time it has older than the retention.
fn prunable(w: &W) -> bool {
    w.processed.is_some()
        && [Some(w.opened), w.closed, w.processed]
            .into_iter()
            .flatten()
            .all(|t| t < NOW - 7 * DAY)
}

/// Returns what the case covered: the time the budget counted before the prune, and the rows the prune took.
async fn case(gc: &Gc, world: World) -> Result<(i64, usize), TestCaseError> {
    build(gc, &world).await;
    let before = probes(gc, "sit1").await;
    let report = gc.retention.run_once().await.expect("the run");
    let after = probes(gc, "sit1").await;
    let expected = world.old.iter().filter(|w| prunable(w)).count();
    prop_assert_eq!(
        report.windows,
        expected,
        "the old processed windows went: {:?}",
        world
    );
    prop_assert_eq!(
        &before,
        &after,
        "the hold budget answers the same before and after the prune: {:?}",
        world
    );
    Ok((before[0].spent_ms, report.windows))
}

#[test]
fn the_hold_budget_answers_the_same_before_and_after_a_prune() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let Some(gc) = rt.block_on(Gc::with(RegistryConfig::default())) else {
        return;
    };
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
    let with_time_held = std::cell::Cell::new(0_usize);
    let with_rows_pruned = std::cell::Cell::new(0_usize);
    let result = runner.run(&world(), |w| {
        let (spent, pruned) = rt.block_on(case(&gc, w))?;
        with_time_held.set(with_time_held.get() + usize::from(spent > 0));
        with_rows_pruned.set(with_rows_pruned.get() + usize::from(pruned > 0));
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
    // The property is not vacuous: the worlds it ran had windows that held time and windows that a prune took.
    assert!(
        with_time_held.get() > 0 && with_rows_pruned.get() > 0,
        "{} {}",
        with_time_held.get(),
        with_rows_pruned.get()
    );
}

/// The property reaches an exhausted budget only by chance; this one is built to: a chain of windows that spends the 4 hours of the
/// rolling day, an open window on top of it, and a heap of old processed windows. The prune takes the old ones and not one of the chain, and
/// the budget (spent, and since when) is the same to the millisecond.
#[tokio::test]
async fn a_spent_budget_is_spent_the_same_way_after_the_prune() {
    let Some(gc) = Gc::with(RegistryConfig::default()).await else {
        return;
    };
    // Four windows that each hold for 90 minutes (open for an hour, then closed and unprocessed for 40 minutes, of which the give-up of 30
    // counts): 6 hours in all within the rolling day, well past the budget of 4.
    for i in 0..4_i64 {
        let opened = NOW - (23 - 5 * i) * HOUR;
        window(
            &gc,
            "sit1",
            &format!("chain-{i}"),
            opened,
            Some(opened + HOUR),
            Some(opened + HOUR + 40 * MIN),
        )
        .await;
    }
    window(&gc, "sit1", "open-now", NOW - 30 * MIN, None, None).await;
    for i in 0..20_i64 {
        let at = NOW - (8 + i) * DAY;
        window(
            &gc,
            "sit1",
            &format!("old-{i}"),
            at,
            Some(at + HOUR),
            Some(at + HOUR + MIN),
        )
        .await;
    }
    let before = probes(&gc, "sit1").await;
    assert!(
        before[0].exhausted && before[0].exhaustion_start_ms.is_some(),
        "the world is built so that the budget is spent: {:?}",
        before[0]
    );
    assert!(
        before.iter().any(|p| !p.exhausted),
        "and it recovers within the instants looked at, so the comparison covers both"
    );

    let report = gc.retention.run_once().await.unwrap();

    assert_eq!(report.windows, 20, "the heap of old windows went");
    assert_eq!(
        window_uids(&gc).await.len(),
        5,
        "and none of the chain, nor the open window"
    );
    assert_eq!(before, probes(&gc, "sit1").await);
}

#[tokio::test]
async fn a_dry_run_counts_the_windows_and_leaves_them() {
    let config = RegistryConfig {
        retention_dry_run: true,
        ..RegistryConfig::default()
    };
    let Some(gc) = Gc::with(config).await else {
        return;
    };
    for i in 0..3_i64 {
        let at = NOW - (9 + i) * DAY;
        window(
            &gc,
            "sit1",
            &format!("w{i}"),
            at,
            Some(at + MIN),
            Some(at + 2 * MIN),
        )
        .await;
    }
    let before = gc.dump().await;
    let report = gc.retention.run_once().await.unwrap();
    assert_eq!(report.windows, 3, "it says what it would prune");
    assert_eq!(gc.dump().await, before, "and prunes nothing");
}

// ---------------------------------------------------------------------------------------------------------------
// The sweep of `delta_staging`.

const STAGING_TTL: i64 = 24 * HOUR;

/// One staged part, stamped `staged_at`. With `blob`, its metadata lists that blob among the entries it staged.
async fn stage(gc: &Gc, seq: usize, staged_at: i64, blob: Option<&domain::ContentHash>) {
    let entries = blob.map_or_else(String::new, |h| {
        format!("{{\"hash\":\"{}\"}}", hex::encode(h.as_bytes()))
    });
    gc.admin(&format!(
        "INSERT INTO delta_staging (swimlane_id, seq, part, base_root, new_root, metadata, staged_at) \
         VALUES ('sit1', {seq}, 0, NULL, '\\x{root}', '{{\"v\":1,\"entries\":[{entries}],\"removed\":[],\"skipped\":[],\"bytes\":0}}', {})",
        lit(staged_at),
        root = hex::encode(ports::content_hash(b"root").as_bytes()),
    ))
    .await;
}

async fn staged(gc: &Gc) -> Vec<i64> {
    sqlx::query_scalar("SELECT seq FROM delta_staging ORDER BY seq")
        .fetch_all(gc.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn stale_staged_parts_are_swept_in_batches_and_the_ones_within_the_ttl_stay() {
    let config = RegistryConfig {
        retention_prune_batch: 3,
        retention_prune_batches: 2,
        ..RegistryConfig::default()
    };
    let Some(gc) = Gc::with(config).await else {
        return;
    };
    // Ten stale parts (seq 1 to 10), the last of them one minute beyond the lifetime; two within it (11, 12), one a minute inside.
    for i in 1..=9_usize {
        stage(&gc, i, NOW - STAGING_TTL - (i as i64) * HOUR, None).await;
    }
    stage(&gc, 10, NOW - STAGING_TTL - MIN, None).await;
    stage(&gc, 11, NOW - STAGING_TTL + MIN, None).await;
    stage(&gc, 12, NOW - HOUR, None).await;

    let mut taken = Vec::new();
    for _ in 0..4 {
        taken.push(gc.retention.run_once().await.unwrap().staging);
    }

    assert_eq!(
        taken,
        vec![6, 4, 0, 0],
        "two batches of three, then the other four (three and one), then nothing"
    );
    assert_eq!(staged(&gc).await, vec![11, 12], "the parts within the ttl stay");
}

#[tokio::test]
async fn the_sweep_on_hello_is_bounded_the_same_way_and_leaves_the_rest() {
    let config = RegistryConfig {
        retention_prune_batch: 3,
        retention_prune_batches: 2,
        ..RegistryConfig::default()
    };
    let Some(gc) = Gc::with(config).await else {
        return;
    };
    for i in 1..=10_usize {
        stage(&gc, i, NOW - STAGING_TTL - (i as i64) * HOUR, None).await;
    }
    assert_eq!(gc.rig.ingest.sweep_staging().await.unwrap(), 6);
    assert_eq!(gc.rig.ingest.sweep_staging().await.unwrap(), 4);
    assert_eq!(gc.rig.ingest.sweep_staging().await.unwrap(), 0);
}

#[tokio::test]
async fn a_stale_part_stops_pinning_its_blob_in_the_run_that_sweeps_it_and_a_fresh_one_does_not() {
    let Some(gc) = Gc::with(RegistryConfig::default()).await else {
        return;
    };
    let from_stale = gc.old("staged long ago, never applied\n").await;
    let from_fresh = gc.old("staged an hour ago\n").await;
    stage(&gc, 1, NOW - STAGING_TTL - HOUR, Some(&from_stale)).await;
    stage(&gc, 2, NOW - HOUR, Some(&from_fresh)).await;

    let report = gc.retention.run_once().await.unwrap();

    assert_eq!((report.staging, report.deleted_blobs), (1, 1));
    assert!(
        !gc.exists(&from_stale).await,
        "the part was dropped first, so its blob was no longer named"
    );
    assert!(
        gc.exists(&from_fresh).await,
        "a part within its lifetime still names its blob"
    );
    assert_eq!(staged(&gc).await, vec![2]);
}

#[tokio::test]
async fn a_dry_run_counts_the_stale_parts_and_leaves_them() {
    let config = RegistryConfig {
        retention_dry_run: true,
        ..RegistryConfig::default()
    };
    let Some(gc) = Gc::with(config).await else {
        return;
    };
    for i in 1..=4_usize {
        stage(&gc, i, NOW - STAGING_TTL - (i as i64) * HOUR, None).await;
    }
    let before = gc.dump().await;
    let report = gc.retention.run_once().await.unwrap();
    assert_eq!(report.staging, 4, "it says what it would sweep");
    assert_eq!(gc.dump().await, before, "and sweeps nothing");
}

// ---------------------------------------------------------------------------------------------------------------
// Configuration.

#[tokio::test]
async fn a_retention_of_windows_that_the_budget_still_reads_is_refused_at_construction() {
    let Some(gc) = Gc::with(RegistryConfig::default()).await else {
        return;
    };
    let floor = RegistryConfig::default().sync_window_retention_floor();
    let too_short = RegistryConfig {
        sync_window_retention: floor,
        ..RegistryConfig::default()
    };
    assert!(
        retention_with(&gc, too_short).is_err(),
        "a retention equal to what the budget reads back is refused"
    );
    let enough = RegistryConfig {
        sync_window_retention: floor + Duration::from_secs(1),
        ..RegistryConfig::default()
    };
    assert!(retention_with(&gc, enough).is_ok());
}

fn retention_with(
    gc: &Gc,
    config: RegistryConfig,
) -> Result<hub_registry::Retention, hub_registry::config::ConfigError> {
    hub_registry::Retention::new(hub_registry::RetentionDeps {
        pool: gc.pool().clone(),
        ingest: gc.rig.ingest.clone(),
        clock: std::sync::Arc::clone(&gc.rig.clock) as _,
        config,
        metrics: RegistryMetrics::default(),
    })
}
