//! A real Postgres 16 with pgvector for the integration tests, the way `crates/xtask/tests/db_migrations.rs` starts one: a
//! testcontainers container from the image the dev compose file pins by digest, the roles of `deploy/dev/init.sql`, and
//! `db/migrations` applied as `lanekeeper_migrator`. The hub side connects as `lanekeeper_app`, so a missing grant fails a test.
//!
//! Without a Docker daemon a test prints `SKIPPED` and passes; with `LK_REQUIRE_DOCKER=1` (set by `just verify-03b`) a missing
//! daemon fails it instead. `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:5432/postgres` points the tests at an existing
//! throwaway server instead of a container (each test creates its own database on it).
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

use async_trait::async_trait;
use domain::AuditId;
use ports::{AuditError, AuditEvent, AuditLog, Tx, canonical_json, chain_hash, genesis_hash};
use sha2::{Digest, Sha256};
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

static NEXT_DB: AtomicU32 = AtomicU32::new(0);

fn repo_root() -> PathBuf {
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
        connect_when_ready(&admin_options, Duration::from_secs(60))
            .await
            .map_err(|e| format!("the Postgres container did not become ready: {e}"))?;
        Ok(Self {
            host,
            port,
            admin_options,
            database: "lanekeeper".to_owned(),
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
        sqlx::query(&format!("CREATE DATABASE {database}"))
            .execute(&mut conn)
            .await
            .unwrap();
        let (host, port) = (base.get_host().to_owned(), base.get_port());
        Self {
            host,
            port,
            admin_options: base,
            database,
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

    /// The pool the hub uses: `lanekeeper_app`, with no DDL and only the granted DML.
    pub async fn app_pool(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(self.options(APP))
            .await
            .unwrap()
    }

    // ------------------------------------------------------------------------------------------ seed data

    /// A project and a swimlane in it, with the registered agent service account if given.
    pub async fn seed_swimlane(&self, pool: &PgPool, id: &str, agent_service_account: Option<&str>) {
        sqlx::query(
            "INSERT INTO projects (id, display_name) VALUES ('p1', 'Project 1') ON CONFLICT DO NOTHING",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO swimlanes (id, project_id, display_name, agent_service_account) VALUES ($1, 'p1', $1, $2)",
        )
        .bind(id)
        .bind(agent_service_account)
        .execute(pool)
        .await
        .unwrap();
    }

    /// An active admin, who issues join tokens.
    pub async fn seed_admin(&self, pool: &PgPool) -> (String, String) {
        let (tid, oid) = ("tenant-1".to_owned(), "admin-1".to_owned());
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) \
             VALUES ($1, $2, 'admin@example.test', 'Admin', 'admin', 'active') ON CONFLICT DO NOTHING",
        )
        .bind(&tid)
        .bind(&oid)
        .execute(pool)
        .await
        .unwrap();
        (tid, oid)
    }

    /// A join token for `swimlane`, stored hashed the way the issuer does (`SHA-256` of the text). It was created
    /// `age_secs` ago and expires `expires_in_secs` from now (negative: already expired).
    pub async fn seed_join_token(
        &self,
        pool: &PgPool,
        swimlane: &str,
        plaintext: &str,
        age_secs: i64,
        expires_in_secs: i64,
    ) -> i64 {
        let (tid, oid) = self.seed_admin(pool).await;
        let hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();
        sqlx::query(
            "INSERT INTO join_tokens (token_hash, swimlane_id, created_by_tid, created_by_oid, created_at, expires_at) \
             VALUES ($1::bytea, $2, $3, $4, now() - make_interval(secs => $5::double precision), now() + make_interval(secs => $6::double precision)) RETURNING id",
        )
        .bind(hash.as_slice())
        .bind(swimlane)
        .bind(tid)
        .bind(oid)
        .bind(age_secs)
        .bind(expires_in_secs)
        .fetch_one(pool)
        .await
        .unwrap()
        .get::<i64, _>("id")
    }

    pub async fn count(&self, pool: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }
}

/// `AuditLog` over the real `audit_events` table and the real chain, for tests that run a state change on Postgres. It stands
/// in for 03a's log; it appends in the caller's transaction, so an entry exists only if that transaction commits. It can be
/// told to fail, to show what a failing audit write rolls back.
#[derive(Debug, Default)]
pub struct PgTestAuditLog {
    fail: std::sync::atomic::AtomicBool,
}

impl PgTestAuditLog {
    pub fn failing() -> Self {
        let log = Self::default();
        log.fail.store(true, Ordering::SeqCst);
        log
    }

    pub fn set_failing(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }
}

#[async_trait]
impl AuditLog for PgTestAuditLog {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(AuditError::Unavailable);
        }
        let json = canonical_json(&e)?;
        let value: serde_json::Value = serde_json::from_slice(&json).map_err(|_| AuditError::Encode)?;
        let conn = tx.pg().map_err(|_| AuditError::WrongTx)?;
        // Appends are serialised for the length of the transaction, so the chain order is the commit order.
        sqlx::query("SELECT pg_advisory_xact_lock(7203)")
            .execute(&mut *conn)
            .await
            .map_err(|_| AuditError::Unavailable)?;
        let prev: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT hash::bytea FROM audit_events ORDER BY seq DESC LIMIT 1")
                .fetch_optional(&mut *conn)
                .await
                .map_err(|_| AuditError::Unavailable)?;
        let prev = prev.map_or_else(genesis_hash, |bytes| {
            domain::ContentHash::from_bytes(bytes.try_into().unwrap_or([0; 32]))
        });
        let hash = chain_hash(&prev, &json);
        let text = |v: &serde_json::Value| v.as_str().map(str::to_owned);
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO audit_events (prev_hash, hash, event_json, at, actor_kind, actor_ref, action, via, swimlane_id) \
             VALUES ($1::bytea, $2::bytea, $3, to_timestamp($4::double precision / 1000.0), $5, $6, $7, $8, $9) RETURNING seq",
        )
        .bind(prev.as_bytes().as_slice())
        .bind(hash.as_bytes().as_slice())
        .bind(String::from_utf8_lossy(&json).into_owned())
        .bind(e.at.unix_millis())
        .bind(text(&value["actor"]["kind"]))
        .bind(text(&value["actor"]["swimlane"]).or_else(|| text(&value["actor"]["user"])))
        .bind(text(&value["action"]))
        .bind(text(&value["via"]))
        .bind(text(&value["swimlane"]))
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| AuditError::Unavailable)?;
        Ok(AuditId::from(seq))
    }
}
