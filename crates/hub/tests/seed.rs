//! `hub seed` (plan 18, T7, G24; S5, S9, S10; rule 8): registers projects, swimlanes, tenants and sentinels from a file.
//!
//! The first group needs no Docker and holds the file's trust boundary (in `src/seed/file.rs`'s own tests) and the example file.
//! The rest runs against a real Postgres 16 with TLS and the roles of `deploy/dev/init.sql`, the way the cluster has them:
//!
//! - a new file creates the rows and exactly one audit event for each, and the audit chain verifies;
//! - a second run changes nothing and writes nothing (no row is rewritten, no event is written), and `--dry-run` prints exactly what
//!   a run would do and changes nothing;
//! - a swimlane that is already running is not disturbed: a field the file does not carry is not touched, nothing is deleted, what the
//!   file does not list is reported;
//! - a file that is refused (a bad service account, an unknown project, a duplicate id, a row that lacks a column, too many changes) changes nothing;
//!   so does a run whose audit events cannot be written (they are one transaction);
//! - two seeds at once take turns; the migrator, the superuser and anyone else are refused;
//! - the real binary prints ids and column names and never a value, a credential or the file;
//! - an agent and a sentinel whose identity was seeded join through the platform's real listener, and could not before.
//!
//! `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing Docker daemon a failure and not a skip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

mod support;

#[path = "support/child.rs"]
mod child;
#[path = "support/git_remote.rs"]
mod git_remote;
#[path = "support/platform_support.rs"]
mod platform_support;
#[path = "support/secrets.rs"]
mod secrets;
#[path = "support/tls_pg.rs"]
mod tls_pg;
// The platform's cluster rig (T3), for the two tests that make an agent and a sentinel join.
#[path = "support/cluster.rs"]
#[macro_use]
mod cluster;

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::Secret;
use hub::config::SeedSettings;
use hub::seed::{FieldPath, Mode, Reason, Report, SeedError, apply, plan::Kind};
use hub_identity::audit::{CHAIN_LOCK_KEY, ChainVerifier, EcdsaP256Verifier, MemoryBucket};
use hub_platform::clock::SystemClock;
use platform_support::ca::csr_for;
use platform_support::grpc::{agent_client, client_tls, dial, sentinel_client};
use proto::pb;
use sqlx::{PgPool, Row};
use support::pki::TestCa;
use tls_pg::TlsPg;
use tonic::Request;

use cluster::Cluster;

/// The synthetic file of `just dev`: one project, four swimlanes with a tenant each, one sentinel. It is also what the tests change.
const EXAMPLE: &str = include_str!("../examples/seed.dev.yaml");

const AGENT_ACCOUNT: &str = "agent-sl-01@lanekeeper-dev.iam.gserviceaccount.com";
const SENTINEL_ACCOUNT: &str = "sentinel-01@lanekeeper-dev.iam.gserviceaccount.com";

/// `EXAMPLE` with one piece of text replaced.
fn with(from: &str, to: &str) -> String {
    assert!(EXAMPLE.contains(from), "{from:?} is not in the example");
    EXAMPLE.replacen(from, to, 1)
}

fn loaded(text: &str) -> Result<hub::seed::Loaded, SeedError> {
    hub::seed::from_bytes(text.as_bytes())
}

// ------------------------------------------------------------------------------------------------ no database

#[test]
fn the_example_file_is_a_valid_seed_file_with_only_synthetic_names() {
    let loaded = loaded(EXAMPLE).unwrap();
    let swimlanes = loaded.file.swimlanes.as_ref().unwrap();
    let ids: Vec<&str> = swimlanes.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["sl-01", "sl-02", "sl-03", "sl-04"]);
    // Tenant sitN for swimlane sl-0N, as `cargo xtask gen-fixtures` names them.
    for (n, lane) in swimlanes.iter().enumerate() {
        let tenants: Vec<&str> = lane
            .tenants
            .iter()
            .flatten()
            .map(domain::TenantId::as_str)
            .collect();
        assert_eq!(tenants, [format!("sit{}", n + 1)]);
        // A swimlane can have an agent only with an account (the survey fact): every swimlane of the example carries one.
        assert!(lane.agent_service_account.is_some(), "{}", lane.id);
    }
    assert_eq!(loaded.file.sentinels.as_ref().unwrap().len(), 1);
    // Invented addresses only: nothing that is not under the example's own project, and no real domain.
    for line in EXAMPLE.lines().filter(|l| l.contains('@')) {
        assert!(line.contains("@lanekeeper-dev.iam.gserviceaccount.com"), "{line}");
    }
    assert_eq!(loaded.sha256.len(), 64);
}

#[test]
fn only_the_application_role_may_seed() {
    assert_eq!(hub::seed::check_role("lanekeeper_app"), Ok(()));
    for other in [
        "lanekeeper_migrator",
        "postgres",
        "",
        "Lanekeeper_App",
        "lanekeeper_app ",
        "lanekeeper_app2",
    ] {
        assert_eq!(
            hub::seed::check_role(other),
            Err(SeedError::WrongRole),
            "{other:?}"
        );
    }
}

// ------------------------------------------------------------------------------------------------ a real database

async fn database() -> Option<TlsPg> {
    let ca = TestCa::new("lanekeeper-test-ca");
    let pg = tls_pg::start(&ca).await?;
    pg.migrate().await;
    Some(pg)
}

fn settings(pg: &TlsPg) -> SeedSettings {
    SeedSettings {
        url: Secret::new(pg.url()),
        ca_file: pg.ca_file(),
        timeout: Duration::from_secs(60),
    }
}

/// Validate `text` and run it, the way `hub seed` does.
async fn seed(pg: &TlsPg, text: &str, mode: Mode) -> Result<Report, SeedError> {
    hub::seed::run(&settings(pg), &loaded(text)?, mode, &SystemClock).await
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// How many rows each table the seed writes holds, and how many audit events there are.
async fn totals(pool: &PgPool) -> [i64; 5] {
    [
        count(pool, "SELECT count(*) FROM projects").await,
        count(pool, "SELECT count(*) FROM swimlanes").await,
        count(pool, "SELECT count(*) FROM swimlane_tenants").await,
        count(pool, "SELECT count(*) FROM sentinels").await,
        count(pool, "SELECT count(*) FROM audit_events").await,
    ]
}

/// Every row of the four tables with the transaction that last wrote it: a row that was rewritten, even to the same values, has a
/// new `xmin`.
async fn versions(pool: &PgPool) -> Vec<String> {
    let mut out = Vec::new();
    for sql in [
        "SELECT 'p ' || id || ' ' || xmin::text FROM projects ORDER BY id",
        "SELECT 's ' || id || ' ' || xmin::text FROM swimlanes ORDER BY id",
        "SELECT 't ' || swimlane_id || ' ' || tenant_id || ' ' || xmin::text FROM swimlane_tenants ORDER BY 1",
        "SELECT 'v ' || vm || ' ' || xmin::text FROM sentinels ORDER BY vm",
        "SELECT 'a ' || seq::text || ' ' || xmin::text FROM audit_events ORDER BY seq",
    ] {
        out.extend(
            sqlx::query_scalar::<_, String>(sql)
                .fetch_all(pool)
                .await
                .unwrap(),
        );
    }
    out
}

async fn audit(pool: &PgPool) -> Vec<(String, String, Option<String>, serde_json::Value)> {
    sqlx::query("SELECT action, actor_kind, swimlane_id, event_json FROM audit_events ORDER BY seq")
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.get("action"),
                r.get("actor_kind"),
                r.get("swimlane_id"),
                serde_json::from_str(&r.get::<String, _>("event_json")).unwrap(),
            )
        })
        .collect()
}

async fn chain_is_clean(pool: &PgPool) -> u64 {
    let report = ChainVerifier::new(
        pool.clone(),
        Arc::new(MemoryBucket::new()),
        Arc::new(EcdsaP256Verifier::new()),
    )
    .verify()
    .await
    .unwrap();
    assert!(report.is_clean(), "{:?}", report.failures);
    report.events_checked
}

async fn row_json(pool: &PgPool, sql: &str) -> serde_json::Value {
    let text: String = sqlx::query_scalar(sql).fetch_one(pool).await.unwrap();
    serde_json::from_str(&text).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_file_creates_the_rows_and_an_audit_event_for_each_and_the_chain_verifies() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    assert_eq!(totals(&pool).await, [0, 0, 0, 0, 0]);

    let report = seed(&pg, EXAMPLE, Mode::Apply).await.unwrap();
    assert!(report.applied && !report.dry_run);
    // One project, four swimlanes and their four tenant sets, one sentinel.
    assert_eq!(report.plan.changes.len(), 10);
    assert_eq!(totals(&pool).await, [1, 4, 4, 1, 10]);

    // The rows hold what the file says...
    let lane = row_json(
        &pool,
        "SELECT to_jsonb(s)::text FROM swimlanes s WHERE id = 'sl-01'",
    )
    .await;
    assert_eq!(lane["project_id"], "lanekeeper-dev");
    assert_eq!(lane["display_name"], "Swimlane 1");
    assert_eq!(lane["cluster"], "gke-dev-1");
    assert_eq!(lane["tier"], "dev");
    assert_eq!(lane["nfs_server"], "nfs-1.corp.example");
    assert_eq!(lane["nfs_export"], "/export/sl-01");
    assert_eq!(lane["mount_root"], "/mnt/config");
    assert_eq!(lane["agent_service_account"], AGENT_ACCOUNT);
    // ...and every column the seed does not write has its default: editing stays locked, no baseline, no notification.
    assert_eq!(lane["editing_unlocked"], false);
    assert_eq!(lane["auto_notify"], false);
    for column in [
        "baseline_confirmed_at",
        "config_server_started_at",
        "last_notified_at",
    ] {
        assert!(lane[column].is_null(), "{column}");
    }
    let bare = row_json(
        &pool,
        "SELECT to_jsonb(s)::text FROM swimlanes s WHERE id = 'sl-02'",
    )
    .await;
    assert!(
        bare["cluster"].is_null() && bare["nfs_server"].is_null(),
        "a field the file does not carry stays empty"
    );
    let tenants: Vec<String> =
        sqlx::query_scalar("SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = 'sl-03'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(tenants, ["sit3"]);
    let sentinel = row_json(
        &pool,
        "SELECT to_jsonb(s)::text FROM sentinels s WHERE vm = 'nfs-vm-01'",
    )
    .await;
    assert_eq!(sentinel["swimlane_id"], "sl-01");
    assert_eq!(sentinel["export_root"], "/export");
    assert_eq!(sentinel["service_account"], SENTINEL_ACCOUNT);
    assert_eq!(sentinel["last_acked_seq"], 0);

    // Exactly one audit event for each changed row, by the seed component, with the file that asked for it.
    let events = audit(&pool).await;
    assert_eq!(events.len(), 10);
    let sha = loaded(EXAMPLE).unwrap().sha256;
    let mut kinds = Vec::new();
    for (action, actor, _swimlane, json) in &events {
        assert_eq!(actor, "system");
        assert_eq!(json["actor"]["component"], "hub-seed");
        assert_eq!(json["via"], "system");
        let diff: serde_json::Value = serde_json::from_str(json["diff"].as_str().unwrap()).unwrap();
        assert_eq!(diff["file_sha256"], sha.as_str());
        kinds.push(format!(
            "{action} {} {} {}",
            diff["kind"].as_str().unwrap(),
            diff["op"].as_str().unwrap(),
            diff["key"].as_str().unwrap()
        ));
    }
    assert_eq!(
        kinds,
        [
            "settings_changed project create lanekeeper-dev",
            "settings_changed swimlane create sl-01",
            "settings_changed swimlane_tenants add sl-01",
            "settings_changed swimlane create sl-02",
            "settings_changed swimlane_tenants add sl-02",
            "settings_changed swimlane create sl-03",
            "settings_changed swimlane_tenants add sl-03",
            "settings_changed swimlane create sl-04",
            "settings_changed swimlane_tenants add sl-04",
            "sentinel_enrolled sentinel create nfs-vm-01",
        ]
    );
    // The events of a swimlane are filed under it.
    assert_eq!(events[1].2.as_deref(), Some("sl-01"));
    assert_eq!(
        events[9].2.as_deref(),
        Some("sl-01"),
        "the sentinel's event is filed under its swimlane"
    );
    assert_eq!(events[0].2, None);
    // And the chain is whole.
    assert_eq!(chain_is_clean(&pool).await, 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_run_changes_nothing_and_writes_nothing() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    seed(&pg, EXAMPLE, Mode::Apply).await.unwrap();
    let before = versions(&pool).await;
    assert_eq!(before.len(), 1 + 4 + 4 + 1 + 10);

    for _ in 0..2 {
        let again = seed(&pg, EXAMPLE, Mode::Apply).await.unwrap();
        assert!(!again.applied, "a run that changed nothing reports so");
        assert!(again.plan.changes.is_empty(), "{:?}", again.plan.changes);
        assert_eq!(again.plan.unchanged.swimlanes, 4);
        assert!(
            again.render().starts_with("nothing to change"),
            "{}",
            again.render()
        );
    }
    // No row was rewritten (not one `xmin` moved, so not even to the same values) and no event was written.
    assert_eq!(versions(&pool).await, before);
    assert_eq!(chain_is_clean(&pool).await, 10);

    // A change of one carried value is exactly one update and one event, and the rest is as it was.
    let renamed = with("display_name: Swimlane 3", "display_name: Swimlane Three");
    let report = seed(&pg, &renamed, Mode::Apply).await.unwrap();
    assert_eq!(report.plan.changes.len(), 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM audit_events").await, 11);
    let after = versions(&pool).await;
    let moved: Vec<&String> = after.iter().filter(|v| !before.contains(v)).collect();
    assert_eq!(moved.len(), 2, "the swimlane row and the new event: {moved:?}");
    assert!(moved[0].starts_with("s sl-03 "), "{moved:?}");
    let events = audit(&pool).await;
    let diff: serde_json::Value = serde_json::from_str(events[10].3["diff"].as_str().unwrap()).unwrap();
    assert_eq!(diff["op"], "update");
    assert_eq!(
        diff["set"]["display_name"],
        serde_json::json!({"from": "Swimlane 3", "to": "Swimlane Three"})
    );
    assert_eq!(diff["set"].as_object().unwrap().len(), 1);
    assert_eq!(chain_is_clean(&pool).await, 11);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dry_run_prints_exactly_what_a_run_would_do_and_changes_nothing() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;

    // On an empty database: ten changes planned, nothing written, no event.
    let dry = seed(&pg, EXAMPLE, Mode::DryRun).await.unwrap();
    assert!(dry.dry_run && !dry.applied);
    assert_eq!(dry.plan.changes.len(), 10);
    assert_eq!(totals(&pool).await, [0, 0, 0, 0, 0]);
    let text = dry.render();
    assert!(text.starts_with("dry run: nothing was written\n"), "{text}");
    assert!(text.ends_with("10 rows would change\n"), "{text}");

    // What a real run then does is exactly what the dry run said.
    let real = seed(&pg, EXAMPLE, Mode::Apply).await.unwrap();
    assert_eq!(real.plan, dry.plan);
    let before = versions(&pool).await;

    // On a database that has the rows: a changed file is planned as one update, and nothing moves.
    let changed = with("tier: sit", "tier: uat");
    let dry = seed(&pg, &changed, Mode::DryRun).await.unwrap();
    assert_eq!(dry.plan.changes.len(), 1);
    assert_eq!(dry.plan.changes[0].kind, Kind::Swimlane);
    assert_eq!(versions(&pool).await, before);
    let again = seed(&pg, EXAMPLE, Mode::DryRun).await.unwrap();
    assert!(again.plan.changes.is_empty());
    assert!(
        again.render().ends_with("nothing would change\n"),
        "{}",
        again.render()
    );
    assert_eq!(versions(&pool).await, before);
}

#[tokio::test]
async fn a_dry_run_runs_in_a_read_only_transaction_so_the_server_itself_refuses_a_write() {
    let Some(pg) = database().await else { return };
    let options =
        hub::db::connect_options_for(&Secret::new(pg.url()), &pg.ca_file(), Duration::from_secs(30)).unwrap();
    let mut conn = hub::db::connect_one(&options).await.unwrap();
    let mut tx = apply::begin(&mut conn, true).await.unwrap();
    let error = sqlx::query("INSERT INTO projects (id, display_name) VALUES ('x', 'X')")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    // 25006: read_only_sql_transaction.
    assert_eq!(
        error
            .as_database_error()
            .and_then(|e| e.code().map(std::borrow::Cow::into_owned))
            .as_deref(),
        Some("25006")
    );
    // And even the audit chain's lock-taking insert would be refused: nothing in a dry run can write.
    drop(tx);
    let mut tx = apply::begin(&mut conn, false).await.unwrap();
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('x', 'X')")
        .execute(&mut *tx)
        .await
        .unwrap();
    // Dropping a transaction without committing rolls it back.
    drop(tx);
    assert_eq!(
        count(&pg.app_pool().await, "SELECT count(*) FROM projects").await,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swimlane_that_is_already_running_is_not_disturbed() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    // A live environment: a swimlane whose agent has joined and said hello (it filled the mount facts), whose baseline an admin
    // confirmed, whose editing is unlocked and whose services were notified; its tenants are the file's and one more; another
    // swimlane that the file does not list.
    for sql in [
        "INSERT INTO projects (id, display_name) VALUES ('lanekeeper-dev', 'Lanekeeper (development)')",
        "INSERT INTO swimlanes (id, project_id, display_name, cluster, tier, nfs_server, nfs_export, mount_root, \
           editing_unlocked, baseline_confirmed_at, auto_notify, config_server_started_at, last_notified_at) \
         VALUES ('sl-01', 'lanekeeper-dev', 'Swimlane 1', 'gke-live', 'prod', 'nfs-live', '/export/live', '/mnt/live', \
                 true, now(), true, now(), now())",
        "INSERT INTO swimlanes (id, project_id, display_name) VALUES ('sl-77', 'lanekeeper-dev', 'Not in the file')",
        "INSERT INTO swimlane_tenants (swimlane_id, tenant_id) VALUES ('sl-01', 'sit1'), ('sl-01', 'sit9')",
        "INSERT INTO agents (swimlane_id, agent_version, joined_at, last_hello_at) VALUES ('sl-01', '1.2.3', now(), now())",
        "INSERT INTO swimlane_registry (swimlane_id) VALUES ('sl-01')",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    let live = row_json(
        &pool,
        "SELECT to_jsonb(s)::text FROM swimlanes s WHERE id = 'sl-01'",
    )
    .await;
    let agent = row_json(
        &pool,
        "SELECT to_jsonb(a)::text FROM agents a WHERE swimlane_id = 'sl-01'",
    )
    .await;
    let registry = row_json(
        &pool,
        "SELECT to_jsonb(r)::text FROM swimlane_registry r WHERE swimlane_id = 'sl-01'",
    )
    .await;
    let other = row_json(
        &pool,
        "SELECT to_jsonb(s)::text FROM swimlanes s WHERE id = 'sl-77'",
    )
    .await;

    // The file carries only the account of the live swimlane, and lists the tenants it knows.
    let only_the_account = format!(
        "version: 1\nswimlanes:\n  - id: sl-01\n    agent_service_account: {AGENT_ACCOUNT}\n    tenants: [sit1]\n"
    );
    let dry = seed(&pg, &only_the_account, Mode::DryRun).await.unwrap();
    assert_eq!(dry.plan.changes.len(), 1);
    assert_eq!(
        dry.plan.absent.swimlanes,
        ["sl-77"],
        "the swimlane that the file does not list is reported"
    );
    assert_eq!(dry.plan.absent.tenants, [("sl-01".to_owned(), "sit9".to_owned())]);
    assert_eq!(count(&pool, "SELECT count(*) FROM audit_events").await, 0);

    let report = seed(&pg, &only_the_account, Mode::Apply).await.unwrap();
    assert_eq!(report.plan, dry.plan, "what the dry run said is what happened");
    assert!(report.applied);

    // Only the account changed. Every other column of the swimlane, its agent, its registry row and the swimlane the file does not
    // list are as they were; the tenant the file does not list is still there.
    let mut want = live.clone();
    want["agent_service_account"] = serde_json::json!(AGENT_ACCOUNT);
    assert_eq!(
        row_json(
            &pool,
            "SELECT to_jsonb(s)::text FROM swimlanes s WHERE id = 'sl-01'"
        )
        .await,
        want
    );
    assert_eq!(
        row_json(
            &pool,
            "SELECT to_jsonb(a)::text FROM agents a WHERE swimlane_id = 'sl-01'"
        )
        .await,
        agent
    );
    assert_eq!(
        row_json(
            &pool,
            "SELECT to_jsonb(r)::text FROM swimlane_registry r WHERE swimlane_id = 'sl-01'"
        )
        .await,
        registry
    );
    assert_eq!(
        row_json(
            &pool,
            "SELECT to_jsonb(s)::text FROM swimlanes s WHERE id = 'sl-77'"
        )
        .await,
        other
    );
    let tenants: Vec<String> =
        sqlx::query_scalar("SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = 'sl-01' ORDER BY 1")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(tenants, ["sit1", "sit9"]);
    // One event: an update that sets the account and nothing else, from nothing.
    let events = audit(&pool).await;
    assert_eq!(events.len(), 1);
    let diff: serde_json::Value = serde_json::from_str(events[0].3["diff"].as_str().unwrap()).unwrap();
    assert_eq!(
        (diff["kind"].as_str(), diff["op"].as_str()),
        (Some("swimlane"), Some("update"))
    );
    assert_eq!(
        diff["set"],
        serde_json::json!({"agent_service_account": {"from": null, "to": AGENT_ACCOUNT}})
    );

    // Again: nothing. And a file that carries exactly the values the live swimlane has is nothing too, even for fields that the
    // agent filled.
    let before = versions(&pool).await;
    assert!(!seed(&pg, &only_the_account, Mode::Apply).await.unwrap().applied);
    let same_as_live = format!(
        "version: 1\nswimlanes:\n  - id: sl-01\n    project: lanekeeper-dev\n    display_name: Swimlane 1\n    cluster: gke-live\n    tier: prod\n    nfs_server: nfs-live\n    nfs_export: /export/live\n    mount_root: /mnt/live\n    agent_service_account: {AGENT_ACCOUNT}\n"
    );
    assert!(!seed(&pg, &same_as_live, Mode::Apply).await.unwrap().applied);
    assert_eq!(versions(&pool).await, before);
    assert_eq!(chain_is_clean(&pool).await, 1);
}

/// Nothing is written when the run is refused, whichever phase refuses it.
async fn assert_refused_and_nothing_changed(pg: &TlsPg, text: &str, want: &SeedError) {
    let pool = pg.app_pool().await;
    let before = versions(&pool).await;
    let error = match seed(pg, text, Mode::Apply).await {
        Err(error) => error,
        Ok(report) => panic!("the file was accepted: {report:?}\n{text}"),
    };
    assert_eq!(&error, want, "{text}");
    assert_eq!(versions(&pool).await, before, "{want}: something changed");
    // And the same refusal is what a dry run says.
    assert_eq!(&seed(pg, text, Mode::DryRun).await.unwrap_err(), want);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bad_email_an_unknown_project_and_a_duplicate_id_are_refused_and_change_nothing() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;

    // A person's address where a service account goes: refused before the database is looked at.
    assert_refused_and_nothing_changed(
        &pg,
        &with(AGENT_ACCOUNT, "alice@gmail.com"),
        &SeedError::Invalid {
            field: FieldPath::field("swimlanes", 0, "agent_service_account"),
            reason: Reason::NotServiceAccount,
        },
    )
    .await;
    assert_refused_and_nothing_changed(
        &pg,
        &with(SENTINEL_ACCOUNT, "not-an-email"),
        &SeedError::Invalid {
            field: FieldPath::field("sentinels", 0, "service_account"),
            reason: Reason::BadEmail,
        },
    )
    .await;

    // A duplicate id, in each section.
    assert_refused_and_nothing_changed(
        &pg,
        &with("  - id: sl-04\n", "  - id: sl-02\n"),
        &SeedError::Duplicate {
            field: FieldPath::field("swimlanes", 3, "id"),
        },
    )
    .await;
    assert_refused_and_nothing_changed(
        &pg,
        &format!("{EXAMPLE}  - vm: nfs-vm-01\n    export_root: /export\n"),
        &SeedError::Duplicate {
            field: FieldPath::field("sentinels", 1, "vm"),
        },
    )
    .await;

    // A project that is neither in the file nor in the database. The entries before it are valid and are not written.
    let unknown = format!(
        "{}  - id: sl-05\n    project: nope\n    display_name: Swimlane 5\n",
        EXAMPLE
            .replace("\nsentinels:", "\n__sentinels:")
            .split("\n__sentinels:")
            .next()
            .unwrap()
    );
    assert_refused_and_nothing_changed(
        &pg,
        &unknown,
        &SeedError::UnknownProject {
            field: FieldPath::field("swimlanes", 4, "project"),
        },
    )
    .await;
    // The same for a sentinel whose swimlane is nowhere.
    assert_refused_and_nothing_changed(
        &pg,
        &with("    swimlane: sl-01\n", "    swimlane: sl-99\n"),
        &SeedError::UnknownSwimlane {
            field: FieldPath::field("sentinels", 0, "swimlane"),
        },
    )
    .await;
    // A new row that lacks a column it needs.
    assert_refused_and_nothing_changed(
        &pg,
        &with("    display_name: Swimlane 2\n", ""),
        &SeedError::MissingOnCreate {
            field: FieldPath::field("swimlanes", 1, "display_name"),
        },
    )
    .await;
    // More than 200 changes: 200 projects (the most a section may list) and one swimlane.
    let mut many = String::from("version: 1\nprojects:\n");
    for i in 0..200 {
        writeln!(many, "  - id: p{i}\n    display_name: P{i}").unwrap();
    }
    many.push_str("swimlanes:\n  - id: sl-01\n    project: p0\n    display_name: One\n");
    assert_refused_and_nothing_changed(&pg, &many, &SeedError::TooManyChanges { changes: 201 }).await;

    assert_eq!(totals(&pool).await, [0, 0, 0, 0, 0]);
    // And the file the refusals were made of is fine once it is right.
    assert!(seed(&pg, EXAMPLE, Mode::Apply).await.unwrap().applied);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rows_and_their_events_are_one_transaction() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    // Another writer holds the audit chain's lock: the seed's events cannot be written (its lock wait is five seconds), so the rows
    // that were written before them must not be kept.
    let mut holder = pg.superuser_conn().await;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(CHAIN_LOCK_KEY)
        .execute(&mut holder)
        .await
        .unwrap();
    let error = seed(&pg, EXAMPLE, Mode::Apply).await.unwrap_err();
    assert_eq!(error, SeedError::Audit);
    assert_eq!(error.exit_code(), 1);
    assert_eq!(
        totals(&pool).await,
        [0, 0, 0, 0, 0],
        "rows were kept without their events"
    );
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(CHAIN_LOCK_KEY)
        .execute(&mut holder)
        .await
        .unwrap();
    // Released, the same run goes through.
    assert!(seed(&pg, EXAMPLE, Mode::Apply).await.unwrap().applied);
    assert_eq!(totals(&pool).await, [1, 4, 4, 1, 10]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_seeds_at_once_take_turns_and_the_second_finds_nothing_to_do() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    let (first, second) = tokio::join!(seed(&pg, EXAMPLE, Mode::Apply), seed(&pg, EXAMPLE, Mode::Apply));
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(
        [first.applied, second.applied].iter().filter(|a| **a).count(),
        1,
        "exactly one of them wrote: {} / {}",
        first.render(),
        second.render()
    );
    assert_eq!(totals(&pool).await, [1, 4, 4, 1, 10]);
    assert_eq!(chain_is_clean(&pool).await, 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_migrator_the_superuser_and_a_stranger_are_refused_before_anything_is_written() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    for url in [
        pg.migrator_url(),
        pg.superuser_url(),
        pg.url_for("somebody_else", "x"),
        format!("postgres://localhost:{}/lanekeeper", pg.port),
    ] {
        let mut s = settings(&pg);
        s.url = Secret::new(url.clone());
        let error = hub::seed::run(&s, &loaded(EXAMPLE).unwrap(), Mode::Apply, &SystemClock)
            .await
            .unwrap_err();
        assert_eq!(error, SeedError::WrongRole, "{url}");
        let shown = format!("{error} {error:?}");
        assert!(
            ![
                tls_pg::MIGRATOR_ROLE.1,
                tls_pg::PLANTED_PASSWORD,
                "lanekeeper_migrator"
            ]
            .iter()
            .any(|secret| shown.contains(secret)),
            "{shown}"
        );
    }
    assert_eq!(totals(&pool).await, [0, 0, 0, 0, 0]);
    // The server is verified like the hub's pool does: a CA that did not sign it is refused, with no host and no password in the error.
    let stranger = TestCa::new("not-the-databases-ca");
    let dir = tempfile::tempdir().unwrap();
    let wrong = dir.path().join("wrong-ca.pem");
    std::fs::write(&wrong, stranger.cert_pem()).unwrap();
    let mut s = settings(&pg);
    s.ca_file = wrong;
    let error = hub::seed::run(&s, &loaded(EXAMPLE).unwrap(), Mode::Apply, &SystemClock)
        .await
        .unwrap_err();
    assert_eq!(error, SeedError::Connect);
    assert!(!format!("{error:?}").contains(tls_pg::APP_ROLE.1));
}

// ------------------------------------------------------------------------------------------------ the real binary

fn binary_env(pg: &TlsPg, extra: &[(&str, String)]) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "LK_DATABASE_URL_FILE".to_owned(),
            pg.url_file().display().to_string(),
        ),
        ("LK_DB_CA_FILE".to_owned(), pg.ca_file().display().to_string()),
    ];
    for (name, value) in extra {
        env.retain(|(n, _)| n != name);
        env.push(((*name).to_owned(), value.clone()));
    }
    env
}

fn run_binary(pg: &TlsPg, args: &[&str], extra: &[(&str, String)]) -> (Option<i32>, String) {
    let mut hub = child::Child::spawn(&child::real_binary(), args, &binary_env(pg, extra));
    let status = hub.exit_within(Duration::from_secs(120));
    (status.code(), hub.output())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_binary_seeds_prints_ids_and_column_names_and_never_a_value_or_a_credential() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    let dir = tempfile::tempdir().unwrap();
    // Values that no report, log or error may repeat.
    let planted = with("display_name: Swimlane 1", "display_name: Display-Planted-4711")
        .replace("gke-dev-1", "Cluster-Planted-4712")
        .replace("nfs-1.corp.example", "nfs-planted-4713.example");
    let file = dir.path().join("seed.yaml");
    std::fs::write(&file, &planted).unwrap();
    let file = file.display().to_string();
    let leaks = [
        "Display-Planted-4711",
        "Cluster-Planted-4712",
        "nfs-planted-4713",
        AGENT_ACCOUNT,
        SENTINEL_ACCOUNT,
        tls_pg::APP_ROLE.1,
        tls_pg::PLANTED_PASSWORD,
        "localhost",
    ];

    // A dry run: exit 0, what would change, nothing written.
    let (code, output) = run_binary(&pg, &["seed", "--dry-run", &file], &[]);
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("dry run: nothing was written"), "{output}");
    assert!(output.contains("create swimlane sl-01 (project, display_name, cluster, tier, nfs_server, nfs_export, mount_root, agent_service_account)"), "{output}");
    assert!(output.contains("10 rows would change"), "{output}");
    assert_eq!(totals(&pool).await, [0, 0, 0, 0, 0]);
    // A run: exit 0, written.
    let (code, output) = run_binary(&pg, &["seed", &file], &[]);
    assert_eq!(code, Some(0), "{output}");
    assert!(
        output.contains("applied: 10 rows changed, one audit event each"),
        "{output}"
    );
    assert!(
        output.contains(r#""message":"seed finished""#) && output.contains(r#""changes":10"#),
        "{output}"
    );
    assert_eq!(totals(&pool).await, [1, 4, 4, 1, 10]);
    // Again: nothing.
    let (code, output) = run_binary(&pg, &["seed", &file], &[]);
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("nothing to change"), "{output}");
    assert_eq!(totals(&pool).await, [1, 4, 4, 1, 10]);

    // None of the three outputs repeated a value of the file or a credential.
    for (args, label) in [
        (vec!["seed", "--dry-run", &file], "dry run"),
        (vec!["seed", &file], "run"),
    ] {
        let (_, output) = run_binary(&pg, &args, &[]);
        for leak in leaks {
            assert!(
                !output.contains(leak),
                "{label}: the output holds {leak}:\n{output}"
            );
        }
    }
    // The values are in the database, and the planted ones are in the audit trail (that is its job), not in what the process printed.
    let events = audit(&pool).await;
    assert!(
        events[1].3["diff"]
            .as_str()
            .unwrap()
            .contains("Display-Planted-4711")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_binary_refuses_a_bad_file_a_bad_setting_and_the_wrong_role_with_the_right_exit_codes() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    };

    // A file that is refused: exit 2, the place and the reason, no value; nothing written.
    let bad = write("bad.yaml", &with(AGENT_ACCOUNT, "hunter2-planted@gmail.com"));
    let (code, output) = run_binary(&pg, &["seed", &bad], &[]);
    assert_eq!(code, Some(2), "{output}");
    assert!(output.contains("swimlanes[0].agent_service_account"), "{output}");
    assert!(output.contains("not a Google service account"), "{output}");
    assert!(!output.contains("hunter2"), "{output}");
    // An unknown field: exit 2, a line and a column, not the field.
    let unknown = write(
        "unknown.yaml",
        &with("    tier: dev\n", "    tier: dev\n    hunter2_planted: 1\n"),
    );
    let (code, output) = run_binary(&pg, &["seed", &unknown], &[]);
    assert_eq!(code, Some(2), "{output}");
    assert!(
        output.contains("line ") && !output.contains("hunter2"),
        "{output}"
    );
    // A file that is not there, one that is a directory: exit 2.
    let (code, output) = run_binary(
        &pg,
        &["seed", &dir.path().join("missing.yaml").display().to_string()],
        &[],
    );
    assert_eq!(code, Some(2), "{output}");
    assert!(
        !output.contains("missing.yaml"),
        "the path is not repeated: {output}"
    );
    let (code, _) = run_binary(&pg, &["seed", &dir.path().display().to_string()], &[]);
    assert_eq!(code, Some(2));
    // Without a file, with two, with an option that does not exist: the usage error.
    for args in [
        vec!["seed"],
        vec!["seed", "a", "b"],
        vec!["seed", "--force", &bad],
    ] {
        let (code, _) = run_binary(&pg, &args, &[]);
        assert_eq!(code, Some(2), "{args:?}");
    }
    // A setting that is malformed: exit 2, the variable and not the value.
    let good = write("good.yaml", EXAMPLE);
    let (code, output) = run_binary(
        &pg,
        &["seed", &good],
        &[("LK_SEED_TIMEOUT_SECS", "planted-9137".to_owned())],
    );
    assert_eq!(code, Some(2), "{output}");
    assert!(
        output.contains("LK_SEED_TIMEOUT_SECS is not valid") && !output.contains("planted-9137"),
        "{output}"
    );
    // The migrator's credential in the application's place: exit 1, the role named, no password.
    let dir2 = tempfile::tempdir().unwrap();
    let migrator = dir2.path().join("migrator-url");
    std::fs::write(&migrator, pg.migrator_url()).unwrap();
    let (code, output) = run_binary(
        &pg,
        &["seed", &good],
        &[("LK_DATABASE_URL_FILE", migrator.display().to_string())],
    );
    assert_eq!(code, Some(1), "{output}");
    assert!(output.contains("lanekeeper_app"), "{output}");
    assert!(
        !output.contains(tls_pg::MIGRATOR_ROLE.1) && !output.contains("lanekeeper-migrator"),
        "{output}"
    );
    // The wrong password: exit 1 and a typed message, with the password in none of it.
    let wrong = dir2.path().join("wrong-url");
    std::fs::write(&wrong, pg.url_for(tls_pg::APP_ROLE.0, "not-the-password-4714")).unwrap();
    let (code, output) = run_binary(
        &pg,
        &["seed", &good],
        &[("LK_DATABASE_URL_FILE", wrong.display().to_string())],
    );
    assert_eq!(code, Some(1), "{output}");
    assert!(
        !output.contains("not-the-password-4714") && !output.contains("localhost"),
        "{output}"
    );
    assert_eq!(totals(&pool).await, [0, 0, 0, 0, 0]);
}

// ------------------------------------------------------------------------------------------------ a seeded identity joins

fn join_agent(swimlane: &str, token: &str, csr: &[u8]) -> pb::JoinRequest {
    cluster::join_request(swimlane, token, csr)
}

fn spiffe_uris(chain_der: &Bytes) -> Vec<String> {
    let leaf = x509_parser::parse_x509_certificate(chain_der).unwrap().1;
    leaf.subject_alternative_name()
        .unwrap()
        .unwrap()
        .value
        .general_names
        .iter()
        .filter_map(|n| match n {
            x509_parser::extensions::GeneralName::URI(u) => Some((*u).to_owned()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_agent_joins_through_the_agent_listener() {
    let cluster = cluster!(1);
    cluster.accept_google("gid-ok", AGENT_ACCOUNT);
    cluster.accept_google("gid-other", "other@lanekeeper-dev.iam.gserviceaccount.com");
    let tls = client_tls(&cluster.agent_ca, None);
    let mut client = agent_client(dial(cluster.agent_addr(0), &tls).await.unwrap());
    let csr = csr_for(&rcgen::KeyPair::generate().unwrap());

    // Before the seed there is no swimlane: a valid token for the account is refused like any other (the survey's fact: until
    // swimlanes are seeded, `Join` always fails).
    let status = client
        .join(Request::new(join_agent("sl-01", "gid-ok", &csr)))
        .await
        .unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert_eq!(cluster.count("SELECT count(*) FROM agents").await, 0);

    let report = hub::seed::run(
        &settings(&cluster.pg),
        &loaded(EXAMPLE).unwrap(),
        Mode::Apply,
        &SystemClock,
    )
    .await
    .unwrap();
    assert!(report.applied);

    // Another account's token is still refused; the seeded account's joins and gets a certificate that names the swimlane.
    let status = client
        .join(Request::new(join_agent("sl-01", "gid-other", &csr)))
        .await
        .unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    let response = client
        .join(Request::new(join_agent("sl-01", "gid-ok", &csr)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        response.cert_chain_der.len(),
        2,
        "the agent's certificate and the CA's"
    );
    assert_eq!(
        spiffe_uris(&response.cert_chain_der[0]),
        ["spiffe://lanekeeper/swimlane/sl-01"]
    );
    assert_eq!(
        cluster
            .count("SELECT count(*) FROM agents WHERE swimlane_id = 'sl-01'")
            .await,
        1
    );
    // The account of one seeded swimlane does not open another: sl-02 has its own.
    let status = client
        .join(Request::new(join_agent("sl-02", "gid-ok", &csr)))
        .await
        .unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);

    // The seed's events and the join's are one chain.
    let pool = cluster.pg.app_pool().await;
    assert_eq!(chain_is_clean(&pool).await, 11, "ten from the seed and one join");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_sentinel_joins() {
    let cluster = cluster!(1);
    cluster.accept_google("gid-sentinel", SENTINEL_ACCOUNT);
    cluster.accept_google("gid-agent", AGENT_ACCOUNT);
    let tls = client_tls(&cluster.agent_ca, None);
    let csr = csr_for(&rcgen::KeyPair::generate().unwrap());
    let join = |token: &str| {
        let csr = csr.clone();
        let token = token.to_owned();
        let addr = cluster.agent_addr(0);
        let tls = &tls;
        async move {
            let mut client = sentinel_client(dial(addr, tls).await.unwrap());
            client
                .join(Request::new(pb::JoinRequest {
                    swimlane_id: "nfs-vm-01".to_owned(),
                    csr_der: Bytes::from(csr),
                    credential: Some(pb::join_request::Credential::GoogleIdToken(token)),
                    kind: pb::PeerKind::Sentinel as i32,
                }))
                .await
        }
    };

    // Not enrolled yet: refused.
    assert_eq!(
        join("gid-sentinel").await.unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    hub::seed::run(
        &settings(&cluster.pg),
        &loaded(EXAMPLE).unwrap(),
        Mode::Apply,
        &SystemClock,
    )
    .await
    .unwrap();
    // The agent's account is not the sentinel's: refused. The sentinel's own joins, with a certificate that names the VM.
    assert_eq!(
        join("gid-agent").await.unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    let response = join("gid-sentinel").await.unwrap().into_inner();
    assert_eq!(
        spiffe_uris(&response.cert_chain_der[0]),
        ["spiffe://lanekeeper/sentinel/nfs-vm-01"]
    );
    let pool = cluster.pg.app_pool().await;
    assert_eq!(
        chain_is_clean(&pool).await,
        11,
        "ten from the seed and the sentinel's join"
    );
}
