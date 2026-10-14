//! The adoption pass (T5, D2, D19, D20, D21, D22, S4, S13, rule 8, D80): from the ranking to the confirmation, over a real Postgres 16, the
//! real audit log and event bus, the real pipeline and the in-memory Git.
//!
//! What is held here: every call requires an active Admin (an Operator for the marks) and answers `ReadError::Forbidden` before it reads
//! anything; the pass persists and resumes, also across a new service; what a resolution decides about a baseline, a mark and the raw
//! version behind it; a confirmation is one transaction with exactly one `BaselineAdopted` event, the marks' events and the outbox
//! events, and a failure anywhere leaves nothing; a file that changed since it was reviewed is never baselined on the strength of the
//! review; the tenant-suffix review of T4 is worked with the same calls.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::Duration;

use domain::{ContentHash, DomainEvent};
use engine::drift_hash;
use hub_registry::metrics::{AdoptionAction, AdoptionOutcome};
use hub_registry::{
    AdoptionError, AdoptionReason, AdoptionStatus, AdoptionStep, Bulk, HintSource, Reason, Resolution,
    ResolutionKind,
};
use ports::{AuditVia, ReadError, content_hash};
use support::adoption::{
    AdoptionRig, audit_count, baseline, baseline_paths, intentional, lane, lane_flags, marks, nfs,
    not_admins, not_operators, tenant, tenants_of,
};
use support::world::{World, state_of, wait_until};

fn forbidden() -> AdoptionError {
    AdoptionError::Read(ReadError::Forbidden)
}

fn drift(text: &str) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(text.to_owned())),
        domain::FileClass::Structured,
    )
    .unwrap()
}

fn raw(text: &str) -> ContentHash {
    content_hash(text.as_bytes())
}

/// A swimlane that has its first inventory and no baseline. Git: a base file set (one of them only Git has), sit1 and sit2, and a template
/// that holds what sit2 holds. NFS: two files that equal Git, three that differ (one of them a tenant file), and one Git does not have.
async fn rig() -> Option<AdoptionRig> {
    let mut r = AdoptionRig::new(&[]).await?;
    r.w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/notes.yml", Some("n: 1\n")),
        ("config/app/other.yml", Some("o: 1\n")),
        ("config/app/gone.yml", Some("g: 1\n")),
    ]);
    r.w.tenant_commit(
        "sit1",
        &[
            ("data/config/app/core.yml", Some("t: 1\n")),
            ("data/config/app/extra.yml", Some("e: 1\n")),
        ],
    );
    r.w.tenant_commit(
        "sit2",
        &[
            ("data/config/app/core.yml", Some("t: 2\n")),
            ("data/config/app/extra.yml", Some("e: 2\n")),
        ],
    );
    r.w.tenant_commit(
        "templates/core",
        &[
            ("data/config/app/core.yml", Some("t: 1\n")),
            ("data/config/app/extra.yml", Some("e: 1\n")),
        ],
    );
    r.w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/notes.yml", "n: CHANGED\n"),
        ("app/other.yml", "o: 2\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/extra-sit1.yml", "e: LOCAL\n"),
        ("app/untracked.yml", "u: 1\n"),
    ])
    .await;
    Some(r)
}

/// The three resolutions of the scenario: notes (NFS is right), other (Git is right) and the tenant file (intentional).
async fn resolve_all_three(r: &AdoptionRig, id: i64) -> hub_registry::AdoptionView {
    r.resolve(
        id,
        &[
            ("app/notes.yml", Resolution::NfsIsRight),
            ("app/other.yml", Resolution::GitIsRight),
            ("app/extra-sit1.yml", intentional("the batch job rewrites it")),
        ],
    )
    .await
}

/// The pass up to the confirmation: ranked, branches and base chosen, resolved, scanned.
async fn ready(r: &AdoptionRig) -> i64 {
    let id = r.start().await.id;
    r.choose(id, &["sit1"]).await;
    resolve_all_three(r, id).await;
    r.svc.scan_secrets(&r.admin, id).await.unwrap();
    id
}

// ------------------------------------------------------------------------------------------------ S4

#[tokio::test]
async fn only_an_active_admin_can_run_or_confirm() {
    let Some(r) = rig().await else { return };
    let id = ready(&r).await;
    let path = nfs("app/notes.yml");
    let reason = Reason::parse("because").unwrap();
    for (who, u) in not_admins() {
        // Every method of the pass, with an id that exists, one that does not, and arguments that would otherwise be refused for
        // other reasons: the answer is the same for all of them, because the role is checked before anything is read.
        let call = |name: &str, got: Result<(), AdoptionError>| {
            assert_eq!(got, Err(forbidden()), "{name} as {who}");
        };
        call(
            "start",
            r.svc
                .start(&u, &lane(), AdoptionReason::Initial)
                .await
                .map(|_| ()),
        );
        call(
            "start review",
            r.svc
                .start(&u, &lane(), AdoptionReason::TenantSuffixChanged)
                .await
                .map(|_| ()),
        );
        call(
            "open",
            r.svc.open(&u, &lane(), AdoptionReason::Initial).await.map(|_| ()),
        );
        call("get", r.svc.get(&u, id).await.map(|_| ()));
        call("get unknown", r.svc.get(&u, 99_999).await.map(|_| ()));
        call(
            "choose_branches",
            r.svc.choose_branches(&u, id, &[tenant("sit1")]).await.map(|_| ()),
        );
        call(
            "choose_branches empty",
            r.svc.choose_branches(&u, id, &[]).await.map(|_| ()),
        );
        call(
            "choose_base_commit",
            r.svc.choose_base_commit(&u, id, None).await.map(|_| ()),
        );
        call(
            "differing_files",
            r.svc.differing_files(&u, id, None, 10).await.map(|_| ()),
        );
        call(
            "resolve",
            r.svc
                .resolve(&u, id, vec![(path.clone(), Resolution::NfsIsRight)])
                .await
                .map(|_| ()),
        );
        call(
            "resolve_all",
            r.svc
                .resolve_all(&u, id, Bulk::default(), Resolution::NfsIsRight)
                .await
                .map(|_| ()),
        );
        call("scan_secrets", r.svc.scan_secrets(&u, id).await.map(|_| ()));
        call("confirm", r.svc.confirm(&u, id).await.map(|_| ()));
        call("confirm unknown", r.svc.confirm(&u, 99_999).await.map(|_| ()));
        call("cancel", r.svc.cancel(&u, id).await);
        if who != "operator" {
            call(
                "mark_intentional",
                r.svc
                    .mark_intentional(&u, &lane(), &path, &reason, AuditVia::Ui)
                    .await,
            );
        }
    }
    assert!(
        r.w.rig
            .metrics
            .adoption_count(AdoptionAction::Confirm, AdoptionOutcome::Forbidden)
            >= 6,
        "every refusal is counted"
    );
    // Nothing happened: the pass is where the admin left it, and nothing was confirmed or cancelled.
    let view = r.svc.get(&r.admin, id).await.unwrap();
    assert_eq!(
        (view.status, view.step),
        (AdoptionStatus::InProgress, AdoptionStep::Confirm)
    );
    assert_eq!(audit_count(&r.w.rig.pool, "baseline_adopted").await, 0);
    assert!(baseline_paths(&r.w.rig.pool, "sit1").await.is_empty());
}

#[tokio::test]
async fn the_marks_need_an_operator_and_say_so_before_they_look() {
    let Some(r) = rig().await else { return };
    let path = nfs("app/notes.yml");
    let reason = Reason::parse("because").unwrap();
    for (who, u) in not_operators() {
        assert_eq!(
            r.svc
                .mark_intentional(&u, &lane(), &path, &reason, AuditVia::Ui)
                .await,
            Err(forbidden()),
            "mark as {who}"
        );
        assert_eq!(
            r.svc.clear_intentional(&u, &lane(), &path, AuditVia::Ui).await,
            Err(forbidden()),
            "clear as {who}"
        );
        // A swimlane and a path that do not exist: still Forbidden, not NotFound.
        assert_eq!(
            r.svc
                .mark_intentional(
                    &u,
                    &domain::SwimlaneId::parse("nowhere").unwrap(),
                    &path,
                    &reason,
                    AuditVia::Ui
                )
                .await,
            Err(forbidden()),
            "{who}"
        );
    }
    assert!(marks(&r.w.rig.pool, "sit1").await.is_empty());
    assert_eq!(audit_count(&r.w.rig.pool, "drift_marked_intentional").await, 0);
}

#[tokio::test]
async fn the_check_is_on_the_service_and_comes_before_any_database_call() {
    let Some(r) = rig().await else { return };
    // A service whose database is gone: an authorised call reaches the database and fails there, a refused one never gets so far.
    let dead = r.w.rig.db.app_pool().await;
    dead.close().await;
    let svc = hub_registry::AdoptionService::new(hub_registry::AdoptionDeps {
        pool: dead,
        git: std::sync::Arc::new(r.w.git.clone()),
        blobs: std::sync::Arc::clone(&r.w.rig.blobs) as _,
        bus: std::sync::Arc::clone(&r.w.rig.bus) as _,
        audit: std::sync::Arc::clone(&r.w.rig.audit) as _,
        clock: std::sync::Arc::clone(&r.w.rig.clock) as _,
        config: hub_registry::RegistryConfig::default(),
        metrics: r.w.rig.metrics.clone(),
        pipeline: None,
    })
    .unwrap();
    for (who, u) in not_admins() {
        assert_eq!(
            svc.start(&u, &lane(), AdoptionReason::Initial).await.map(|_| ()),
            Err(forbidden()),
            "{who}"
        );
        assert_eq!(svc.confirm(&u, 1).await.map(|_| ()), Err(forbidden()), "{who}");
    }
    let admitted = svc.start(&r.admin, &lane(), AdoptionReason::Initial).await;
    assert!(
        matches!(admitted, Err(AdoptionError::Registry(_))),
        "an admin is let in, and the dead database is what fails: {admitted:?}"
    );
}

// ------------------------------------------------------------------------------------------------ the whole pass

#[tokio::test]
async fn an_initial_pass_runs_from_ranking_to_confirmation() {
    let Some(r) = rig().await else { return };
    // The cluster says what it runs.
    sqlx::query(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) \
         VALUES ('sit1', 'csp', 'api', '{\"SPRING_PROFILES_ACTIVE\": \"sit1,default\"}')",
    )
    .execute(&r.w.rig.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO release_hints (swimlane_id, namespace, service, key, value) \
         VALUES ('sit1', 'csp', 'tenant-data', 'helm.sh/chart', 'csp-tenant-data-sit1-0.3.1')",
    )
    .execute(&r.w.rig.pool)
    .await
    .unwrap();

    // 1. The pass opens with the ranking and the hints.
    let started = r.start().await;
    let id = started.id;
    assert_eq!(
        (started.status, started.step),
        (AdoptionStatus::InProgress, AdoptionStep::Branch)
    );
    assert_eq!(started.reason, AdoptionReason::Initial);
    let ranked: Vec<(&str, u64, u64)> = started
        .ranking
        .iter()
        .map(|b| (b.branch.as_str(), b.shared, b.union))
        .collect();
    assert_eq!(
        ranked,
        vec![("sit1", 1, 3), ("sit2", 0, 4)],
        "the branch whose files NFS has comes first; the template that holds the same is never ranked (D22)"
    );
    let hints: Vec<(HintSource, &str, &str, &str)> = started
        .hints
        .iter()
        .map(|h| (h.source, h.service.as_str(), h.key.as_str(), h.tenant.as_str()))
        .collect();
    assert_eq!(
        hints,
        vec![
            (HintSource::Env, "csp/api", "SPRING_PROFILES_ACTIVE", "sit1"),
            (HintSource::Release, "csp/tenant-data", "helm.sh/chart", "sit1"),
        ]
    );
    assert!(started.tenants.is_empty() && started.base_commit.is_none());
    assert_eq!(
        audit_count(&r.w.rig.pool, "settings_changed").await,
        1,
        "the opening is audited"
    );

    // Calls out of order are refused, and refused calls change nothing.
    assert_eq!(
        r.svc
            .resolve(&r.admin, id, vec![(nfs("app/notes.yml"), Resolution::NfsIsRight)])
            .await
            .map(|_| ()),
        Err(AdoptionError::WrongStep)
    );
    assert_eq!(
        r.svc.choose_base_commit(&r.admin, id, None).await.map(|_| ()),
        Err(AdoptionError::WrongStep)
    );
    assert_eq!(
        r.svc.scan_secrets(&r.admin, id).await.map(|_| ()),
        Err(AdoptionError::WrongStep)
    );
    assert_eq!(
        r.svc.confirm(&r.admin, id).await.map(|_| ()),
        Err(AdoptionError::WrongStep)
    );
    assert_eq!(
        r.svc.differing_files(&r.admin, id, None, 10).await.map(|_| ()),
        Err(AdoptionError::WrongStep)
    );

    // 2. The branches. A branch the mirror does not have, a template and nothing at all are refused.
    assert_eq!(
        r.svc
            .choose_branches(&r.admin, id, &[tenant("sit9")])
            .await
            .map(|_| ()),
        Err(AdoptionError::NotDeployable)
    );
    assert_eq!(
        r.svc.choose_branches(&r.admin, id, &[]).await.map(|_| ()),
        Err(AdoptionError::Read(ReadError::BadRequest))
    );
    let chosen = r
        .svc
        .choose_branches(&r.admin, id, &[tenant("sit1")])
        .await
        .unwrap();
    assert_eq!(chosen.step, AdoptionStep::BaseVersion);
    assert_eq!(chosen.tenants, vec![tenant("sit1")]);
    let sit1_head = r.w.head(domain::RepoKind::Tenant, "sit1");
    assert_eq!(
        chosen.tenant_commits[&tenant("sit1")],
        sit1_head,
        "the head when the branch was chosen"
    );
    let base_head = r.w.head(domain::RepoKind::Base, "master");
    assert_eq!(
        chosen.base_commit,
        Some(base_head.clone()),
        "the one commit there is is the best match"
    );
    assert!(chosen.base_label.is_some());

    // 3. The base version, and the files that differ.
    let at_files = r.svc.choose_base_commit(&r.admin, id, None).await.unwrap();
    assert_eq!(at_files.step, AdoptionStep::Files, "three files differ");
    let page = r.svc.differing_files(&r.admin, id, None, 2).await.unwrap();
    assert_eq!(
        page.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["app/extra-sit1.yml", "app/notes.yml"]
    );
    assert_eq!(
        (
            page.total,
            page.resolved,
            page.unresolved,
            page.equal,
            page.missing_on_nfs
        ),
        (3, 0, 3, 2, 1),
        "core and its tenant copy equal Git; `gone.yml` is in Git and not on NFS"
    );
    assert_eq!(page.next, Some(nfs("app/notes.yml")));
    let rest = r
        .svc
        .differing_files(&r.admin, id, page.next.as_ref(), 2)
        .await
        .unwrap();
    assert_eq!(
        rest.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["app/other.yml"]
    );
    assert_eq!(rest.next, None);
    assert_eq!(page.files[1].nfs, drift("n: CHANGED\n"));
    assert_eq!(page.files[1].git, drift("n: 1\n"));

    // A path that does not differ is not recorded, and neither are the others of that call.
    assert_eq!(
        r.svc
            .resolve(
                &r.admin,
                id,
                vec![
                    (nfs("app/notes.yml"), Resolution::NfsIsRight),
                    (nfs("app/core.yml"), Resolution::GitIsRight),
                ],
            )
            .await
            .map(|_| ()),
        Err(AdoptionError::NotDiffering)
    );
    assert_eq!(
        r.w.rig.count("SELECT count(*) FROM adoption_resolutions").await,
        0
    );
    let half = r
        .resolve(
            id,
            &[
                ("app/notes.yml", Resolution::NfsIsRight),
                ("app/other.yml", Resolution::GitIsRight),
            ],
        )
        .await;
    assert_eq!((half.step, half.resolutions), (AdoptionStep::Files, 2));
    let done = r
        .resolve(
            id,
            &[("app/extra-sit1.yml", intentional("the batch job rewrites it"))],
        )
        .await;
    assert_eq!(
        done.step,
        AdoptionStep::Secrets,
        "the last resolution moves the pass on"
    );
    let all = r.svc.differing_files(&r.admin, id, None, 10).await.unwrap();
    assert_eq!((all.resolved, all.unresolved), (3, 0));
    assert_eq!(
        all.files
            .iter()
            .map(|f| (f.path.as_str(), f.resolution.as_ref().map(|x| x.kind), f.stale))
            .collect::<Vec<_>>(),
        vec![
            ("app/extra-sit1.yml", Some(ResolutionKind::Intentional), false),
            ("app/notes.yml", Some(ResolutionKind::NfsIsRight), false),
            ("app/other.yml", Some(ResolutionKind::GitIsRight), false),
        ]
    );
    assert_eq!(
        all.files[0].resolution.as_ref().and_then(|x| x.reason.as_deref()),
        Some("the batch job rewrites it")
    );

    // 4. The secret scan (nothing planted here), and the pass is at its last step.
    let scanned = r.svc.scan_secrets(&r.admin, id).await.unwrap();
    assert_eq!(scanned.step, AdoptionStep::Confirm);
    assert!(scanned.secret_scan.is_empty());

    // 5. The confirmation.
    let outcome = r.svc.confirm(&r.admin, id).await.unwrap();
    assert_eq!(
        (
            outcome.baselines,
            outcome.equal,
            outcome.nfs_is_right,
            outcome.git_is_right,
            outcome.intentional
        ),
        (5, 2, 1, 1, 1)
    );
    assert_eq!(outcome.tenants, vec![tenant("sit1")]);
    assert!(outcome.tenants_changed && outcome.editing_unlocked);
    assert_eq!(outcome.reason, AdoptionReason::Initial);

    let pool = &r.w.rig.pool;
    let admin_oid = Some(r.admin.id.oid().as_str().to_owned());
    // Equal to Git: B = N = G, the raw version is NFS's. The tenant file's baseline names the tenant branch and its commit.
    let core = baseline(pool, "sit1", "app/core.yml").await.unwrap();
    assert_eq!(
        core,
        (
            drift("a: 1\n"),
            "base".to_owned(),
            None,
            base_head.as_str().to_owned(),
            Some(raw("a: 1\n")),
            admin_oid.clone()
        )
    );
    let tenant_file = baseline(pool, "sit1", "app/core-sit1.yml").await.unwrap();
    assert_eq!(
        tenant_file,
        (
            drift("t: 1\n"),
            "tenant".to_owned(),
            Some("sit1".to_owned()),
            sit1_head.as_str().to_owned(),
            Some(raw("t: 1\n")),
            admin_oid.clone()
        )
    );
    // NFS is right: B is Git's, and Git's raw version is not something the registry holds, so none is named.
    let notes = baseline(pool, "sit1", "app/notes.yml").await.unwrap();
    assert_eq!((notes.0, notes.4), (drift("n: 1\n"), None));
    // Git is right: B is NFS's, with NFS's raw version.
    let other = baseline(pool, "sit1", "app/other.yml").await.unwrap();
    assert_eq!((other.0, other.4), (drift("o: 2\n"), Some(raw("o: 2\n"))));
    // Intentional: B is Git's.
    let extra = baseline(pool, "sit1", "app/extra-sit1.yml").await.unwrap();
    assert_eq!((extra.0, extra.4), (drift("e: 1\n"), None));
    assert_eq!(
        baseline_paths(pool, "sit1").await,
        vec![
            "app/core-sit1.yml",
            "app/core.yml",
            "app/extra-sit1.yml",
            "app/notes.yml",
            "app/other.yml"
        ],
        "no baseline for the file Git does not have, nor for the one only Git has"
    );
    // The raw versions that baselines stand on are referenced, and only those.
    let refs: Vec<String> = sqlx::query_scalar(
        "SELECT ref_id FROM blob_refs WHERE ref_kind = 'baseline' ORDER BY ref_id COLLATE \"C\"",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        refs,
        vec![
            "sit1:app/core-sit1.yml",
            "sit1:app/core.yml",
            "sit1:app/other.yml"
        ]
    );

    // The mark of the intentional one, with N and G and the reason.
    assert_eq!(
        marks(pool, "sit1").await,
        vec![(
            "app/extra-sit1.yml".to_owned(),
            Some(drift("e: LOCAL\n")),
            Some(drift("e: 1\n")),
            Some("the batch job rewrites it".to_owned()),
            r.admin.id.oid().as_str().to_owned(),
        )]
    );
    // The swimlane: editing unlocked, the baseline confirmed by this admin, the tenant set the chosen one.
    assert_eq!(lane_flags(pool, "sit1").await, (true, true, admin_oid.clone()));
    assert_eq!(tenants_of(pool, "sit1").await, vec!["sit1".to_owned()]);
    let closed = r.svc.get(&r.admin, id).await.unwrap();
    assert_eq!(closed.status, AdoptionStatus::Confirmed);

    // Rule 8: exactly one BaselineAdopted for the confirmation, one DriftMarkedIntentional for the mark, and the tenant set's change.
    assert_eq!(audit_count(pool, "baseline_adopted").await, 1);
    assert_eq!(audit_count(pool, "drift_marked_intentional").await, 1);
    assert_eq!(
        audit_count(pool, "settings_changed").await,
        2,
        "the pass opened, and the tenant set changed"
    );
    let (actor_kind, actor_ref, via): (String, Option<String>, String) = sqlx::query_as(
        "SELECT actor_kind, actor_ref, via FROM audit_events WHERE action = 'baseline_adopted'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!((actor_kind.as_str(), via.as_str()), ("user", "ui"));
    assert!(
        actor_ref.is_some_and(|a| a.contains(r.admin.id.oid().as_str())),
        "the admin is the actor"
    );
    let diff: String = sqlx::query_scalar(
        "SELECT event_json::jsonb ->> 'diff' FROM audit_events WHERE action = 'baseline_adopted'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(
        diff.contains("baselines: 5 (equal 2, nfs_is_right 1, git_is_right 1, intentional 1)"),
        "{diff}"
    );
    assert!(
        diff.contains("tenants: sit1") && diff.contains("intentional:\napp/extra-sit1.yml\n"),
        "{diff}"
    );

    // D80: the events are in the outbox, from the transaction.
    assert_eq!(
        r.w.drift_events().await,
        vec![vec![
            "app/core-sit1.yml".to_owned(),
            "app/core.yml".to_owned(),
            "app/extra-sit1.yml".to_owned(),
            "app/notes.yml".to_owned(),
            "app/other.yml".to_owned()
        ]]
    );
    assert_eq!(r.w.findings_changed_events().await, 1);
    assert_eq!(r.w.rig.metrics.adoption_baselines_count(), 5);
    assert_eq!(
        r.w.rig
            .metrics
            .adoption_count(AdoptionAction::Confirm, AdoptionOutcome::Ok),
        1
    );

    // What the pipeline makes of it: the states each resolution means.
    r.w.whole().await;
    for (path, state) in [
        ("app/core.yml", "in_sync"),
        ("app/core-sit1.yml", "in_sync"),
        ("app/notes.yml", "nfs_ahead"),
        ("app/other.yml", "git_ahead"),
        ("app/extra-sit1.yml", "intentional_divergence"),
        ("app/untracked.yml", "untracked"),
        ("app/gone.yml", "unknown"),
    ] {
        assert_eq!(state_of(&r.w, path).await.as_deref(), Some(state), "{path}");
    }

    // A closed pass cannot be confirmed again, and an adopted swimlane has no first pass to open.
    assert_eq!(
        r.svc.confirm(&r.admin, id).await.map(|_| ()),
        Err(AdoptionError::NotOpen)
    );
    assert_eq!(
        r.svc
            .start(&r.admin, &lane(), AdoptionReason::Initial)
            .await
            .map(|_| ()),
        Err(AdoptionError::AlreadyAdopted)
    );
    assert_eq!(
        r.svc
            .start(&r.admin, &lane(), AdoptionReason::TenantSuffixChanged)
            .await
            .map(|_| ()),
        Err(AdoptionError::NotOpen),
        "nothing to review"
    );
}

#[tokio::test]
async fn a_pass_with_nothing_that_differs_goes_straight_to_the_secret_scan() {
    let Some(mut r) = AdoptionRig::new(&[]).await else {
        return;
    };
    r.w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    r.w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("t: 1\n"))]);
    r.w.inventory(&[("app/core.yml", "a: 1\n"), ("app/core-sit1.yml", "t: 1\n")])
        .await;
    let id = r.start().await.id;
    let view = r.choose(id, &["sit1"]).await;
    assert_eq!(view.step, AdoptionStep::Secrets);
    assert_eq!(
        r.svc.scan_secrets(&r.admin, id).await.unwrap().step,
        AdoptionStep::Confirm
    );
    let outcome = r.svc.confirm(&r.admin, id).await.unwrap();
    assert_eq!((outcome.baselines, outcome.equal, outcome.intentional), (2, 2, 0));
    assert_eq!(audit_count(&r.w.rig.pool, "drift_marked_intentional").await, 0);
    assert!(marks(&r.w.rig.pool, "sit1").await.is_empty());
}

#[tokio::test]
async fn a_bulk_action_resolves_a_folder_or_what_is_left() {
    let Some(r) = rig().await else { return };
    let id = r.start().await.id;
    r.choose(id, &["sit1"]).await;
    // Everything under `app` that still needs a decision: all three, in one call.
    let by_folder = r
        .resolve_all(
            id,
            Bulk {
                prefix: Some(nfs("app")),
                unresolved_only: true,
            },
            Resolution::GitIsRight,
        )
        .await;
    assert_eq!(
        (by_folder.resolutions, by_folder.step),
        (3, AdoptionStep::Secrets)
    );
    // A folder that has none of them reaches none; a prefix is whole names, so `ap` is not `app`.
    let none = r
        .resolve_all(
            id,
            Bulk {
                prefix: Some(nfs("ap")),
                unresolved_only: false,
            },
            Resolution::NfsIsRight,
        )
        .await;
    assert_eq!(none.resolutions, 3);
    let kinds = |page: &hub_registry::DifferingFiles| -> Vec<ResolutionKind> {
        page.files
            .iter()
            .filter_map(|f| f.resolution.as_ref().map(|x| x.kind))
            .collect()
    };
    assert_eq!(
        kinds(&r.svc.differing_files(&r.admin, id, None, 10).await.unwrap()),
        vec![ResolutionKind::GitIsRight; 3],
        "nothing was changed by the call that reached nothing"
    );
    // Only the ones without a decision: the one file changed by hand keeps its own.
    r.resolve(id, &[("app/notes.yml", Resolution::NfsIsRight)]).await;
    r.resolve_all(
        id,
        Bulk {
            prefix: None,
            unresolved_only: true,
        },
        Resolution::GitIsRight,
    )
    .await;
    let page = r.svc.differing_files(&r.admin, id, None, 10).await.unwrap();
    assert_eq!(
        page.files
            .iter()
            .map(|f| (f.path.as_str(), f.resolution.as_ref().map(|x| x.kind)))
            .collect::<Vec<_>>(),
        vec![
            ("app/extra-sit1.yml", Some(ResolutionKind::GitIsRight)),
            ("app/notes.yml", Some(ResolutionKind::NfsIsRight)),
            ("app/other.yml", Some(ResolutionKind::GitIsRight)),
        ]
    );
    // Over everything, whatever was decided.
    r.resolve_all(id, Bulk::default(), intentional("all of it, on purpose"))
        .await;
    assert_eq!(
        kinds(&r.svc.differing_files(&r.admin, id, None, 10).await.unwrap()),
        vec![ResolutionKind::Intentional; 3]
    );
    // An intentional resolution needs a reason, and one is checked before anything is written.
    assert!(matches!(Reason::parse("  "), Err(AdoptionError::InvalidReason)));
}

// ------------------------------------------------------------------------------------------------ persistence

#[tokio::test]
async fn progress_persists_and_resumes_after_a_restart() {
    let Some(r) = rig().await else { return };
    let id = r.start().await.id;
    r.choose(id, &["sit1"]).await;
    let before = r
        .resolve(
            id,
            &[
                ("app/notes.yml", Resolution::NfsIsRight),
                ("app/extra-sit1.yml", intentional("on purpose")),
            ],
        )
        .await;
    assert_eq!((before.step, before.resolutions), (AdoptionStep::Files, 2));

    // A restart: a service that has never seen the pass, over the same database.
    let again = AdoptionRig::service_of(&r.w, hub_registry::RegistryConfig::default());
    let found = again
        .open(&r.admin, &lane(), AdoptionReason::Initial)
        .await
        .unwrap()
        .expect("the open pass is found");
    assert_eq!(found, before, "the same pass, as it was left");
    assert_eq!(found.tenants, vec![tenant("sit1")]);
    assert!(found.base_commit.is_some() && !found.ranking.is_empty());
    // Starting again resumes it: no second pass, no second audit event for the opening.
    let resumed = again
        .start(&r.admin, &lane(), AdoptionReason::Initial)
        .await
        .unwrap();
    assert_eq!(resumed.id, id);
    assert_eq!(r.w.rig.count("SELECT count(*) FROM adoptions").await, 1);
    assert_eq!(audit_count(&r.w.rig.pool, "settings_changed").await, 1);
    // The files show the decisions that were made, and the rest of the pass is done on the new service.
    let page = again.differing_files(&r.admin, id, None, 10).await.unwrap();
    assert_eq!((page.resolved, page.unresolved), (2, 1));
    again
        .resolve(&r.admin, id, vec![(nfs("app/other.yml"), Resolution::GitIsRight)])
        .await
        .unwrap();
    again.scan_secrets(&r.admin, id).await.unwrap();
    let outcome = again.confirm(&r.admin, id).await.unwrap();
    assert_eq!(outcome.baselines, 5);
    assert_eq!(audit_count(&r.w.rig.pool, "baseline_adopted").await, 1);
}

#[tokio::test]
async fn choosing_again_forgets_what_was_decided_about_other_commits_and_cancelling_closes_the_pass() {
    let Some(r) = rig().await else { return };
    let id = r.start().await.id;
    r.choose(id, &["sit1"]).await;
    r.resolve(id, &[("app/notes.yml", Resolution::NfsIsRight)]).await;
    assert_eq!(
        r.w.rig.count("SELECT count(*) FROM adoption_resolutions").await,
        1
    );
    // Another branch: what was decided was about sit1's commit.
    let view = r
        .svc
        .choose_branches(&r.admin, id, &[tenant("sit2")])
        .await
        .unwrap();
    assert_eq!((view.step, view.resolutions), (AdoptionStep::BaseVersion, 0));
    assert_eq!(view.tenants, vec![tenant("sit2")]);

    r.svc.cancel(&r.admin, id).await.unwrap();
    assert_eq!(
        r.svc.get(&r.admin, id).await.unwrap().status,
        AdoptionStatus::Cancelled
    );
    assert_eq!(
        audit_count(&r.w.rig.pool, "settings_changed").await,
        2,
        "opened and cancelled"
    );
    assert_eq!(r.svc.cancel(&r.admin, id).await, Err(AdoptionError::NotOpen));
    for result in [
        r.svc
            .choose_branches(&r.admin, id, &[tenant("sit1")])
            .await
            .map(|_| ()),
        r.svc.scan_secrets(&r.admin, id).await.map(|_| ()),
        r.svc.confirm(&r.admin, id).await.map(|_| ()),
        r.svc.resolve(&r.admin, id, Vec::new()).await.map(|_| ()),
    ] {
        assert_eq!(result, Err(AdoptionError::NotOpen));
    }
    // A cancelled pass does not stand in the way of a new one.
    let next = r.start().await;
    assert_ne!(next.id, id);
    assert!(baseline_paths(&r.w.rig.pool, "sit1").await.is_empty());
}

// ------------------------------------------------------------------------------------------------ what was reviewed is what is written

#[tokio::test]
async fn a_file_that_changed_since_it_was_reviewed_needs_a_new_decision() {
    let Some(mut r) = rig().await else { return };
    let id = ready(&r).await;
    // Somebody edits a file after the admin resolved it.
    r.w.nfs_change(&[("app/notes.yml", "n: CHANGED AGAIN\n")]).await;
    let page = r.svc.differing_files(&r.admin, id, None, 10).await.unwrap();
    let notes = page
        .files
        .iter()
        .find(|f| f.path.as_str() == "app/notes.yml")
        .unwrap();
    assert!(notes.stale && notes.resolution.is_none(), "{notes:?}");
    assert_eq!(notes.nfs, drift("n: CHANGED AGAIN\n"));
    assert_eq!((page.resolved, page.unresolved), (2, 1));

    // The confirmation refuses it with the count, writes nothing, and sends the pass back to the files.
    assert_eq!(
        r.svc.confirm(&r.admin, id).await.map(|_| ()),
        Err(AdoptionError::Unresolved(1))
    );
    assert!(baseline_paths(&r.w.rig.pool, "sit1").await.is_empty());
    assert_eq!(audit_count(&r.w.rig.pool, "baseline_adopted").await, 0);
    let view = r.svc.get(&r.admin, id).await.unwrap();
    assert_eq!(
        (view.status, view.step),
        (AdoptionStatus::InProgress, AdoptionStep::Files)
    );
    assert_eq!(lane_flags(&r.w.rig.pool, "sit1").await, (false, false, None));

    // The new decision, the scan again, and then it goes through, with the version that was decided about.
    r.resolve(id, &[("app/notes.yml", Resolution::GitIsRight)]).await;
    r.svc.scan_secrets(&r.admin, id).await.unwrap();
    r.svc.confirm(&r.admin, id).await.unwrap();
    let notes = baseline(&r.w.rig.pool, "sit1", "app/notes.yml").await.unwrap();
    assert_eq!(
        notes.0,
        drift("n: CHANGED AGAIN\n"),
        "Git is right: B is the NFS version that was reviewed"
    );
}

/// Hold the lock that every apply of a delta takes in an open transaction, as a long ingestion would, and wait until a confirmation is
/// queued behind it. What the holder then does inside its transaction is what the confirmation finds when it gets the lock.
async fn hold_the_lock(r: &AdoptionRig) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = r.w.rig.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('lanekeeper.registry.apply.' || 'sit1', 0))")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

async fn waiting_for_the_lock(r: &AdoptionRig, waiters: i64) {
    let pool = r.w.rig.pool.clone();
    wait_until(
        "the confirmation to wait for the lock",
        Duration::from_secs(30),
        || {
            let pool = pool.clone();
            async move {
                let n: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted",
                )
                .fetch_one(&pool)
                .await
                .unwrap();
                n >= waiters
            }
        },
    )
    .await;
}

#[tokio::test]
async fn a_file_that_changes_while_the_confirmation_waits_for_the_lock_aborts_it_whole() {
    let Some(r) = rig().await else { return };
    let id = ready(&r).await;
    let events_before = r.w.rig.bus.count().await;
    let held = hold_the_lock(&r).await;
    let svc = r.svc.clone();
    let admin = r.admin.clone();
    let confirming = tokio::spawn(async move { svc.confirm(&admin, id).await });
    waiting_for_the_lock(&r, 1).await;
    // An apply of a delta (which holds this lock) changes a file that the admin reviewed and commits.
    let mut held = held;
    sqlx::query("UPDATE file_state SET nfs_drift_hash = sha256('somebody edited it'::bytea) WHERE swimlane_id = 'sit1' AND path = 'app/other.yml'")
        .execute(&mut *held)
        .await
        .unwrap();
    held.commit().await.unwrap();

    let result = confirming.await.unwrap();
    assert_eq!(result.map(|_| ()), Err(AdoptionError::Changed));
    // Nothing was written, and the pass is as it was: still waiting for a confirmation.
    assert!(baseline_paths(&r.w.rig.pool, "sit1").await.is_empty());
    assert!(marks(&r.w.rig.pool, "sit1").await.is_empty());
    assert_eq!(lane_flags(&r.w.rig.pool, "sit1").await, (false, false, None));
    assert!(tenants_of(&r.w.rig.pool, "sit1").await.is_empty());
    assert_eq!(audit_count(&r.w.rig.pool, "baseline_adopted").await, 0);
    assert_eq!(
        r.w.rig.bus.count().await,
        events_before,
        "and no event reached the outbox"
    );
    let view = r.svc.get(&r.admin, id).await.unwrap();
    assert_eq!(
        (view.status, view.step),
        (AdoptionStatus::InProgress, AdoptionStep::Confirm)
    );
}

#[tokio::test]
async fn a_decision_changed_while_the_confirmation_waits_is_not_applied_unseen() {
    let Some(r) = rig().await else { return };
    let id = ready(&r).await;
    let held = hold_the_lock(&r).await;
    let (svc, admin) = (r.svc.clone(), r.admin.clone());
    let confirming = tokio::spawn(async move { svc.confirm(&admin, id).await });
    waiting_for_the_lock(&r, 1).await;
    // Another admin changes a decision (recording one does not take the swimlane's lock) while the confirmation is queued behind it.
    r.resolve(id, &[("app/notes.yml", Resolution::GitIsRight)]).await;
    held.rollback().await.unwrap();
    assert_eq!(confirming.await.unwrap().map(|_| ()), Err(AdoptionError::Changed));
    assert!(
        baseline_paths(&r.w.rig.pool, "sit1").await.is_empty(),
        "the old decision was not written"
    );
    let view = r.svc.get(&r.admin, id).await.unwrap();
    assert_eq!(
        (view.status, view.step),
        (AdoptionStatus::InProgress, AdoptionStep::Confirm)
    );

    // Confirmed again, it is the new decision that is written: Git is right, so B is what NFS has.
    r.svc.confirm(&r.admin, id).await.unwrap();
    let notes = baseline(&r.w.rig.pool, "sit1", "app/notes.yml").await.unwrap();
    assert_eq!(notes.0, drift("n: CHANGED\n"));
}

#[tokio::test]
async fn of_two_confirmations_of_one_pass_exactly_one_commits() {
    let Some(r) = rig().await else { return };
    let id = ready(&r).await;
    let held = hold_the_lock(&r).await;
    let spawn = || {
        let (svc, admin) = (r.svc.clone(), r.admin.clone());
        tokio::spawn(async move { svc.confirm(&admin, id).await })
    };
    let (first, second) = (spawn(), spawn());
    waiting_for_the_lock(&r, 2).await;
    held.rollback().await.unwrap();
    let results = [first.await.unwrap(), second.await.unwrap()];
    let committed = results.iter().filter(|x| x.is_ok()).count();
    let closed = results
        .iter()
        .filter(|x| matches!(x, Err(AdoptionError::NotOpen)))
        .count();
    assert_eq!((committed, closed), (1, 1), "{results:?}");
    assert_eq!(
        audit_count(&r.w.rig.pool, "baseline_adopted").await,
        1,
        "one confirmation, one event"
    );
    assert_eq!(baseline_paths(&r.w.rig.pool, "sit1").await.len(), 5);
}

// ------------------------------------------------------------------------------------------------ rule 8, D80: one transaction

#[tokio::test]
async fn a_failure_anywhere_in_the_confirmation_leaves_nothing_behind() {
    let Some(r) = rig().await else { return };
    let id = ready(&r).await;
    let events_before = r.w.rig.bus.count().await;
    let audit_before = r.w.rig.count("SELECT count(*) FROM audit_events").await;

    // The audit log fails: it is the last write, after the baselines, the marks, the tenants, the flags and the outbox events.
    r.w.rig.audit.fail_after(Some(0));
    assert!(matches!(
        r.svc.confirm(&r.admin, id).await,
        Err(AdoptionError::Registry(_))
    ));
    r.w.rig.audit.fail_after(None);
    // The outbox fails: the events come before the audit events.
    r.w.rig.bus.set_failing(true);
    assert!(matches!(
        r.svc.confirm(&r.admin, id).await,
        Err(AdoptionError::Registry(_))
    ));
    r.w.rig.bus.set_failing(false);

    let pool = &r.w.rig.pool;
    assert!(baseline_paths(pool, "sit1").await.is_empty(), "no baseline");
    assert!(marks(pool, "sit1").await.is_empty(), "no mark");
    assert!(tenants_of(pool, "sit1").await.is_empty(), "no tenant");
    assert_eq!(
        lane_flags(pool, "sit1").await,
        (false, false, None),
        "editing is still locked"
    );
    assert_eq!(
        r.w.rig.bus.count().await,
        events_before,
        "no event of the confirmation reached the outbox"
    );
    assert_eq!(
        r.w.rig.count("SELECT count(*) FROM audit_events").await,
        audit_before,
        "and no audit event"
    );
    assert_eq!(
        r.w.rig
            .count("SELECT count(*) FROM blob_refs WHERE ref_kind = 'baseline'")
            .await,
        0
    );
    let view = r.svc.get(&r.admin, id).await.unwrap();
    assert_eq!(
        (view.status, view.step),
        (AdoptionStatus::InProgress, AdoptionStep::Confirm),
        "the pass is still open"
    );

    // The same call, once the failure is gone, goes through, and writes what a confirmation writes, once.
    r.svc.confirm(&r.admin, id).await.unwrap();
    assert_eq!(audit_count(pool, "baseline_adopted").await, 1);
    assert_eq!(baseline_paths(pool, "sit1").await.len(), 5);
    assert_eq!(r.w.drift_events().await.len(), 1);
}

// ------------------------------------------------------------------------------------------------ the tenant-suffix review of T4

/// An adopted swimlane `sit1` that deploys sit1 and has been redeployed from sit2: the pipeline raises a review (T4).
async fn review_world() -> Option<AdoptionRig> {
    let mut w = World::new(&["sit1"]).await?;
    w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    for (branch, n) in [("sit1", 1), ("sit2", 2), ("sit3", 3)] {
        w.tenant_commit(
            branch,
            &[
                ("data/config/app/core.yml", Some(&format!("t: {n}\n"))),
                ("data/config/app/extra.yml", Some(&format!("e: {n}\n"))),
            ],
        );
    }
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/extra-sit1.yml", "e: 1\n"),
    ])
    .await;
    w.adopt().await;
    for (path, text) in [
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/extra-sit1.yml", "e: 1\n"),
    ] {
        w.baseline(path, drift(text)).await;
    }
    w.whole().await;
    // The sync copied sit2's files and cleaned sit1's.
    w.nfs_delete(&["app/core-sit1.yml", "app/extra-sit1.yml"]).await;
    w.nfs_change(&[("app/core-sit2.yml", "t: 2\n"), ("app/extra-sit2.yml", "e: 2\n")])
        .await;
    w.paths(&[
        "app/core-sit1.yml",
        "app/extra-sit1.yml",
        "app/core-sit2.yml",
        "app/extra-sit2.yml",
    ])
    .await;
    AdoptionRig::over(w).await
}

#[tokio::test]
async fn a_review_the_registry_raised_is_started_worked_and_confirmed_like_a_first_pass() {
    let Some(mut r) = review_world().await else { return };
    let pool = &r.w.rig.pool.clone();
    let (_, _, confirmed_by_before) = lane_flags(pool, "sit1").await;
    let confirmed_at_before: Option<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM baseline_confirmed_at) * 1000)::bigint FROM swimlanes WHERE id = 'sit1'",
    )
    .fetch_one(pool)
    .await
    .unwrap();

    // What T4 wrote: a review waiting for an admin, with its ranking and the suffixes.
    let waiting = r
        .svc
        .open(&r.admin, &lane(), AdoptionReason::TenantSuffixChanged)
        .await
        .unwrap()
        .expect("the registry raised a review");
    assert_eq!(
        (waiting.status, waiting.step, waiting.reason),
        (
            AdoptionStatus::Review,
            AdoptionStep::Branch,
            AdoptionReason::TenantSuffixChanged
        )
    );
    assert_eq!(waiting.foreign_suffixes, vec![tenant("sit2")]);
    assert_eq!(
        waiting
            .ranking
            .iter()
            .map(|b| b.branch.as_str())
            .collect::<Vec<_>>(),
        vec!["sit2", "sit1", "sit3"]
    );
    // Working a review before it is started is refused: it belongs to nobody yet.
    assert_eq!(
        r.svc
            .choose_branches(&r.admin, waiting.id, &[tenant("sit2")])
            .await
            .map(|_| ()),
        Err(AdoptionError::NotOpen)
    );

    // Starting it takes it: from now on the registry leaves it alone.
    let id = waiting.id;
    let taken = r
        .svc
        .start(&r.admin, &lane(), AdoptionReason::TenantSuffixChanged)
        .await
        .unwrap();
    assert_eq!((taken.id, taken.status), (id, AdoptionStatus::InProgress));
    assert_eq!(
        taken.foreign_suffixes,
        vec![tenant("sit2")],
        "the review's own facts stay"
    );
    assert_eq!(
        r.w.rig.count("SELECT count(*) FROM adoptions").await,
        1,
        "the same row, no second pass"
    );
    // Another suffix shows up while the admin works: the registry does not change the row under them.
    r.w.nfs_change(&[("app/core-sit3.yml", "t: 3\n")]).await;
    r.w.paths(&["app/core-sit3.yml"]).await;
    assert_eq!(
        r.svc.get(&r.admin, id).await.unwrap().foreign_suffixes,
        vec![tenant("sit2")],
        "an admin's pass is not refreshed"
    );
    r.w.nfs_delete(&["app/core-sit3.yml"]).await;
    r.w.paths(&["app/core-sit3.yml"]).await;

    // The same steps: the branch the swimlane is deployed from now, and nothing that differs, so on to the secret scan.
    let chosen = r
        .svc
        .choose_branches(&r.admin, id, &[tenant("sit2")])
        .await
        .unwrap();
    assert_eq!(chosen.step, AdoptionStep::BaseVersion);
    let at = r.svc.choose_base_commit(&r.admin, id, None).await.unwrap();
    assert_eq!(at.step, AdoptionStep::Secrets, "NFS equals sit2");
    r.svc.scan_secrets(&r.admin, id).await.unwrap();
    let outcome = r.svc.confirm(&r.admin, id).await.unwrap();
    assert_eq!(outcome.reason, AdoptionReason::TenantSuffixChanged);
    assert_eq!(outcome.tenants, vec![tenant("sit2")]);
    assert!(outcome.tenants_changed);
    assert!(
        !outcome.editing_unlocked,
        "the swimlane was adopted already: its baseline confirmation is left as it was"
    );
    assert_eq!(outcome.baselines, 3, "core, and sit2's two files");

    let (unlocked, confirmed, confirmed_by) = lane_flags(pool, "sit1").await;
    assert!(unlocked && confirmed);
    assert_eq!(confirmed_by, confirmed_by_before);
    let confirmed_at_after: Option<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM baseline_confirmed_at) * 1000)::bigint FROM swimlanes WHERE id = 'sit1'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        confirmed_at_after, confirmed_at_before,
        "the time of the first confirmation is not moved"
    );
    assert_eq!(tenants_of(pool, "sit1").await, vec!["sit2".to_owned()]);
    for path in ["app/core-sit2.yml", "app/extra-sit2.yml"] {
        let row = baseline(pool, "sit1", path).await.unwrap();
        assert_eq!(
            (row.1.as_str(), row.2.as_deref()),
            ("tenant", Some("sit2")),
            "{path}"
        );
    }
    assert_eq!(audit_count(pool, "baseline_adopted").await, 1);
    assert_eq!(
        r.svc.get(&r.admin, id).await.unwrap().status,
        AdoptionStatus::Confirmed
    );

    // The registry's side again: the suffixes are the swimlane's now, so there is no finding for them, and a confirmed review is not
    // reopened for the same suffixes.
    r.w.whole().await;
    assert_eq!(
        r.w.tables()
            .await
            .findings
            .iter()
            .filter(|f| f.0 == "orphan_file" && f.2.as_deref().is_some_and(|p| p.contains("sit2")))
            .count(),
        0
    );
    assert_eq!(
        r.w.rig
            .count("SELECT count(*) FROM adoptions WHERE reason = 'tenant_suffix_changed'")
            .await,
        1
    );
    assert_eq!(
        r.svc
            .start(&r.admin, &lane(), AdoptionReason::TenantSuffixChanged)
            .await
            .map(|_| ()),
        Err(AdoptionError::NotOpen)
    );
}

#[tokio::test]
async fn a_pass_for_the_wrong_reason_is_refused() {
    let Some(r) = rig().await else { return };
    // The review of a swimlane nobody has adopted.
    assert_eq!(
        r.svc
            .start(&r.admin, &lane(), AdoptionReason::TenantSuffixChanged)
            .await
            .map(|_| ()),
        Err(AdoptionError::NotAdopted)
    );
    // A swimlane the hub does not know.
    assert_eq!(
        r.svc
            .start(
                &r.admin,
                &domain::SwimlaneId::parse("nowhere").unwrap(),
                AdoptionReason::Initial
            )
            .await
            .map(|_| ()),
        Err(AdoptionError::Read(ReadError::NotFound))
    );
    // A swimlane that has no inventory yet has nothing to rank.
    let Some(empty) = AdoptionRig::new(&[]).await else {
        return;
    };
    assert_eq!(
        empty
            .svc
            .start(&empty.admin, &lane(), AdoptionReason::Initial)
            .await
            .map(|_| ()),
        Err(AdoptionError::NoInventory)
    );
}

// ------------------------------------------------------------------------------------------------ the marks

/// An adopted swimlane where `app/core.yml` is `nfs_ahead` (NFS has a change Git does not), `app/same.yml` is in sync, and a file only Git
/// has is `unknown`.
async fn marks_world() -> Option<AdoptionRig> {
    let mut w = World::new(&["sit1"]).await?;
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/same.yml", Some("s: 1\n")),
        ("config/app/only-git.yml", Some("g: 1\n")),
    ]);
    w.inventory(&[
        ("app/core.yml", "a: 2\n"),
        ("app/same.yml", "s: 1\n"),
        ("app/untracked.yml", "u: 1\n"),
    ])
    .await;
    w.adopt().await;
    w.baseline("app/core.yml", drift("a: 1\n")).await;
    w.baseline("app/same.yml", drift("s: 1\n")).await;
    w.whole().await;
    AdoptionRig::over(w).await
}

#[tokio::test]
async fn a_divergence_is_marked_intentional_with_one_audit_event_and_cleared_again() {
    let Some(r) = marks_world().await else { return };
    let pool = &r.w.rig.pool;
    assert_eq!(state_of(&r.w, "app/core.yml").await.as_deref(), Some("nfs_ahead"));
    let reason = Reason::parse("the batch job rewrites it every night").unwrap();
    let signals =
        r.w.rig
            .metrics
            .pipeline_signal_count(hub_registry::metrics::SignalResult::Accepted);
    let events = r.w.rig.bus.count().await;

    // An operator marks it (an admin could, a viewer and an editor cannot: see the test of the roles).
    r.svc
        .mark_intentional(&r.operator, &lane(), &nfs("app/core.yml"), &reason, AuditVia::Ui)
        .await
        .unwrap();
    assert_eq!(
        marks(pool, "sit1").await,
        vec![(
            "app/core.yml".to_owned(),
            Some(drift("a: 2\n")),
            Some(drift("a: 1\n")),
            Some("the batch job rewrites it every night".to_owned()),
            r.operator.id.oid().as_str().to_owned(),
        )],
        "N and G as they are now"
    );
    // One audit event, by the operator, for the file, with both hashes.
    let rows = support::pg_audit::audit_rows(pool).await;
    let marked: Vec<_> = rows
        .iter()
        .filter(|a| a.action == "drift_marked_intentional")
        .collect();
    assert_eq!(marked.len(), 1);
    assert_eq!(
        (
            marked[0].path.as_deref(),
            marked[0].hash_before,
            marked[0].hash_after,
            marked[0].via.as_str()
        ),
        (
            Some("app/core.yml"),
            Some(drift("a: 2\n")),
            Some(drift("a: 1\n")),
            "ui"
        )
    );
    // One outbox event, from the transaction, and the pipeline was told.
    assert_eq!(r.w.rig.bus.count().await, events + 1);
    assert!(matches!(
        r.w.rig.bus.published().await.last(),
        Some(DomainEvent::DriftChanged { paths, .. }) if paths.len() == 1 && paths[0].as_str() == "app/core.yml"
    ));
    assert_eq!(
        r.w.rig
            .metrics
            .pipeline_signal_count(hub_registry::metrics::SignalResult::Accepted),
        signals + 1
    );
    // The pipeline reads it: the file is `intentional_divergence`, and not a finding.
    r.w.paths(&["app/core.yml"]).await;
    assert_eq!(
        state_of(&r.w, "app/core.yml").await.as_deref(),
        Some("intentional_divergence")
    );
    assert!(
        !r.w.tables()
            .await
            .findings
            .iter()
            .any(|f| f.0 == "drift" && f.2.as_deref() == Some("app/core.yml")),
        "C5 does not name an intentional divergence"
    );

    // A file that is marked is not marked again; clear it first to change the reason.
    assert_eq!(
        r.svc
            .mark_intentional(&r.admin, &lane(), &nfs("app/core.yml"), &reason, AuditVia::Ui)
            .await,
        Err(AdoptionError::NotDivergent)
    );

    // Cleared by an admin (who is more than an operator): the mark goes, with one event, and the state is the table's again.
    r.svc
        .clear_intentional(&r.admin, &lane(), &nfs("app/core.yml"), AuditVia::Ui)
        .await
        .unwrap();
    assert!(marks(pool, "sit1").await.is_empty());
    assert_eq!(audit_count(pool, "drift_mark_cleared").await, 1);
    assert_eq!(audit_count(pool, "drift_marked_intentional").await, 1);
    r.w.paths(&["app/core.yml"]).await;
    assert_eq!(state_of(&r.w, "app/core.yml").await.as_deref(), Some("nfs_ahead"));
    assert_eq!(
        r.svc
            .clear_intentional(&r.admin, &lane(), &nfs("app/core.yml"), AuditVia::Ui)
            .await,
        Err(AdoptionError::Read(ReadError::NotFound)),
        "nothing to clear"
    );
    assert_eq!(
        audit_count(pool, "drift_mark_cleared").await,
        1,
        "a refused call writes nothing"
    );
}

#[tokio::test]
async fn only_a_file_in_a_state_a_mark_can_override_is_marked() {
    let Some(r) = marks_world().await else { return };
    let reason = Reason::parse("on purpose").unwrap();
    // In sync: nothing to accept. Untracked: not in the list of states a mark overrides (D20).
    for path in ["app/same.yml", "app/untracked.yml"] {
        assert_eq!(
            r.svc
                .mark_intentional(&r.operator, &lane(), &nfs(path), &reason, AuditVia::Mcp)
                .await,
            Err(AdoptionError::NotDivergent),
            "{path}"
        );
    }
    // A path the swimlane does not have.
    assert_eq!(
        r.svc
            .mark_intentional(
                &r.operator,
                &lane(),
                &nfs("app/nowhere.yml"),
                &reason,
                AuditVia::Ui
            )
            .await,
        Err(AdoptionError::Read(ReadError::NotFound))
    );
    // A file only Git has is `unknown`: it can be marked (NFS has none of it, which is the divergence), and the mark holds an empty N.
    assert_eq!(
        state_of(&r.w, "app/only-git.yml").await.as_deref(),
        Some("unknown")
    );
    r.svc
        .mark_intentional(
            &r.operator,
            &lane(),
            &nfs("app/only-git.yml"),
            &reason,
            AuditVia::Mcp,
        )
        .await
        .unwrap();
    let marked = marks(&r.w.rig.pool, "sit1").await;
    assert_eq!(marked.len(), 1);
    assert_eq!((marked[0].1, marked[0].2), (None, Some(drift("g: 1\n"))));
    let via: String =
        sqlx::query_scalar("SELECT via FROM audit_events WHERE action = 'drift_marked_intentional'")
            .fetch_one(&r.w.rig.pool)
            .await
            .unwrap();
    assert_eq!(
        via, "mcp",
        "how the caller reached the service is what is recorded"
    );
    r.w.paths(&["app/only-git.yml"]).await;
    assert_eq!(
        state_of(&r.w, "app/only-git.yml").await.as_deref(),
        Some("intentional_divergence")
    );
    // Nothing of the refused calls was written.
    assert_eq!(audit_count(&r.w.rig.pool, "drift_marked_intentional").await, 1);
}
