//! The security of the retention job (T9, S10, S21, S4, D116): what it writes to a log, a metric or an error, and what it may do as a
//! database role. Over a real Postgres 16 as `lanekeeper_app`.
//!
//! - S10: the log lines, the metrics and the errors of a run carry counts and bytes, never a hash, a path or a byte of content. A planted
//!   hash, path and content are in the database; real, dry, contended and failed runs are made; none of them appears anywhere.
//! - The grant: the job runs as the app role and needs nothing more than that role has. It is not a superuser, cannot truncate or alter,
//!   and its one `UPDATE` on `blobs` is the age column that the contract gives it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use hub_registry::{RegistryConfig, RegistryMetrics, Retention, RetentionDeps};
use prometheus_client::registry::Registry;
use support::log_capture::LogCapture;
use support::retention::{Attach, Gc};

const PATH: &str = "PLANTED-PATH-5e2a/secret-app.yml";
const CONTENT: &str = "PLANTED-CONTENT-7c1d";

fn config() -> RegistryConfig {
    RegistryConfig {
        gc_batch: 1_000,
        gc_chunk: 3,
        gc_lock_timeout: Duration::from_millis(300),
        ..RegistryConfig::default()
    }
}

fn exposition(metrics: &RegistryMetrics) -> String {
    let mut registry = Registry::default();
    metrics.register(&mut registry);
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    text
}

#[tokio::test]
async fn no_hash_path_or_content_reaches_a_log_line_a_metric_or_an_error() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let metrics = RegistryMetrics::default();
    let job = support::retention::retention_of(&gc.rig, config(), metrics.clone());
    let dry = support::retention::retention_of(
        &gc.rig,
        RegistryConfig {
            retention_dry_run: true,
            ..config()
        },
        metrics.clone(),
    );

    // The world: blobs with planted content that nothing references (collected), one that a file at a planted path references (kept), and
    // one that is stuck behind a lock (contended).
    let mut hashes = Vec::new();
    for i in 0..4 {
        hashes.push(gc.old(&format!("{CONTENT} orphan {i}\n")).await);
    }
    let kept = gc.old(&format!("{CONTENT} referenced\n")).await;
    gc.admin(&format!(
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, nfs_hash, state) \
         VALUES ('sit1', '{PATH}', '{PATH}', 'base', 'structured', '\\x{}', 'in_sync')",
        hex::encode(kept.as_bytes())
    ))
    .await;
    gc.observe(PATH, Some(&kept), 0.5).await;
    hashes.push(kept);
    let stuck = gc.old(&format!("{CONTENT} stuck\n")).await;
    gc.index_rows(&stuck, 2).await;
    hashes.push(stuck);

    // From here on everything that is logged is the job's (the setup above wrote planted literals with its own statements, which the
    // driver logs at debug level).
    let (capture, _guard) = LogCapture::install();

    // A dry run, a plan that is printed, a run that waits for a lock and gives up, a run that deletes, a run with nothing to do, and a run
    // whose database is gone.
    let dry_report = dry.run_once().await.unwrap();
    let plan = job.plan().await.unwrap();
    let mut holder = gc.rig.db.admin().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT 1 FROM settings_index WHERE blob_hash = $1::bytea FOR UPDATE")
        .bind(stuck.as_bytes().as_slice())
        .fetch_all(&mut holder)
        .await
        .unwrap();
    let contended = job.run_once().await.unwrap();
    sqlx::raw_sql("ROLLBACK").execute(&mut holder).await.unwrap();
    let real = job.run_once().await.unwrap();
    let nothing = job.run_once().await.unwrap();
    let closed = gc.rig.db.app_pool_of(1).await;
    closed.close().await;
    let broken = Retention::new(RetentionDeps {
        pool: closed,
        ingest: gc.rig.ingest.clone(),
        clock: Arc::clone(&gc.rig.clock) as _,
        config: config(),
        metrics: metrics.clone(),
    })
    .unwrap();
    let failure = broken
        .run_once()
        .await
        .expect_err("a run without a database fails");

    // The runs did what the test says they did (so that the capture has something to hold).
    assert_eq!(dry_report.planned_blobs, 5);
    assert!(
        contended.contended_blobs >= 1 && contended.contended_blobs <= 3,
        "the chunk with the stuck blob gave up at its lock timeout: {contended:?}"
    );
    assert_eq!(
        contended.deleted_blobs + contended.contended_blobs,
        5,
        "and the other chunk went"
    );
    assert_eq!(
        real.deleted_blobs, contended.contended_blobs,
        "with the lock gone the rest went"
    );
    assert_eq!(nothing.planned_blobs, 0);

    // What there is to look through: every line of the log, every line of the metrics, every error and report as it is printed.
    let log = capture.text();
    let metrics_text = exposition(&metrics);
    let printed = format!("{failure:?} {failure} {plan:?} {dry_report:?} {contended:?} {real:?} {nothing:?}");
    assert!(
        log.contains("retention run") && log.contains("deleted_blobs"),
        "the capture caught the job's own lines, so an empty search means something: {log}"
    );
    assert!(log.contains("WARN"), "and its warnings: {log}");
    assert!(metrics_text.contains("registry_retention"), "{metrics_text}");

    let mut forbidden: Vec<String> = vec![PATH.to_owned(), CONTENT.to_owned(), "secret-app".to_owned()];
    for hash in &hashes {
        let hex = hex::encode(hash.as_bytes());
        forbidden.push(hex[..12].to_owned());
        forbidden.push(hex[52..].to_owned());
        forbidden.push(hex);
        forbidden.push(format!("{:?}", hash.as_bytes()));
    }
    for (what, text) in [
        ("log", &log),
        ("metrics", &metrics_text),
        ("errors and reports", &printed),
    ] {
        let lower = text.to_lowercase();
        for needle in &forbidden {
            assert!(
                !lower.contains(&needle.to_lowercase()),
                "{what} holds {needle:?}: {text}"
            );
        }
    }

    // The metrics are made of closed words: every label value is letters only, and every sample is a plain number.
    for line in metrics_text
        .lines()
        .filter(|l| l.starts_with("registry_retention"))
    {
        let (head, value) = line.rsplit_once(' ').expect("name{labels} value");
        assert!(value.chars().all(|c| c.is_ascii_digit()), "{line}");
        if let Some((_, labels)) = head.split_once('{') {
            for pair in labels.trim_end_matches('}').split(',') {
                let (_, v) = pair.split_once('=').expect("label=value");
                assert!(
                    v.trim_matches('"')
                        .chars()
                        .all(|c| c.is_ascii_alphabetic() || c == '_'),
                    "{line}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_job_runs_as_the_app_role_and_needs_nothing_the_app_role_does_not_have() {
    let Some(gc) = Gc::with(config()).await else {
        return;
    };
    let pool = gc.pool();
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(role, "lanekeeper_app", "the job's pool is the app role's");
    let (superuser, createdb, createrole, bypass): (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT rolsuper, rolcreatedb, rolcreaterole, rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(
        !(superuser || createdb || createrole || bypass),
        "the app role has no attribute that the job could lean on"
    );
    let allowed = |privilege: &'static str, table: &'static str| async move {
        sqlx::query_scalar::<_, bool>("SELECT has_table_privilege(current_user, $1, $2)")
            .bind(table)
            .bind(privilege)
            .fetch_one(pool)
            .await
            .unwrap()
    };
    // What the job does, each with the grant that the migrations give the app role (migration README, section 0008).
    for (privilege, table) in [
        ("SELECT", "blobs"),
        ("DELETE", "blobs"),
        ("DELETE", "sync_windows"),
        ("DELETE", "delta_staging"),
        ("SELECT", "retention_settings"),
    ] {
        assert!(allowed(privilege, table).await, "{privilege} on {table}");
    }
    // What it never needs, and the role does not have.
    for table in [
        "blobs",
        "sync_windows",
        "delta_staging",
        "audit_events",
        "retention_settings",
    ] {
        assert!(
            !allowed("TRUNCATE", table).await,
            "TRUNCATE on {table} is not granted"
        );
    }
    for (table, column) in [("blobs", "content"), ("blobs", "hash"), ("blobs", "created_at")] {
        let can: bool = sqlx::query_scalar("SELECT has_column_privilege(current_user, $1, $2, 'UPDATE')")
            .bind(table)
            .bind(column)
            .fetch_one(pool)
            .await
            .unwrap();
        assert!(!can, "{table}.{column} is write-once for the app role");
    }
    let touch: bool =
        sqlx::query_scalar("SELECT has_column_privilege(current_user, 'blobs', 'touched_at', 'UPDATE')")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        touch,
        "the age column is the one UPDATE on blobs (put refreshes it)"
    );
    let audit_delete: bool =
        sqlx::query_scalar("SELECT has_table_privilege(current_user, 'audit_events', 'DELETE')")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        !audit_delete,
        "a collection cannot touch the audit chain: it is append-only for the app role"
    );

    // And it works with exactly that: a full run, as that role, deletes.
    let h = gc.old("collectable as the app role\n").await;
    gc.attach(Attach::FileStateGit, &h, 1).await;
    let report = gc.retention.run_once().await.unwrap();
    assert_eq!(report.deleted_blobs, 1);
}
