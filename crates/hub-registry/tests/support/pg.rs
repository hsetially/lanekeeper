//! A real Postgres 16 with pgvector for the integration tests, the way `crates/xtask/tests/db_migrations.rs` starts one: a
//! testcontainers container from the image the dev compose file pins by digest, the roles of `deploy/dev/init.sql`, and
//! `db/migrations` (0001 to 0003) applied as `lanekeeper_migrator`. The registry connects as `lanekeeper_app`, so a missing
//! grant fails a test.
//!
//! Without a Docker daemon a test prints `SKIPPED` and passes; with `LK_REQUIRE_DOCKER=1` (set by `just verify-05`) a missing
//! daemon fails it instead. `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:5432/postgres` points the tests at an existing
//! throwaway server instead of a container: each test creates its own database there and drops it when the test is over.
//!
//! **Every test database is created on [`COLLATION`] (D124), whatever the server's default is.** The image CI runs on has
//! `en_US.utf8` as its default; a developer's server may have `C` (or `C.UTF-8`), under which `text_pattern_ops` indexes can supply
//! an `ORDER BY` and the planner picks other indexes, so the plan snapshots of `tests/explain.rs` (plans of the pinned image) did
//! not hold on that server and the sort order of text differed. Creating the database from `template0` with the locale named makes
//! the plans and the order of strings the same on every server, or fails with a message when the server lacks the locale.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions, PgSslMode};
use sqlx::{Connection, Row};
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const IMAGE: &str = "pgvector/pgvector";
const IMAGE_TAG: &str = "pg16@sha256:7b822b0aac60967beb1ea5e576b8602c94c300a157d187f385ae3e0da199b90a";
const SUPERUSER_PASSWORD: &str = "lanekeeper-test-superuser-only";
const MIGRATOR: (&str, &str) = ("lanekeeper_migrator", "lanekeeper-migrator-dev-only");
const APP: (&str, &str) = ("lanekeeper_app", "lanekeeper-app-dev-only");
const INIT_LOCK: &str = "hashtextextended('lanekeeper.test.run_init', 0)";

/// The locale (collation and character class) of every test database: the default of the pinned image (Debian's `postgres` images set
/// `LANG=en_US.utf8`), under which the plan snapshots of `tests/explain.rs` are taken (D124).
pub const COLLATION: &str = "en_US.utf8";
/// The database the container's tests use: made by the same statement as an external server's, not the image's own `lanekeeper`.
const CONTAINER_DATABASE: &str = "lanekeeper_test";

static NEXT_DB: AtomicU32 = AtomicU32::new(0);

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn require_docker() -> bool {
    std::env::var("LK_REQUIRE_DOCKER").is_ok_and(|v| v == "1")
}

pub fn skip_or_fail(why: &str) {
    assert!(
        !require_docker(),
        "LK_REQUIRE_DOCKER=1 and Postgres is unavailable: {why}"
    );
    eprintln!("SKIPPED: {why}");
}

pub struct TestDb {
    host: String,
    port: u16,
    admin_options: PgConnectOptions,
    database: String,
    /// A database made on an external server is dropped with the test; a container goes away whole.
    drop_database: bool,
    _container: Option<ContainerAsync<GenericImage>>,
}

async fn connect(options: &PgConnectOptions) -> Result<PgConnection, sqlx::Error> {
    tokio::time::timeout(Duration::from_secs(10), PgConnection::connect_with(options))
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)?
}

async fn connect_when_ready(
    options: &PgConnectOptions,
    deadline: Duration,
) -> Result<PgConnection, sqlx::Error> {
    let start = tokio::time::Instant::now();
    let mut delay = Duration::from_millis(100);
    loop {
        match connect(options).await {
            Ok(c) => return Ok(c),
            Err(e) if start.elapsed() >= deadline => return Err(e),
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        }
    }
}

/// `CREATE DATABASE` on [`COLLATION`], from `template0` (the template that may be given a locale of its own); panics, saying how to fix
/// it, when the server cannot (a server without the locale, or a role that may not create databases).
async fn create_database(conn: &mut PgConnection, name: &str) {
    let sql = format!("CREATE DATABASE {name} TEMPLATE template0 ENCODING 'UTF8' LOCALE '{COLLATION}'");
    if let Err(e) = sqlx::query(&sql).execute(conn).await {
        panic!(
            "cannot create the test database {name} on {COLLATION}: {e}. The registry tests run on {COLLATION}, the default of the \
             pinned pgvector image (D124): install that locale on the server (Debian: `localedef -i en_US -f UTF-8 en_US.UTF-8`, \
             then restart the server) or use the image"
        );
    }
}

impl TestDb {
    /// A database with the roles of `deploy/dev/init.sql` and the migrations applied, or `None` (after saying why) when there
    /// is no Docker and it is not required.
    pub async fn ready() -> Option<Self> {
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
        db.run_init().await;
        db.migrate().await;
        Some(db)
    }

    async fn container() -> Result<Self, String> {
        let container = GenericImage::new(IMAGE, IMAGE_TAG)
            .with_exposed_port(5432.tcp())
            .with_env_var("POSTGRES_PASSWORD", SUPERUSER_PASSWORD)
            .with_env_var("POSTGRES_DB", "lanekeeper")
            .with_cmd(["postgres", "-c", "fsync=off"])
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
            .database("lanekeeper")
            .ssl_mode(PgSslMode::Disable);
        let mut conn = connect_when_ready(&admin_options, Duration::from_secs(60))
            .await
            .map_err(|e| format!("the Postgres container did not become ready: {e}"))?;
        create_database(&mut conn, CONTAINER_DATABASE).await;
        Ok(Self {
            host,
            port,
            admin_options,
            database: CONTAINER_DATABASE.to_owned(),
            drop_database: false,
            _container: Some(container),
        })
    }

    async fn external(url: &str) -> Self {
        let base = PgConnectOptions::from_str(url).expect("LK_TEST_PG_ADMIN_URL");
        let mut conn = connect(&base).await.expect("connect to LK_TEST_PG_ADMIN_URL");
        let database = format!(
            "lk_t_{}_{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        );
        create_database(&mut conn, &database).await;
        let (host, port) = (base.get_host().to_owned(), base.get_port());
        Self {
            host,
            port,
            admin_options: base,
            database,
            drop_database: true,
            _container: None,
        }
    }

    fn options(&self, role: (&str, &str)) -> PgConnectOptions {
        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(role.0)
            .password(role.1)
            .database(&self.database)
            .ssl_mode(PgSslMode::Disable)
    }

    /// The collation and the character class the database was created with, as the catalog states them.
    pub async fn locale(&self) -> (String, String) {
        let mut conn = self.admin().await;
        sqlx::query_as("SELECT datcollate, datctype FROM pg_database WHERE datname = current_database()")
            .fetch_one(&mut conn)
            .await
            .unwrap()
    }

    pub async fn admin(&self) -> PgConnection {
        connect(&self.admin_options.clone().database(&self.database))
            .await
            .unwrap()
    }

    /// `deploy/dev/init.sql`: the same SQL the dev compose file runs.
    async fn run_init(&self) {
        let mut sql = std::fs::read_to_string(repo_root().join("deploy/dev/init.sql")).unwrap();
        sql = sql.replace(
            "ON DATABASE lanekeeper ",
            &format!("ON DATABASE {} ", self.database),
        );
        // Roles are cluster-wide: tests that share a server race on CREATE ROLE, so one advisory lock serialises them.
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

    async fn migrate(&self) {
        let migrator = Migrator::new(repo_root().join("db/migrations").as_path())
            .await
            .unwrap();
        let mut conn = connect(&self.options(MIGRATOR)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(120), migrator.run(&mut conn))
            .await
            .expect("migrations timed out")
            .unwrap();
    }

    /// The pool the registry uses: `lanekeeper_app`, with no DDL and only the granted DML.
    pub async fn app_pool(&self) -> PgPool {
        self.app_pool_of(8).await
    }

    pub async fn app_pool_of(&self, connections: u32) -> PgPool {
        PgPoolOptions::new()
            .max_connections(connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(self.options(APP))
            .await
            .unwrap()
    }

    // ------------------------------------------------------------------------------------------ seed data

    /// A project and a swimlane in it, as an admin would create them.
    pub async fn seed_swimlane(&self, pool: &PgPool, id: &str) {
        sqlx::query(
            "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1') ON CONFLICT DO NOTHING",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO swimlanes (id, project_id, display_name) VALUES ($1, 'p1', $1)")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    pub async fn seed_tenant(&self, pool: &PgPool, swimlane: &str, tenant: &str) {
        sqlx::query(
            "INSERT INTO swimlane_tenants (swimlane_id, tenant_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(swimlane)
        .bind(tenant)
        .execute(pool)
        .await
        .unwrap();
    }

    pub async fn count(&self, pool: &PgPool, sql: &str) -> i64 {
        sqlx::query(sql).fetch_one(pool).await.unwrap().get::<i64, _>(0)
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        if !self.drop_database {
            return;
        }
        // The pools of the test may still be open: FORCE disconnects them. A runtime of its own, because the test's may be gone.
        let (options, database) = (self.admin_options.clone(), self.database.clone());
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            rt.block_on(async {
                if let Ok(mut conn) = connect(&options).await {
                    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)"))
                        .execute(&mut conn)
                        .await;
                }
            });
        })
        .join();
    }
}
