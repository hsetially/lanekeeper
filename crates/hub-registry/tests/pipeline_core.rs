//! The pipeline's batch (T3, D19, D20, D80, P5): a swimlane computed whole, brought up to date by the paths that changed, and what each
//! leaves in the tables and in the outbox. Over a real Postgres 16, the real event bus and the in-memory Git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{ContentHash, DomainEvent, RepoKind};
use engine::drift_hash;
use hub_registry::metrics::LaneResult;
use ports::content_hash;
use support::pg_audit::{AuditRow, audit_rows};
use support::world::{World, state_of};

/// The audit events with this action. Ingestion audits the out-of-band changes the tests make, which is not what is asked about.
async fn audited(w: &World, action: &str) -> Vec<AuditRow> {
    audit_rows(&w.rig.pool)
        .await
        .into_iter()
        .filter(|a| a.action == action)
        .collect()
}

fn h(text: &str) -> ContentHash {
    content_hash(text.as_bytes())
}

/// The hash for drift of a structured file with this text.
fn drift(text: &str) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

/// A world with Git and an inventory the tests share: a base file, a tenant file for sit1, a file that differs, a file Git does not
/// have and a file only Git has.
async fn world() -> Option<World> {
    let mut w = World::new(&["sit1"]).await?;
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/drifted.yml", Some("d: 0\n")),
        ("config/app/only-git.yml", Some("g: 1\n")),
    ]);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("a: 2\n"))]);
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "a: 2\n"),
        ("app/drifted.yml", "d: 1\n"),
        ("app/untracked.yml", "u: 1\n"),
    ])
    .await;
    Some(w)
}

// ------------------------------------------------------------------------------------------------ a whole swimlane

#[tokio::test]
async fn the_first_inventory_is_computed_whole_and_every_path_gets_its_state() {
    let Some(w) = world().await else { return };
    let report = w.whole().await;
    assert_eq!(report.lanes.len(), 1, "{report:?}");
    assert_eq!(report.lanes[0].result, LaneResult::Whole);
    assert_eq!(report.lanes[0].paths, 5, "four files on NFS and one only Git has");

    let t = w.tables().await;
    let state = |p: &str| t.rows.get(p).map(|r| r.0.as_str());
    assert_eq!(state("app/core.yml"), Some("in_sync"), "N = G with no baseline");
    assert_eq!(
        state("app/core-sit1.yml"),
        Some("in_sync"),
        "the tenant file against the tenant branch"
    );
    assert_eq!(
        state("app/drifted.yml"),
        Some("unknown"),
        "N differs from G and there is no baseline"
    );
    assert_eq!(
        state("app/untracked.yml"),
        Some("untracked"),
        "N with no G and no baseline"
    );
    assert_eq!(
        state("app/only-git.yml"),
        Some("unknown"),
        "a file Git has and NFS lacks gets a row of its own"
    );
    let only_git = &t.rows["app/only-git.yml"];
    assert_eq!(only_git.6, None, "no content on NFS");
    assert_eq!(only_git.1, Some(drift("g: 1\n").to_string()), "its hash in Git");
    assert_eq!(t.rows["app/core-sit1.yml"].4, "tenant");
    assert_eq!(t.rows["app/untracked.yml"].4, "untracked");
    assert_eq!(t.rows["app/core.yml"].3, "app/core.yml");
    assert_eq!(
        t.rows["app/core-sit1.yml"].3, "app/core.yml",
        "a tenant file and its base file are one logical file"
    );
    assert!(
        t.baselines.is_empty(),
        "before adoption the pipeline sets no baseline"
    );

    // The drift events name exactly the paths that were written, in chunks.
    let events = w.drift_events().await;
    let named: Vec<&String> = events.iter().flatten().collect();
    assert_eq!(named.len(), 5, "{events:?}");

    // Findings: drift (C5) for what is not in sync, an orphan (C4) for the file only NFS has.
    let kinds: Vec<(&str, Option<&str>)> = t
        .findings
        .iter()
        .map(|f| (f.0.as_str(), f.2.as_deref()))
        .collect();
    assert!(kinds.contains(&("drift", Some("app/drifted.yml"))), "{kinds:?}");
    assert!(
        kinds.contains(&("orphan_file", Some("app/untracked.yml"))),
        "{kinds:?}"
    );
    assert!(
        !kinds
            .iter()
            .any(|(k, p)| *k == "drift" && *p == Some("app/core.yml"))
    );
    assert!(w.findings_changed_events().await >= 1);

    // The effective tables: a hash for each logical file and directory the swimlane serves.
    assert!(t.files.contains_key("app/core.yml") && t.files.contains_key("app/untracked.yml"));
    assert!(
        t.dirs.contains_key("") && t.dirs.contains_key("app"),
        "{:?}",
        t.dirs
    );
    let (version, counts, finding_count, fingerprint) = w.registry_row().await;
    assert!(version >= 1 && fingerprint);
    assert_eq!(counts["in_sync"], 2);
    assert_eq!(counts["unknown"], 2);
    assert_eq!(counts["untracked"], 1);
    assert_eq!(usize::try_from(finding_count).unwrap(), t.findings.len());
}

#[tokio::test]
async fn a_second_whole_computation_of_a_swimlane_that_is_right_writes_nothing() {
    let Some(w) = world().await else { return };
    w.whole().await;
    let before = (w.tables().await, w.rig.count("SELECT count(*) FROM outbox").await);
    let again = w.whole().await;
    assert_eq!(
        again.lanes[0].rows_written + again.lanes[0].rows_deleted,
        0,
        "{again:?}"
    );
    assert_eq!(again.lanes[0].findings_changed, 0);
    assert_eq!(again.lanes[0].repaired, 0, "memory and the new computation agree");
    assert_eq!(
        (w.tables().await, w.rig.count("SELECT count(*) FROM outbox").await),
        before,
        "no row, no finding, no event"
    );
}

#[tokio::test]
async fn a_swimlane_with_no_inventory_is_skipped_and_computed_after_it_has_one() {
    let Some(w) = World::new(&["sit1"]).await else {
        return;
    };
    let report = w.whole().await;
    assert!(
        report.lanes.is_empty() && report.skipped == [w.lane()],
        "{report:?}"
    );
    assert_eq!(w.rig.metrics.pipeline_lane_count(LaneResult::NoInventory), 1);
    let mut w = w;
    w.base_commit(&[("config/a.yml", Some("a: 1\n"))]);
    w.inventory(&[("a.yml", "a: 1\n")]).await;
    let report = w.whole().await;
    assert_eq!(report.lanes[0].result, LaneResult::Whole);
    assert_eq!(state_of(&w, "a.yml").await.as_deref(), Some("in_sync"));
}

// ------------------------------------------------------------------------------------------------ incremental

#[tokio::test]
async fn a_change_recomputes_only_the_path_that_changed() {
    let Some(mut w) = world().await else { return };
    w.whole().await;
    let rows = w.rig.metrics.pipeline_rows_count();
    let events = w.drift_events().await.len();

    w.nfs_change(&[("app/core.yml", "a: 9\n")]).await;
    let report = w.paths(&["app/core.yml"]).await;
    assert_eq!(report.lanes[0].result, LaneResult::Incremental, "{report:?}");
    assert_eq!(report.lanes[0].paths, 1);
    assert_eq!(
        state_of(&w, "app/core.yml").await.as_deref(),
        Some("unknown"),
        "N is no longer G"
    );
    assert_eq!(
        w.rig.metrics.pipeline_rows_count() - rows,
        1,
        "one row written for one changed path"
    );
    let events_after = w.drift_events().await;
    assert_eq!(events_after.len(), events + 1);
    assert_eq!(events_after.last().unwrap(), &vec!["app/core.yml".to_owned()]);
    // Nothing else moved.
    let t = w.tables().await;
    assert_eq!(t.rows["app/core-sit1.yml"].0, "in_sync");
    assert_eq!(
        w.rig.metrics.pipeline_lane_count(LaneResult::Whole),
        1,
        "the swimlane was not loaded again"
    );
    assert_eq!(w.rig.metrics.pipeline_lane_count(LaneResult::Incremental), 1);
}

#[tokio::test]
async fn a_change_that_does_not_move_the_state_is_still_named_in_drift_changed() {
    let Some(mut w) = world().await else { return };
    w.whole().await;
    // drifted.yml is `unknown` and stays `unknown` when it changes again: the file changed, which is news.
    w.nfs_change(&[("app/drifted.yml", "d: 2\n")]).await;
    let before = w.rig.metrics.pipeline_rows_count();
    w.paths(&["app/drifted.yml"]).await;
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("unknown"));
    assert_eq!(
        w.drift_events().await.last().unwrap(),
        &vec!["app/drifted.yml".to_owned()]
    );
    assert_eq!(
        w.rig.metrics.pipeline_rows_count(),
        before,
        "but no row needed a write: ingestion wrote the new hash, and nothing derived changed"
    );
}

#[tokio::test]
async fn an_event_for_a_path_that_did_not_change_writes_and_publishes_nothing() {
    let Some(w) = world().await else { return };
    w.whole().await;
    let outbox = w.rig.count("SELECT count(*) FROM outbox").await;
    let report = w.paths(&["app/core.yml", "app/does-not-exist.yml"]).await;
    assert_eq!(report.lanes[0].drift_paths, 0, "{report:?}");
    assert_eq!(report.lanes[0].rows_written, 0);
    assert_eq!(w.rig.count("SELECT count(*) FROM outbox").await, outbox);
}

#[tokio::test]
async fn a_file_deleted_on_nfs_keeps_its_row_when_git_has_it_and_loses_it_when_not() {
    let Some(mut w) = world().await else { return };
    w.whole().await;
    w.nfs_delete(&["app/core-sit1.yml", "app/untracked.yml"]).await;
    let report = w.paths(&["app/core-sit1.yml", "app/untracked.yml"]).await;
    assert_eq!(report.lanes[0].rows_deleted, 1, "{report:?}");
    let t = w.tables().await;
    assert!(
        !t.rows.contains_key("app/untracked.yml"),
        "on neither side any more: not part of the swimlane"
    );
    let tenant = &t.rows["app/core-sit1.yml"];
    assert_eq!(tenant.0, "unknown", "Git has it and NFS no longer does");
    assert_eq!(tenant.6, None);
    assert!(
        !t.findings
            .iter()
            .any(|f| f.2.as_deref() == Some("app/untracked.yml")),
        "its findings are resolved with it"
    );
}

#[tokio::test]
async fn a_head_that_moves_recomputes_the_paths_of_its_diff_and_nothing_else() {
    let Some(w) = world().await else { return };
    w.whole().await;
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[
        ("config/app/drifted.yml", Some("d: 1\n")),
        ("README.md", Some("x")),
    ]);
    let work = w.moved(RepoKind::Base, "master", Some(&from), &to).await;
    assert_eq!(work.len(), 1, "one swimlane has an inventory");
    let report = w.pipeline.process(work).await.unwrap();
    assert_eq!(report.lanes[0].result, LaneResult::Incremental);
    assert_eq!(report.lanes[0].paths, 1, "the README is not configuration");
    assert_eq!(
        state_of(&w, "app/drifted.yml").await.as_deref(),
        Some("in_sync"),
        "Git moved to what NFS has"
    );
    assert_eq!(
        w.drift_events().await.last().unwrap(),
        &vec!["app/drifted.yml".to_owned()]
    );
    // The finding for the drifted file is resolved by the move.
    let t = w.tables().await;
    assert!(
        !t.findings
            .iter()
            .any(|f| f.0 == "drift" && f.2.as_deref() == Some("app/drifted.yml"))
    );
}

#[tokio::test]
async fn a_new_file_in_git_gets_a_row_and_a_deleted_one_loses_it() {
    let Some(w) = world().await else { return };
    w.whole().await;
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[
        ("config/app/brand-new.yml", Some("n: 1\n")),
        ("config/app/only-git.yml", None),
    ]);
    let work = w.moved(RepoKind::Base, "master", Some(&from), &to).await;
    let report = w.pipeline.process(work).await.unwrap();
    assert_eq!(report.lanes[0].paths, 2, "{report:?}");
    let t = w.tables().await;
    assert_eq!(t.rows["app/brand-new.yml"].0, "unknown");
    assert!(
        !t.rows.contains_key("app/only-git.yml"),
        "gone from Git and never on NFS"
    );
}

#[tokio::test]
async fn a_tenant_branch_move_recomputes_only_the_swimlanes_that_deploy_it() {
    let Some(w) = world().await else { return };
    w.second_lane("sit2", &["sit2"], &[("app/core.yml", "a: 1\n")])
        .await;
    w.whole().await;
    w.pipeline
        .process(vec![(
            domain::SwimlaneId::parse("sit2").unwrap(),
            hub_registry::Work::Whole,
        )])
        .await
        .unwrap();
    let from = w.head(RepoKind::Tenant, "sit1");
    let to = w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("a: 3\n"))]);
    let work = w.moved(RepoKind::Tenant, "sit1", Some(&from), &to).await;
    let lanes: Vec<&str> = work.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(lanes, ["sit1"], "sit2 does not deploy the sit1 branch");
    w.pipeline.process(work).await.unwrap();
    assert_eq!(
        state_of(&w, "app/core-sit1.yml").await.as_deref(),
        Some("unknown"),
        "NFS has a: 2, Git has a: 3"
    );
}

// ------------------------------------------------------------------------------------------------ content

#[tokio::test]
async fn incremental_never_needs_the_bytes_of_clean_files() {
    let Some(mut w) = world().await else { return };
    w.whole().await;
    w.nfs_change(&[("app/core-sit1.yml", "a: 7\n")]).await;
    w.rig.blobs.forget_requests();
    w.paths(&["app/core-sit1.yml"]).await;
    let asked: std::collections::BTreeSet<ContentHash> = w.rig.blobs.requested().into_iter().collect();
    let group = [h("a: 1\n"), h("a: 7\n")];
    assert!(
        asked.iter().all(|a| group.contains(a)),
        "only the logical file's own blobs (and the root's, there is none): asked {asked:?}"
    );
    assert!(
        !asked.contains(&h("d: 1\n")),
        "a file of another group is never read for this update"
    );
    assert!(!asked.contains(&h("u: 1\n")));

    // The incremental state is what a full computation gives.
    let incremental = w.pipeline.lane_state(&w.lane()).await.unwrap();
    let t_incremental = w.tables().await;
    let (fresh, _) = w.rig.pipeline(
        std::sync::Arc::new(w.git.clone()),
        hub_registry::RegistryConfig::default(),
    );
    fresh.refresh_lane(&w.lane()).await.unwrap();
    assert_eq!(fresh.lane_state(&w.lane()).await.unwrap(), incremental);
    assert_eq!(
        w.tables().await,
        t_incremental,
        "and it had nothing to repair in the tables"
    );
}

#[tokio::test]
async fn a_denied_file_is_compared_by_hash_and_its_git_content_is_never_read() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/server.key", Some("KEY")),
    ]);
    w.inventory(&[("app/core.yml", "a: 1\n")]).await;
    w.nfs_raw(|at| {
        let mut e = support::rig::entry("server.key", b"KEY", at);
        e.denied = true;
        e.bytes = None;
        vec![e]
    })
    .await;
    w.git.reset_reads();
    w.whole().await;
    let row = w.rig.state("server.key").await.unwrap();
    assert_eq!(row.class, "denied");
    assert_eq!(
        row.state, "in_sync",
        "the same hash on both sides, without either being read"
    );
    assert_eq!(row.git_hash, Some(content_hash(b"KEY")));
    assert_eq!(
        w.git.reads(),
        1,
        "only the YAML file was read from Git, never the key"
    );
    let t = w.tables().await;
    assert!(
        !t.files.contains_key("server.key"),
        "never an effective file of a denied file"
    );
    assert!(!t.findings.iter().any(|f| f.2.as_deref() == Some("server.key")));

    // A different key on NFS is drift, and still nothing is read.
    w.nfs_raw(|at| {
        let mut e = support::rig::entry("server.key", b"OTHER", at);
        e.denied = true;
        e.bytes = None;
        vec![e]
    })
    .await;
    w.paths(&["server.key"]).await;
    assert_eq!(w.rig.state("server.key").await.unwrap().state, "unknown");
    assert_eq!(w.git.reads(), 1);
}

// ------------------------------------------------------------------------------------------------ decisions

#[tokio::test]
async fn nfs_and_git_converging_set_a_baseline_after_adoption_and_not_before() {
    let Some(mut w) = world().await else { return };
    // The baseline of the drifted file is what Git has; NFS has moved on, so it is `nfs_ahead`.
    w.baseline("app/drifted.yml", drift("d: 0\n")).await;
    w.whole().await;
    assert_eq!(
        state_of(&w, "app/drifted.yml").await.as_deref(),
        Some("nfs_ahead")
    );

    // Git moves to what NFS has: they converge. The swimlane is not adopted, so the pipeline sets no baseline.
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[("config/app/drifted.yml", Some("d: 1\n"))]);
    w.pipeline
        .process(w.moved(RepoKind::Base, "master", Some(&from), &to).await)
        .await
        .unwrap();
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("in_sync"));
    assert_eq!(
        w.tables().await.baselines.get("app/drifted.yml"),
        Some(&drift("d: 0\n").to_string()),
        "the old baseline stands before adoption"
    );
    assert!(audited(&w, "baseline_adopted").await.is_empty());

    // Adoption: the files that already agree get their baselines at once (the swimlane is recomputed whole when its rules change,
    // and adoption is one of them), in one commit and so with one audit event, whatever the number of paths.
    w.adopt().await;
    let adopted = w.whole().await;
    assert_eq!(
        adopted.lanes[0].baselines_set, 3,
        "core, its tenant copy and the drifted file: {adopted:?}"
    );
    assert_eq!(audited(&w, "baseline_adopted").await.len(), 1);
    assert_eq!(w.tables().await.baselines.len(), 3);

    // After adoption, a convergence moves the baseline, with one audit event for the commit.
    w.nfs_change(&[("app/drifted.yml", "d: 5\n")]).await;
    w.paths(&["app/drifted.yml"]).await;
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[
        ("config/app/drifted.yml", Some("d: 5\n")),
        ("config/app/core.yml", Some("a: 5\n")),
    ]);
    w.nfs_change(&[("app/core.yml", "a: 5\n")]).await;
    let work = w.moved(RepoKind::Base, "master", Some(&from), &to).await;
    let report = w.pipeline.process(work).await.unwrap();
    assert_eq!(report.lanes[0].baselines_set, 2, "{report:?}");
    let t = w.tables().await;
    assert_eq!(
        t.baselines.get("app/drifted.yml"),
        Some(&drift("d: 5\n").to_string())
    );
    assert_eq!(
        t.rows["app/drifted.yml"].2,
        Some(drift("d: 5\n").to_string()),
        "the row says what its baseline is"
    );
    let audit = audited(&w, "baseline_adopted").await;
    assert_eq!(
        audit.len(),
        2,
        "one event for each commit, not one for each path: {audit:?}"
    );
    assert_eq!(audit[1].via, "system");
    assert_eq!(w.rig.metrics.pipeline_baselines_count(), 5);
    // The next difference is read against the baseline: Git moves on and NFS stays, which is `git_ahead`.
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[("config/app/drifted.yml", Some("d: 6\n"))]);
    w.pipeline
        .process(w.moved(RepoKind::Base, "master", Some(&from), &to).await)
        .await
        .unwrap();
    assert_eq!(
        state_of(&w, "app/drifted.yml").await.as_deref(),
        Some("git_ahead")
    );
}

#[tokio::test]
async fn a_mark_that_holds_overrides_the_state_and_one_that_no_longer_holds_is_cleared_and_audited() {
    let Some(mut w) = world().await else { return };
    w.whole().await;
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("unknown"));
    // Marked while N = "d: 1" and G = "d: 0".
    w.mark("app/drifted.yml", Some(drift("d: 1\n")), Some(drift("d: 0\n")))
        .await;
    w.paths(&["app/drifted.yml"]).await;
    assert_eq!(
        state_of(&w, "app/drifted.yml").await.as_deref(),
        Some("intentional_divergence")
    );
    assert!(w.tables().await.marks.contains("app/drifted.yml"));
    assert!(
        audited(&w, "drift_mark_cleared").await.is_empty(),
        "a mark that holds changes nothing to audit here"
    );

    // NFS changes: the mark no longer holds.
    w.nfs_change(&[("app/drifted.yml", "d: 2\n")]).await;
    let report = w.paths(&["app/drifted.yml"]).await;
    assert_eq!(report.lanes[0].marks_cleared, 1, "{report:?}");
    assert_eq!(state_of(&w, "app/drifted.yml").await.as_deref(), Some("unknown"));
    assert!(
        !w.tables().await.marks.contains("app/drifted.yml"),
        "the mark is deleted"
    );
    let audit = audited(&w, "drift_mark_cleared").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].path.as_deref(), Some("app/drifted.yml"));
}

// ------------------------------------------------------------------------------------------------ findings

#[tokio::test]
async fn a_finding_keeps_its_row_while_the_same_problem_lasts_and_is_resolved_when_it_goes() {
    let Some(mut w) = world().await else { return };
    w.baseline("app/drifted.yml", drift("d: 0\n")).await;
    w.whole().await;
    let open = |t: &support::world::Tables| {
        t.findings
            .iter()
            .find(|f| f.0 == "drift" && f.2.as_deref() == Some("app/drifted.yml"))
            .cloned()
    };
    let first = open(&w.tables().await).expect("a drift finding");
    let detected: i64 = sqlx_scalar(&w, "SELECT (EXTRACT(EPOCH FROM detected_at) * 1000)::bigint FROM findings WHERE kind = 'drift' AND path = 'app/drifted.yml' AND resolved_at IS NULL").await;

    // The same file gets worse (nfs_ahead becomes a conflict when Git moves too): the same finding, updated.
    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[("config/app/drifted.yml", Some("d: 9\n"))]);
    w.pipeline
        .process(w.moved(RepoKind::Base, "master", Some(&from), &to).await)
        .await
        .unwrap();
    let second = open(&w.tables().await).expect("still a drift finding");
    assert_eq!(second.1, first.1, "the same fingerprint");
    assert_ne!(second.4, first.4, "with the message of what it is now");
    let again: i64 = sqlx_scalar(&w, "SELECT (EXTRACT(EPOCH FROM detected_at) * 1000)::bigint FROM findings WHERE kind = 'drift' AND path = 'app/drifted.yml' AND resolved_at IS NULL").await;
    assert_eq!(again, detected, "open since the same time");
    assert_eq!(
        sqlx_scalar(
            &w,
            "SELECT count(*) FROM findings WHERE kind = 'drift' AND path = 'app/drifted.yml'"
        )
        .await,
        1_i64,
        "one row, not a new one"
    );

    // NFS and Git agree: resolved, not deleted.
    w.nfs_change(&[("app/drifted.yml", "d: 9\n")]).await;
    w.paths(&["app/drifted.yml"]).await;
    assert!(open(&w.tables().await).is_none());
    assert_eq!(
        sqlx_scalar(&w, "SELECT count(*) FROM findings WHERE kind = 'drift' AND path = 'app/drifted.yml' AND resolved_at IS NOT NULL").await,
        1_i64,
        "kept as history, with a time it was resolved"
    );
}

async fn sqlx_scalar(w: &World, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(&w.rig.pool).await.unwrap()
}

// ------------------------------------------------------------------------------------------------ rules

#[tokio::test]
async fn a_change_of_rules_recomputes_the_whole_swimlane_and_not_the_paths() {
    let Some(mut w) = world().await else { return };
    w.whole().await;
    w.nfs_change(&[("app/core.yml", "a: 4\n")]).await;
    // sit2 is added to the swimlane's tenants: what is a tenant file changes for every path.
    w.add_tenant("sit2").await;
    let report = w.paths(&["app/core.yml"]).await;
    assert_eq!(
        report.lanes[0].result,
        LaneResult::Whole,
        "the fingerprint changed: {report:?}"
    );
    assert!(report.lanes[0].paths >= 5);
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("unknown"));
}

#[tokio::test]
async fn nothing_but_the_event_bus_is_needed_to_publish() {
    // The pipeline publishes through `publish_in_tx` only: the outbox holds what the commit wrote, even with a bus that cannot fan out.
    let Some(w) = world().await else { return };
    w.whole().await;
    let events = w.rig.bus.published().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DomainEvent::DriftChanged { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DomainEvent::FindingsChanged { .. }))
    );
}

// ------------------------------------------------------------------------------------------------ commits in parallel

/// A push to the base repository concerns every swimlane, and their commits run `persist_concurrency` at a time. Each swimlane is
/// still exactly what a full computation says, and each has its own `DriftChanged`.
#[tokio::test]
async fn a_push_to_every_swimlane_commits_them_in_parallel_and_each_is_what_a_full_computation_says() {
    use hub_registry::RegistryConfig;

    let config = RegistryConfig {
        persist_concurrency: 4,
        ..RegistryConfig::default()
    };
    let Some(mut w) = World::with_config(&["sit1"], config).await else {
        return;
    };
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/other.yml", Some("o: 1\n")),
    ]);
    w.tenant_commit("sit1", &[]);
    w.inventory(&[("app/core.yml", "a: 1\n"), ("app/other.yml", "o: 1\n")])
        .await;
    let others = ["sit2", "sit3", "sit4", "sit5", "sit6", "sit7"];
    for id in others {
        w.second_lane(
            id,
            &[id],
            &[("app/core.yml", "a: 1\n"), ("app/other.yml", "o: 1\n")],
        )
        .await;
    }
    w.pipeline.refresh_all().await.unwrap();
    let seen = w.rig.metrics.pipeline_lane_count(LaneResult::Incremental);

    let from = w.head(RepoKind::Base, "master");
    let to = w.base_commit(&[("config/app/core.yml", Some("a: 9\n"))]);
    let work = w.moved(RepoKind::Base, "master", Some(&from), &to).await;
    assert_eq!(
        work.len(),
        7,
        "the base repository concerns every swimlane with an inventory"
    );
    let report = w.pipeline.process(work).await.unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.lanes.len(), 7);
    assert_eq!(
        w.rig.metrics.pipeline_lane_count(LaneResult::Incremental) - seen,
        7
    );

    // Every swimlane has the new state of the file, and has been told.
    let states: Vec<(String, String)> = sqlx::query_as(
        "SELECT swimlane_id, state FROM file_state WHERE path = 'app/core.yml' ORDER BY swimlane_id",
    )
    .fetch_all(&w.rig.pool)
    .await
    .unwrap();
    assert_eq!(states.len(), 7);
    assert!(
        states.iter().all(|(_, s)| s == "unknown"),
        "NFS has a: 1, Git has a: 9: {states:?}"
    );
    let told: i64 = w
        .rig
        .count(
            "SELECT count(DISTINCT event #>> '{event,swimlane}') FROM outbox \
             WHERE event #>> '{event,type}' = 'drift_changed' AND (event #> '{event,paths}') @> '\"app/core.yml\"'::jsonb \
             AND id > (SELECT COALESCE(max(id), 0) - 100 FROM outbox)",
        )
        .await;
    assert_eq!(told, 7, "one DriftChanged for each swimlane");

    // A pipeline that knows nothing computes every swimlane from scratch and finds nothing to change.
    let (fresh, _) = w
        .rig
        .pipeline(std::sync::Arc::new(w.git.clone()), RegistryConfig::default());
    let nightly = fresh.refresh_all().await.unwrap();
    assert_eq!(nightly.lanes, 7);
    assert_eq!(
        (nightly.rows_changed, nightly.repaired_paths),
        (0, 0),
        "{nightly:?}"
    );
}
