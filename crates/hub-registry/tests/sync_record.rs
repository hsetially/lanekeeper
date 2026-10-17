//! `SyncService::record_sync` (T4, D19, D21, D75): the checks it makes, the baselines it sets at the commits it is given, the
//! deployment record, what it writes in chunks and how it fails. The C7 half is `sequence_adoption_oob_sync_c7.rs`.
//!
//! Over a real Postgres 16, the in-memory Git, the real audit log and outbox, and an agent that answers a request for a scan with a
//! full scan through ingestion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use domain::{AgentReply, CommitId, ContentHash, HubCommand, OpResult, RequestId, SwimlaneId};
use engine::drift_hash;
use hub_registry::metrics::SyncResult;
use hub_registry::{SyncError, SyncSource};
use ports::content_hash;
use sqlx::Row;
use support::log_capture::LogCapture;
use support::pg_audit::audit_rows;
use support::statements::{inserts, statements};
use support::sync::{SyncRig, job, lane, source, tenant};
use support::world::state_of;

fn drift(text: &str) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

/// An adopted swimlane `sit1` (tenants as given) whose NFS holds `files`, with Git at the commits returned: the base repository holds
/// `base`, the tenant repository's branch `sit1` holds `tenant`.
async fn adopted(
    tenants: &[&str],
    base: &[(&str, &str)],
    tenant_files: &[(&str, &str)],
    nfs: &[(&str, &str)],
) -> Option<(SyncRig, CommitId, CommitId)> {
    let mut s = SyncRig::new(tenants).await?;
    let base_changes: Vec<(&str, Option<&str>)> = base.iter().map(|(p, c)| (*p, Some(*c))).collect();
    let tenant_changes: Vec<(&str, Option<&str>)> =
        tenant_files.iter().map(|(p, c)| (*p, Some(*c))).collect();
    let base_commit = s.w.base_commit(&base_changes);
    let tenant_commit = s.w.tenant_commit("sit1", &tenant_changes);
    s.w.inventory(nfs).await;
    s.w.adopt().await;
    Some((s, tenant_commit, base_commit))
}

async fn baseline_row(
    s: &SyncRig,
    path: &str,
) -> Option<(ContentHash, String, Option<String>, String, Option<ContentHash>)> {
    let row = sqlx::query(
        "SELECT hash::bytea, repo, branch, commit_id, blob_hash::bytea FROM baselines WHERE swimlane_id = 'sit1' AND path = $1",
    )
    .bind(path)
    .fetch_optional(&s.w.rig.pool)
    .await
    .unwrap()?;
    let h = |b: Vec<u8>| ContentHash::from_bytes(b.try_into().unwrap());
    Some((
        h(row.get("hash")),
        row.get("repo"),
        row.get("branch"),
        row.get("commit_id"),
        row.get::<Option<Vec<u8>>, _>("blob_hash").map(h),
    ))
}

async fn refs_of(s: &SyncRig, ref_id: &str) -> Vec<ContentHash> {
    sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT hash::bytea FROM blob_refs WHERE ref_kind = 'baseline' AND ref_id = $1 ORDER BY hash",
    )
    .bind(ref_id)
    .fetch_all(&s.w.rig.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|b| ContentHash::from_bytes(b.try_into().unwrap()))
    .collect()
}

async fn audit_count(s: &SyncRig, action: &str) -> usize {
    audit_rows(&s.w.rig.pool)
        .await
        .iter()
        .filter(|a| a.action == action)
        .count()
}

async fn deployments(s: &SyncRig) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT tenant_id, tenant_commit, base_commit, source FROM swimlane_deployments WHERE swimlane_id = 'sit1' ORDER BY tenant_id",
    )
    .fetch_all(&s.w.rig.pool)
    .await
    .unwrap()
}

// ------------------------------------------------------------------------------------------------ baselines

#[tokio::test]
async fn baselines_follow_the_commit_the_report_names_and_not_the_head() {
    let Some((s, tenant_commit, c1)) = adopted(
        &["sit1"],
        &[("config/app/core.yml", "a: 1\n")],
        &[("data/config/app/other.yml", "x: 1\n")],
        &[("app/core.yml", "a: 1\n")],
    )
    .await
    else {
        return;
    };
    // Git moves on after the sync deployed c1: the head is c2.
    let c2 = s.w.base_commit(&[("config/app/core.yml", Some("a: 2\n"))]);
    assert_ne!(c1, c2);
    // The baseline is a stale one, from before.
    s.w.baseline("app/core.yml", drift("a: 0\n")).await;
    s.w.whole().await;
    assert_eq!(state_of(&s.w, "app/core.yml").await.as_deref(), Some("conflict"));

    s.agent_scans(&[("app/core.yml", "a: 1\n")]);
    let outcome = s.sync("sit1", &tenant_commit, &c1).await.unwrap();
    assert_eq!(outcome.baselines_set, 1);
    assert_eq!(outcome.overwritten_total, 0, "nothing was ahead on NFS");
    assert!(!outcome.notified && s.notifier.sent().is_empty());

    let (hash, repo, branch, commit, blob) = baseline_row(&s, "app/core.yml").await.unwrap();
    assert_eq!(hash, drift("a: 1\n"), "the hash of Git at c1, for comparing");
    assert_eq!((repo.as_str(), branch), ("base", None));
    assert_eq!(
        commit,
        c1.as_str(),
        "the commit the report named, not the head c2"
    );
    assert_eq!(blob, Some(content_hash(b"a: 1\n")), "the raw version behind it");
    assert_eq!(
        refs_of(&s, "sit1:app/core.yml").await,
        vec![content_hash(b"a: 1\n")]
    );

    // Git is ahead of what was deployed, which only a baseline at c1 can say.
    s.w.paths(&["app/core.yml"]).await;
    assert_eq!(state_of(&s.w, "app/core.yml").await.as_deref(), Some("git_ahead"));

    let audit = audit_rows(&s.w.rig.pool).await;
    let adopted: Vec<_> = audit.iter().filter(|a| a.action == "baseline_adopted").collect();
    assert_eq!(adopted.len(), 1, "one event for each baseline that changed");
    assert_eq!(adopted[0].via, "sync");
    assert_eq!(adopted[0].path.as_deref(), Some("app/core.yml"));
    assert_eq!(adopted[0].hash_before, Some(drift("a: 0\n")));
    assert_eq!(adopted[0].hash_after, Some(drift("a: 1\n")));
    assert_eq!(
        deployments(&s).await,
        vec![(
            "sit1".to_owned(),
            tenant_commit.as_str().to_owned(),
            c1.as_str().to_owned(),
            "job-report".to_owned()
        )]
    );

    // The next sync deploys c2: the baseline moves, and the reference of the version it replaced goes.
    s.agent_scans(&[("app/core.yml", "a: 2\n")]);
    let outcome = s.sync("sit1", &tenant_commit, &c2).await.unwrap();
    assert_eq!(outcome.baselines_set, 1);
    let (hash, _, _, commit, blob) = baseline_row(&s, "app/core.yml").await.unwrap();
    assert_eq!((hash, commit.as_str()), (drift("a: 2\n"), c2.as_str()));
    assert_eq!(blob, Some(content_hash(b"a: 2\n")));
    assert_eq!(
        refs_of(&s, "sit1:app/core.yml").await,
        vec![content_hash(b"a: 2\n")],
        "one reference for the path: the new version's"
    );
    assert_eq!(audit_count(&s, "baseline_adopted").await, 2);
    assert_eq!(
        deployments(&s).await.len(),
        1,
        "one row for each tenant, replaced by each report"
    );
    assert!(
        s.w.rig.metrics.sync_baselines_count() >= 2
            && s.w
                .rig
                .metrics
                .pipeline_signal_count(hub_registry::metrics::SignalResult::Accepted)
                >= 2,
        "the pipeline was told which paths changed"
    );
}

#[tokio::test]
async fn a_baseline_that_says_the_same_is_not_written_again() {
    let Some((s, tenant_commit, base)) = adopted(
        &["sit1"],
        &[("config/app/core.yml", "a: 1\n")],
        &[("data/config/app/x.yml", "x: 1\n")],
        &[("app/core.yml", "a: 1\n")],
    )
    .await
    else {
        return;
    };
    s.agent_scans(&[("app/core.yml", "a: 1\n")]);
    assert_eq!(
        s.sync("sit1", &tenant_commit, &base).await.unwrap().baselines_set,
        1
    );
    assert_eq!(audit_count(&s, "baseline_adopted").await, 1);
    let again = s.sync("sit1", &tenant_commit, &base).await.unwrap();
    assert_eq!(again.baselines_set, 0, "the baseline is there already");
    assert_eq!(
        audit_count(&s, "baseline_adopted").await,
        1,
        "no event for a baseline that did not change"
    );
    assert_eq!(
        audit_count(&s, "settings_changed").await,
        2,
        "but each report is a deployment record, and each is one event"
    );
    assert_eq!(deployments(&s).await.len(), 1);
}

#[tokio::test]
async fn each_file_is_compared_with_the_git_side_its_name_stands_for() {
    // sit1 and sit2 are both deployed here. The report is for sit1.
    let Some(mut s) = SyncRig::new(&["sit1", "sit2"]).await else {
        return;
    };
    let base = s.w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    let sit1 =
        s.w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t1: 1\n"))]);
    let _sit2 =
        s.w.tenant_commit("sit2", &[("data/config/app/core.yml", Some("t2: 1\n"))]);
    let files = [
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t1: 1\n"),
        ("app/core-sit2.yml", "t2: 1\n"),
        ("app/untracked.yml", "u: 1\n"),
        ("app/differs.yml", "d: 1\n"),
    ];
    s.w.base_commit(&[("config/app/differs.yml", Some("d: 0\n"))]);
    s.w.inventory(&files).await;
    s.w.adopt().await;
    // `differs.yml` is in Git at the head, but at the reported base commit it did not exist yet.
    s.agent_scans(&files);
    let outcome = s.sync("sit1", &sit1, &base).await.unwrap();

    let sit1_row = baseline_row(&s, "app/core-sit1.yml").await.unwrap();
    assert_eq!(
        (sit1_row.1.as_str(), sit1_row.2.as_deref(), sit1_row.3.as_str()),
        ("tenant", Some("sit1"), sit1.as_str()),
        "a tenant file of the reported tenant is compared with the tenant branch at the tenant commit"
    );
    assert_eq!(sit1_row.0, drift("t1: 1\n"));
    let base_row = baseline_row(&s, "app/core.yml").await.unwrap();
    assert_eq!(
        (base_row.1.as_str(), base_row.2, base_row.3.as_str()),
        ("base", None, base.as_str())
    );
    assert!(
        baseline_row(&s, "app/core-sit2.yml").await.is_none(),
        "another tenant's file is that tenant's report, not this one's"
    );
    assert!(
        baseline_row(&s, "app/untracked.yml").await.is_none(),
        "Git has no such file"
    );
    assert!(
        baseline_row(&s, "app/differs.yml").await.is_none(),
        "at the reported commit Git has no such file, whatever it has at the head"
    );
    assert_eq!(outcome.baselines_set, 2);
}

#[tokio::test]
async fn a_denied_file_gets_a_baseline_with_no_blob_behind_it() {
    let Some((s, tenant_commit, base)) = adopted(
        &["sit1"],
        &[("config/app/server.pem", "KEY-MATERIAL")],
        &[("data/config/app/x.yml", "x: 1\n")],
        &[("app/server.pem", "KEY-MATERIAL")],
    )
    .await
    else {
        return;
    };
    s.agent_scans(&[("app/server.pem", "KEY-MATERIAL")]);
    let outcome = s.sync("sit1", &tenant_commit, &base).await.unwrap();
    assert_eq!(outcome.baselines_set, 1);
    let (hash, _, _, _, blob) = baseline_row(&s, "app/server.pem").await.unwrap();
    assert_eq!(
        hash,
        content_hash(b"KEY-MATERIAL"),
        "a denied file is compared as it is"
    );
    assert_eq!(
        blob, None,
        "D79: its content is never stored, so nothing is kept for it"
    );
    assert!(refs_of(&s, "sit1:app/server.pem").await.is_empty());
}

// ------------------------------------------------------------------------------------------------ before the adoption

#[tokio::test]
async fn a_swimlane_nobody_has_adopted_only_gets_its_deployment_recorded() {
    let Some(mut s) = SyncRig::new(&["sit1"]).await else {
        return;
    };
    let base = s.w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    let tenant_commit =
        s.w.tenant_commit("sit1", &[("data/config/app/x.yml", Some("x: 1\n"))]);
    s.w.inventory(&[("app/core.yml", "a: 1\n")]).await;
    // No agent is attached and nothing is adopted: the report must not need either.
    let outcome = s.sync("sit1", &tenant_commit, &base).await.unwrap();
    assert!(!outcome.adopted);
    assert_eq!((outcome.baselines_set, outcome.overwritten_total), (0, 0));
    assert!(s.commands().is_empty(), "no scan was asked for");
    assert!(
        baseline_row(&s, "app/core.yml").await.is_none(),
        "the first baseline is the adoption pass's (D22)"
    );
    assert_eq!(deployments(&s).await.len(), 1);
    assert_eq!(audit_count(&s, "settings_changed").await, 1);
    assert_eq!(audit_count(&s, "baseline_adopted").await, 0);
    assert_eq!(s.w.rig.metrics.sync_count(SyncResult::DeploymentOnly), 1);
}

// ------------------------------------------------------------------------------------------------ what is refused

async fn nothing_was_written(s: &SyncRig) {
    assert!(deployments(s).await.is_empty(), "no deployment record");
    assert_eq!(audit_count(s, "settings_changed").await, 0);
    assert_eq!(audit_count(s, "baseline_adopted").await, 0);
    assert_eq!(audit_count(s, "attribution_upgraded").await, 0);
    assert_eq!(s.w.rig.count("SELECT count(*) FROM baselines").await, 0);
}

#[tokio::test]
async fn a_report_the_hub_does_not_accept_is_refused_and_writes_nothing() {
    let Some((s, tenant_commit, base)) = adopted(
        &["sit1"],
        &[("config/app/core.yml", "a: 1\n")],
        &[("data/config/app/x.yml", "x: 1\n")],
        &[("app/core.yml", "a: 1\n")],
    )
    .await
    else {
        return;
    };
    s.agent_scans(&[("app/core.yml", "a: 1\n")]);
    let unknown_lane = SwimlaneId::parse("sit9").unwrap();
    assert_eq!(
        s.service
            .record_sync(
                &unknown_lane,
                &tenant("sit1"),
                &tenant_commit,
                &base,
                &job(),
                &source()
            )
            .await,
        Err(SyncError::UnknownSwimlane)
    );
    // A branch that is a valid tenant id and that no rule makes deployable.
    assert_eq!(
        s.sync("main", &tenant_commit, &base).await,
        Err(SyncError::NotDeployable)
    );
    // A branch that is deployable and that this swimlane does not deploy: an admin confirms a changed set, a sync does not.
    assert_eq!(
        s.sync("sit2", &tenant_commit, &base).await,
        Err(SyncError::TenantNotDeployed)
    );
    // A commit the mirror does not have.
    let missing = CommitId::parse(&"0".repeat(40)).unwrap();
    assert_eq!(
        s.sync("sit1", &missing, &base).await,
        Err(SyncError::UnknownCommit)
    );
    assert_eq!(
        s.sync("sit1", &tenant_commit, &missing).await,
        Err(SyncError::UnknownCommit)
    );
    // The admin took the branch out.
    sqlx::query(
        "INSERT INTO path_mappings (repo, repo_root, branch_exclude) VALUES ('tenant', 'data/config', '{sit1}')",
    )
    .execute(&s.w.rig.pool)
    .await
    .unwrap();
    assert_eq!(
        s.sync("sit1", &tenant_commit, &base).await,
        Err(SyncError::NotDeployable)
    );

    nothing_was_written(&s).await;
    assert!(
        s.commands().is_empty(),
        "no scan was asked for by a report that was refused"
    );
    assert!(!s.w.rig.needs_full_scan().await);
    assert_eq!(s.w.rig.metrics.sync_count(SyncResult::Rejected), 6);
    assert!(s.notifier.sent().is_empty());
}

#[tokio::test]
async fn an_admin_pattern_makes_a_branch_deployable_but_the_swimlane_must_still_deploy_it() {
    let Some((s, tenant_commit, base)) = adopted(
        &["sit1"],
        &[("config/app/core.yml", "a: 1\n")],
        &[("data/config/app/x.yml", "x: 1\n")],
        &[("app/core.yml", "a: 1\n")],
    )
    .await
    else {
        return;
    };
    sqlx::query("INSERT INTO path_mappings (repo, repo_root, branch_include) VALUES ('tenant', 'data/config', '{uat*}')")
        .execute(&s.w.rig.pool)
        .await
        .unwrap();
    assert_eq!(
        s.sync("uat1", &tenant_commit, &base).await,
        Err(SyncError::TenantNotDeployed),
        "uat1 is deployable now, and this swimlane's tenants are sit1"
    );
    nothing_was_written(&s).await;
}

#[tokio::test]
async fn an_agent_that_is_not_connected_fails_fast_and_the_swimlane_stays_flagged() {
    let Some((s, tenant_commit, base)) = adopted(
        &["sit1"],
        &[("config/app/core.yml", "a: 1\n")],
        &[("data/config/app/x.yml", "x: 1\n")],
        &[("app/core.yml", "a: 1\n")],
    )
    .await
    else {
        return;
    };
    // No agent is attached to the gateway.
    assert_eq!(
        s.sync("sit1", &tenant_commit, &base).await,
        Err(SyncError::AgentNotConnected)
    );
    nothing_was_written(&s).await;
    assert!(
        s.w.rig.needs_full_scan().await,
        "the swimlane is flagged: the agent's next heartbeat after it comes back asks for the scan"
    );
    assert_eq!(s.w.rig.metrics.sync_count(SyncResult::AgentNotConnected), 1);
}

#[tokio::test]
async fn a_scan_that_does_not_come_in_time_is_a_timeout_and_writes_nothing() {
    let Some((s, tenant_commit, base)) = adopted(
        &["sit1"],
        &[("config/app/core.yml", "a: 1\n")],
        &[("data/config/app/x.yml", "x: 1\n")],
        &[("app/core.yml", "a: 1\n")],
    )
    .await
    else {
        return;
    };
    // An agent that acknowledges the request and never sends the scan, and tells the test when the request arrived.
    let arrived = Arc::new(tokio::sync::Notify::new());
    let seen = Arc::clone(&arrived);
    s.gateway.attach(
        &lane(),
        Arc::new(move |cmd: &HubCommand| {
            if matches!(cmd, HubCommand::RequestFullScan) {
                seen.notify_one();
                Some(AgentReply::Op(OpResult {
                    request_id: RequestId::parse("ack").unwrap(),
                    ok: true,
                    error: None,
                    current_hash: None,
                }))
            } else {
                None
            }
        }),
    );
    let service = s.service.clone();
    let (t, b) = (tenant_commit.clone(), base.clone());
    let report = tokio::spawn(async move {
        service
            .record_sync(
                &lane(),
                &tenant("sit1"),
                &t,
                &b,
                &job(),
                &SyncSource::parse("job-report").unwrap(),
            )
            .await
    });
    arrived.notified().await;
    // The report is waiting. The deadline was fixed before the request, so moving the clock now ends the wait: no real waiting.
    s.w.rig.clock.advance_secs(121);
    assert_eq!(report.await.unwrap(), Err(SyncError::ScanTimeout));

    nothing_was_written(&s).await;
    assert!(
        s.w.rig.needs_full_scan().await,
        "still flagged: the scan will come, and a second report finds it applied"
    );
    assert_eq!(s.w.rig.metrics.sync_count(SyncResult::ScanTimeout), 1);
    assert!(s.notifier.sent().is_empty());

    // The agent comes back and the report is made again: it finds the scan and does the rest.
    s.agent_scans(&[("app/core.yml", "a: 1\n")]);
    let outcome = s.sync("sit1", &tenant_commit, &base).await.unwrap();
    assert_eq!(outcome.baselines_set, 1);
    assert_eq!(deployments(&s).await.len(), 1);
}

// ------------------------------------------------------------------------------------------------ chunks and statements

#[tokio::test]
async fn fifty_baselines_are_one_insert_each_for_the_baselines_the_refs_and_the_audit_events() {
    let names: Vec<String> = (0..60).map(|i| format!("svc/f{i:02}.yml")).collect();
    let base: Vec<(String, String)> = names
        .iter()
        .map(|n| (format!("config/{n}"), "v: 1\n".to_owned()))
        .collect();
    let base_refs: Vec<(&str, &str)> = base.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let nfs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "v: 1\n")).collect();
    let Some((s, tenant_commit, base_commit)) = adopted(
        &["sit1"],
        &base_refs,
        &[("data/config/app/x.yml", "x: 1\n")],
        &nfs,
    )
    .await
    else {
        return;
    };
    // Fifty files have a stale baseline, ten have the right one.
    for (i, n) in names.iter().enumerate() {
        s.w.baseline(n, drift(if i < 50 { "v: 0\n" } else { "v: 1\n" }))
            .await;
    }
    s.agent_scans(&nfs);
    let (capture, _guard) = LogCapture::install();
    let outcome = s.sync("sit1", &tenant_commit, &base_commit).await.unwrap();
    let all = statements(&capture);
    assert!(
        !all.is_empty(),
        "no statement was captured: the counts below would pass for nothing"
    );
    assert_eq!(outcome.baselines_set, 50);

    assert_eq!(
        inserts(&all, "INSERT INTO baselines"),
        vec![50],
        "one insert for the chunk, not one for each path"
    );
    assert_eq!(inserts(&all, "INSERT INTO blob_refs"), vec![50]);
    assert_eq!(
        inserts(&all, "INSERT INTO audit_events"),
        vec![50, 1],
        "the 50 baselines are one batch, the deployment record is one event"
    );
    assert_eq!(inserts(&all, "INSERT INTO swimlane_deployments"), vec![1]);
    assert_eq!(
        s.w.rig.audit.batch_calls(),
        1,
        "the registry called record_many once, and the decorator forwarded it"
    );
    assert_eq!(
        s.w.rig.audit.single_calls(),
        1,
        "and `record` only for the single deployment event"
    );
    assert_eq!(audit_count(&s, "baseline_adopted").await, 50);
}

#[tokio::test]
async fn a_hundred_and_twenty_baselines_are_three_chunks_of_their_own_transactions() {
    let names: Vec<String> = (0..120).map(|i| format!("svc/f{i:03}.yml")).collect();
    let base: Vec<(String, String)> = names
        .iter()
        .map(|n| (format!("config/{n}"), "v: 1\n".to_owned()))
        .collect();
    let base_refs: Vec<(&str, &str)> = base.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let nfs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "v: 1\n")).collect();
    let Some((s, tenant_commit, base_commit)) = adopted(
        &["sit1"],
        &base_refs,
        &[("data/config/app/x.yml", "x: 1\n")],
        &nfs,
    )
    .await
    else {
        return;
    };
    s.agent_scans(&nfs);
    let (capture, _guard) = LogCapture::install();
    let outcome = s.sync("sit1", &tenant_commit, &base_commit).await.unwrap();
    let all = statements(&capture);
    assert_eq!(
        outcome.baselines_set, 120,
        "no baseline at all yet: every file gets one"
    );
    assert_eq!(inserts(&all, "INSERT INTO baselines"), vec![50, 50, 20]);
    assert_eq!(inserts(&all, "INSERT INTO audit_events"), vec![50, 50, 20, 1]);
    assert_eq!(s.w.rig.count("SELECT count(*) FROM baselines").await, 120);
}

// ------------------------------------------------------------------------------------------------ S10

#[tokio::test]
async fn no_content_path_or_hash_reaches_a_log_line() {
    let secret = "PLANTED-SECRET-VALUE-4711";
    let Some((s, tenant_commit, base)) = adopted(
        &["sit1"],
        &[("config/app/core.yml", "password: PLANTED-SECRET-VALUE-4711\n")],
        &[("data/config/app/x.yml", "x: 1\n")],
        &[("app/core.yml", "password: PLANTED-SECRET-VALUE-4711\n")],
    )
    .await
    else {
        return;
    };
    let capture = LogCapture::install_everywhere();
    s.agent_scans(&[("app/core.yml", "password: PLANTED-SECRET-VALUE-4711\n")]);
    s.sync("sit1", &tenant_commit, &base).await.unwrap();
    // A refused report logs too.
    assert_eq!(
        s.sync("main", &tenant_commit, &base).await,
        Err(SyncError::NotDeployable)
    );
    let text = capture.text();
    assert!(
        !text.is_empty(),
        "the capture saw nothing: the check below would pass for nothing"
    );
    assert!(!text.contains(secret), "file content reached a log line");
    assert!(
        !text.contains(&drift("password: PLANTED-SECRET-VALUE-4711\n").to_string()),
        "a hash of content reached a log line"
    );
}
