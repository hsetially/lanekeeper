//! Sync windows (T8, D75): what the hub does while a sync Job runs and when its window closes.
//!
//! The windows come through the real ingestion (`ClusterReport.sync_windows`), the drift through the real pipeline, the closing
//! processing through the watch job's `watch` (the pieces of `record_sync`, once for the swimlane), over a real Postgres 16, the
//! in-memory Git, the real audit log and outbox, an agent that answers a request for a scan with a full scan through ingestion, and a
//! notifier that remembers what it was asked to send.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use domain::{
    ClusterReport, ContentHash, DomainEvent, HubCommand, JobRef, RepoKind, Severity, SyncWindowEvent,
    SyncWindowKind, Timestamp,
};
use engine::drift_hash;
use hub_registry::metrics::{AlertResult, WindowResult};
use hub_registry::{Alerts, AlertsDeps, ManualClock, RegistryConfig, RegistryMetrics, WatchReport};
use ports::NotificationKind;
use support::log_capture::LogCapture;
use support::pg_audit::audit_rows;
use support::rig::T;
use support::statements::{inserts, statements};
use support::sync::SyncRig;
use support::world::{RunningJob, state_of, wait_until};

const NOW: i64 = ManualClock::T0_MS;
/// The sync Job started a moment after the first inventory.
const OPENED: i64 = T + 100;
/// And its window closed a second before the closing scan (which is observed at the hub's clock).
const CLOSED: i64 = NOW - 1_000;

fn drift(text: &str) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

/// The configuration of these tests: the scan is looked for every 10 ms, the pipeline gathers for 20 ms, and the state has to be quiet
/// for 300 ms before a window's alerts are evaluated (longer than the pipeline's gathering and recompute).
fn config() -> RegistryConfig {
    RegistryConfig {
        sync_poll_interval: Duration::from_millis(10),
        coalesce_window: Duration::from_millis(20),
        sync_settle: Duration::from_millis(300),
        sync_settle_max: Duration::from_secs(10),
        ..RegistryConfig::default()
    }
}

fn job(uid: &str) -> JobRef {
    JobRef::new("sync-sit1", uid).unwrap()
}

fn report(events: Vec<SyncWindowEvent>) -> ClusterReport {
    ClusterReport {
        full: false,
        deployments: Vec::new(),
        release_hints: Vec::new(),
        config_server_started_at: None,
        sync_windows: events,
    }
}

fn event(kind: SyncWindowKind, uid: &str, at: i64) -> SyncWindowEvent {
    SyncWindowEvent {
        kind,
        job: job(uid),
        at: Timestamp::from_unix_millis(at),
    }
}

async fn open(s: &SyncRig, uid: &str, at: i64) {
    s.w.rig
        .cluster(report(vec![event(SyncWindowKind::Opened, uid, at)]))
        .await
        .unwrap();
}

async fn close(s: &SyncRig, uid: &str, at: i64) {
    s.w.rig
        .cluster(report(vec![event(SyncWindowKind::Closed, uid, at)]))
        .await
        .unwrap();
}

/// One run of the watch job, with the lease taken for granted.
async fn tick(s: &SyncRig) -> WatchReport {
    s.service.watch(|| Ok(())).await.unwrap()
}

fn scans(s: &SyncRig) -> usize {
    s.commands()
        .iter()
        .filter(|c| matches!(c, HubCommand::RequestFullScan))
        .count()
}

/// `(job uid, closed, processed, warned)` of every window row.
async fn windows(s: &SyncRig) -> Vec<(String, bool, bool, bool)> {
    sqlx::query_as(
        "SELECT job_uid, closed_at IS NOT NULL, processed_at IS NOT NULL, warned_at IS NOT NULL \
         FROM sync_windows ORDER BY id",
    )
    .fetch_all(&s.w.rig.pool)
    .await
    .unwrap()
}

async fn events_of(s: &SyncRig, pick: fn(&DomainEvent) -> bool) -> usize {
    s.w.rig.bus.published().await.iter().filter(|e| pick(e)).count()
}

// ---------------------------------------------------------------------------------------------------------------- scenario: 600 files

const FILES: usize = 600;
/// The files the sync rewrites to Git's new content.
const SYNCED: usize = 500;

fn name(i: usize) -> String {
    format!("svc/f{i:03}.yml")
}

/// An adopted swimlane `sit1` of 600 files in sync with Git, and Git then moves on for 500 of them (a push). The pipeline has seen it.
async fn lane_of_600(alerting: bool) -> Option<SyncRig> {
    let mut s = if alerting {
        SyncRig::alerting(&["sit1"], config()).await?
    } else {
        SyncRig::with_config(&["sit1"], config()).await?
    };
    let names: Vec<String> = (0..FILES).map(name).collect();
    let v1: Vec<(String, Option<&str>)> = names
        .iter()
        .map(|n| (format!("config/{n}"), Some("v: 1\n")))
        .collect();
    let v1: Vec<(&str, Option<&str>)> = v1.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    s.w.base_commit(&v1);
    s.w.tenant_commit("sit1", &[("data/config/app/x.yml", Some("x: 1\n"))]);
    let mut nfs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "v: 1\n")).collect();
    nfs.push(("app/x-sit1.yml", "x: 1\n"));
    s.w.inventory(&nfs).await;
    s.w.adopt().await;
    for n in &names {
        s.w.baseline(n, drift("v: 1\n")).await;
    }
    s.w.baseline("app/x-sit1.yml", drift("x: 1\n")).await;
    s.w.whole().await;
    // The push: Git has the new content of the first 500.
    let pushed: Vec<String> = names[..SYNCED].iter().map(|n| format!("config/{n}")).collect();
    let pushed: Vec<(&str, Option<&str>)> = pushed.iter().map(|p| (p.as_str(), Some("v: 2\n"))).collect();
    s.w.base_commit(&pushed);
    s.w.whole().await;
    assert_eq!(
        state_of(&s.w, &name(0)).await.as_deref(),
        Some("git_ahead"),
        "the push: waiting for its sync"
    );
    Some(s)
}

/// What a sync does while its window is open: the first 500 files get Git's content; and, meanwhile, someone edits 100 others by
/// hand. The pipeline sees all of it.
async fn the_sync_runs(s: &mut SyncRig) {
    open(s, "uid-1", OPENED).await;
    let synced: Vec<String> = (0..SYNCED).map(name).collect();
    let edited: Vec<String> = (SYNCED..FILES).map(name).collect();
    let synced: Vec<(&str, &str)> = synced.iter().map(|n| (n.as_str(), "v: 2\n")).collect();
    let edited: Vec<(&str, &str)> = edited.iter().map(|n| (n.as_str(), "v: 99\n")).collect();
    s.w.nfs_change(&synced).await;
    s.w.nfs_change(&edited).await;
    s.w.whole().await;
}

/// What the agent will scan when the hub asks: the files as they are.
fn nfs_now() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = (0..FILES)
        .map(|i| (name(i), if i < SYNCED { "v: 2\n" } else { "v: 99\n" }.to_owned()))
        .collect();
    files.push(("app/x-sit1.yml".to_owned(), "x: 1\n".to_owned()));
    files
}

fn scan_of(files: &[(String, String)]) -> Vec<(&str, &str)> {
    files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect()
}

async fn sever(s: &SyncRig, kind: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT DISTINCT severity FROM findings WHERE swimlane_id = 'sit1' AND kind = $1 AND resolved_at IS NULL",
    )
    .bind(kind)
    .fetch_all(&s.w.rig.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_600_file_sync_produces_one_alert_evaluation() {
    let Some(mut s) = lane_of_600(true).await else {
        return;
    };
    the_sync_runs(&mut s).await;

    // While the window is open the drift is recorded and the alerts are held.
    assert_eq!(
        state_of(&s.w, &name(0)).await.as_deref(),
        Some("in_sync"),
        "the sync's files converged"
    );
    assert_eq!(
        state_of(&s.w, &name(SYNCED)).await.as_deref(),
        Some("nfs_ahead"),
        "an edit during the sync is drift"
    );
    assert_eq!(
        s.w.tables()
            .await
            .findings
            .iter()
            .filter(|f| f.0 == "drift")
            .count(),
        FILES - SYNCED,
        "the 100 edits are recorded as findings"
    );
    assert!(
        s.notifier.sent().is_empty(),
        "held: nobody is told while the Job runs: {:?}",
        s.notifier.sent()
    );
    assert!(s.w.rig.metrics.alert_count(AlertResult::Held) >= 1);
    assert_eq!(s.w.rig.metrics.alert_evaluation_count(), 0);

    // The window closes. The hub asks for ONE scan and evaluates ONCE.
    close(&s, "uid-1", CLOSED).await;
    s.agent_scans(&scan_of(&nfs_now()));
    let ran = tick(&s).await;
    assert_eq!(
        ran,
        WatchReport {
            expired: 0,
            warned: 0,
            swimlanes: 1,
            processed: 1,
            given_up: 0,
            deferred: 0
        }
    );
    assert_eq!(scans(&s), 1, "one scan for the swimlane");
    assert_eq!(
        s.w.rig.metrics.alert_evaluation_count(),
        1,
        "one evaluation for the window"
    );

    let sent = s.notifier.sent();
    assert_eq!(
        sent.len(),
        1,
        "one notification for the swimlane and the one severity there is: {sent:?}"
    );
    assert_eq!(sent[0].kind, NotificationKind::DriftDetected);
    assert_eq!(sent[0].swimlane, Some(support::sync::lane()));
    assert_eq!(
        vec![sent[0].severity.to_string()],
        sever(&s, "drift").await,
        "the severity of the drift findings (D76)"
    );
    assert!(
        sent[0].summary.as_str().contains("100 file(s)"),
        "the 100 that still differ, not the 500 that the sync put right: {:?}",
        sent[0].summary
    );
    let text = format!("{} {}", sent[0].title, sent[0].summary);
    assert!(
        !text.contains("svc/") && !text.contains("v: 99") && !text.contains(&drift("v: 99\n").to_string()),
        "ids and counts only (S10): {text}"
    );

    // The window is done: it holds nothing, and a second run does nothing.
    assert_eq!(windows(&s).await, vec![("uid-1".to_owned(), true, true, false)]);
    let again = tick(&s).await;
    assert_eq!(again, WatchReport::default());
    assert_eq!(s.notifier.sent().len(), 1);
    assert_eq!(scans(&s), 1);
    assert_eq!(s.w.rig.metrics.window_count(WindowResult::Processed), 1);

    // The deployment is recorded at the heads of the mirror, by the window.
    let row: (String, String, String, String, serde_json::Value) = sqlx::query_as(
        "SELECT tenant_id, tenant_commit, base_commit, source, actor FROM swimlane_deployments WHERE swimlane_id = 'sit1'",
    )
    .fetch_one(&s.w.rig.pool)
    .await
    .unwrap();
    assert_eq!(row.0, "sit1");
    assert_eq!(row.1, s.w.head(RepoKind::Tenant, "sit1").as_str());
    assert_eq!(row.2, s.w.head(RepoKind::Base, "master").as_str());
    assert_eq!(row.3, "window");
    assert_eq!(row.4["kind"], "job");

    // Alerts flow again: a new edit is told at once (the window holds nothing now).
    s.w.nfs_change(&[(&name(0), "v: 7\n")]).await;
    s.w.paths(&[&name(0)]).await;
    wait_until("the alert of the new edit", Duration::from_secs(10), || async {
        s.notifier.sent().len() == 2
    })
    .await;
}

#[tokio::test]
async fn the_same_drift_outside_a_window_is_alerted_at_once() {
    // The control of the test above: without a window the pipeline's commit sends the alert, so the hold is what held it.
    let Some(mut s) = lane_of_600(true).await else {
        return;
    };
    let edited: Vec<String> = (SYNCED..FILES).map(name).collect();
    let edited: Vec<(&str, &str)> = edited.iter().map(|n| (n.as_str(), "v: 99\n")).collect();
    s.w.nfs_change(&edited).await;
    s.w.whole().await;
    wait_until("the live alert", Duration::from_secs(10), || async {
        !s.notifier.sent().is_empty()
    })
    .await;
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 1, "one notification for the one severity: {sent:?}");
    assert_eq!(sent[0].kind, NotificationKind::DriftDetected);
    assert!(
        sent[0].summary.as_str().contains("100 file(s)"),
        "{:?}",
        sent[0].summary
    );
    assert!(
        !sent[0].summary.as_str().contains("held"),
        "the live text does not talk about a window: {:?}",
        sent[0].summary
    );
    assert_eq!(s.w.rig.metrics.alert_count(AlertResult::Held), 0);
    assert_eq!(s.w.rig.metrics.alert_count(AlertResult::Sent), 1);
}

// ---------------------------------------------------------------------------------------------------------------- C7

const CORE: &str = "app/core.yml";
const CORE_SIT1: &str = "app/core-sit1.yml";
const OTHER: &str = "app/other.yml";
const GIT: [(&str, &str); 3] = [(CORE, "a: 1\n"), (CORE_SIT1, "t: 1\n"), (OTHER, "o: 1\n")];

/// An adopted swimlane `sit1` in sync with Git, then two files edited on NFS out of band that the pipeline has seen: ahead of Git.
async fn ahead_on_nfs(alerting: bool, config: RegistryConfig) -> Option<SyncRig> {
    let mut s = if alerting {
        SyncRig::alerting(&["sit1"], config).await?
    } else {
        SyncRig::with_config(&["sit1"], config).await?
    };
    s.w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/other.yml", Some("o: 1\n")),
    ]);
    s.w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t: 1\n"))]);
    s.w.inventory(&GIT).await;
    s.w.adopt().await;
    for (path, content) in GIT {
        s.w.baseline(path, drift(content)).await;
    }
    s.w.whole().await;
    s.w.nfs_change(&[(CORE, "a: 99\n"), (CORE_SIT1, "t: 99\n")]).await;
    s.w.paths(&[CORE, CORE_SIT1]).await;
    assert_eq!(state_of(&s.w, CORE).await.as_deref(), Some("nfs_ahead"));
    assert_eq!(state_of(&s.w, CORE_SIT1).await.as_deref(), Some("nfs_ahead"));
    Some(s)
}

#[tokio::test]
async fn c7_fires_for_an_overwritten_nfs_ahead_file() {
    // The sync's version reaches the hub only with the closing scan: nobody tagged it, and the report of the window is what says so.
    let Some(s) = ahead_on_nfs(true, config()).await else {
        return;
    };
    // The out-of-band edits were alerted when the pipeline saw them (no window was open): drift, told at once.
    wait_until("the alert of the edits", Duration::from_secs(10), || async {
        s.notifier.sent().len() == 1
    })
    .await;
    assert_eq!(s.notifier.sent()[0].kind, NotificationKind::DriftDetected);
    let pipeline = RunningJob::start(&s.w.pipeline);

    open(&s, "uid-1", OPENED).await;
    close(&s, "uid-1", CLOSED).await;
    s.agent_scans(&GIT);
    let ran = tick(&s).await;
    assert_eq!(ran.processed, 1);

    // C7 first: the sync job is the author of each replaced version, and the context says what it replaced.
    for (path, edited) in [(CORE, "a: 99\n"), (CORE_SIT1, "t: 99\n")] {
        let obs = s.w.rig.observations(path).await;
        let sync = obs.last().unwrap();
        assert_eq!(sync.attribution_source, "sync_job", "{path}");
        assert_eq!(sync.attribution_actor.as_ref().unwrap()["kind"], "job");
        assert_eq!(sync.previous_state.as_deref(), Some("nfs_ahead"), "{path}");
        assert_eq!(sync.previous_hash, Some(drift(edited)), "{path}");
    }
    let audit = audit_rows(&s.w.rig.pool).await;
    assert_eq!(
        audit
            .iter()
            .filter(|a| a.action == "attribution_upgraded")
            .count(),
        2,
        "one audit event for each upgrade"
    );

    // Then ONE evaluation: one notification for the C7, a critical finding, naming the count and no file.
    let sent = s.notifier.sent();
    assert_eq!(
        sent.len(),
        2,
        "the live alert of the edits and the window's: {sent:?}"
    );
    let c7 = &sent[1];
    assert_eq!(c7.kind, NotificationKind::CriticalFinding);
    assert_eq!(c7.severity, Severity::Critical, "C7 is critical by default (D76)");
    assert_eq!(c7.swimlane, Some(support::sync::lane()));
    assert!(c7.summary.as_str().contains("2 file(s)"), "{:?}", c7.summary);
    assert!(
        !c7.summary.as_str().contains("more file(s)"),
        "the replaced files are C7 and not drift as well: {:?}",
        c7.summary
    );
    let text = format!("{} {}", c7.title, c7.summary);
    assert!(
        !text.contains("core") && !text.contains("a: 99") && !text.contains(&drift("a: 99\n").to_string()),
        "ids and counts only (S10): {text}"
    );

    // The pipeline computes C7 from the context.
    wait_until("the C7 findings", Duration::from_secs(10), || async {
        s.w.tables()
            .await
            .findings
            .iter()
            .filter(|f| f.0 == "overwritten_nfs_change")
            .count()
            == 2
    })
    .await;
    let findings = s.w.tables().await.findings;
    assert!(
        !findings.iter().any(|f| f.0 == "drift"),
        "everything is in sync again"
    );
    assert_eq!(s.w.rig.metrics.sync_overwritten_count(), 2);
    drop(pipeline);
}

#[tokio::test]
async fn a_c7_written_while_the_window_was_open_is_alerted_when_it_closes() {
    // The sync's version arrives during the window and is tagged with the Job when it does (ingestion writes the context): by the
    // time the window closes the pipeline has recomputed the file, nothing is ahead any more, and the report has nothing to claim.
    // The C7 finding the pipeline wrote is what the evaluation finds.
    let Some(mut s) = ahead_on_nfs(false, config()).await else {
        return;
    };
    open(&s, "uid-1", OPENED).await;
    s.w.nfs_change(&[(CORE, "a: 1\n"), (CORE_SIT1, "t: 1\n")]).await;
    s.w.paths(&[CORE, CORE_SIT1]).await;
    assert_eq!(state_of(&s.w, CORE).await.as_deref(), Some("in_sync"));
    assert_eq!(
        s.w.tables()
            .await
            .findings
            .iter()
            .filter(|f| f.0 == "overwritten_nfs_change")
            .count(),
        2
    );
    assert!(s.notifier.sent().is_empty());

    close(&s, "uid-1", CLOSED).await;
    s.agent_scans(&GIT);
    let ran = tick(&s).await;
    assert_eq!(ran.processed, 1);
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].kind, NotificationKind::CriticalFinding);
    assert_eq!(sent[0].severity, Severity::Critical);
    assert!(
        sent[0].summary.as_str().contains("2 file(s)"),
        "{:?}",
        sent[0].summary
    );
    assert_eq!(
        s.w.rig.metrics.sync_overwritten_count(),
        0,
        "the report claimed nothing: the context was there already"
    );
}

// ---------------------------------------------------------------------------------------------------------------- stale windows

#[tokio::test]
async fn a_window_open_over_2_hours_warns_once() {
    let Some(s) = SyncRig::new(&["sit1"]).await else {
        return;
    };
    s.w.rig.hello().await;
    let capture = LogCapture::install_everywhere();
    let two_hours = 2 * 3600 * 1000;
    // One that is a minute past the limit, one exactly at it (the moment its hold ends, so the warning is the same moment), and one a
    // minute short of it.
    open(&s, "stale", NOW - two_hours - 60_000).await;
    open(&s, "exact", NOW - two_hours).await;
    open(&s, "young", NOW - two_hours + 60_000).await;
    let ran = tick(&s).await;
    assert_eq!(ran.warned, 2);
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent.iter().all(|n| n.kind == NotificationKind::SyncWindowStale));
    assert!(sent.iter().all(|n| n.swimlane == Some(support::sync::lane())));
    assert!(
        sent[0].summary.as_str().contains("120 minutes"),
        "{:?}",
        sent[0].summary
    );
    assert_eq!(s.w.rig.metrics.window_stale_warning_count(), 2, "the metric");
    assert!(
        capture.text().contains("open for too long"),
        "and a log line: {}",
        capture.text()
    );
    assert_eq!(
        windows(&s).await,
        vec![
            ("stale".to_owned(), false, false, true),
            ("exact".to_owned(), false, false, true),
            ("young".to_owned(), false, false, false)
        ]
    );

    // Again, and again: once.
    for _ in 0..3 {
        assert_eq!(tick(&s).await.warned, 0);
    }
    assert_eq!(s.notifier.sent().len(), 2);
    assert_eq!(s.w.rig.metrics.window_stale_warning_count(), 2);

    // Time passes: the young one becomes stale and is warned about, once, and the others are not warned about again.
    s.w.rig.clock.advance_secs(120);
    assert_eq!(tick(&s).await.warned, 1);
    assert_eq!(s.notifier.sent().len(), 3);
    assert_eq!(s.w.rig.metrics.window_stale_warning_count(), 3);

    // A window that closed is not stale, however long it was open.
    open(&s, "closed-late", NOW - two_hours - 600_000).await;
    close(&s, "closed-late", NOW - 1_000).await;
    assert_eq!(tick(&s).await.warned, 0);
}

#[tokio::test]
async fn a_failing_or_hanging_notifier_does_not_stop_the_warning_or_the_close() {
    for hang in [false, true] {
        let cfg = RegistryConfig {
            notify_timeout: Duration::from_millis(100),
            ..config()
        };
        let Some(s) = ahead_on_nfs(false, cfg.clone()).await else {
            return;
        };
        let service = s.service_with(Arc::new(BadNotifier { hang }), cfg);
        open(&s, "stale", NOW - 3 * 3600 * 1000).await;
        open(&s, "uid-1", OPENED).await;
        close(&s, "uid-1", CLOSED).await;
        s.agent_scans(&GIT);
        let ran = tokio::time::timeout(Duration::from_secs(20), service.watch(|| Ok(())))
            .await
            .expect("a notifier that never answers does not hold the run up")
            .unwrap();
        assert_eq!(ran.warned, 1, "hang={hang}: the warning was claimed and logged");
        assert_eq!(
            ran.processed, 1,
            "hang={hang}: the window was processed all the same"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM swimlane_deployments")
                .fetch_one(&s.w.rig.pool)
                .await
                .unwrap(),
            1,
            "hang={hang}: the deployment is recorded"
        );
        let obs = s.w.rig.observations(CORE).await;
        assert_eq!(
            obs.last().unwrap().previous_state.as_deref(),
            Some("nfs_ahead"),
            "hang={hang}: C7's context is written whatever the notifier does"
        );
        let metrics = &s.w.rig.metrics;
        let failed = if hang {
            metrics.alert_count(AlertResult::TimedOut)
        } else {
            metrics.alert_count(AlertResult::Failed)
        };
        assert!(
            failed >= 2,
            "hang={hang}: the stale warning and the C7 alert were counted: {failed}"
        );
        assert_eq!(metrics.alert_count(AlertResult::Sent), 0);
    }
}

/// A notifier that answers with an error, or never answers.
struct BadNotifier {
    hang: bool,
}

#[async_trait::async_trait]
impl ports::Notifier for BadNotifier {
    async fn notify(&self, _: ports::Notification) -> Result<(), ports::NotifyError> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        Err(ports::NotifyError::Unavailable)
    }
}

// ---------------------------------------------------------------------------------------------------------------- idempotency

#[tokio::test]
async fn duplicate_window_events_are_idempotent() {
    let Some(s) = ahead_on_nfs(false, config()).await else {
        return;
    };
    s.agent_scans(&GIT);
    let opened = report(vec![event(SyncWindowKind::Opened, "uid-1", OPENED)]);
    let closed = report(vec![event(SyncWindowKind::Closed, "uid-1", CLOSED)]);
    // Twice each, and the opening again after the close (a replayed report).
    for r in [&opened, &opened, &closed, &closed, &opened] {
        s.w.rig.cluster(r.clone()).await.unwrap();
    }
    assert_eq!(
        windows(&s).await,
        vec![("uid-1".to_owned(), true, false, false)],
        "one row"
    );
    assert_eq!(
        events_of(&s, |e| matches!(e, DomainEvent::SyncWindowOpened { .. })).await,
        1
    );
    assert_eq!(
        events_of(&s, |e| matches!(e, DomainEvent::SyncWindowClosed { .. })).await,
        1
    );

    // Processed once, however often the job runs.
    assert_eq!(tick(&s).await.processed, 1);
    assert_eq!(tick(&s).await, WatchReport::default());
    assert_eq!(tick(&s).await, WatchReport::default());
    assert_eq!(scans(&s), 1);
    assert_eq!(
        s.notifier.sent().len(),
        1,
        "one C7 notification: {:?}",
        s.notifier.sent()
    );
    let audit = audit_rows(&s.w.rig.pool).await;
    let count = |action: &str| audit.iter().filter(|a| a.action == action).count();
    assert_eq!(count("settings_changed"), 1, "one deployment record");
    assert_eq!(count("attribution_upgraded"), 2);
    assert_eq!(s.w.rig.metrics.window_count(WindowResult::Processed), 1);

    // The reports that arrive after the processing change nothing and start nothing.
    for r in [&opened, &closed] {
        s.w.rig.cluster(r.clone()).await.unwrap();
    }
    assert_eq!(windows(&s).await, vec![("uid-1".to_owned(), true, true, false)]);
    assert_eq!(tick(&s).await, WatchReport::default());
    assert_eq!(
        events_of(&s, |e| matches!(e, DomainEvent::SyncWindowClosed { .. })).await,
        1
    );
    assert_eq!(scans(&s), 1);

    // A window the hub never saw open, reported closed, is processed too (once).
    close(&s, "uid-2", CLOSED + 500).await;
    close(&s, "uid-2", CLOSED + 500).await;
    assert_eq!(tick(&s).await.processed, 1);
    assert_eq!(tick(&s).await, WatchReport::default());
    assert_eq!(scans(&s), 2);
}

#[tokio::test]
async fn a_window_time_far_ahead_of_the_hub_is_the_hubs_time() {
    let Some(s) = SyncRig::new(&["sit1"]).await else {
        return;
    };
    s.w.rig.hello().await;
    // Within the tolerated skew it is kept; beyond it the hub's clock stands (a hostile clock cannot keep a window young for ever).
    open(&s, "near", NOW + 60_000).await;
    open(&s, "far", NOW + 10 * 24 * 3600 * 1000).await;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT job_uid, (EXTRACT(EPOCH FROM opened_at) * 1000)::bigint FROM sync_windows ORDER BY id",
    )
    .fetch_all(&s.w.rig.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![("near".to_owned(), NOW + 60_000), ("far".to_owned(), NOW)]
    );
    // And so it ages out.
    s.w.rig.clock.advance_secs(2 * 3600 + 61);
    assert_eq!(tick(&s).await.warned, 2);
}

// ---------------------------------------------------------------------------------------------------------------- tenants and heads

#[tokio::test]
async fn several_tenants_are_one_scan_and_each_is_recorded_at_its_own_head() {
    let Some(mut s) = SyncRig::new(&["sit1", "sit2"]).await else {
        return;
    };
    s.w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    s.w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t1: 1\n"))]);
    s.w.tenant_commit("sit2", &[("data/config/app/core.yml", Some("t2: 1\n"))]);
    let files = [
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t1: 1\n"),
        ("app/core-sit2.yml", "t2: 1\n"),
    ];
    s.w.inventory(&files).await;
    s.w.adopt().await;
    for (p, c) in files {
        s.w.baseline(p, drift(c)).await;
    }
    s.w.whole().await;
    // The Git of both tenants moves on after the first inventory: the heads are what the sync deployed (A33).
    s.w.base_commit(&[("config/app/core.yml", Some("a: 2\n"))]);
    s.w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t1: 2\n"))]);
    s.w.tenant_commit("sit2", &[("data/config/app/core.yml", Some("t2: 2\n"))]);
    // And an edit of the second tenant's file that is on NFS only.
    s.w.nfs_change(&[("app/core-sit2.yml", "t2: 99\n")]).await;
    s.w.whole().await;
    assert_eq!(
        state_of(&s.w, "app/core-sit2.yml").await.as_deref(),
        Some("conflict")
    );

    open(&s, "uid-1", OPENED).await;
    close(&s, "uid-1", CLOSED).await;
    // The sync put everyone's new content on NFS.
    s.agent_scans(&[
        ("app/core.yml", "a: 2\n"),
        ("app/core-sit1.yml", "t1: 2\n"),
        ("app/core-sit2.yml", "t2: 2\n"),
    ]);
    let ran = tick(&s).await;
    assert_eq!(ran.processed, 1);
    assert_eq!(scans(&s), 1, "one scan for the swimlane, not one for each tenant");

    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT tenant_id, tenant_commit, base_commit, source FROM swimlane_deployments WHERE swimlane_id = 'sit1' ORDER BY tenant_id",
    )
    .fetch_all(&s.w.rig.pool)
    .await
    .unwrap();
    let base = s.w.head(RepoKind::Base, "master").as_str().to_owned();
    assert_eq!(
        rows,
        vec![
            (
                "sit1".to_owned(),
                s.w.head(RepoKind::Tenant, "sit1").as_str().to_owned(),
                base.clone(),
                "window".to_owned()
            ),
            (
                "sit2".to_owned(),
                s.w.head(RepoKind::Tenant, "sit2").as_str().to_owned(),
                base,
                "window".to_owned()
            )
        ],
        "each tenant at the head of its own branch, the base at the head of its default branch"
    );
    let audit = audit_rows(&s.w.rig.pool).await;
    assert_eq!(
        audit.iter().filter(|a| a.action == "settings_changed").count(),
        2,
        "one deployment record for each tenant"
    );

    // C7 for the second tenant's file: the tenant file is claimed by its own tenant, and once.
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].kind, NotificationKind::CriticalFinding);
    assert!(
        sent[0].summary.as_str().contains("1 file(s)"),
        "{:?}",
        sent[0].summary
    );
    assert_eq!(s.w.rig.metrics.sync_overwritten_count(), 1);
}

#[tokio::test]
async fn windows_that_close_together_are_one_scan_and_one_evaluation() {
    let Some(s) = ahead_on_nfs(false, config()).await else {
        return;
    };
    s.agent_scans(&GIT);
    open(&s, "uid-1", OPENED).await;
    open(&s, "uid-2", OPENED + 10).await;
    close(&s, "uid-1", CLOSED - 500).await;
    // A Job is still running in the swimlane: the closed window waits for it.
    let waiting = tick(&s).await;
    assert_eq!(waiting.processed, 0);
    assert_eq!(waiting.deferred, 1);
    assert_eq!(scans(&s), 0);
    assert_eq!(s.notifier.sent().len(), 0);
    close(&s, "uid-2", CLOSED).await;
    let ran = tick(&s).await;
    assert_eq!(ran.processed, 2, "both windows, as one");
    assert_eq!(scans(&s), 1);
    assert_eq!(s.notifier.sent().len(), 1);
    assert_eq!(s.w.rig.metrics.alert_evaluation_count(), 1);
    assert_eq!(
        windows(&s).await,
        vec![
            ("uid-1".to_owned(), true, true, false),
            ("uid-2".to_owned(), true, true, false)
        ]
    );
}

// ---------------------------------------------------------------------------------------------------------------- not now

#[tokio::test]
async fn a_swimlane_nobody_has_adopted_only_gets_its_deployments_recorded() {
    let Some(mut s) = SyncRig::new(&["sit1"]).await else {
        return;
    };
    s.w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    s.w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t: 1\n"))]);
    s.w.inventory(&GIT).await;
    s.w.whole().await;
    open(&s, "uid-1", OPENED).await;
    close(&s, "uid-1", CLOSED).await;
    let ran = tick(&s).await;
    assert_eq!(ran.processed, 1);
    assert_eq!(scans(&s), 0, "no baseline to check a scan against (D22)");
    assert_eq!(s.notifier.sent().len(), 0);
    assert_eq!(s.w.rig.metrics.alert_evaluation_count(), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM swimlane_deployments")
            .fetch_one(&s.w.rig.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(s.w.rig.metrics.window_count(WindowResult::DeploymentOnly), 1);
    assert_eq!(windows(&s).await, vec![("uid-1".to_owned(), true, true, false)]);
}

#[tokio::test]
async fn an_agent_that_is_not_there_leaves_the_window_for_later_and_a_window_given_up_is_still_evaluated() {
    let Some(s) = ahead_on_nfs(false, config()).await else {
        return;
    };
    open(&s, "uid-1", OPENED).await;
    close(&s, "uid-1", CLOSED).await;
    // No agent is attached: the scan cannot be asked for.
    let first = tick(&s).await;
    assert_eq!((first.processed, first.deferred), (0, 1));
    assert_eq!(windows(&s).await, vec![("uid-1".to_owned(), true, false, false)]);
    assert_eq!(
        s.notifier.sent().len(),
        0,
        "nothing is evaluated on a state the sync has not been checked against"
    );
    assert_eq!(s.w.rig.metrics.window_count(WindowResult::Deferred), 1);
    assert_eq!(tick(&s).await.deferred, 1, "and again on the next run");

    // The give-up: the drift the window held is not lost because the agent never came back.
    s.w.rig.clock.advance_secs(31 * 60);
    let last = tick(&s).await;
    assert_eq!((last.given_up, last.deferred), (1, 0));
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].kind, NotificationKind::DriftDetected);
    assert!(
        sent[0].summary.as_str().contains("2 file(s)"),
        "{:?}",
        sent[0].summary
    );
    assert_eq!(windows(&s).await, vec![("uid-1".to_owned(), true, true, false)]);
    assert_eq!(s.w.rig.metrics.window_count(WindowResult::GivenUp), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM swimlane_deployments")
            .fetch_one(&s.w.rig.pool)
            .await
            .unwrap(),
        0,
        "no sync was recorded: there was no scan"
    );
}

#[tokio::test]
async fn a_closed_window_nobody_processed_for_a_day_is_marked_done_and_says_nothing() {
    // The hub was down, or the row is older than the job: a sync that old is neither worth a scan nor an alert, and a backlog of them
    // must not turn into a storm on the first run.
    let Some(s) = ahead_on_nfs(true, config()).await else {
        return;
    };
    wait_until("the live alert of the edits", Duration::from_secs(10), || async {
        s.notifier.sent().len() == 1
    })
    .await;
    let day = 24 * 3600 * 1000;
    open(&s, "old", NOW - 2 * day - 600_000).await;
    close(&s, "old", NOW - 2 * day).await;
    open(&s, "just-over", NOW - day - 600_000).await;
    close(&s, "just-over", NOW - day - 1).await;
    let ran = tick(&s).await;
    assert_eq!((ran.expired, ran.processed, ran.deferred), (2, 0, 0));
    assert_eq!(scans(&s), 0, "no scan for a sync that old");
    assert_eq!(
        s.notifier.sent().len(),
        1,
        "and nothing is said: only the live alert of the edits"
    );
    assert_eq!(
        windows(&s).await,
        vec![
            ("old".to_owned(), true, true, false),
            ("just-over".to_owned(), true, true, false)
        ]
    );
    assert_eq!(s.w.rig.metrics.window_count(WindowResult::Expired), 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM swimlane_deployments")
            .fetch_one(&s.w.rig.pool)
            .await
            .unwrap(),
        0
    );
    // A window that closed a moment ago is not old: it is processed.
    open(&s, "new", OPENED).await;
    close(&s, "new", CLOSED).await;
    s.agent_scans(&GIT);
    let next = tick(&s).await;
    assert_eq!((next.expired, next.processed), (0, 1));
}

#[tokio::test]
async fn a_window_open_past_the_stale_limit_does_not_re_alert_what_it_did_not_hold() {
    // The Job ran for three hours. For the first two the alerts were held; then the window was stale and they flowed again; when the
    // Job finally ended the hold came back for the processing. The evaluation tells what was held and not what was told in between.
    let Some(mut s) = SyncRig::alerting(&["sit1"], config()).await else {
        return;
    };
    s.w.base_commit(&[
        ("config/app/a.yml", Some("a: 1\n")),
        ("config/app/b.yml", Some("b: 1\n")),
    ]);
    s.w.tenant_commit("sit1", &[("data/config/app/x.yml", Some("x: 1\n"))]);
    s.w.inventory(&[("app/a.yml", "a: 1\n"), ("app/b.yml", "b: 1\n")])
        .await;
    s.w.adopt().await;
    s.w.baseline("app/a.yml", drift("a: 1\n")).await;
    s.w.baseline("app/b.yml", drift("b: 1\n")).await;
    s.w.whole().await;

    open(&s, "uid-1", NOW - 3 * 3600 * 1000).await;
    // An edit now, three hours into the window: the hold ended an hour ago, so it is alerted at once.
    s.w.nfs_change(&[("app/a.yml", "a: 99\n")]).await;
    s.w.paths(&["app/a.yml"]).await;
    wait_until("the live alert", Duration::from_secs(10), || async {
        s.notifier.sent().len() == 1
    })
    .await;
    assert_eq!(s.w.rig.metrics.alert_count(AlertResult::Held), 0);

    // The Job ends. Another edit while the hub processes the close: held.
    s.w.rig.clock.advance_secs(600);
    close(&s, "uid-1", NOW + 599_000).await;
    s.w.nfs_change(&[("app/b.yml", "b: 99\n")]).await;
    s.w.paths(&["app/b.yml"]).await;
    assert_eq!(s.w.rig.metrics.alert_count(AlertResult::Held), 1);
    assert_eq!(s.notifier.sent().len(), 1);

    s.agent_scans(&[("app/a.yml", "a: 99\n"), ("app/b.yml", "b: 99\n")]);
    let ran = tick(&s).await;
    assert_eq!(ran.processed, 1);
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[1].kind, NotificationKind::DriftDetected);
    assert!(
        sent[1].summary.as_str().contains("1 file(s)"),
        "the file held during the processing, and not the one that was alerted while the window was stale: {:?}",
        sent[1].summary
    );
}

// ---------------------------------------------------------------------------------------------------------------- the hold

#[tokio::test]
async fn what_holds_the_alerts_of_a_swimlane() {
    // (what the window row says, whether it holds). The hub's clock is NOW; the stale limit is 2 h, the give-up 30 minutes.
    struct Case {
        what: &'static str,
        opened: i64,
        closed: Option<i64>,
        processed: bool,
        holds: bool,
    }
    let cases = [
        Case {
            what: "open and young",
            opened: NOW - 60_000,
            closed: None,
            processed: false,
            holds: true,
        },
        Case {
            what: "open for just under two hours",
            opened: NOW - 2 * 3600 * 1000 + 1_000,
            closed: None,
            processed: false,
            holds: true,
        },
        Case {
            what: "open for two hours",
            opened: NOW - 2 * 3600 * 1000,
            closed: None,
            processed: false,
            holds: false,
        },
        Case {
            what: "open for much longer",
            opened: NOW - 5 * 3600 * 1000,
            closed: None,
            processed: false,
            holds: false,
        },
        Case {
            what: "closed a minute ago, not processed",
            opened: NOW - 3 * 3600 * 1000,
            closed: Some(NOW - 60_000),
            processed: false,
            holds: true,
        },
        Case {
            what: "closed 31 minutes ago, not processed",
            opened: NOW - 3 * 3600 * 1000,
            closed: Some(NOW - 31 * 60 * 1000),
            processed: false,
            holds: false,
        },
        Case {
            what: "closed a minute ago and processed",
            opened: NOW - 600_000,
            closed: Some(NOW - 60_000),
            processed: true,
            holds: false,
        },
    ];
    // One swimlane for all of them: each case has a file of its own, and its own window row (the previous one is removed).
    let Some(mut s) = SyncRig::alerting(&["sit1"], config()).await else {
        return;
    };
    let files: Vec<String> = (0..cases.len()).map(|i| format!("app/c{i}.yml")).collect();
    let base: Vec<(String, Option<&str>)> = files
        .iter()
        .map(|f| (format!("config/{f}"), Some("a: 1\n")))
        .collect();
    let base: Vec<(&str, Option<&str>)> = base.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    s.w.base_commit(&base);
    let nfs: Vec<(&str, &str)> = files.iter().map(|f| (f.as_str(), "a: 1\n")).collect();
    s.w.inventory(&nfs).await;
    s.w.adopt().await;
    for f in &files {
        s.w.baseline(f, drift("a: 1\n")).await;
    }
    s.w.whole().await;
    let (mut held_so_far, mut sent_so_far) = (0, 0);
    for (case, file) in cases.iter().zip(&files) {
        sqlx::query("DELETE FROM sync_windows")
            .execute(&s.w.rig.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at, closed_at, processed_at) \
             VALUES ('sit1', 'sync-sit1', 'uid-1', timestamptz 'epoch' + ($1::bigint::text || ' milliseconds')::interval, \
                     CASE WHEN $2::bigint IS NULL THEN NULL ELSE timestamptz 'epoch' + ($2::bigint::text || ' milliseconds')::interval END, \
                     CASE WHEN $3 THEN now() END)",
        )
        .bind(case.opened)
        .bind(case.closed)
        .bind(case.processed)
        .execute(&s.w.rig.pool)
        .await
        .unwrap();
        s.w.nfs_change(&[(file.as_str(), "a: 99\n")]).await;
        s.w.paths(&[file.as_str()]).await;
        assert_eq!(
            state_of(&s.w, file).await.as_deref(),
            Some("nfs_ahead"),
            "{}",
            case.what
        );
        if case.holds {
            held_so_far += 1;
            assert_eq!(
                s.w.rig.metrics.alert_count(AlertResult::Held),
                held_so_far,
                "{}: held",
                case.what
            );
            assert_eq!(
                s.notifier.sent().len(),
                sent_so_far,
                "{}: nothing was sent",
                case.what
            );
        } else {
            sent_so_far += 1;
            wait_until(case.what, Duration::from_secs(10), || async {
                s.notifier.sent().len() == sent_so_far
            })
            .await;
            assert_eq!(
                s.w.rig.metrics.alert_count(AlertResult::Held),
                held_so_far,
                "{}: not held",
                case.what
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- batches, S10

#[tokio::test]
async fn the_close_writes_in_batches_and_not_one_event_at_a_time() {
    // 120 files that were ahead on NFS and that the sync replaces with the closing scan: C7 for all of them. The pipeline is not
    // running, so the statements counted are the close's (the scan through ingestion, C7, the baselines, the deployment).
    let Some(mut s) = SyncRig::with_config(&["sit1"], config()).await else {
        return;
    };
    let names: Vec<String> = (0..120).map(|i| format!("svc/f{i:03}.yml")).collect();
    let git: Vec<(String, Option<&str>)> = names
        .iter()
        .map(|n| (format!("config/{n}"), Some("v: 1\n")))
        .collect();
    let git: Vec<(&str, Option<&str>)> = git.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    s.w.base_commit(&git);
    s.w.tenant_commit("sit1", &[("data/config/app/x.yml", Some("x: 1\n"))]);
    let nfs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "v: 1\n")).collect();
    s.w.inventory(&nfs).await;
    s.w.adopt().await;
    for n in &names {
        s.w.baseline(n, drift("v: 1\n")).await;
    }
    s.w.whole().await;
    let edited: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "v: 99\n")).collect();
    s.w.nfs_change(&edited).await;
    s.w.whole().await;
    assert_eq!(state_of(&s.w, &names[0]).await.as_deref(), Some("nfs_ahead"));

    open(&s, "uid-1", OPENED).await;
    close(&s, "uid-1", CLOSED).await;
    s.agent_scans(&nfs);
    let (single_before, batch_before) = (s.w.rig.audit.single_calls(), s.w.rig.audit.batch_calls());
    let (capture, _guard) = LogCapture::install();
    let ran = tick(&s).await;
    let all = statements(&capture);
    assert!(
        !all.is_empty(),
        "no statement was captured: the counts below would pass for nothing"
    );
    assert_eq!(ran.processed, 1);
    assert_eq!(
        s.w.rig.metrics.sync_overwritten_count(),
        120,
        "C7 for every one of them"
    );

    let mut audit = inserts(&all, "INSERT INTO audit_events");
    audit.sort_unstable();
    assert_eq!(
        audit,
        vec![1, 20, 20, 50, 50, 50, 50],
        "the scan's out-of-band changes and C7's attribution upgrades are 50, 50 and 20 each, the deployment is one"
    );
    let mut outbox = inserts(&all, "INSERT INTO outbox");
    outbox.sort_unstable();
    assert_eq!(
        outbox,
        vec![20, 20, 50, 50, 50, 50],
        "FileObserved and AttributionUpdated, in chunks of 50"
    );
    assert_eq!(inserts(&all, "INSERT INTO swimlane_deployments"), vec![1]);
    // The pipeline has not recomputed the 120 files, so each is still `nfs_ahead` with its drift finding; they were replaced, so they are
    // C7 and are not counted again as drift.
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].kind, NotificationKind::CriticalFinding);
    assert!(
        sent[0].summary.as_str().contains("120 file(s)"),
        "{:?}",
        sent[0].summary
    );
    assert!(
        !sent[0].summary.as_str().contains("more file(s)"),
        "{:?}",
        sent[0].summary
    );
    assert_eq!(
        s.w.rig.audit.single_calls() - single_before,
        1,
        "`record` is called for the one deployment record and for nothing else"
    );
    assert_eq!(
        s.w.rig.audit.batch_calls() - batch_before,
        6,
        "`record_many` once for each chunk of the scan and of C7"
    );
    assert_eq!(
        audit_rows(&s.w.rig.pool)
            .await
            .iter()
            .filter(|a| a.action == "attribution_upgraded")
            .count(),
        120,
        "and every upgrade has its own audit event (rule 8)"
    );
}

#[tokio::test]
async fn no_content_path_or_hash_reaches_a_log_line_or_a_notification() {
    let secret = "PLANTED-SECRET-VALUE-4711";
    let Some(mut s) = ahead_on_nfs(true, config()).await else {
        return;
    };
    s.w.nfs_change(&[(OTHER, &format!("password: {secret}\n"))]).await;
    s.w.paths(&[OTHER]).await;
    let capture = LogCapture::install_everywhere();
    open(&s, "uid-1", OPENED).await;
    close(&s, "uid-1", CLOSED).await;
    open(&s, "stale", NOW - 3 * 3600 * 1000).await;
    s.agent_scans(&[
        (CORE, "a: 1\n"),
        (CORE_SIT1, "t: 1\n"),
        (OTHER, &format!("password: {secret}\n")),
    ]);
    let ran = tick(&s).await;
    assert_eq!(ran.processed, 1);
    assert_eq!(ran.warned, 1);
    let log = capture.text();
    assert!(
        !log.is_empty(),
        "the log must have been captured for this to mean anything"
    );
    for forbidden in [
        secret,
        "app/core",
        "app/other",
        "a: 99",
        &drift("a: 99\n").to_string(),
        &drift(&format!("password: {secret}\n")).to_string(),
    ] {
        assert!(!log.contains(forbidden), "{forbidden} reached a log line:\n{log}");
    }
    for n in s.notifier.sent() {
        let text = format!("{} {} {:?}", n.title, n.summary, n.link);
        for forbidden in [secret, "app/core", "app/other", "a: 99", "sha256"] {
            assert!(!text.contains(forbidden), "{forbidden} in a notification: {text}");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the hold budget

const HOUR: i64 = 3_600_000;
const MIN: i64 = 60_000;

/// An adopted swimlane `sit1` with the files `app/f1.yml` to `app/f8.yml` in sync with Git, so that editing one of them on NFS is one
/// drift alert.
async fn eight_files(cfg: RegistryConfig) -> Option<SyncRig> {
    let mut s = SyncRig::alerting(&["sit1"], cfg).await?;
    let names: Vec<String> = (1..=8).map(|i| format!("app/f{i}.yml")).collect();
    let git: Vec<(String, Option<&str>)> = names
        .iter()
        .map(|n| (format!("config/{n}"), Some("v: 1\n")))
        .collect();
    let git: Vec<(&str, Option<&str>)> = git.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    s.w.base_commit(&git);
    let nfs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "v: 1\n")).collect();
    s.w.inventory(&nfs).await;
    s.w.adopt().await;
    for n in &names {
        s.w.baseline(n, drift("v: 1\n")).await;
    }
    s.w.whole().await;
    Some(s)
}

/// Edit file `i` on NFS (one drift alert for the pipeline to offer) and let the pipeline see it.
async fn edit(s: &mut SyncRig, i: usize) {
    let file = format!("app/f{i}.yml");
    s.w.nfs_change(&[(file.as_str(), "v: 99\n")]).await;
    s.w.paths(&[file.as_str()]).await;
}

fn at(s: &SyncRig, ms_after_now: i64) {
    s.w.rig.clock.set_ms(NOW + ms_after_now);
}

/// The notices that the budget of a swimlane is spent, among what the notifier was given.
fn budget_notices(s: &SyncRig) -> usize {
    s.notifier
        .sent()
        .iter()
        .filter(|n| {
            n.kind == NotificationKind::SyncWindowStale && n.title.as_str().contains("no longer held")
        })
        .count()
}

fn stale_notices(s: &SyncRig) -> usize {
    s.notifier
        .sent()
        .iter()
        .filter(|n| {
            n.kind == NotificationKind::SyncWindowStale && !n.title.as_str().contains("no longer held")
        })
        .count()
}

fn drift_alerts(s: &SyncRig) -> usize {
    s.notifier
        .sent()
        .iter()
        .filter(|n| n.kind == NotificationKind::DriftDetected)
        .count()
}

#[tokio::test]
async fn a_chain_of_windows_stops_holding_once_the_budget_is_spent_and_the_notice_is_said_once() {
    let Some(mut s) = eight_files(config()).await else {
        return;
    };
    let held = |s: &SyncRig| s.w.rig.metrics.alert_count(AlertResult::Held);

    // Two Jobs, one after the other, each left open until it is stale: the hold is [0, 2 h] and [2 h 01, 4 h 01].
    open(&s, "uid-1", NOW).await;
    at(&s, HOUR);
    edit(&mut s, 1).await;
    assert_eq!(held(&s), 1, "an hour into the first window: held");
    at(&s, 2 * HOUR + MIN);
    open(&s, "uid-2", NOW + 2 * HOUR + MIN).await;
    edit(&mut s, 2).await;
    assert_eq!(held(&s), 2, "the first window is stale and the second holds");
    at(&s, 3 * HOUR);
    edit(&mut s, 3).await;
    assert_eq!(held(&s), 3);
    at(&s, 4 * HOUR);
    edit(&mut s, 4).await;
    assert_eq!(held(&s), 4, "3 h 59 min have been held: the budget is not spent");
    assert_eq!(drift_alerts(&s), 0, "nothing has been told: all of it was held");

    // A third window with a new Job uid opens; the budget (4 h in 24 h) is spent, so the hold has ended and the alert flows.
    at(&s, 4 * HOUR + 2 * MIN);
    open(&s, "uid-3", NOW + 4 * HOUR + 2 * MIN).await;
    edit(&mut s, 5).await;
    wait_until(
        "the live alert that the budget lets through",
        Duration::from_secs(10),
        || async { drift_alerts(&s) == 1 },
    )
    .await;
    assert_eq!(held(&s), 4, "not held although a window is open");
    assert_eq!(s.w.rig.metrics.alert_count(AlertResult::Sent), 1);

    // The watch job says so, once: a notice that the budget is spent, and the stale warnings of the two windows that are stale.
    let ran = tick(&s).await;
    assert_eq!(ran.warned, 2, "uid-1 and uid-2 are stale; uid-3 is not");
    assert_eq!(budget_notices(&s), 1);
    assert_eq!(stale_notices(&s), 2);
    assert_eq!(s.w.rig.metrics.hold_budget_exhaustion_count(), 1);
    let notice = s
        .notifier
        .sent()
        .into_iter()
        .find(|n| n.title.as_str().contains("no longer held"))
        .unwrap();
    assert_eq!(notice.severity, Severity::Medium);
    assert_eq!(notice.swimlane, Some(support::sync::lane()));
    assert!(
        notice.summary.as_str().contains("of the last 1440 minutes"),
        "{:?}",
        notice.summary
    );
    assert!(
        notice.summary.as_str().contains("budget of 240"),
        "{:?}",
        notice.summary
    );
    let text = format!("{} {}", notice.title, notice.summary);
    assert!(
        !text.contains("app/") && !text.contains("uid-"),
        "ids and counts only (S10): {text}"
    );

    // Not again: not on the next runs, and not for the next window of the same episode.
    for _ in 0..3 {
        tick(&s).await;
    }
    at(&s, 4 * HOUR + 3 * MIN);
    open(&s, "uid-4", NOW + 4 * HOUR + 3 * MIN).await;
    at(&s, 4 * HOUR + 13 * MIN);
    edit(&mut s, 6).await;
    wait_until("the second live alert", Duration::from_secs(10), || async {
        drift_alerts(&s) == 2
    })
    .await;
    tick(&s).await;
    assert_eq!(
        budget_notices(&s),
        1,
        "one for the episode, not one for each window"
    );
    assert_eq!(held(&s), 4);

    // A replica that has just started (no memory, the same database) sees the same budget and says nothing again.
    let other = Alerts::new(AlertsDeps {
        pool: s.w.rig.pool.clone(),
        notifier: Arc::new(s.notifier.clone()),
        clock: Arc::clone(&s.w.rig.clock) as _,
        config: config(),
        metrics: RegistryMetrics::default(),
    })
    .unwrap();
    other.offer(&support::sync::lane(), &[Severity::Medium]).await;
    wait_until(
        "the alert of the other replica",
        Duration::from_secs(10),
        || async { drift_alerts(&s) == 3 },
    )
    .await;
    let service = s.service_with(Arc::new(s.notifier.clone()), config());
    service.watch(|| Ok(())).await.unwrap();
    assert_eq!(
        budget_notices(&s),
        1,
        "the marker is in the database, not in a process"
    );

    // The notice did not take the stale warning of the newest window away: when it goes stale it is warned about, once.
    at(&s, 6 * HOUR + 2 * MIN + 30_000);
    let later = tick(&s).await;
    assert_eq!(later.warned, 1, "uid-3 went stale (uid-4 has a minute left)");
    assert_eq!(stale_notices(&s), 3);
    assert_eq!(budget_notices(&s), 1);
}

#[tokio::test]
async fn the_budget_refills_as_the_rolling_window_moves_and_a_new_episode_is_said_again() {
    // One hour in three: small enough to walk through with the clock.
    let cfg = RegistryConfig {
        sync_hold_budget: Duration::from_secs(3600),
        sync_hold_budget_window: Duration::from_secs(3 * 3600),
        ..config()
    };
    let Some(mut s) = eight_files(cfg).await else {
        return;
    };
    let held = |s: &SyncRig| s.w.rig.metrics.alert_count(AlertResult::Held);

    open(&s, "uid-1", NOW).await;
    at(&s, 30 * MIN);
    edit(&mut s, 1).await;
    assert_eq!(held(&s), 1, "half the budget is spent");
    at(&s, 61 * MIN);
    edit(&mut s, 2).await;
    wait_until(
        "the alert that the spent budget lets through",
        Duration::from_secs(10),
        || async { drift_alerts(&s) == 1 },
    )
    .await;
    assert_eq!(held(&s), 1);
    tick(&s).await;
    assert_eq!(budget_notices(&s), 1, "the first episode");

    // The Job ends at 90 minutes. The window still holds until it is processed or given up (30 min): the hold was [0, 2 h].
    at(&s, 90 * MIN);
    close(&s, "uid-1", NOW + 90 * MIN).await;
    // Ten minutes later the closed window is still in its holding phase, but 100 minutes of it were held: the budget is spent whether
    // the window is open or closed, and the alert flows.
    at(&s, 100 * MIN);
    edit(&mut s, 6).await;
    wait_until(
        "the alert while the closed window holds",
        Duration::from_secs(10),
        || async { drift_alerts(&s) == 2 },
    )
    .await;
    assert_eq!(held(&s), 1);
    // At 3 h 30 the rolling window is (30 min, 3 h 30]: 90 minutes of it were held, still over the budget.
    at(&s, 210 * MIN);
    edit(&mut s, 3).await;
    wait_until("still spent", Duration::from_secs(10), || async {
        drift_alerts(&s) == 3
    })
    .await;
    assert_eq!(held(&s), 1);

    // At 4 h 01 the rolling window is (1 h 01, 4 h 01]: 59 minutes of it were held, under the budget: the next window holds again.
    at(&s, 241 * MIN);
    open(&s, "uid-2", NOW + 241 * MIN).await;
    edit(&mut s, 4).await;
    assert_eq!(held(&s), 2, "the budget has refilled");
    assert_eq!(drift_alerts(&s), 3);

    // It is spent again at 5 h 01 (an hour of the new window), and that is a new episode, said again.
    at(&s, 302 * MIN);
    edit(&mut s, 5).await;
    wait_until(
        "the alert of the second episode",
        Duration::from_secs(10),
        || async { drift_alerts(&s) == 3 },
    )
    .await;
    assert_eq!(held(&s), 2);
    tick(&s).await;
    assert_eq!(budget_notices(&s), 2, "one for each episode");
    assert_eq!(s.w.rig.metrics.hold_budget_exhaustion_count(), 2);
}
