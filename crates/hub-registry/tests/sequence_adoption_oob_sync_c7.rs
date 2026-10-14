//! T4 Verify, the sequence: adoption, then an out-of-band edit, then a sync that overwrites it, which produces C7 and a notifier call.
//!
//! The adoption is a stand-in (T5 builds the adoption service later): the swimlane is confirmed and the baselines are written
//! directly, as the confirmation would write them. Everything after is the real code: ingestion of the agent's deltas, the pipeline, and
//! `SyncService::record_sync` with a scripted agent that answers the request for a scan the way the real one does, with a full scan
//! through ingestion. Over a real Postgres 16, the in-memory Git, the real audit log and outbox.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{ContentHash, DomainEvent, RepoKind, Severity};
use engine::drift_hash;
use ports::NotificationKind;
use support::pg_audit::audit_rows;
use support::sync::{SyncRig, lane, tenant};
use support::world::state_of;

fn drift(text: &str) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

const CORE: &str = "app/core.yml";
const CORE_SIT1: &str = "app/core-sit1.yml";
const OTHER: &str = "app/other.yml";

const GIT: [(&str, &str); 3] = [(CORE, "a: 1\n"), (CORE_SIT1, "t: 1\n"), (OTHER, "o: 1\n")];

/// Steps 1 and 2: an adopted swimlane `sit1` in sync with Git, then two files edited out of band that the pipeline has seen, so that
/// they are ahead of Git and of their baselines. The commits are the ones the sync will report.
async fn ahead_on_nfs(
    config: Option<hub_registry::RegistryConfig>,
) -> Option<(SyncRig, domain::CommitId, domain::CommitId)> {
    let mut s = match config {
        Some(c) => SyncRig::with_config(&["sit1"], c).await?,
        None => SyncRig::new(&["sit1"]).await?,
    };

    // Git: a base file, another base file, and the tenant's copy of the first.
    let base = s.w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/other.yml", Some("o: 1\n")),
    ]);
    let tenant_commit =
        s.w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t: 1\n"))]);

    // 1. Adoption: NFS equals Git, an admin confirmed it, and every path has its baseline.
    s.w.inventory(&GIT).await;
    s.w.adopt().await;
    for (path, content) in GIT {
        s.w.baseline(path, drift(content)).await;
    }
    s.w.whole().await;
    for (path, _) in GIT {
        assert_eq!(state_of(&s.w, path).await.as_deref(), Some("in_sync"), "{path}");
    }

    // 2. Out-of-band edits of two files, which the pipeline sees: they are ahead of Git and of the baseline.
    s.w.nfs_change(&[(CORE, "a: 99\n"), (CORE_SIT1, "t: 99\n")]).await;
    s.w.paths(&[CORE, CORE_SIT1]).await;
    assert_eq!(state_of(&s.w, CORE).await.as_deref(), Some("nfs_ahead"));
    assert_eq!(state_of(&s.w, CORE_SIT1).await.as_deref(), Some("nfs_ahead"));
    assert!(
        s.notifier.sent().is_empty(),
        "nobody has been told anything: the sync is what gets the alert"
    );
    Some((s, tenant_commit, base))
}

#[tokio::test]
async fn adoption_then_oob_edit_then_overwriting_sync_gives_c7_and_a_notifier_call() {
    let Some((s, tenant_commit, base)) = ahead_on_nfs(None).await else {
        return;
    };
    let git = GIT;

    // 3. The sync puts Git's content back. Nobody tagged it: no window, no job in the delta. The report is what says so.
    s.agent_scans(&git);
    let outcome = s.sync("sit1", &tenant_commit, &base).await.unwrap();

    assert!(outcome.adopted);
    assert_eq!(outcome.overwritten_total, 2, "both edits were replaced");
    assert_eq!(
        outcome
            .overwritten
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec![CORE_SIT1.to_owned(), CORE.to_owned()],
        "in path order"
    );
    assert!(outcome.notified);
    assert_eq!(outcome.baselines_set, 0, "the baselines were Git's already");
    assert!(
        s.commands()
            .iter()
            .any(|c| matches!(c, domain::HubCommand::RequestFullScan)),
        "the hub asked the agent for a full scan"
    );
    assert!(!s.w.rig.needs_full_scan().await, "the scan was applied");

    // The observation of each sync version says what it replaced, and the sync job is its author.
    for (path, edited) in [(CORE, "a: 99\n"), (CORE_SIT1, "t: 99\n")] {
        let obs = s.w.rig.observations(path).await;
        let sync = obs.last().unwrap();
        assert_eq!(
            (
                sync.attribution_source.as_str(),
                sync.attribution_confidence.as_str()
            ),
            ("sync_job", "high"),
            "{path}"
        );
        assert_eq!(sync.attribution_actor.as_ref().unwrap()["kind"], "job");
        assert_eq!(sync.previous_state.as_deref(), Some("nfs_ahead"), "{path}");
        assert_eq!(
            sync.previous_hash,
            Some(drift(edited)),
            "{path}: N of the version it replaced"
        );
        assert!(
            obs[..obs.len() - 1].iter().all(|o| o.previous_state.is_none()),
            "only the sync's own version carries it"
        );
    }

    // The notifier was called, once, with severity from the rules, naming counts and ids and no file.
    let sent = s.notifier.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].kind, NotificationKind::CriticalFinding);
    assert_eq!(sent[0].severity, Severity::Critical, "C7 is critical (D76)");
    assert_eq!(sent[0].swimlane, Some(lane()));
    assert!(
        sent[0].summary.as_str().contains("2 file(s)"),
        "{:?}",
        sent[0].summary
    );
    assert!(sent[0].summary.as_str().contains("sit1"));
    let text = format!("{} {}", sent[0].title, sent[0].summary);
    assert!(
        !text.contains("core") && !text.contains("a: 1") && !text.contains(&drift("a: 1\n").to_string()),
        "no path, content or hash in a notification: {text}"
    );
    assert_eq!(
        sent[0].link.as_ref().map(domain::ShortText::as_str),
        Some("/swimlanes/sit1")
    );

    // The pipeline now computes C7 from that context.
    s.w.paths(&[CORE, CORE_SIT1]).await;
    assert_eq!(state_of(&s.w, CORE).await.as_deref(), Some("in_sync"));
    let findings = s.w.tables().await.findings;
    let mut c7: Vec<_> = findings
        .iter()
        .filter(|f| f.0 == "overwritten_nfs_change")
        .map(|f| (f.2.clone().unwrap(), f.3.clone()))
        .collect();
    c7.sort();
    assert_eq!(
        c7,
        vec![
            (CORE_SIT1.to_owned(), "critical".to_owned()),
            (CORE.to_owned(), "critical".to_owned())
        ],
        "C7 for each overwritten edit, and for no other file"
    );
    assert!(
        !findings.iter().any(|f| f.0 == "drift"),
        "everything is in sync again"
    );

    // What was written, and the events that went with it (rule 8, D80).
    let audit = audit_rows(&s.w.rig.pool).await;
    let count = |action: &str| audit.iter().filter(|a| a.action == action).count();
    assert_eq!(
        count("attribution_upgraded"),
        2,
        "one event for each upgraded attribution"
    );
    assert_eq!(
        count("settings_changed"),
        1,
        "one event for the deployment record"
    );
    assert_eq!(count("baseline_adopted"), 0, "no baseline changed");
    let upgraded: Vec<_> = audit
        .iter()
        .filter(|a| a.action == "attribution_upgraded")
        .collect();
    assert!(
        upgraded
            .iter()
            .all(|a| a.via == "sync" && a.attribution_source.as_deref() == Some("sync_job"))
    );
    assert_eq!(
        audit.last().map(|a| a.action.as_str()),
        Some("settings_changed"),
        "the deployment is recorded last, after the baselines"
    );
    let updated =
        s.w.rig
            .bus
            .published()
            .await
            .into_iter()
            .filter(|e| matches!(e, DomainEvent::AttributionUpdated { .. }))
            .count();
    assert_eq!(
        updated, 2,
        "an AttributionUpdated for each upgrade, in the same transaction"
    );

    // The deployment record: what the last sync put on this swimlane.
    let row: (String, String, String, String, serde_json::Value) = sqlx::query_as(
        "SELECT tenant_id, tenant_commit, base_commit, source, actor FROM swimlane_deployments WHERE swimlane_id = 'sit1'",
    )
    .fetch_one(&s.w.rig.pool)
    .await
    .unwrap();
    assert_eq!(
        (row.0, row.1, row.2, row.3),
        (
            tenant("sit1").as_str().to_owned(),
            tenant_commit.as_str().to_owned(),
            base.as_str().to_owned(),
            "job-report".to_owned()
        )
    );
    assert_eq!(row.4["kind"], "job");
    assert_eq!(s.w.head(RepoKind::Tenant, "sit1"), tenant_commit);
    assert_eq!(s.w.rig.metrics.sync_overwritten_count(), 2);
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

#[tokio::test]
async fn a_notifier_that_fails_or_never_answers_does_not_fail_the_report() {
    for hang in [false, true] {
        let config = hub_registry::RegistryConfig {
            sync_poll_interval: std::time::Duration::from_millis(10),
            // The shortest the configuration allows: a hanging notifier costs this much real time, and no more.
            notify_timeout: std::time::Duration::from_millis(100),
            ..hub_registry::RegistryConfig::default()
        };
        let Some((s, tenant_commit, base)) = ahead_on_nfs(Some(config.clone())).await else {
            return;
        };
        s.agent_scans(&GIT);
        let service = s.service_with(std::sync::Arc::new(BadNotifier { hang }), config);
        let outcome = service
            .record_sync(
                &lane(),
                &tenant("sit1"),
                &tenant_commit,
                &base,
                &support::sync::job(),
                &support::sync::source(),
            )
            .await
            .unwrap_or_else(|e| panic!("hang={hang}: {e}"));
        assert_eq!(
            outcome.overwritten_total, 2,
            "hang={hang}: the changes were claimed all the same"
        );
        assert!(
            !outcome.notified,
            "hang={hang}: nobody was told, and the report says so"
        );
        let obs = s.w.rig.observations(CORE).await;
        assert_eq!(
            obs.last().unwrap().previous_state.as_deref(),
            Some("nfs_ahead"),
            "hang={hang}: C7's context is written whatever the notifier does"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM swimlane_deployments")
                .fetch_one(&s.w.rig.pool)
                .await
                .unwrap(),
            1,
            "hang={hang}: the deployment is recorded"
        );
    }
}
