//! A real Postgres 16 for the integration tests (T2 on): the image of `deploy/dev/compose.yaml` in a container, migrated
//! as `lanekeeper_migrator`, used as `lanekeeper_app`, so the grants are the production ones (the audit tables are
//! append-only for the app, and so on).
//!
//! - Without a Docker daemon the tests print a `SKIPPED` line and pass. With `LK_REQUIRE_DOCKER=1` (set by `just verify-03a`
//!   and CI) a missing daemon fails them instead.
//! - `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:5432/postgres` uses an existing throwaway server instead of a
//!   container: each test creates its own database on it. If that server has no pgvector, the pgvector block of
//!   `0001_init.sql` is left out (this crate does not use it). `LK_REQUIRE_DOCKER=1` refuses such a server.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions};
use sqlx::{Connection, PgPool};
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const SUPERUSER_PASSWORD: &str = "lanekeeper-test-superuser-only";
const MIGRATOR: (&str, &str) = ("lanekeeper_migrator", "lanekeeper-migrator-dev-only");
const APP: (&str, &str) = ("lanekeeper_app", "lanekeeper-app-dev-only");

/// The advisory lock key that serialises `deploy/dev/init.sql` across test threads (roles are cluster-wide).
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

fn migrations_dir() -> PathBuf {
    repo_root().join("db/migrations")
}

/// The pinned image of the dev compose file, split into repository and tag (`pg16@sha256:...`).
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

/// Connect, retrying while the server starts. Bounded by `deadline`.
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

/// The container behind a [`TestDb`], as far as tearing it down goes. It is a trait so that the teardown can be tested without
/// Docker (`tests/pg_teardown.rs`).
pub trait Container: Send + 'static {
    fn id(&self) -> String;
    /// Remove the container through the async API. Needs a runtime.
    fn remove(&mut self) -> BoxFuture<'_, Result<(), String>>;
    /// Remove the container without a runtime or a reactor (a child process). Idempotent, bounded, never panics.
    fn remove_blocking(&self);
    /// Give the container up without running its own `Drop`, which needs a reactor.
    fn abandon(self: Box<Self>);
}

/// A real container started with `testcontainers`.
struct Docker {
    id: String,
    container: Option<ContainerAsync<GenericImage>>,
}

impl Container for Docker {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn remove(&mut self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            match self.container.take() {
                Some(container) => container.rm().await.map_err(|e| e.to_string()),
                None => Ok(()),
            }
        })
    }

    fn remove_blocking(&self) {
        let id = self.id.as_str();
        let child = std::process::Command::new("docker")
            .args(["rm", "--force", "--volumes", id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            eprintln!("could not run `docker rm`: remove the test container {id} by hand");
            return;
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return,
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        eprintln!("`docker rm` did not succeed: remove the test container {id} by hand");
    }

    fn abandon(mut self: Box<Self>) {
        if let Some(container) = self.container.take() {
            std::mem::forget(container);
        }
    }
}

pub struct TestDb {
    host: String,
    port: u16,
    admin_options: PgConnectOptions,
    pub database: String,
    pgvector: bool,
    /// Set for a database this rig created on an external server: dropped again when the rig is shut down or dropped, so a
    /// throwaway server does not accumulate one database per test run.
    drop_on_exit: bool,
    container: Option<Box<dyn Container>>,
}

async fn drop_database(opts: &PgConnectOptions, database: &str) {
    // `FORCE` ends any connection a pool of the test still holds.
    let stmt = format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)");
    if let Ok(mut conn) = connect(opts).await {
        let _ = sqlx::query(&stmt).execute(&mut conn).await;
    }
}

impl Drop for TestDb {
    /// The fallback for a rig that was not shut down (a test; a benchmark that panicked). Best effort, and it never needs a
    /// reactor of its own: the benchmarks drop their rig outside any runtime, where `testcontainers`' own `Drop` panics.
    fn drop(&mut self) {
        if let Some(container) = self.container.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                // On a runtime's thread (every test): the container's own `Drop` is supported there and has always worked.
                drop(container);
            } else {
                // Outside any runtime (a benchmark's teardown): its own `Drop` would panic, so remove the container with a
                // child process and let the Rust value go without running its `Drop`.
                container.remove_blocking();
                container.abandon();
            }
        }
        if self.drop_on_exit {
            // On a thread of its own: `drop` may run on a runtime worker, and the test runtime is shutting down.
            let opts = self.admin_options.clone();
            let database = self.database.clone();
            let _ = std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    return;
                };
                rt.block_on(drop_database(&opts, &database));
            })
            .join();
        }
    }
}

impl TestDb {
    /// Tear the rig down inside a live runtime: the container is removed through the async API and an external server's
    /// database is dropped. Benchmarks call it with `rt.block_on(db.shutdown())`; tests may rely on `Drop` instead.
    pub async fn shutdown(mut self) {
        if let Some(mut container) = self.container.take() {
            let id = container.id();
            match tokio::time::timeout(Duration::from_secs(60), container.remove()).await {
                Ok(Ok(())) => {}
                other => {
                    let why = match other {
                        Ok(Err(e)) => e,
                        _ => "timed out".to_owned(),
                    };
                    eprintln!("could not remove the test container {id} ({why}); trying `docker rm`");
                    container.remove_blocking();
                }
            }
        }
        if std::mem::take(&mut self.drop_on_exit) {
            drop_database(&self.admin_options, &self.database).await;
        }
    }

    /// A rig around a container and nothing else, for the tests of the teardown itself (`tests/pg_teardown.rs`).
    pub fn detached(container: Box<dyn Container>) -> TestDb {
        TestDb {
            host: "127.0.0.1".to_owned(),
            port: 1,
            admin_options: PgConnectOptions::new().host("127.0.0.1").port(1),
            database: "detached".to_owned(),
            pgvector: false,
            drop_on_exit: false,
            container: Some(container),
        }
    }

    fn options(&self, user: &str, password: &str) -> PgConnectOptions {
        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(user)
            .password(password)
            .database(&self.database)
    }

    /// A superuser connection, for tests that must do what the app role cannot (disable a trigger to tamper with a row).
    pub async fn admin(&self) -> PgConnection {
        connect(&self.admin_options.clone().database(&self.database))
            .await
            .unwrap()
    }

    /// The schema owner's connection.
    pub async fn migrator(&self) -> PgConnection {
        connect(&self.options(MIGRATOR.0, MIGRATOR.1)).await.unwrap()
    }

    /// Connection options for `lanekeeper_app`, the role the hub runs as.
    pub fn app_options(&self) -> PgConnectOptions {
        self.options(APP.0, APP.1)
    }

    /// A pool as `lanekeeper_app`. `max` connections; every acquire is bounded.
    pub async fn app_pool(&self, max: u32) -> PgPool {
        PgPoolOptions::new()
            .max_connections(max)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(self.app_options())
            .await
            .unwrap()
    }

    /// A pool as another role (a test role created with `admin()`).
    pub async fn pool_as(&self, user: &str, password: &str, max: u32) -> PgPool {
        PgPoolOptions::new()
            .max_connections(max)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(self.options(user, password))
            .await
            .unwrap()
    }

    /// Apply `db/migrations` as `lanekeeper_migrator`.
    async fn migrate(&self) {
        let dir = if self.pgvector {
            migrations_dir()
        } else {
            let dir = std::env::temp_dir().join(format!(
                "lk-identity-migrations-{}-{}",
                std::process::id(),
                NEXT_DB.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let sql = std::fs::read_to_string(migrations_dir().join("0001_init.sql")).unwrap();
            std::fs::write(dir.join("0001_init.sql"), without_pgvector(&sql)).unwrap();
            dir
        };
        let migrator = Migrator::new(dir.as_path()).await.unwrap();
        let mut conn = self.migrator().await;
        tokio::time::timeout(Duration::from_secs(120), migrator.run(&mut conn))
            .await
            .expect("migrate in time")
            .unwrap();
    }

    /// A fresh database with the roles of `deploy/dev/init.sql` and the migrations applied. `None` when no database is
    /// available and the environment does not require one.
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
            // Only returns when the requirement is off: carry on without pgvector, which this crate does not use.
            skip_or_fail("the server has no pgvector; running without it (nothing in this crate needs it)");
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
        let id = container.id().to_owned();
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
            container: Some(Box::new(Docker {
                id,
                container: Some(container),
            })),
        })
    }

    async fn external(url: &str) -> TestDb {
        let base = PgConnectOptions::from_str(url).expect("LK_TEST_PG_ADMIN_URL");
        let mut conn = connect(&base).await.expect("connect to LK_TEST_PG_ADMIN_URL");
        let database = format!(
            "lk_i_{}_{}",
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
        // Roles are cluster-wide, so tests in parallel threads race on `CREATE ROLE`. A session advisory lock on the base
        // database (shared by every test) serialises them.
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
}

/// The test's database, or return from the test (it printed `SKIPPED`).
#[macro_export]
macro_rules! db {
    () => {
        match $crate::support::pg::TestDb::ready().await {
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
