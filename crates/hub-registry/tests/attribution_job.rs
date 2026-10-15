//! The leased job `registry-attribution-grace` (T7, A19, P14, rule 5): bounded by swimlanes and rows, quiet when there is nothing to do (it does
//! not take the swimlane's lock), stops when the lease is lost, one swimlane's failure is counted and does not stop the others, and a run
//! that has done its work finds nothing more. The windows of the job (the look-back, and the age past which a change is no NFS client's).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hub_platform::jobs::{JobContext, JobError};
use hub_registry::metrics::AttributionResult;
use hub_registry::{AttributionGrace, ManualClock, RegistryConfig};
use support::log_capture::LogCapture;
use support::rig::Rig;
use support::sentinel::{Agent, batch, record};
use support::statements::statements;

const T0: i64 = ManualClock::T0_MS;
const S: i64 = 1_000;

/// The swimlane's lock, as an apply takes it, written out so that this test fails if the job were to take another one.
const LOCK: &str = "SELECT pg_advisory_xact_lock(hashtextextended('lanekeeper.registry.apply.' || $1, 0))";

/// A swimlane with a sentinel, one file changed and unattributed, and the record of that change already stored (the usual order).
async fn with_a_record_waiting(rig: &Rig) -> Agent<'_> {
    rig.sentinel_for_the_swimlane().await;
    rig.add_user(1, "ada@example.com", "active").await;
    let mut agent = Agent::start(rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    rig.clock.advance_secs(26);
    agent.change("svc/a.yml", "v: 1", T0 + 25 * S).await;
    agent
}

#[tokio::test]
async fn the_job_runs_through_the_runners_context() {
    let Some(rig) = Rig::new().await else { return };
    let _agent = with_a_record_waiting(&rig).await;
    let spec = AttributionGrace::spec(&RegistryConfig::default()).unwrap();
    assert_eq!(spec.name(), "registry-attribution-grace");
    let (ctx, _expiry) = JobContext::held_until_told_otherwise(spec.name());
    rig.sentinel.grace().job().run(&ctx).await.unwrap();
    assert!(rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"));
}

#[tokio::test]
async fn the_job_stops_when_the_lease_is_lost_before_it_writes() {
    let Some(rig) = Rig::new().await else { return };
    let _agent = with_a_record_waiting(&rig).await;
    // The lease is held for the start of the run, and lost before the first swimlane is looked at.
    let asked = AtomicUsize::new(0);
    let lost = rig
        .sentinel
        .grace()
        .sweep(|| {
            if asked.fetch_add(1, Ordering::SeqCst) < 1 {
                Ok(())
            } else {
                Err(JobError::LeaseLost)
            }
        })
        .await;
    assert_eq!(lost, Err(JobError::LeaseLost));
    assert!(
        rig.attribution("svc/a.yml", 1).await.is("unknown", "low"),
        "nothing was written"
    );
    assert!(rig.upgrade_audits().await.is_empty());
    // A job whose lease expired through the runner's context ends the same way.
    let (ctx, expiry) = JobContext::held_until_told_otherwise("registry-attribution-grace");
    expiry
        .send(tokio::time::Instant::now() - Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        rig.sentinel.grace().job().run(&ctx).await,
        Err(JobError::LeaseLost)
    );
}

#[tokio::test]
async fn the_job_takes_the_swimlanes_lock_only_when_it_has_something_to_attribute() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.add_user(1, "ada@example.com", "active").await;
    let mut agent = Agent::start(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    // Someone holds the swimlane's lock, as an apply does for the length of its chunk.
    let mut holder = rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query(LOCK).bind("sit1").execute(&mut holder).await.unwrap();

    // Nothing to attribute: no change, and then a change that is too young to be an NFS client's and has no record. The job must not
    // queue behind the swimlane's applies for that (it is the usual state, every 5 s, for every swimlane that has a sentinel).
    let grace = rig.sentinel.grace();
    let quiet = tokio::time::timeout(Duration::from_secs(5), grace.run_once()).await;
    assert!(quiet.expect("no waiting for the lock").is_ok());
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    rig.records(batch(1, vec![record("svc/other.yml", T0 + 6 * S)]))
        .await
        .unwrap();
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query(LOCK).bind("sit1").execute(&mut holder).await.unwrap();
    let young = tokio::time::timeout(Duration::from_secs(5), grace.run_once()).await;
    assert_eq!(young.expect("no waiting for the lock").unwrap().nfs_client, 0);
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
}

#[tokio::test]
async fn the_job_waits_for_the_lock_when_a_record_is_waiting_for_its_change() {
    let Some(rig) = Rig::new().await else { return };
    let _agent = with_a_record_waiting(&rig).await;
    let mut holder = rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query(LOCK).bind("sit1").execute(&mut holder).await.unwrap();
    let grace = rig.sentinel.grace();
    let run = tokio::spawn(async move { grace.run_once().await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!run.is_finished(), "it is waiting for the swimlane's lock");
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
    let report = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(report.sentinel_login, 1);
    assert!(rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"));
}

#[tokio::test]
async fn a_run_that_has_done_its_work_finds_nothing_more() {
    let Some(rig) = Rig::new().await else { return };
    let _agent = with_a_record_waiting(&rig).await;
    let grace = rig.sentinel.grace();
    let first = grace.run_once().await.unwrap();
    assert_eq!(
        (first.lanes, first.sentinel_login, first.nfs_client, first.failed),
        (1, 1, 0, 0)
    );
    let audits = rig.upgrade_audits().await.len();
    for _ in 0..3 {
        rig.clock.advance_secs(60);
        let again = grace.run_once().await.unwrap();
        assert_eq!((again.sentinel_login, again.nfs_client), (0, 0));
    }
    assert_eq!(rig.upgrade_audits().await.len(), audits);
    assert_eq!(rig.attribution_updates().await.len(), audits);
}

#[tokio::test]
async fn swimlanes_without_a_sentinel_are_not_looked_at_and_the_run_is_bounded_by_swimlanes() {
    let config = RegistryConfig {
        attribution_lanes: 1,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    // No sentinel anywhere: nothing to look at.
    let report = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!((report.lanes, report.truncated), (0, false));
    // A sentinel without a swimlane is not a swimlane either.
    rig.register_sentinel("nfs-spare", None, "/export").await;
    assert_eq!(rig.sentinel.grace().run_once().await.unwrap().lanes, 0);
    // Two swimlanes with sentinels, a bound of one: it looks at one and says there were more.
    rig.add_swimlane("sit2", &[], true).await;
    rig.register_sentinel("nfs-vm-1", Some("sit1"), "/export").await;
    rig.register_sentinel("nfs-vm-2", Some("sit2"), "/export").await;
    let report = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!((report.lanes, report.truncated), (1, true));
}

#[tokio::test]
async fn one_swimlane_failing_is_counted_and_the_next_run_tries_again() {
    let Some(rig) = Rig::new().await else { return };
    let _agent = with_a_record_waiting(&rig).await;
    rig.audit.fail_with(Some(ports::AuditError::Unavailable));
    let failed = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!(
        (failed.failed, failed.sentinel_login),
        (1, 0),
        "the run does not fail for it"
    );
    assert_eq!(rig.metrics.attribution_count(AttributionResult::LaneFailed), 1);
    assert!(
        rig.attribution("svc/a.yml", 1).await.is("unknown", "low"),
        "rolled back whole"
    );
    rig.audit.fail_with(None);
    let again = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!((again.failed, again.sentinel_login), (0, 1));
}

#[tokio::test]
async fn a_run_reads_at_most_the_batch_and_a_change_nothing_explains_does_not_take_a_slot() {
    let config = RegistryConfig {
        attribution_batch: 2,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.sentinel_for_the_swimlane().await;
    rig.add_user(1, "ada@example.com", "active").await;
    let files: Vec<String> = (0..6).map(|i| format!("svc/f{i}.yml")).collect();
    let initial: Vec<(&str, &str)> = files.iter().map(|f| (f.as_str(), "v: 0")).collect();
    let mut agent = Agent::start(&rig, &initial).await;
    rig.sentinel_hello().await.unwrap();
    // Records for f2..f5 only: f0 and f1 are changes that nothing explains, and they come first by id.
    let records = files[2..].iter().map(|f| record(f, T0 + 10 * S)).collect();
    rig.records(batch(1, records)).await.unwrap();
    rig.clock.advance_secs(20);
    let changes: Vec<(&str, &str)> = files.iter().map(|f| (f.as_str(), "v: 1")).collect();
    agent.change_all(&changes, T0 + 15 * S).await;
    let grace = rig.sentinel.grace();
    let mut per_run = Vec::new();
    for _ in 0..4 {
        per_run.push(grace.run_once().await.unwrap().sentinel_login);
    }
    assert_eq!(
        per_run,
        vec![2, 2, 0, 0],
        "two at a time (the bound), the unexplained ones took no slot, and it ends"
    );
    for f in &files[2..] {
        assert!(rig.attribution(f, 1).await.is("sentinel_login", "high"), "{f}");
    }
}

#[tokio::test]
async fn the_job_looks_back_only_as_far_as_the_lookback_and_calls_nothing_old_an_nfs_client() {
    let config = RegistryConfig {
        sentinel_lookback: Duration::from_secs(600),
        nfs_client_window: Duration::from_secs(300),
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.sentinel_for_the_swimlane().await;
    rig.add_user(1, "ada@example.com", "active").await;
    let mut agent = Agent::start(
        &rig,
        &[
            ("svc/a.yml", "v: 0"),
            ("svc/b.yml", "v: 0"),
            ("svc/c.yml", "v: 0"),
        ],
    )
    .await;
    rig.sentinel_hello().await.unwrap();
    // a has a record and is seen 20 minutes ago; b has none and is seen 20 minutes ago; c has none and is seen 6 minutes ago.
    rig.clock.advance_secs(30);
    agent.change("svc/a.yml", "v: 1", T0 + 20 * S).await;
    agent.change("svc/b.yml", "v: 1", T0 + 20 * S).await;
    rig.clock.advance_secs(1_200 - 30);
    agent.change("svc/c.yml", "v: 1", T0 + 1_200 * S - 360 * S).await;
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 18 * S)]))
        .await
        .ok();
    // (a's record came by the sink and attributed a at once: undo that to put the job to the test.)
    sqlx::query("UPDATE file_observations SET attribution_source = 'unknown', attribution_confidence = 'low', attribution_actor = NULL WHERE path = 'svc/a.yml'")
        .execute(&rig.pool)
        .await
        .unwrap();
    rig.records(batch(2, vec![record("svc/other.yml", T0 + 1_200 * S)]))
        .await
        .unwrap();
    rig.sentinel.grace().run_once().await.unwrap();
    assert!(
        rig.attribution("svc/a.yml", 1).await.is("unknown", "low"),
        "20 minutes is beyond the look-back of 10: the job does not look"
    );
    assert!(
        rig.attribution("svc/b.yml", 1).await.is("unknown", "low"),
        "a change seen beyond the window says nothing about whether the sentinel was up when it was made"
    );
    assert!(
        rig.attribution("svc/c.yml", 1).await.is("unknown", "low"),
        "6 minutes is beyond the window of 5 as well"
    );
}

#[tokio::test]
async fn an_nfs_client_needs_every_sentinel_of_the_swimlane_seen_since_the_change() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.register_sentinel("nfs-vm-2", Some("sit1"), "/export").await;
    let mut agent = Agent::start(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    rig.clock.advance_secs(31);
    // Only nfs-vm-1 is seen: the change might have been made on the other VM, whose sentinel has said nothing since.
    rig.records(batch(1, vec![record("svc/other.yml", T0 + 30 * S)]))
        .await
        .unwrap();
    assert_eq!(rig.sentinel.grace().run_once().await.unwrap().nfs_client, 0);
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));
    // Now nfs-vm-2 is seen as well.
    let second = ports::SentinelIdentity::new(support::sentinel::text("nfs-vm-2"));
    ports::SentinelSink::hello(
        &rig.sentinel,
        &second,
        support::sentinel::hello_of("nfs-vm-2", "/export"),
    )
    .await
    .unwrap();
    assert_eq!(rig.sentinel.grace().run_once().await.unwrap().nfs_client, 1);
    assert!(rig.attribution("svc/a.yml", 1).await.is("nfs_client", "medium"));
}

#[tokio::test]
async fn a_sentinel_that_was_never_seen_makes_no_nfs_client() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    let mut agent = Agent::start(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    rig.clock.advance_secs(120);
    let report = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!(
        report.nfs_client, 0,
        "nobody has heard from the sentinel: nothing is known about what it would have seen"
    );
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));
}

#[tokio::test]
async fn the_statements_of_a_run_take_the_lock_once_and_log_counts_only() {
    let Some(rig) = Rig::new().await else { return };
    let _agent = with_a_record_waiting(&rig).await;
    let (capture, _guard) = LogCapture::install();
    let report = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!(report.sentinel_login, 1);
    let all = statements(&capture);
    assert!(!all.is_empty(), "the statements were captured");
    let locks = all
        .iter()
        .filter(|s| s.text.contains("lanekeeper.registry.apply."))
        .count();
    assert_eq!(locks, 1, "one lock for the swimlane, however many observations");
    let logs = capture.text();
    assert!(logs.contains("attribution grace"));
    assert!(
        !logs.contains("svc/a.yml") && !logs.contains("ada_example_com"),
        "{logs}"
    );
}
