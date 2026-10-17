//! S10 for the adoption pass: logs hold ids, paths and counts, never file contents, secrets, reasons, environment values or hashes of
//! content. Secrets are planted in the repositories, on NFS, in a reason and in an environment value; the pass runs from the ranking to
//! the confirmation, through refusals and failures, and none of them may reach a log line.
//!
//! This binary holds only this test: the secret scan runs on the blocking pool and on rayon threads, and `install_everywhere` captures
//! every thread that has no capture of its own.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use hub_registry::{AdoptionReason, Reason, Resolution};
use ports::{AuditVia, content_hash};
use support::adoption::{intentional, lane, nfs, not_admins, secret_world};
use support::log_capture::LogCapture;

const REASON: &str = "PLANTED-REASON-9911";
const PROFILE: &str = "PLANTED-PROFILE-VALUE-0042";

#[tokio::test]
async fn planted_secret_never_reaches_a_log_line_of_the_adoption_pass() {
    let Some((r, planted)) = secret_world().await else {
        return;
    };
    let capture = LogCapture::install_everywhere();
    sqlx::query(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) VALUES ('sit1', 'csp', 'api', $1::jsonb)",
    )
    .bind(format!("{{\"SPRING_PROFILES_ACTIVE\": \"sit1,{PROFILE}\"}}"))
    .execute(&r.w.rig.pool)
    .await
    .unwrap();

    // The whole pass, with a resolution that carries a reason.
    let id = r.start().await.id;
    r.choose(id, &["sit1"]).await;
    r.resolve_all(id, hub_registry::Bulk::default(), intentional(REASON))
        .await;
    let scanned = r.svc.scan_secrets(&r.admin, id).await.unwrap();
    assert!(!scanned.secret_scan.is_empty());
    r.svc.confirm(&r.admin, id).await.unwrap();

    // Refusals and failures log too: a caller that is not an admin, a commit the mirror does not have, a pass that is closed, a file
    // that is not diverged, and a database that is gone.
    for (_, u) in not_admins() {
        let _ = r.svc.start(&u, &lane(), AdoptionReason::Initial).await;
    }
    let _ = r.svc.confirm(&r.admin, id).await;
    let _ = r
        .svc
        .mark_intentional(
            &r.operator,
            &lane(),
            &nfs("app/clean.yml"),
            &Reason::parse(REASON).unwrap(),
            AuditVia::Ui,
        )
        .await;
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
    let failed = svc.start(&r.admin, &lane(), AdoptionReason::Initial).await;
    assert!(failed.is_err());
    let _ = svc
        .resolve(&r.admin, id, vec![(nfs("app/base.yml"), Resolution::NfsIsRight)])
        .await;

    let text = capture.text();
    let ours: Vec<&str> = text.lines().filter(|l| l.contains(" hub_registry")).collect();
    assert!(
        ours.iter().any(|l| l.contains("adoption secret scan finished"))
            && ours.iter().any(|l| l.contains("adoption confirmed")),
        "the capture saw the pass: {ours:?}"
    );
    assert!(
        ours.iter().any(|l| l.contains("database call failed")),
        "and the failing database: {ours:?}"
    );
    let mut forbidden: Vec<String> = planted.all().iter().map(|s| (*s).to_owned()).collect();
    forbidden.extend([REASON.to_owned(), PROFILE.to_owned()]);
    // The contents of the files, and the hashes of those contents.
    for content in ["name: base\n", "clean: true\n"] {
        forbidden.push(content_hash(content.as_bytes()).to_string());
    }
    forbidden.push(content_hash(format!("token: {}\n", planted.nfs).as_bytes()).to_string());
    for value in &forbidden {
        assert!(!text.contains(value.as_str()), "{value} reached a log line");
    }
    for line in ours {
        for sql in ["SELECT", "INSERT", "UPDATE", "unnest"] {
            assert!(
                !line.contains(sql),
                "SQL text reached a log line of the registry: {line}"
            );
        }
    }
}
