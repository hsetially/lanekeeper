//! A real Postgres 16 for the tests of `PgTokenStore`: the container image of `deploy/dev/compose.yaml`, or the throwaway server
//! named by `LK_TEST_PG_ADMIN_URL`, migrated as `lanekeeper_migrator` and used as `lanekeeper_app`, so the grants are the
//! production ones (no DELETE on `mcp_tokens`, UPDATE of two columns only).
//!
//! - Without either, a test prints a `SKIPPED` line and passes. With `LK_REQUIRE_DOCKER=1` (set by `just verify-07`) a missing
//!   database fails it instead. **A `SKIPPED` line means the test did not run.**
//! - On an external server each test makes its own database `lk_t_mcp_<pid>_<n>` and drops it again when the rig is dropped. If
//!   that server has no pgvector, the one block of `0001_init.sql` between the `lk:pgvector` markers is left out; nothing here
//!   uses it.
#![allow(dead_code, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use domain::AuditId;
use ports::{AuditError, AuditEvent, AuditLog, Tx};
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions};
use sqlx::{Connection, PgPool};
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const SUPERUSER_PASSWORD: &str = "lanekeeper-test-superuser-only";
const MIGRATOR: (&str, &str) = ("lanekeeper_migrator", "lanekeeper-migrator-dev-only");
const APP: (&str, &str) = ("lanekeeper_app", "lanekeeper-app-dev-only");
/// Serialises `deploy/dev/init.sql` across test threads (roles are cluster-wide).
const INIT_LOCK: &str = "hashtextextended('lanekeeper.test.run_init', 0)";
const PGVECTOR_BEGIN: &str = "-- lk:pgvector:begin";
const PGVECTOR_END: &str = "-- lk:pgvector:end";

static NEXT_DB: AtomicU32 = AtomicU32::new(0);

fn require_docker() -> bool {
    std::env::var("LK_REQUIRE_DOCKER").is_ok_and(|v| v == "1")
}

/// Fail when the database is required, otherwise say loudly that the test did not run.
pub fn skip_or_fail(why: &str) {
    assert!(
        !require_docker(),
        "LK_REQUIRE_DOCKER=1 but the database tests cannot run: {why}"
    );
    eprintln!("SKIPPED (set LK_REQUIRE_DOCKER=1 to make this a failure): {why}");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn compose_image() -> (String, String) {
    let text = std::fs::read_to_string(repo_root().join("deploy/dev/compose.yaml")).unwrap();
    let image = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("image: pgvector/pgvector:"))
        .expect("the compose file pins the pgvector image");
    ("pgvector/pgvector".to_owned(), image.trim().to_owned())
}

fn without_pgvector(sql: &str) -> String {
    let begin = sql.find(PGVECTOR_BEGIN).expect("begin marker");
    let end = sql.find(PGVECTOR_END).expect("end marker");
    assert!(begin < end);
    format!("{}{}", &sql[..begin], &sql[end + PGVECTOR_END.len()..])
}

async fn connect(opts: &PgConnectOptions) -> Result<PgConnection, sqlx::Error> {
    tokio::time::timeout(Duration::from_secs(10), PgConnection::connect_with(opts))
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)?
}

async fn connect_when_ready(
    opts: &PgConnectOptions,
    deadline: Duration,
) -> Result<PgConnection, sqlx::Error> {
    let start = tokio::time::Instant::now();
    let mut delay = Duration::from_millis(100);
    loop {
        match connect(opts).await {
            Ok(c) => return Ok(c),
            Err(e) if start.elapsed() >= deadline => return Err(e),
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        }
    }
}

pub struct TestDb {
    host: String,
    port: u16,
    admin_options: PgConnectOptions,
    pub database: String,
    pgvector: bool,
    drop_on_exit: bool,
    container: Option<ContainerAsync<GenericImage>>,
}

impl Drop for TestDb {
    fn drop(&mut self) {
        if self.drop_on_exit {
            let opts = self.admin_options.clone();
            let database = self.database.clone();
            // On a thread of its own: `drop` may run on a runtime worker, and the test runtime is shutting down.
            let _ = std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    return;
                };
                rt.block_on(async {
                    if let Ok(mut conn) = connect(&opts).await {
                        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)"))
                            .execute(&mut conn)
                            .await;
                    }
                });
            })
            .join();
        }
    }
}

impl TestDb {
    fn options(&self, user: &str, password: &str) -> PgConnectOptions {
        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(user)
            .password(password)
            .database(&self.database)
    }

    /// A superuser connection, for what the app role cannot do.
    pub async fn admin(&self) -> PgConnection {
        connect(&self.admin_options.clone().database(&self.database))
            .await
            .unwrap()
    }

    /// A pool as `lanekeeper_app`, the role the hub runs as.
    pub async fn app_pool(&self, max: u32) -> PgPool {
        PgPoolOptions::new()
            .max_connections(max)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(self.options(APP.0, APP.1))
            .await
            .unwrap()
    }

    /// The schema owner's connection.
    pub async fn migrator(&self) -> PgConnection {
        connect(&self.options(MIGRATOR.0, MIGRATOR.1)).await.unwrap()
    }

    async fn migrate(&self) {
        let dir = std::env::temp_dir().join(format!(
            "lk-mcp-migrations-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for entry in std::fs::read_dir(repo_root().join("db/migrations")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "sql") {
                let name = path.file_name().unwrap().to_owned();
                let sql = std::fs::read_to_string(&path).unwrap();
                let sql = if name.to_string_lossy().starts_with("0001_") && !self.pgvector {
                    without_pgvector(&sql)
                } else {
                    sql
                };
                std::fs::write(dir.join(name), sql).unwrap();
            }
        }
        let migrator = Migrator::new(dir.as_path()).await.unwrap();
        let mut conn = self.migrator().await;
        tokio::time::timeout(Duration::from_secs(120), migrator.run(&mut conn))
            .await
            .expect("migrate in time")
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh database with the roles of `deploy/dev/init.sql` and the migrations applied. `None` when no database is available
    /// and the environment does not require one.
    pub async fn ready() -> Option<TestDb> {
        let db = if let Ok(url) = std::env::var("LK_TEST_PG_ADMIN_URL") {
            Self::external(&url).await
        } else {
            match Self::container().await {
                Ok(db) => db,
                Err(why) => {
                    skip_or_fail(&why);
                    return None;
                }
            }
        };
        if !db.pgvector {
            // Nothing in this crate uses pgvector, so the tests run without it, unless the environment demands the real image.
            assert!(
                !require_docker(),
                "LK_REQUIRE_DOCKER=1 refuses a server without pgvector (the image of deploy/dev/compose.yaml has it)"
            );
            eprintln!(
                "NOTE: the server has no pgvector; the tests run without it (nothing in this crate needs it)"
            );
        }
        db.run_init().await;
        db.migrate().await;
        Some(db)
    }

    async fn container() -> Result<TestDb, String> {
        let (image, tag) = compose_image();
        let container = GenericImage::new(image, tag)
            .with_exposed_port(5432.tcp())
            .with_env_var("POSTGRES_PASSWORD", SUPERUSER_PASSWORD)
            .with_env_var("POSTGRES_DB", "lanekeeper")
            .with_cmd(["postgres", "-c", "fsync=off", "-c", "max_connections=300"])
            .start()
            .await
            .map_err(|e| format!("cannot start the Postgres container: {e}"))?;
        let host = container.get_host().await.map_err(|e| e.to_string())?.to_string();
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .map_err(|e| e.to_string())?;
        let admin_options = PgConnectOptions::new()
            .host(&host)
            .port(port)
            .username("postgres")
            .password(SUPERUSER_PASSWORD)
            .database("lanekeeper");
        connect_when_ready(&admin_options, Duration::from_secs(60))
            .await
            .map_err(|e| format!("the Postgres container did not become ready: {e}"))?;
        Ok(TestDb {
            host,
            port,
            admin_options,
            database: "lanekeeper".to_owned(),
            pgvector: true,
            drop_on_exit: false,
            container: Some(container),
        })
    }

    async fn external(url: &str) -> TestDb {
        let base = PgConnectOptions::from_str(url).expect("LK_TEST_PG_ADMIN_URL");
        let mut conn = connect(&base).await.expect("connect to LK_TEST_PG_ADMIN_URL");
        let database = format!(
            "lk_t_mcp_{}_{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        );
        sqlx::query(&format!("CREATE DATABASE {database}"))
            .execute(&mut conn)
            .await
            .unwrap();
        let pgvector: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'vector')")
                .fetch_one(&mut conn)
                .await
                .unwrap();
        let (host, port) = (base.get_host().to_owned(), base.get_port());
        TestDb {
            host,
            port,
            admin_options: base,
            database,
            pgvector,
            drop_on_exit: true,
            container: None,
        }
    }

    /// `deploy/dev/init.sql`: the same SQL the dev compose file runs.
    async fn run_init(&self) {
        let mut sql = std::fs::read_to_string(repo_root().join("deploy/dev/init.sql")).unwrap();
        sql = sql.replace(
            "ON DATABASE lanekeeper ",
            &format!("ON DATABASE {} ", self.database),
        );
        if !self.pgvector {
            sql = sql.replace("CREATE EXTENSION IF NOT EXISTS vector;", "");
        }
        // Roles are cluster-wide, so tests in parallel threads race on `CREATE ROLE`. A session advisory lock on the base database
        // (shared by every test) serialises them.
        let mut lock = connect(&self.admin_options).await.unwrap();
        sqlx::query(&format!("SELECT pg_advisory_lock({INIT_LOCK})"))
            .execute(&mut lock)
            .await
            .unwrap();
        let mut conn = self.admin().await;
        sqlx::raw_sql(&sql).execute(&mut conn).await.unwrap();
        sqlx::query(&format!("SELECT pg_advisory_unlock({INIT_LOCK})"))
            .execute(&mut lock)
            .await
            .unwrap();
    }

    /// Add a user row (the foreign key of `mcp_tokens` needs one).
    pub async fn add_user(&self, user: &domain::UserId) {
        let mut conn = self.admin().await;
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, 'Test', 'viewer', 'active')",
        )
        .bind(user.tid().to_string())
        .bind(user.oid().to_string())
        .bind(format!("{}@example.test", user.oid()))
        .execute(&mut conn)
        .await
        .unwrap();
    }

    /// A table the audit stand-in below writes to, which the app role may insert into and read.
    pub async fn make_test_audit_table(&self) {
        let mut conn = self.admin().await;
        sqlx::raw_sql(
            "CREATE TABLE test_audit (seq bigserial PRIMARY KEY, action text NOT NULL, actor text NOT NULL, diff text);\
             GRANT SELECT, INSERT ON test_audit TO lanekeeper_app;\
             GRANT USAGE ON SEQUENCE test_audit_seq_seq TO lanekeeper_app;",
        )
        .execute(&mut conn)
        .await
        .unwrap();
    }
}

/// The test's database, or return from the test (it printed `SKIPPED`).
#[macro_export]
macro_rules! db {
    () => {
        match $crate::pg::TestDb::ready().await {
            Some(db) => db,
            None => return,
        }
    };
}

/// The SQLSTATE of a database error.
pub fn sqlstate(e: &sqlx::Error) -> Option<String> {
    e.as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
}

/// An `AuditLog` that writes one row to `test_audit` **on the transaction it is given**. It stands in for the real log, which is
/// 03a's and is tested there: what these tests prove is that the token store records its event on the same transaction as its
/// row, so that the two commit or roll back together. With `fail_after_write` the row is written and the call then fails, which
/// is the case that matters (the event is in the transaction when the failure comes).
#[derive(Debug, Clone, Copy, Default)]
pub struct PgTestAudit {
    pub fail_after_write: bool,
}

#[async_trait]
impl AuditLog for PgTestAudit {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        let conn = tx.pg().map_err(|_| AuditError::WrongTx)?;
        let action = serde_json::to_value(e.action).unwrap();
        let actor = serde_json::to_string(&e.actor).unwrap();
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO test_audit (action, actor, diff) VALUES ($1, $2, $3) RETURNING seq",
        )
        .bind(action.as_str().unwrap())
        .bind(actor)
        .bind(e.diff)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| AuditError::Unavailable)?;
        if self.fail_after_write {
            return Err(AuditError::Unavailable);
        }
        Ok(AuditId::from(seq))
    }
}
