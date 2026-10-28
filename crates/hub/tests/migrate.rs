//! `hub migrate` (plan 18, T2; G4, G22, G29; S6, S7, S10): the schema comes from a Job that holds the migrator credential, and
//! the serving hub never migrates.
//!
//! The first group needs no Docker: the embedded table of migrations is the files in `db/migrations`, and every migration has a
//! marker in the schema probe. The second runs against a real Postgres 16 with TLS 1.3 and the roles of `deploy/dev/init.sql`,
//! the way the cluster has them: an empty database ends fully migrated, a second run changes nothing, `lanekeeper_app` is
//! refused before anything is touched, and a database that is ahead of the binary is refused with a typed error. The third runs
//! the real `hub` binary as a child process (`hub migrate` needs no Google) and reads what it writes.
//! `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing Docker daemon a failure and not a skip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

#[path = "support/child.rs"]
mod child;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::borrow::Cow;
use std::path::Path;
use std::time::Duration;

use hub::migrate::{self, EMBEDDED, MigrateError};
use hub::schema::{self, MARKERS, SchemaError};
use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::{PgPool, Row};
use support::pki::TestCa;
use tls_pg::TlsPg;

// ------------------------------------------------------------------------------------------------ no database

fn migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../db/migrations")
}

/// Why the table of embedded migrations is not the directory. Empty when it is.
fn table_problems(embedded: &[Migration], on_disk: &[Migration]) -> Vec<String> {
    let mut problems = Vec::new();
    if embedded.len() != on_disk.len() {
        problems.push(format!(
            "{} migrations embedded, {} files in db/migrations",
            embedded.len(),
            on_disk.len()
        ));
    }
    for (e, d) in embedded.iter().zip(on_disk) {
        if (e.version, &e.description, e.migration_type, e.no_tx)
            != (d.version, &d.description, d.migration_type, d.no_tx)
        {
            problems.push(format!(
                "migration {}: the name differs ({:?} embedded, {:?} on disk)",
                d.version, e.description, d.description
            ));
        } else if e.checksum != d.checksum {
            problems.push(format!(
                "migration {} ({}): the SQL differs from the file (checksums {} and {})",
                d.version,
                d.description,
                hex_prefix(&e.checksum),
                hex_prefix(&d.checksum)
            ));
        }
    }
    problems
}

fn hex_prefix(bytes: &[u8]) -> String {
    hex::encode(&bytes[..bytes.len().min(4)])
}

#[tokio::test]
async fn the_embedded_table_equals_the_files_in_db_migrations() {
    // The files, resolved by sqlx the way the platform's tests and `db/migrations/README.md` apply them.
    let on_disk = Migrator::new(migrations_dir().as_path()).await.unwrap();
    let on_disk: Vec<Migration> = on_disk.iter().cloned().collect();
    assert!(on_disk.len() >= 8, "found only {} files", on_disk.len());
    let problems = table_problems(&migrate::embedded(), &on_disk);
    assert!(
        problems.is_empty(),
        "a migration was added, changed or renamed in db/migrations without updating src/migrate.rs: {problems:#?}"
    );
}

#[tokio::test]
async fn the_comparison_refuses_a_forgotten_file_a_changed_file_and_a_renamed_one() {
    let on_disk = Migrator::new(migrations_dir().as_path()).await.unwrap();
    let on_disk: Vec<Migration> = on_disk.iter().cloned().collect();
    let embedded = migrate::embedded();
    assert!(table_problems(&embedded, &on_disk).is_empty());

    // A file nobody put in the table.
    let mut forgotten = embedded.clone();
    forgotten.pop();
    assert!(
        table_problems(&forgotten, &on_disk)
            .iter()
            .any(|p| p.contains("files in db/migrations"))
    );

    // A migration edited after the table was written.
    let mut changed = embedded.clone();
    let last = changed.last().unwrap().clone();
    *changed.last_mut().unwrap() = Migration::new(
        last.version,
        last.description.clone(),
        MigrationType::Simple,
        Cow::Owned(format!("{}\n-- edited", last.sql)),
        false,
    );
    assert!(
        table_problems(&changed, &on_disk)
            .iter()
            .any(|p| p.contains("the SQL differs"))
    );

    // The same SQL under another name.
    let mut renamed = embedded;
    let first = renamed[0].clone();
    renamed[0] = Migration::new(
        first.version,
        Cow::Borrowed("something else"),
        MigrationType::Simple,
        first.sql.clone(),
        false,
    );
    assert!(
        table_problems(&renamed, &on_disk)
            .iter()
            .any(|p| p.contains("the name differs"))
    );
}

#[test]
fn every_migration_has_a_probe_marker() {
    // G22: a migration that adds nothing the probe looks for would let a hub start on a schema without it.
    let table: Vec<i64> = EMBEDDED.iter().map(|m| m.version).collect();
    let markers: Vec<i64> = MARKERS.iter().map(|m| m.version).collect();
    assert_eq!(
        markers, table,
        "src/schema.rs must have exactly one marker for each migration in src/migrate.rs, in the same order"
    );
    // And the versions are the contiguous run the files use, so "the first missing" is well defined.
    let expected: Vec<i64> = (1..=i64::try_from(table.len()).unwrap()).collect();
    assert_eq!(table, expected);
}

#[test]
fn the_migration_text_never_asks_for_a_transaction_to_be_skipped() {
    // `-- no-transaction` would make a half-applied migration possible; the table has none and says so if one appears.
    for m in &EMBEDDED {
        assert!(
            !m.sql.starts_with("-- no-transaction"),
            "{} opts out of its transaction",
            m.version
        );
    }
}

// ------------------------------------------------------------------------------------------------ a real database

async fn database() -> Option<TlsPg> {
    let ca = TestCa::new("lanekeeper-test-ca");
    tls_pg::start(&ca).await
}

/// A pool as the migrator, for reading `_sqlx_migrations`, which only the migrator can see.
async fn migrator_pool(pg: &TlsPg) -> PgPool {
    hub::db::connect(&hub::config::DatabaseSettings {
        url: domain::Secret::new(pg.migrator_url()),
        ca_file: pg.ca_file(),
        max_connections: 2,
        acquire_timeout: Duration::from_secs(5),
        statement_timeout: Duration::from_secs(30),
    })
    .await
    .unwrap()
}

async fn migration_rows(pool: &PgPool) -> Vec<(i64, String, bool, Vec<u8>, i64)> {
    sqlx::query(
        "SELECT version, description, success, checksum, execution_time FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .iter()
    .map(|r| {
        (
            r.get("version"),
            r.get("description"),
            r.get("success"),
            r.get("checksum"),
            r.get("execution_time"),
        )
    })
    .collect()
}

/// Whether the migration table exists: the first thing a migrator creates, and so a sign that something was touched.
async fn touched(pg: &TlsPg) -> bool {
    let pool = pg.app_pool().await;
    // The app role cannot read the table, but it can ask the catalog whether there is one.
    sqlx::query_scalar::<_, bool>("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
        .fetch_one(&pool)
        .await
        .unwrap()
}

async fn count_relations(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn an_empty_database_ends_with_every_migration_applied() {
    let Some(pg) = database().await else { return };
    assert!(!touched(&pg).await);

    let report = migrate::run(&pg.migrator_settings()).await.unwrap();

    let versions: Vec<i64> = report.applied.iter().map(|a| a.version).collect();
    let all: Vec<i64> = EMBEDDED.iter().map(|m| m.version).collect();
    assert_eq!(versions, all);
    assert_eq!(report.latest, *all.last().unwrap());
    assert_eq!(report.applied[0].description, "init");
    assert_eq!(report.applied[7].description, "blob touch");

    // The migrator's own record agrees with the report, checksum for checksum.
    let pool = pg.app_pool().await;
    let rows = migration_rows(&migrator_pool(&pg).await).await;
    assert_eq!(rows.len(), EMBEDDED.len());
    for ((version, description, success, checksum, _), embedded) in rows.iter().zip(migrate::embedded()) {
        assert_eq!(*version, embedded.version);
        assert_eq!(description, embedded.description.as_ref());
        assert!(*success);
        assert_eq!(checksum.as_slice(), embedded.checksum.as_ref());
    }

    // The app role, which the hub runs as, finds a complete schema, and the schema is the migrator's (the objects are owned by
    // it, so the app role can never change them).
    schema::verify(&pool).await.unwrap();
    let owner: String =
        sqlx::query_scalar("SELECT tableowner::text FROM pg_tables WHERE tablename = 'users'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(owner, "lanekeeper_migrator");
}

#[tokio::test]
async fn a_second_run_changes_nothing() {
    let Some(pg) = database().await else { return };
    let first = pg.migrate().await;
    assert_eq!(first.applied.len(), EMBEDDED.len());
    let options = || pg.migrator_settings();
    let reader = migrator_pool(&pg).await;
    let before_rows = migration_rows(&reader).await;
    let before_relations = count_relations(&reader).await;
    let installed_before: Vec<String> =
        sqlx::query_scalar("SELECT installed_on::text FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&reader)
            .await
            .unwrap();

    let second = migrate::run(&options()).await.unwrap();
    let third = migrate::run(&options()).await.unwrap();

    for report in [&second, &third] {
        assert!(report.applied.is_empty(), "{report:?}");
        assert_eq!(report.latest, first.latest);
    }
    assert_eq!(migration_rows(&reader).await, before_rows);
    assert_eq!(count_relations(&reader).await, before_relations);
    let installed_after: Vec<String> =
        sqlx::query_scalar("SELECT installed_on::text FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&reader)
            .await
            .unwrap();
    assert_eq!(installed_after, installed_before, "a row was rewritten");
}

#[tokio::test]
async fn lanekeeper_app_is_refused() {
    let Some(pg) = database().await else { return };
    // The app role, the superuser and a role that does not exist: only `lanekeeper_migrator` may migrate. Each is refused
    // before a connection is made, so a wrong password does not matter and nothing is touched.
    for url in [
        pg.url(),
        pg.superuser_url(),
        pg.url_for("somebody_else", "x"),
        format!("postgres://localhost:{}/lanekeeper", pg.port),
    ] {
        let error = migrate::run(&pg.migrator_settings_with(&url)).await.unwrap_err();
        assert_eq!(error, MigrateError::WrongRole, "{url}");
        assert!(
            !format!("{error} {error:?}").contains(tls_pg::APP_ROLE.1),
            "the refusal repeats a password"
        );
    }
    assert!(!touched(&pg).await, "a refused run created the migration table");

    // And after a real run the refusal still touches nothing.
    pg.migrate().await;
    let pool = pg.app_pool().await;
    let relations = count_relations(&pool).await;
    assert_eq!(
        migrate::run(&pg.migrator_settings_with(&pg.url()))
            .await
            .unwrap_err(),
        MigrateError::WrongRole
    );
    assert_eq!(count_relations(&pool).await, relations);
}

#[test]
fn the_role_is_checked_again_once_connected() {
    // A pooler or a `SET ROLE` default could log in as someone other than the URL's user; the session says who it is.
    assert_eq!(migrate::check_role("lanekeeper_migrator"), Ok(()));
    for other in [
        "lanekeeper_app",
        "postgres",
        "",
        "Lanekeeper_Migrator",
        "lanekeeper_migrator ",
    ] {
        assert_eq!(
            migrate::check_role(other),
            Err(MigrateError::WrongRole),
            "{other:?}"
        );
    }
}

/// A migration this binary has never heard of, as a newer image would carry.
fn newer(version: i64, sql: &'static str) -> Migration {
    Migration::new(
        version,
        Cow::Borrowed("from a newer image"),
        MigrationType::Simple,
        Cow::Borrowed(sql),
        false,
    )
}

#[tokio::test]
async fn a_database_with_a_later_migration_is_refused_with_a_typed_error() {
    let Some(pg) = database().await else { return };
    pg.migrate().await;
    // A newer hub's Job applied migration 9.
    let mut ahead = migrate::embedded();
    ahead.push(newer(
        9,
        "CREATE TABLE added_by_a_newer_hub (id bigint PRIMARY KEY);",
    ));
    let applied = migrate::run_set(&pg.migrator_settings(), &ahead).await.unwrap();
    assert_eq!(applied.applied.len(), 1);

    // This binary knows eight. It must not run, and it must say which version it does not know.
    let error = migrate::run(&pg.migrator_settings()).await.unwrap_err();
    assert_eq!(error, MigrateError::DatabaseIsNewer(9));
    let shown = error.to_string();
    assert!(shown.contains('9') && shown.contains("newer"), "{shown}");
    // Nothing was changed by the refusal: the newer hub's table is still there.
    let pool = pg.app_pool().await;
    let table: bool = sqlx::query_scalar("SELECT to_regclass('public.added_by_a_newer_hub') IS NOT NULL")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(table);
}

#[tokio::test]
async fn a_migration_that_was_edited_after_it_was_applied_is_refused() {
    let Some(pg) = database().await else { return };
    pg.migrate().await;
    let mut edited = migrate::embedded();
    let last = edited.pop().unwrap();
    edited.push(Migration::new(
        last.version,
        last.description.clone(),
        MigrationType::Simple,
        Cow::Owned(format!("{}\n-- somebody edited a merged migration", last.sql)),
        false,
    ));
    assert_eq!(
        migrate::run_set(&pg.migrator_settings(), &edited)
            .await
            .unwrap_err(),
        MigrateError::Changed(8)
    );
}

#[tokio::test]
async fn a_migration_that_fails_rolls_back_and_names_the_version_and_the_code_only() {
    let Some(pg) = database().await else { return };
    pg.migrate().await;
    let mut broken = migrate::embedded();
    broken.push(newer(
        9,
        "CREATE TABLE half_done (id bigint); INSERT INTO half_done VALUES (1); \
         DO $$ BEGIN RAISE EXCEPTION 'planted-text-in-the-driver-message' USING ERRCODE = '22023'; END $$;",
    ));
    let error = migrate::run_set(&pg.migrator_settings(), &broken)
        .await
        .unwrap_err();
    // The version and the SQLSTATE the migration raised, and none of the driver's words.
    assert_eq!(error, MigrateError::Failed(9, "22023".to_owned()));
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains("planted-text"), "{shown}");
    assert!(!shown.contains("half_done"), "{shown}");

    // The whole migration was rolled back: no table, no row.
    let reader = migrator_pool(&pg).await;
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('public.half_done') IS NOT NULL")
        .fetch_one(&reader)
        .await
        .unwrap();
    assert!(!exists, "a failed migration left its table behind");
    assert_eq!(migration_rows(&reader).await.len(), EMBEDDED.len());
    // And a later run with the right set still works.
    assert!(pg.migrate().await.applied.is_empty());
}

#[tokio::test]
async fn a_run_that_takes_too_long_is_stopped_and_says_so() {
    let Some(pg) = database().await else { return };
    pg.migrate().await;
    let mut slow = migrate::embedded();
    slow.push(newer(
        9,
        "CREATE TABLE slow_one (id bigint); SELECT pg_sleep(60);",
    ));
    let mut settings = pg.migrator_settings();
    settings.timeout = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let error = migrate::run_set(&settings, &slow).await.unwrap_err();
    assert_eq!(error, MigrateError::TimedOut(2));
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it waited for the migration"
    );
    // The server may still be running the abandoned statement (it notices a dropped client when it next writes), and its session
    // holds the migration lock. End it, as the session's own `statement_timeout` would a little later, and look: nothing of the
    // slow migration was recorded, and the next run is not blocked by it.
    let superuser = hub::db::connect(&hub::config::DatabaseSettings {
        url: domain::Secret::new(pg.superuser_url()),
        ca_file: pg.ca_file(),
        max_connections: 1,
        acquire_timeout: Duration::from_secs(5),
        statement_timeout: Duration::from_secs(30),
    })
    .await
    .unwrap();
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = 'lanekeeper_migrator'",
    )
    .execute(&superuser)
    .await
    .unwrap();
    assert!(
        migrate::run(&pg.migrator_settings())
            .await
            .unwrap()
            .applied
            .is_empty()
    );
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('public.slow_one') IS NOT NULL")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert!(!exists, "the abandoned migration committed");
}

#[tokio::test]
async fn two_migrators_at_once_apply_each_migration_once() {
    let Some(pg) = database().await else { return };
    // Two Jobs started together (a retry on top of the first attempt): the lock makes one wait, and what each reports is
    // what it did.
    let (first, second) = (pg.migrator_settings(), pg.migrator_settings());
    let (a, b) = tokio::join!(migrate::run(&first), migrate::run(&second));
    let (a, b) = (a.unwrap(), b.unwrap());
    let mut versions: Vec<i64> = a.applied.iter().chain(&b.applied).map(|x| x.version).collect();
    versions.sort_unstable();
    let all: Vec<i64> = EMBEDDED.iter().map(|m| m.version).collect();
    assert_eq!(
        versions, all,
        "each migration was applied exactly once between them"
    );
    assert_eq!(a.latest, b.latest);
}

#[tokio::test]
async fn the_schema_probe_refuses_an_unmigrated_database_and_names_the_migration() {
    let Some(pg) = database().await else { return };
    let pool = pg.app_pool().await;

    let error = schema::verify(&pool).await.unwrap_err();
    assert!(
        matches!(error, SchemaError::Missing { version: 1, .. }),
        "{error:?}"
    );
    let shown = error.to_string();
    assert!(
        shown.contains("0001") && shown.contains("init") && shown.contains("hub migrate"),
        "{shown}"
    );
    assert_eq!(
        schema::missing(&pool).await.unwrap(),
        EMBEDDED.iter().map(|m| m.version).collect::<Vec<_>>()
    );

    // Migration by migration, the probe names exactly the ones not applied yet: each marker belongs to its migration, and none
    // is satisfied by an earlier one.
    let all = migrate::embedded();
    for applied in 1..=all.len() {
        migrate::run_set(&pg.migrator_settings(), &all[..applied])
            .await
            .unwrap();
        let want: Vec<i64> = all[applied..].iter().map(|m| m.version).collect();
        assert_eq!(
            schema::missing(&pool).await.unwrap(),
            want,
            "after migration {applied}"
        );
        let verdict = schema::verify(&pool).await;
        match want.first() {
            None => verdict.unwrap(),
            Some(first) => {
                let error = verdict.unwrap_err();
                assert!(
                    matches!(error, SchemaError::Missing { version, .. } if version == *first),
                    "{error:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_readiness_check_follows_the_schema() {
    use hub::ops::ReadyCheck;
    let Some(pg) = database().await else { return };
    let check = hub::schema::SchemaReady::new(pg.app_pool().await);
    assert!(!check.ready().await, "an unmigrated database is not ready");
    pg.migrate().await;
    assert!(check.ready().await);
}

#[tokio::test]
async fn the_migrator_connection_verifies_the_server() {
    let Some(pg) = database().await else { return };
    // S6: a CA that did not sign the server's certificate.
    let stranger = TestCa::new("not-the-databases-ca");
    let dir = tempfile::tempdir().unwrap();
    let wrong_ca = dir.path().join("wrong-ca.pem");
    std::fs::write(&wrong_ca, stranger.cert_pem()).unwrap();
    let mut settings = pg.migrator_settings();
    settings.ca_file = wrong_ca;
    let error = migrate::run(&settings).await.unwrap_err();
    assert_eq!(error, MigrateError::Connect);

    // A name the certificate does not hold (it holds `localhost`): verify-full checks the name, not only the chain.
    let by_address = pg.migrator_url().replace("@localhost:", "@127.0.0.1:");
    let error = migrate::run(&pg.migrator_settings_with(&by_address))
        .await
        .unwrap_err();
    assert_eq!(error, MigrateError::Connect);

    // Neither failure repeats the host, the user or the password, and neither touched the database.
    let shown = format!("{error} {error:?}");
    for secret in ["127.0.0.1", tls_pg::MIGRATOR_ROLE.1, "lanekeeper_migrator"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(!touched(&pg).await);

    // `sslmode=disable` in the URL is overridden, not obeyed: with the right CA the same URL works.
    assert!(pg.migrator_url().contains("sslmode=disable"));
    migrate::run(&pg.migrator_settings()).await.unwrap();
}

// ------------------------------------------------------------------------------------------------ the real binary

fn env(pg: &TlsPg, extra: &[(&str, String)]) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "LK_MIGRATOR_URL_FILE".to_owned(),
            pg.migrator_url_file().display().to_string(),
        ),
        ("LK_DB_CA_FILE".to_owned(), pg.ca_file().display().to_string()),
    ];
    for (name, value) in extra {
        env.retain(|(n, _)| n != name);
        env.push(((*name).to_owned(), value.clone()));
    }
    env
}

fn run_binary(pg: &TlsPg, extra: &[(&str, String)]) -> (Option<i32>, String) {
    let mut hub = child::Child::spawn(&child::real_binary(), &["migrate"], &env(pg, extra));
    let status = hub.exit_within(Duration::from_secs(120));
    (status.code(), hub.output())
}

#[tokio::test]
async fn the_binary_applies_the_migrations_prints_the_versions_and_never_a_credential() {
    let Some(pg) = database().await else { return };
    let (code, output) = run_binary(&pg, &[]);
    assert_eq!(code, Some(0), "{output}");
    // Every line is one JSON object (S10), like the serving hub's.
    for line in output.lines() {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "not JSON: {line}"
        );
    }
    for m in &EMBEDDED {
        assert!(
            output.contains(&format!(r#""version":{}"#, m.version)),
            "migration {} is not in the output: {output}",
            m.version
        );
    }
    assert!(output.contains("migration applied"), "{output}");
    assert!(output.contains(r#""applied":8"#), "{output}");
    for secret in [
        tls_pg::MIGRATOR_ROLE.1,
        tls_pg::APP_ROLE.1,
        tls_pg::PLANTED_PASSWORD,
        &pg.migrator_url(),
        "localhost",
    ] {
        assert!(!output.contains(secret), "the output holds {secret}: {output}");
    }

    // Again: nothing to do, and it says so.
    let (code, output) = run_binary(&pg, &[]);
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("nothing to apply"), "{output}");
    assert!(!output.contains("migration applied"), "{output}");
}

#[tokio::test]
async fn the_binary_refuses_the_app_role_and_a_bad_setting_with_the_right_exit_codes() {
    let Some(pg) = database().await else { return };
    // The app role's URL in the migrator's place: exit 1, a typed message, and nothing touched.
    let dir = tempfile::tempdir().unwrap();
    let app_url = dir.path().join("app-url");
    std::fs::write(&app_url, pg.url()).unwrap();
    let (code, output) = run_binary(&pg, &[("LK_MIGRATOR_URL_FILE", app_url.display().to_string())]);
    assert_eq!(code, Some(1), "{output}");
    assert!(output.contains("lanekeeper_migrator"), "{output}");
    assert!(!output.contains(tls_pg::APP_ROLE.1), "{output}");
    assert!(!touched(&pg).await);

    // A setting that is malformed is exit 2 and names the variable, not the value.
    let (code, output) = run_binary(&pg, &[("LK_MIGRATE_TIMEOUT_SECS", "planted-9137".to_owned())]);
    assert_eq!(code, Some(2), "{output}");
    assert!(
        output.contains("LK_MIGRATE_TIMEOUT_SECS is not valid"),
        "{output}"
    );
    assert!(!output.contains("planted-9137"), "{output}");
    assert!(!touched(&pg).await);
}
