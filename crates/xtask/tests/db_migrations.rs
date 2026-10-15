//! Integration tests for `db/migrations` (prompt 01, T6; S9, S21; `0002_audit_verify_state.sql` for prompt 03a;
//! `0003_registry.sql` for prompt 05; `0004_write_additions.sql` for prompt 06; `0005_mcp_tokens.sql` and `0006_mcp_tokens_created_not_future.sql` for prompt 07).
//!
//! They run against a real Postgres 16 with pgvector, started with testcontainers from the same image digest as
//! `deploy/dev/compose.yaml`. The tests use the runtime `sqlx::migrate::Migrator` and no `query!` macros, so no
//! `.sqlx` metadata is needed.
//!
//! Without a Docker daemon the tests print a `SKIPPED` line and pass. With `LK_REQUIRE_DOCKER=1` (set by
//! `just verify-01` and CI) a missing daemon fails them instead (plan Q7).
//!
//! `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:5432/postgres` points the tests at an existing superuser
//! connection instead of a container (a throwaway server only: every test creates a database on it). When that
//! server has no pgvector, the one block of `0001_init.sql` between the `lk:pgvector` markers is left out (every later
//! migration is applied as it is) and the
//! checks that need it print `NOT PROVEN`. `LK_REQUIRE_DOCKER=1` refuses such a server.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgListener};
use sqlx::{Connection, Row};
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// The image the tests start. Keep it equal to `deploy/dev/compose.yaml` (`image_matches_compose` checks).
const IMAGE: &str = "pgvector/pgvector";
const IMAGE_TAG: &str = "pg16@sha256:7b822b0aac60967beb1ea5e576b8602c94c300a157d187f385ae3e0da199b90a";

const SUPERUSER_PASSWORD: &str = "lanekeeper-test-superuser-only";
const MIGRATOR: (&str, &str) = ("lanekeeper_migrator", "lanekeeper-migrator-dev-only");
const APP: (&str, &str) = ("lanekeeper_app", "lanekeeper-app-dev-only");

/// The advisory lock key that serialises `deploy/dev/init.sql` across test threads (see `run_init`).
const INIT_LOCK: &str = "hashtextextended('lanekeeper.test.run_init', 0)";

const PGVECTOR_BEGIN: &str = "-- lk:pgvector:begin";
const PGVECTOR_END: &str = "-- lk:pgvector:end";

/// Every table `0001_init.sql` creates (plan Appendix B, Q1). Partitions of `sentinel_records` are not listed.
/// Later migrations list their tables in `LATER_TABLES`, with the file that creates them.
const EXPECTED_TABLES: &[&str] = &[
    // identity
    "users",
    "access_requests",
    "sessions",
    "github_credentials",
    "join_tokens",
    // topology
    "projects",
    "swimlanes",
    "swimlane_tenants",
    "agents",
    "agent_connections",
    "agent_acks",
    "path_mappings",
    "merge_modes",
    "host_tiers",
    "folder_mappings",
    "services",
    "service_state",
    // content
    "blobs",
    "blob_refs",
    "file_observations",
    "baselines",
    "file_state",
    "intentional_divergence",
    "settings_index",
    "settings_index_blobs",
    "git_tree_index",
    "swimlane_merkle",
    "effective_tree_hashes",
    "findings",
    "severity_rules",
    // writes
    "proposals",
    "drafts",
    "approvals",
    "pr_links",
    "pr_jobs",
    "idempotency_keys",
    "webhook_deliveries",
    // audit and events
    "audit_events",
    "audit_checkpoints",
    "outbox",
    "leases",
    "sync_windows",
    // sentinel
    "sentinels",
    "sentinel_records",
    "os_login_user_map",
    // docs
    "docs",
    "doc_versions",
    "doc_chunks",
    "documented_flags",
    // settings
    "retention_settings",
    "notification_settings",
    "feature_flags",
];

/// Tables added by migrations after `0001_init.sql`: (table, the migration file that creates it).
const LATER_TABLES: &[(&str, &str)] = &[
    ("audit_verify_state", "0002_audit_verify_state.sql"),
    // Prompt 05 (registry): see `REGISTRY_TABLES` and `migration_0003_*` below.
    ("swimlane_registry", "0003_registry.sql"),
    ("swimlane_deployments", "0003_registry.sql"),
    ("swimlane_folders", "0003_registry.sql"),
    ("release_hints", "0003_registry.sql"),
    ("effective_files", "0003_registry.sql"),
    ("adoptions", "0003_registry.sql"),
    ("adoption_resolutions", "0003_registry.sql"),
    ("delta_staging", "0003_registry.sql"),
    // Prompt 07 (MCP server): personal tokens. See `migration_0005_*` and `mcp_tokens_*` below.
    ("mcp_tokens", "0005_mcp_tokens.sql"),
];

/// The tables `0003_registry.sql` creates, all `Grant::Mutable`: the pipeline upserts and prunes them, and every one hangs
/// off `swimlanes` with `ON DELETE CASCADE`, which `no_cascading_foreign_key_touches_a_protected_table` allows only for tables
/// the app may delete from.
const REGISTRY_TABLES: &[&str] = &[
    "swimlane_registry",
    "swimlane_deployments",
    "swimlane_folders",
    "release_hints",
    "effective_files",
    "adoptions",
    "adoption_resolutions",
    "delta_staging",
];

/// Every table the migrations create, `_sqlx_migrations` aside.
fn all_expected_tables() -> BTreeSet<&'static str> {
    EXPECTED_TABLES
        .iter()
        .copied()
        .chain(LATER_TABLES.iter().map(|(t, _)| *t))
        .collect()
}

/// What `lanekeeper_app` may do with a table. The grant decision for every table lives here and in
/// `0001_init.sql`; a table without a decision fails `every_table_has_a_grant_decision` (plan Q19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grant {
    /// SELECT, INSERT. History that nobody may rewrite or prune: the audit chain and sentinel records.
    AppendOnly,
    /// SELECT, INSERT, DELETE. Content-addressed or write-once rows that retention may remove.
    Immutable,
    /// SELECT, INSERT, UPDATE, DELETE.
    Mutable,
    /// SELECT, UPDATE. A singleton row the migration inserts and the app only reads and updates: it can neither
    /// create nor remove it (`audit_verify_state`).
    StateRow,
    /// Nothing. The migrator's own bookkeeping.
    None,
}

impl Grant {
    fn privileges(self) -> BTreeSet<&'static str> {
        match self {
            Grant::AppendOnly => ["SELECT", "INSERT"].into(),
            Grant::Immutable => ["SELECT", "INSERT", "DELETE"].into(),
            Grant::Mutable => ["SELECT", "INSERT", "UPDATE", "DELETE"].into(),
            Grant::StateRow => ["SELECT", "UPDATE"].into(),
            Grant::None => BTreeSet::new(),
        }
    }
}

/// The only column-level `UPDATE` grants `lanekeeper_app` holds: (table, the columns). A column-level grant is invisible to
/// `has_table_privilege`, so a table that has one is listed here, and no other table may have one.
const COLUMN_UPDATES: &[(&str, &[&str])] = &[("mcp_tokens", &["revoked_at", "last_used_at"])];

fn grant_decisions() -> BTreeMap<&'static str, Grant> {
    let mut m = BTreeMap::new();
    for t in ["audit_events", "audit_checkpoints", "sentinel_records"] {
        m.insert(t, Grant::AppendOnly);
    }
    for t in [
        "blobs",
        "blob_refs",
        "settings_index",
        "settings_index_blobs",
        "git_tree_index",
        "doc_versions",
        "webhook_deliveries",
    ] {
        m.insert(t, Grant::Immutable);
    }
    for t in EXPECTED_TABLES {
        m.entry(t).or_insert(Grant::Mutable);
    }
    for t in REGISTRY_TABLES {
        m.insert(t, Grant::Mutable);
    }
    m.insert("audit_verify_state", Grant::StateRow);
    // 0004: an approval is a decision nobody rewrites or removes, like the audit chain.
    m.insert("approvals", Grant::AppendOnly);
    // 0005: a personal token is written once and never removed. The app may also set two of its columns, `revoked_at` and
    // `last_used_at`, which is a column-level grant that `has_table_privilege` does not see: `COLUMN_UPDATES` names it, and
    // `every_table_has_a_grant_decision` and `the_app_can_revoke_and_touch_a_token_and_change_nothing_else` check it.
    m.insert("mcp_tokens", Grant::AppendOnly);
    m.insert("_sqlx_migrations", Grant::None);
    m
}

// ---------------------------------------------------------------------------------------------------------------
// Harness

fn require_docker() -> bool {
    std::env::var("LK_REQUIRE_DOCKER").is_ok_and(|v| v == "1")
}

/// Fail when the database is required, otherwise say loudly that the test did not run.
fn skip_or_fail(require: bool, why: &str) {
    assert!(
        !require,
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

fn migration_sql() -> String {
    std::fs::read_to_string(migrations_dir().join("0001_init.sql")).unwrap()
}

/// Every migration file as (version, file name, text), in version order.
fn migration_files() -> Vec<(u32, String, String)> {
    let mut files: Vec<(u32, String, String)> = std::fs::read_dir(migrations_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| Path::new(n).extension().is_some_and(|x| x == "sql"))
        .map(|n| {
            let version: u32 = n.split('_').next().unwrap().parse().unwrap();
            let sql = std::fs::read_to_string(migrations_dir().join(&n)).unwrap();
            (version, n, sql)
        })
        .collect();
    files.sort();
    files
}

/// The text of one migration file.
fn migration_file(name: &str) -> String {
    std::fs::read_to_string(migrations_dir().join(name)).unwrap()
}

/// The lines of `sql` that are not full-line comments.
fn code_lines(sql: &str) -> Vec<&str> {
    sql.lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect()
}

/// `sql` without the block between the pgvector markers.
fn without_pgvector(sql: &str) -> String {
    let begin = sql.find(PGVECTOR_BEGIN).expect("begin marker");
    let end = sql.find(PGVECTOR_END).expect("end marker");
    assert!(begin < end);
    format!("{}{}", &sql[..begin], &sql[end + PGVECTOR_END.len()..])
}

static NEXT_DB: AtomicU32 = AtomicU32::new(0);

struct TestDb {
    host: String,
    port: u16,
    admin_options: PgConnectOptions,
    database: String,
    pgvector: bool,
    _container: Option<ContainerAsync<GenericImage>>,
}

async fn within<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out: {what}"))
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

impl TestDb {
    fn options(&self, user: &str, password: &str) -> PgConnectOptions {
        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(user)
            .password(password)
            .database(&self.database)
    }

    async fn admin(&self) -> PgConnection {
        connect(&self.admin_options.clone().database(&self.database))
            .await
            .unwrap()
    }

    async fn as_role(&self, role: (&str, &str)) -> PgConnection {
        connect(&self.options(role.0, role.1)).await.unwrap()
    }

    async fn migrator(&self) -> PgConnection {
        self.as_role(MIGRATOR).await
    }

    async fn app(&self) -> PgConnection {
        self.as_role(APP).await
    }

    /// Apply `db/migrations` as `lanekeeper_migrator`.
    async fn migrate(&self) {
        self.migrate_upto(u32::MAX).await;
    }

    /// Apply the migrations up to and including `version`, as `lanekeeper_migrator`. Without pgvector only the block
    /// of `0001_init.sql` between the markers is left out; every other file is applied as it is.
    async fn migrate_upto(&self, version: u32) {
        let dir = if self.pgvector && version == u32::MAX {
            migrations_dir()
        } else {
            let dir = std::env::temp_dir().join(format!(
                "lk-migrations-{}-{}",
                std::process::id(),
                NEXT_DB.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            for (v, name, sql) in migration_files() {
                if v > version {
                    continue;
                }
                let sql = if v == 1 && !self.pgvector {
                    without_pgvector(&sql)
                } else {
                    sql
                };
                std::fs::write(dir.join(name), sql).unwrap();
            }
            dir
        };
        let migrator = Migrator::new(dir.as_path()).await.unwrap();
        let mut conn = self.migrator().await;
        within("migrate", migrator.run(&mut conn)).await.unwrap();
    }

    /// A database with the roles of `deploy/dev/init.sql` and the migrations applied.
    async fn ready() -> Option<TestDb> {
        let db = Self::bootstrap().await?;
        db.migrate().await;
        Some(db)
    }

    /// A database with the roles but no migrations.
    async fn bootstrap() -> Option<TestDb> {
        let db = if let Ok(url) = std::env::var("LK_TEST_PG_ADMIN_URL") {
            Self::external(&url).await
        } else {
            match Self::container().await {
                Ok(db) => db,
                Err(why) => {
                    skip_or_fail(require_docker(), &why);
                    return None;
                }
            }
        };
        if !db.pgvector {
            skip_or_fail(
                require_docker(),
                "the server has no pgvector; running without it (pgvector checks are NOT PROVEN)",
            );
            // `skip_or_fail` only returns when the requirement is off: carry on in degraded mode.
        }
        db.run_init().await;
        Some(db)
    }

    async fn container() -> Result<TestDb, String> {
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
            _container: Some(container),
        })
    }

    async fn external(url: &str) -> TestDb {
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
            _container: None,
        }
    }

    /// `deploy/dev/init.sql`: the same SQL the dev compose file runs (Q6).
    async fn run_init(&self) {
        let mut sql = std::fs::read_to_string(repo_root().join("deploy/dev/init.sql")).unwrap();
        sql = sql.replace(
            "ON DATABASE lanekeeper ",
            &format!("ON DATABASE {} ", self.database),
        );
        if !self.pgvector {
            sql = sql.replace("CREATE EXTENSION IF NOT EXISTS vector;", "");
        }
        // Roles are cluster-wide, so tests running in parallel threads against one server race on `CREATE ROLE`
        // (`duplicate key ... pg_authid_rolname_index`). An advisory lock is scoped to the database of the connection
        // that takes it, so the lock must sit on a connection to one database every test shares: the base database of
        // `admin_options`, never the per-test database. It is a session lock, released at unlock or when `lock` drops.
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

macro_rules! db {
    () => {
        match TestDb::ready().await {
            Some(db) => db,
            None => return,
        }
    };
}

fn sqlstate(e: &sqlx::Error) -> Option<String> {
    e.as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
}

const INSUFFICIENT_PRIVILEGE: &str = "42501";
/// The SQLSTATE the append-only trigger raises (`restrict_violation`), so a trigger refusal differs from a grant refusal.
const RESTRICT_VIOLATION: &str = "23001";
const UNIQUE_VIOLATION: &str = "23505";
const CHECK_VIOLATION: &str = "23514";
const NOT_NULL_VIOLATION: &str = "23502";
const FOREIGN_KEY_VIOLATION: &str = "23503";
/// `generated_always`: a value was given for a `GENERATED ALWAYS AS IDENTITY` column.
const GENERATED_ALWAYS: &str = "428C9";

async fn expect_sqlstate(conn: &mut PgConnection, sql: &str, want: &str) {
    let err = sqlx::query(sql)
        .execute(&mut *conn)
        .await
        .expect_err(&format!("expected failure: {sql}"));
    assert_eq!(sqlstate(&err).as_deref(), Some(want), "{sql}: {err}");
}

/// Appends an audit event whose `hash` is `SHA-256(prev_hash || event_json)`, as `ports::audit::chain_hash` computes it.
async fn append_audit(conn: &mut PgConnection, prev: &[u8], json: &str) -> Result<Vec<u8>, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO audit_events (prev_hash, hash, event_json, at, actor_kind, action, via) \
         VALUES ($1::bytea, sha256($1::bytea || convert_to($2::text, 'UTF8')), $2::text, now(), 'system', 'settings_changed', 'system') \
         RETURNING hash",
    )
    .bind(prev)
    .bind(json)
    .fetch_one(conn)
    .await
}

const GENESIS: [u8; 32] = [0; 32];

// ---------------------------------------------------------------------------------------------------------------
// Tests that need no database

#[test]
fn image_matches_compose() {
    let compose = std::fs::read_to_string(repo_root().join("deploy/dev/compose.yaml")).unwrap();
    assert!(
        compose.contains(&format!("{IMAGE}:{IMAGE_TAG}")),
        "the test image must be the compose image, pinned by digest (S19)"
    );
}

#[test]
fn pgvector_statements_are_isolated_between_markers() {
    let sql = migration_sql();
    assert_eq!(sql.matches(PGVECTOR_BEGIN).count(), 1);
    assert_eq!(sql.matches(PGVECTOR_END).count(), 1);
    let rest = without_pgvector(&sql);
    for needle in ["vector(384)", "hnsw", "CREATE EXTENSION IF NOT EXISTS vector"] {
        assert!(
            !rest.contains(needle),
            "`{needle}` must sit between the pgvector markers"
        );
    }
}

#[test]
fn migration_never_creates_roles_or_superuser_objects() {
    // Roles pre-exist (Q6): created by deploy/dev/init.sql, the test harness and CloudNativePG managed roles.
    for (_, name, sql) in migration_files() {
        let sql = sql.to_uppercase();
        for banned in [
            "CREATE ROLE",
            "CREATE USER",
            "ALTER ROLE",
            "ALTER DEFAULT PRIVILEGES",
            "SUPERUSER",
        ] {
            let hits = code_lines(&sql).iter().filter(|l| l.contains(banned)).count();
            assert_eq!(hits, 0, "{name} must not contain `{banned}`");
        }
    }
}

#[test]
fn every_expected_table_is_created_by_the_migration_text() {
    let sql = migration_sql();
    for t in EXPECTED_TABLES {
        assert!(
            sql.contains(&format!("CREATE TABLE {t} (")) || sql.contains(&format!("CREATE TABLE {t}\n")),
            "no CREATE TABLE for {t}"
        );
    }
    for (t, file) in LATER_TABLES {
        let sql = migration_file(file);
        assert!(
            sql.contains(&format!("CREATE TABLE {t} (")),
            "no CREATE TABLE for {t} in {file}"
        );
    }
}

/// `0002_audit_verify_state.sql` is expand-only (the contract-change rule): one new table, one row, one grant. It
/// drops, alters, deletes and truncates nothing, and it does not name the audit tables.
#[test]
fn migration_0002_is_additive_only() {
    let sql = migration_file("0002_audit_verify_state.sql");
    let code = code_lines(&sql).join("\n").to_uppercase();
    for banned in [
        "DROP ",
        "ALTER ",
        "TRUNCATE",
        "DELETE",
        "RENAME",
        "GRANT ALL",
        "CASCADE",
        "AUDIT_EVENTS",
        "AUDIT_CHECKPOINTS",
        "CREATE TRIGGER",
        "CREATE FUNCTION",
        "CREATE INDEX",
    ] {
        assert!(
            !code.contains(banned),
            "0002 must not contain `{banned}` outside comments"
        );
    }
    assert_eq!(code.matches("CREATE TABLE").count(), 1, "one new table");
    assert_eq!(code.matches("INSERT INTO").count(), 1, "one seeded row");
    assert_eq!(code.matches("GRANT ").count(), 1, "one grant");
    assert!(
        code.contains("GRANT SELECT, UPDATE ON AUDIT_VERIFY_STATE TO LANEKEEPER_APP"),
        "the app gets SELECT and UPDATE on the one table, nothing more"
    );
}

#[test]
fn missing_docker_is_skipped_unless_required() {
    skip_or_fail(false, "unit test: this is the skip path and must not panic");
}

#[test]
#[should_panic(expected = "LK_REQUIRE_DOCKER=1")]
fn missing_docker_fails_when_required() {
    skip_or_fail(true, "unit test: this is the failure path");
}

// ---------------------------------------------------------------------------------------------------------------
// Tests against a real server

#[tokio::test]
async fn migration_applies_on_pg16() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    let mut admin = db.admin().await;
    let version: String = sqlx::query_scalar("SHOW server_version_num")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    assert!(version.starts_with("16"), "PostgreSQL 16 expected, got {version}");

    db.migrate().await;

    let mut conn = db.migrator().await;
    let tables: BTreeSet<String> = sqlx::query_scalar(
        "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND NOT c.relispartition AND c.relname <> '_sqlx_migrations'",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap()
    .into_iter()
    .collect();
    let expected: BTreeSet<String> = all_expected_tables().into_iter().map(str::to_owned).collect();
    assert_eq!(tables, expected);

    // Re-running is a no-op: each migration is recorded once, each applied on top of the one before it (0002 on 0001, 0003 on
    // 0002, 0004 on 0003, 0005 on 0004, 0006 on 0005). The schema is owned by the migrator, never by the app.
    db.migrate().await;
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    let files: Vec<i64> = migration_files().iter().map(|(v, _, _)| i64::from(*v)).collect();
    assert_eq!(applied, files);
    assert_eq!(applied, [1, 2, 3, 4, 5, 6]);
    let foreign: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_tables WHERE schemaname = 'public' AND tableowner <> 'lanekeeper_migrator'",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(foreign, 0, "every table is owned by lanekeeper_migrator");
}

#[tokio::test]
async fn migration_refuses_to_run_as_another_role() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    let sql = if db.pgvector {
        migration_sql()
    } else {
        without_pgvector(&migration_sql())
    };
    let mut app = db.app().await;
    let err = sqlx::raw_sql(&sql)
        .execute(&mut app)
        .await
        .expect_err("must refuse");
    assert!(err.to_string().contains("lanekeeper_migrator"), "{err}");
    let mut admin = db.admin().await;
    let tables: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    assert_eq!(tables, 0, "nothing was created");
}

/// The migrator owns every object. `0002` carries the same guard as `0001`, so a superuser or the app cannot create the
/// table under another owner by running the file by hand.
#[tokio::test]
async fn migration_0002_refuses_to_run_as_another_role() {
    let db = db!();
    let sql = migration_file("0002_audit_verify_state.sql");
    for role in [APP, ("postgres", SUPERUSER_PASSWORD)] {
        let mut conn = if role.0 == "postgres" {
            db.admin().await
        } else {
            db.as_role(role).await
        };
        let err = sqlx::raw_sql(&sql)
            .execute(&mut conn)
            .await
            .expect_err("must refuse");
        assert_eq!(sqlstate(&err).as_deref(), Some("55000"), "{}: {err}", role.0);
        assert!(
            err.to_string().contains("lanekeeper_migrator"),
            "{}: {err}",
            role.0
        );
    }
    let mut admin = db.admin().await;
    let owner: String =
        sqlx::query_scalar("SELECT tableowner::text FROM pg_tables WHERE tablename = 'audit_verify_state'")
            .fetch_one(&mut admin)
            .await
            .unwrap();
    assert_eq!(owner, "lanekeeper_migrator");
}

/// 0002 adds `audit_verify_state` and changes nothing else: every other relation keeps its grants, and every trigger,
/// constraint and column stays as 0001 left it, the append-only audit tables included.
#[tokio::test]
async fn migration_0002_adds_only_audit_verify_state() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    db.migrate_upto(1).await;
    let mut admin = db.admin().await;
    let snapshot = |sql: &'static str| async {
        let mut c = db.admin().await;
        sqlx::query_scalar::<_, String>(sql)
            .fetch_all(&mut c)
            .await
            .unwrap()
    };
    // Relations with their owner and ACL; triggers; constraints; columns; functions. All except the new table.
    let queries: [&'static str; 5] = [
        "SELECT c.relname::text || ' ' || c.relkind::text || ' ' || c.relowner::regrole::text || ' ' || coalesce(c.relacl::text, '-') \
         FROM pg_class c WHERE c.relnamespace = 'public'::regnamespace AND c.relname NOT LIKE 'audit\\_verify\\_state%' ORDER BY 1",
        "SELECT t.tgrelid::regclass::text || ' ' || pg_get_triggerdef(t.oid, true) || ' ' || t.tgenabled::text \
         FROM pg_trigger t WHERE NOT t.tgisinternal AND t.tgrelid::regclass::text <> 'audit_verify_state' ORDER BY 1",
        "SELECT conrelid::regclass::text || ' ' || conname || ' ' || pg_get_constraintdef(oid) FROM pg_constraint \
         WHERE connamespace = 'public'::regnamespace AND conrelid::regclass::text <> 'audit_verify_state' ORDER BY 1",
        "SELECT attrelid::regclass::text || ' ' || attname || ' ' || format_type(atttypid, atttypmod) || ' ' || attnotnull::text \
         FROM pg_attribute WHERE attnum > 0 AND NOT attisdropped AND attrelid IN (SELECT oid FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relkind IN ('r', 'p')) \
         AND attrelid::regclass::text <> 'audit_verify_state' ORDER BY 1",
        "SELECT p.proname || ' ' || p.proowner::regrole::text || ' ' || coalesce(p.proacl::text, '-') || ' ' || md5(p.prosrc) \
         FROM pg_proc p WHERE p.pronamespace = 'public'::regnamespace ORDER BY 1",
    ];
    let mut before = Vec::new();
    for q in queries {
        before.push(snapshot(q).await);
    }
    let had_table: bool = sqlx::query_scalar("SELECT to_regclass('public.audit_verify_state') IS NOT NULL")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    assert!(!had_table, "0001 alone has no audit_verify_state");
    for (q, b) in queries.iter().zip(&before) {
        assert!(!b.is_empty(), "the snapshot must see something: {q}");
    }

    // Up to 0002 only: 0003 and 0004 change existing tables on purpose (`migration_0003_changes_existing_objects_only_by_adding`,
    // `migration_0004_changes_only_what_it_names`).
    db.migrate_upto(2).await;

    for (q, b) in queries.iter().zip(&before) {
        assert_eq!(&snapshot(q).await, b, "0002 changed an existing object: {q}");
    }
    let new_relations: Vec<String> = sqlx::query_scalar(
        "SELECT relname::text FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relname LIKE 'audit\\_verify\\_state%' ORDER BY 1",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap();
    assert_eq!(new_relations, ["audit_verify_state", "audit_verify_state_pkey"]);
}

/// The row the migration seeds: the one and only row, with the starting values.
#[tokio::test]
async fn audit_verify_state_starts_as_one_row_of_zero() {
    let db = db!();
    let mut app = db.app().await;
    let rows = sqlx::query("SELECT id, unfinished_runs, last_error, updated_at <= now() AS not_future, updated_at > now() - interval '1 hour' AS recent FROM audit_verify_state")
        .fetch_all(&mut app)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one row");
    let row = &rows[0];
    assert!(row.get::<bool, _>("id"));
    assert_eq!(row.get::<i32, _>("unfinished_runs"), 0);
    assert_eq!(row.get::<Option<String>, _>("last_error"), None);
    assert!(
        row.get::<bool, _>("not_future") && row.get::<bool, _>("recent"),
        "updated_at defaults to the migration time"
    );

    // The column defaults, from the catalog: id true, updated_at now().
    let mut admin = db.admin().await;
    let defaults: BTreeMap<String, Option<String>> = sqlx::query(
        "SELECT a.attname::text, pg_get_expr(d.adbin, d.adrelid) FROM pg_attribute a \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = 'public.audit_verify_state'::regclass AND a.attnum > 0 AND NOT a.attisdropped",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.get(0), r.get(1)))
    .collect();
    assert_eq!(defaults["id"].as_deref(), Some("true"));
    assert_eq!(defaults["updated_at"].as_deref(), Some("now()"));
    assert_eq!(
        defaults["unfinished_runs"], None,
        "no default: the writer always says the count"
    );
    assert_eq!(defaults["last_error"], None);
    assert_eq!(defaults.len(), 4, "{defaults:?}");
}

/// The CHECKs and the primary key keep the table harmless as data, even for a caller that may write to it.
#[tokio::test]
async fn audit_verify_state_constraints_reject_bad_rows() {
    let db = db!();
    let mut app = db.app().await;
    let mut owner = db.migrator().await;

    // A negative count.
    expect_sqlstate(
        &mut app,
        "UPDATE audit_verify_state SET unfinished_runs = -1, updated_at = now()",
        CHECK_VIOLATION,
    )
    .await;
    // A missing count.
    expect_sqlstate(
        &mut app,
        "UPDATE audit_verify_state SET unfinished_runs = NULL",
        NOT_NULL_VIOLATION,
    )
    .await;
    // A missing time.
    expect_sqlstate(
        &mut app,
        "UPDATE audit_verify_state SET updated_at = NULL",
        NOT_NULL_VIOLATION,
    )
    .await;
    // The singleton key can only be true: the app cannot move the row to another key.
    expect_sqlstate(
        &mut app,
        "UPDATE audit_verify_state SET id = false",
        CHECK_VIOLATION,
    )
    .await;
    // An over-long error text: 201 characters. (200 is the cap.)
    expect_sqlstate(
        &mut app,
        "UPDATE audit_verify_state SET last_error = repeat('x', 201)",
        CHECK_VIOLATION,
    )
    .await;
    // A second row, even for the owner: another id is refused by the CHECK, the same id by the primary key.
    expect_sqlstate(
        &mut owner,
        "INSERT INTO audit_verify_state (id, unfinished_runs, updated_at) VALUES (false, 0, now())",
        CHECK_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut owner,
        "INSERT INTO audit_verify_state (id, unfinished_runs, updated_at) VALUES (true, 0, now())",
        UNIQUE_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut owner,
        "INSERT INTO audit_verify_state (unfinished_runs) VALUES (0)",
        UNIQUE_VIOLATION,
    )
    .await;

    // Nothing above changed the row.
    let (runs, err, count): (i32, Option<String>, i64) = {
        let row = sqlx::query("SELECT unfinished_runs, last_error, (SELECT count(*) FROM audit_verify_state) FROM audit_verify_state")
            .fetch_one(&mut app)
            .await
            .unwrap();
        (row.get(0), row.get(1), row.get(2))
    };
    assert_eq!((runs, err, count), (0, None, 1));

    // The edges that are allowed: zero, a large count, no error, exactly 200 characters (multi-byte ones count as one).
    for sql in [
        "UPDATE audit_verify_state SET unfinished_runs = 0, last_error = NULL, updated_at = now()",
        "UPDATE audit_verify_state SET unfinished_runs = 2147483647, updated_at = now()",
        "UPDATE audit_verify_state SET last_error = repeat('x', 200), updated_at = now()",
        "UPDATE audit_verify_state SET last_error = repeat(chr(233), 200), updated_at = now()",
        "UPDATE audit_verify_state SET last_error = '57014', updated_at = now()",
    ] {
        let done = sqlx::query(sql).execute(&mut app).await.unwrap();
        assert_eq!(done.rows_affected(), 1, "{sql}");
    }
}

/// `lanekeeper_app` reads and updates the row. It cannot insert (the migration seeds it), delete or truncate, so the
/// singleton cannot be created again or removed through the app (S9, S21).
#[tokio::test]
async fn app_can_read_and_update_audit_verify_state_but_not_insert_delete_or_truncate() {
    let db = db!();
    let mut admin = db.admin().await;
    let mut held = BTreeSet::new();
    for p in [
        "SELECT",
        "INSERT",
        "UPDATE",
        "DELETE",
        "TRUNCATE",
        "REFERENCES",
        "TRIGGER",
    ] {
        let has: bool = sqlx::query_scalar(
            "SELECT has_table_privilege('lanekeeper_app', 'public.audit_verify_state'::regclass, $1)",
        )
        .bind(p)
        .fetch_one(&mut admin)
        .await
        .unwrap();
        if has {
            held.insert(p);
        }
    }
    assert_eq!(held, BTreeSet::from(["SELECT", "UPDATE"]));
    for p in ["INSERT", "REFERENCES"] {
        let any_column: bool = sqlx::query_scalar(
            "SELECT has_any_column_privilege('lanekeeper_app', 'public.audit_verify_state'::regclass, $1)",
        )
        .bind(p)
        .fetch_one(&mut admin)
        .await
        .unwrap();
        assert!(!any_column, "no column-level {p} either");
    }

    let mut app = db.app().await;
    let done = sqlx::query(
        "UPDATE audit_verify_state SET unfinished_runs = 3, last_error = 'timeout', updated_at = now() WHERE id",
    )
    .execute(&mut app)
    .await
    .unwrap();
    assert_eq!(done.rows_affected(), 1);
    let (runs, err): (i32, Option<String>) = {
        let row = sqlx::query("SELECT unfinished_runs, last_error FROM audit_verify_state WHERE id")
            .fetch_one(&mut app)
            .await
            .unwrap();
        (row.get(0), row.get(1))
    };
    assert_eq!((runs, err.as_deref()), (3, Some("timeout")));

    for sql in [
        "DELETE FROM audit_verify_state",
        "DELETE FROM audit_verify_state WHERE id",
        "TRUNCATE audit_verify_state",
        "INSERT INTO audit_verify_state (id, unfinished_runs, updated_at) VALUES (true, 0, now())",
        "INSERT INTO audit_verify_state (id, unfinished_runs, updated_at) VALUES (true, 0, now()) ON CONFLICT DO NOTHING",
        "INSERT INTO audit_verify_state (id, unfinished_runs, updated_at) VALUES (true, 0, now()) \
         ON CONFLICT (id) DO UPDATE SET unfinished_runs = 0",
        "ALTER TABLE audit_verify_state DROP CONSTRAINT audit_verify_state_unfinished_runs_check",
        "DROP TABLE audit_verify_state",
    ] {
        expect_sqlstate(&mut app, sql, INSUFFICIENT_PRIVILEGE).await;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_verify_state")
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(count, 1, "the singleton is still there");
}

/// The table is operational state, not a chain: no trigger, so nothing in it is tamper-evident and nothing in it is
/// an audit event (see db/migrations/README.md). It must also not reference or be referenced by any other table.
#[tokio::test]
async fn audit_verify_state_is_plain_operational_state() {
    let db = db!();
    let mut admin = db.admin().await;
    let triggers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger WHERE tgrelid = 'public.audit_verify_state'::regclass AND NOT tgisinternal",
    )
    .fetch_one(&mut admin)
    .await
    .unwrap();
    assert_eq!(triggers, 0);
    let fks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint WHERE contype = 'f' AND 'public.audit_verify_state'::regclass IN (conrelid, confrelid)",
    )
    .fetch_one(&mut admin)
    .await
    .unwrap();
    assert_eq!(fks, 0);
    // An update writes no audit event: the app does the update and the chain has not grown.
    let mut app = db.app().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&mut app)
        .await
        .unwrap();
    sqlx::query("UPDATE audit_verify_state SET unfinished_runs = 1, updated_at = now()")
        .execute(&mut app)
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn extensions_and_lz4_toast_are_active() {
    let db = db!();
    let mut conn = db.migrator().await;

    let trgm: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_extension WHERE extname = 'pg_trgm'")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(trgm, 1, "pg_trgm");
    let trgm_index: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE tablename = 'settings_index' AND indexdef ILIKE '%gin_trgm_ops%'",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(trgm_index.contains("setting_path"), "{trgm_index}");

    // blobs.content uses lz4 TOAST compression: by column setting and in practice.
    let compression: String =
        sqlx::query_scalar("SELECT attcompression::text FROM pg_attribute WHERE attrelid = 'blobs'::regclass AND attname = 'content'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(compression, "l", "blobs.content must be COMPRESSION lz4");
    sqlx::query(
        "WITH b AS (SELECT convert_to(repeat('tx-infinity: enabled' || chr(10), 20000), 'UTF8') AS c) \
         INSERT INTO blobs (hash, content, size) SELECT sha256(c), c, octet_length(c) FROM b",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let used: Option<String> = sqlx::query_scalar("SELECT pg_column_compression(content)::text FROM blobs")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(
        used.as_deref(),
        Some("lz4"),
        "a stored blob is compressed with lz4, not pglz"
    );

    if db.pgvector {
        let vector: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_extension WHERE extname = 'vector'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(vector, 1, "pgvector");
        let ty: String = sqlx::query_scalar(
            "SELECT format_type(atttypid, atttypmod) FROM pg_attribute \
             WHERE attrelid = 'doc_chunks'::regclass AND attname = 'embedding'",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!(ty, "vector(384)");
        let hnsw: String = sqlx::query_scalar(
            "SELECT indexdef FROM pg_indexes WHERE tablename = 'doc_chunks' AND indexdef ILIKE '%USING hnsw%'",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert!(hnsw.contains("embedding"), "{hnsw}");
    } else {
        eprintln!(
            "NOT PROVEN: pgvector extension, doc_chunks.embedding vector(384) and its HNSW index (no pgvector on this server)"
        );
    }
}

#[tokio::test]
async fn app_cannot_update_audit_events() {
    let db = db!();
    let mut app = db.app().await;
    append_audit(&mut app, &GENESIS, r#"{"n":1}"#).await.unwrap();
    expect_sqlstate(
        &mut app,
        "UPDATE audit_events SET action = 'x'",
        INSUFFICIENT_PRIVILEGE,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "UPDATE audit_checkpoints SET gcs_object = 'x'",
        INSUFFICIENT_PRIVILEGE,
    )
    .await;
}

#[tokio::test]
async fn app_cannot_delete_audit_events() {
    let db = db!();
    let mut app = db.app().await;
    append_audit(&mut app, &GENESIS, r#"{"n":1}"#).await.unwrap();
    expect_sqlstate(&mut app, "DELETE FROM audit_events", INSUFFICIENT_PRIVILEGE).await;
    expect_sqlstate(&mut app, "DELETE FROM audit_checkpoints", INSUFFICIENT_PRIVILEGE).await;
}

#[tokio::test]
async fn app_cannot_truncate_audit_events() {
    let db = db!();
    let mut app = db.app().await;
    expect_sqlstate(&mut app, "TRUNCATE audit_events", INSUFFICIENT_PRIVILEGE).await;
    expect_sqlstate(&mut app, "TRUNCATE audit_checkpoints", INSUFFICIENT_PRIVILEGE).await;
}

#[tokio::test]
async fn trigger_blocks_update_and_delete_even_for_owner() {
    let db = db!();
    // The owner has every privilege, so only the trigger can refuse these (a different SQLSTATE from a grant failure).
    let mut owner = db.migrator().await;
    let head = append_audit(&mut owner, &GENESIS, r#"{"n":1}"#).await.unwrap();
    sqlx::query(
        "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
         VALUES (1, $1, 'v1', decode('00', 'hex'), 'gs://bucket/checkpoint-1')",
    )
    .bind(&head)
    .execute(&mut owner)
    .await
    .unwrap();
    for sql in [
        "UPDATE audit_events SET action = 'x'",
        "DELETE FROM audit_events",
        // audit_events is referenced by audit_checkpoints, so it can only be truncated together with it.
        "TRUNCATE audit_events, audit_checkpoints",
        "UPDATE audit_checkpoints SET gcs_object = 'x'",
        "DELETE FROM audit_checkpoints",
        "TRUNCATE audit_checkpoints",
    ] {
        let err = sqlx::query(sql).execute(&mut owner).await.expect_err(sql);
        assert_eq!(
            sqlstate(&err).as_deref(),
            Some(RESTRICT_VIOLATION),
            "{sql}: {err}"
        );
        assert!(err.to_string().contains("append-only"), "{sql}: {err}");
    }
    // The superuser is bound too: the triggers fire in every session_replication_role (tgenabled = 'A').
    let mut admin = db.admin().await;
    let modes: Vec<String> = sqlx::query_scalar(
        "SELECT tgenabled::text FROM pg_trigger \
         WHERE tgrelid IN ('audit_events'::regclass, 'audit_checkpoints'::regclass) AND NOT tgisinternal",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap();
    assert!(
        modes.len() >= 4,
        "row and truncate triggers on both tables, found {}",
        modes.len()
    );
    assert!(modes.iter().all(|m| m == "A"), "{modes:?}");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn app_cannot_run_ddl() {
    let db = db!();
    let mut app = db.app().await;
    for sql in [
        "CREATE TABLE public.evil (id int)",
        "CREATE TABLE audit_events_copy (LIKE audit_events)",
        "ALTER TABLE audit_events ADD COLUMN evil int",
        "ALTER TABLE audit_events DISABLE TRIGGER ALL",
        "DROP TRIGGER audit_events_no_row_change ON audit_events",
        "DROP TABLE blobs",
        "CREATE INDEX evil ON blobs (size)",
        "CREATE FUNCTION public.evil() RETURNS int LANGUAGE sql AS 'SELECT 1'",
        "CREATE SCHEMA evil",
        "ALTER ROLE lanekeeper_app SUPERUSER",
        "DROP TABLE _sqlx_migrations",
    ] {
        expect_sqlstate(&mut app, sql, INSUFFICIENT_PRIVILEGE).await;
    }
    let mut admin = db.admin().await;
    let row = sqlx::query(
        "SELECT rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls FROM pg_roles WHERE rolname = 'lanekeeper_app'",
    )
    .fetch_one(&mut admin)
    .await
    .unwrap();
    for col in 0..5 {
        assert!(
            !row.get::<bool, _>(col),
            "lanekeeper_app has a role attribute it must not have (column {col})"
        );
    }
    let owned: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM pg_class c JOIN pg_roles r ON r.oid = c.relowner WHERE r.rolname = 'lanekeeper_app') \
         + (SELECT count(*) FROM pg_proc p JOIN pg_roles r ON r.oid = p.proowner WHERE r.rolname = 'lanekeeper_app')",
    )
    .fetch_one(&mut admin)
    .await
    .unwrap();
    assert_eq!(owned, 0, "lanekeeper_app owns no relation or function");
}

#[tokio::test]
async fn app_has_only_insert_select_on_audit_events() {
    let db = db!();
    let mut admin = db.admin().await;
    for table in ["audit_events", "audit_checkpoints"] {
        let mut held = BTreeSet::new();
        for p in [
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "TRUNCATE",
            "REFERENCES",
            "TRIGGER",
        ] {
            let has: bool = sqlx::query_scalar("SELECT has_table_privilege('lanekeeper_app', $1, $2)")
                .bind(table)
                .bind(p)
                .fetch_one(&mut admin)
                .await
                .unwrap();
            if has {
                held.insert(p);
            }
        }
        assert_eq!(held, BTreeSet::from(["INSERT", "SELECT"]), "{table}");
        let column_update: bool =
            sqlx::query_scalar("SELECT has_any_column_privilege('lanekeeper_app', $1, 'UPDATE')")
                .bind(table)
                .fetch_one(&mut admin)
                .await
                .unwrap();
        assert!(!column_update, "{table}: no column-level UPDATE either");
    }
    // The app can append and read, and the sequence behind `seq` works for it.
    let mut app = db.app().await;
    let h = append_audit(&mut app, &GENESIS, r#"{"n":1}"#).await.unwrap();
    let seq: i64 = sqlx::query_scalar("SELECT seq FROM audit_events WHERE hash = $1")
        .bind(h)
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(seq, 1);
}

#[tokio::test]
async fn every_table_has_a_grant_decision() {
    let db = db!();
    let mut admin = db.admin().await;
    let decisions = grant_decisions();

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND NOT c.relispartition ORDER BY 1",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap();
    let known: BTreeSet<&str> = decisions.keys().copied().collect();
    let found: BTreeSet<&str> = tables.iter().map(String::as_str).collect();
    assert_eq!(
        found, known,
        "a table was added or removed without a grant decision (add it to `grant_decisions` and to 0001's GRANT lists)"
    );

    for table in &tables {
        let mut held = BTreeSet::new();
        for p in [
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "TRUNCATE",
            "REFERENCES",
            "TRIGGER",
        ] {
            let has: bool =
                sqlx::query_scalar("SELECT has_table_privilege('lanekeeper_app', $1::regclass, $2)")
                    .bind(format!("public.{table}"))
                    .bind(p)
                    .fetch_one(&mut admin)
                    .await
                    .unwrap();
            if has {
                held.insert(p);
            }
        }
        assert_eq!(
            held,
            decisions[table.as_str()].privileges(),
            "privileges of lanekeeper_app on {table}"
        );
        // A grant on some columns only: the table-level answer above is `false` for it, so ask about the columns.
        let any_column_update: bool =
            sqlx::query_scalar("SELECT has_any_column_privilege('lanekeeper_app', $1::regclass, 'UPDATE')")
                .bind(format!("public.{table}"))
                .fetch_one(&mut admin)
                .await
                .unwrap();
        let allowed = held.contains("UPDATE") || COLUMN_UPDATES.iter().any(|(t, _)| t == table);
        assert_eq!(
            any_column_update, allowed,
            "column-level UPDATE of lanekeeper_app on {table} (list it in COLUMN_UPDATES, with its columns, or remove it)"
        );
    }

    // Sequences: USAGE where a serial column needs it, never UPDATE; schema objects stay with the migrator.
    let sequences: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind = 'S' AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = c.oid AND d.deptype = 'i') ORDER BY 1",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap();
    assert_eq!(
        sequences,
        ["audit_events_seq_seq"],
        "only the bigserial sequences are standalone; the rest are identity columns"
    );
    for s in &sequences {
        for (p, want) in [("USAGE", true), ("SELECT", false), ("UPDATE", false)] {
            let has: bool =
                sqlx::query_scalar("SELECT has_sequence_privilege('lanekeeper_app', $1::regclass, $2)")
                    .bind(format!("public.{s}"))
                    .bind(p)
                    .fetch_one(&mut admin)
                    .await
                    .unwrap();
            assert_eq!(has, want, "{s} {p}");
        }
    }
    let create: bool =
        sqlx::query_scalar("SELECT has_schema_privilege('lanekeeper_app', 'public', 'CREATE')")
            .fetch_one(&mut admin)
            .await
            .unwrap();
    assert!(!create);
}

#[tokio::test]
async fn outbox_insert_notifies_on_commit_only() {
    let db = db!();
    let mut listener = PgListener::connect_with(&sqlx::PgPool::connect_lazy_with(db.options(APP.0, APP.1)))
        .await
        .unwrap();
    listener.listen("lanekeeper_outbox").await.unwrap();

    let mut writer = db.app().await;
    let mut other = db.app().await;
    let insert = "INSERT INTO outbox (event) VALUES ('{\"type\":\"resync\"}'::jsonb) RETURNING id";

    // 1. A rolled-back insert never notifies.
    let mut tx = writer.begin().await.unwrap();
    let rolled_back: i64 = sqlx::query_scalar(insert).fetch_one(&mut *tx).await.unwrap();
    tx.rollback().await.unwrap();

    // 2. An insert in an open transaction does not notify until it commits: a later, committed insert from another
    //    session arrives first, and the pending one only after its COMMIT.
    let mut pending = writer.begin().await.unwrap();
    let pending_id: i64 = sqlx::query_scalar(insert).fetch_one(&mut *pending).await.unwrap();
    let marker_id: i64 = sqlx::query_scalar(insert).fetch_one(&mut other).await.unwrap();
    assert_ne!(rolled_back, pending_id);

    let first = tokio::time::timeout(Duration::from_secs(10), listener.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.channel(), "lanekeeper_outbox");
    assert_eq!(
        first.payload(),
        marker_id.to_string(),
        "the committed marker arrives first; neither the rolled-back nor the open insert did"
    );

    pending.commit().await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(10), listener.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        second.payload(),
        pending_id.to_string(),
        "the notification is delivered by the commit"
    );
}

#[tokio::test]
async fn pr_links_unique_open_per_swimlane_path_hash() {
    let db = db!();
    let mut app = db.app().await;
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('p1', 'P1')")
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query("INSERT INTO swimlanes (id, project_id, display_name) VALUES ('sit1', 'p1', 'SIT 1')")
        .execute(&mut app)
        .await
        .unwrap();
    let hash = vec![7u8; 32];
    let other_hash = vec![8u8; 32];
    let insert = "INSERT INTO pr_links (swimlane_id, path, nfs_hash, repo, branch, pr_number, state) \
                  VALUES ('sit1', 'a/b.yml', $1, 'tenant', 'sit1', $2, $3)";
    sqlx::query(insert)
        .bind(&hash)
        .bind(1_i32)
        .bind("open")
        .execute(&mut app)
        .await
        .unwrap();

    // A second open PR for the same (swimlane, path, nfs_hash) is refused ...
    let err = sqlx::query(insert)
        .bind(&hash)
        .bind(2_i32)
        .bind("open")
        .execute(&mut app)
        .await
        .expect_err("duplicate open PR");
    assert_eq!(sqlstate(&err).as_deref(), Some(UNIQUE_VIOLATION), "{err}");
    // ... but another hash, or another path, is a different change ...
    sqlx::query(insert)
        .bind(&other_hash)
        .bind(3_i32)
        .bind("open")
        .execute(&mut app)
        .await
        .unwrap();
    // ... and once the first PR is closed or merged, a new open one is allowed, and closed ones may repeat.
    sqlx::query("UPDATE pr_links SET state = 'closed' WHERE pr_number = 1")
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query(insert)
        .bind(&hash)
        .bind(4_i32)
        .bind("open")
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query("UPDATE pr_links SET state = 'merged' WHERE pr_number = 4")
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query(insert)
        .bind(&hash)
        .bind(5_i32)
        .bind("merged")
        .execute(&mut app)
        .await
        .unwrap();
    // Values outside the PR state machine are refused.
    let err = sqlx::query(insert)
        .bind(&hash)
        .bind(6_i32)
        .bind("reopened")
        .execute(&mut app)
        .await
        .expect_err("bad state");
    assert_eq!(sqlstate(&err).as_deref(), Some(CHECK_VIOLATION), "{err}");
}

#[tokio::test]
async fn audit_prev_hash_cannot_fork() {
    let db = db!();
    let mut app = db.app().await;
    let first = append_audit(&mut app, &GENESIS, r#"{"n":1}"#).await.unwrap();
    // Two successors of the same event would fork the chain.
    append_audit(&mut app, &first, r#"{"n":2}"#).await.unwrap();
    let err = append_audit(&mut app, &first, r#"{"n":3}"#)
        .await
        .expect_err("fork");
    assert_eq!(sqlstate(&err).as_deref(), Some(UNIQUE_VIOLATION), "{err}");
    // Neither may a second event claim the genesis position.
    let err = append_audit(&mut app, &GENESIS, r#"{"n":4}"#)
        .await
        .expect_err("second genesis");
    assert_eq!(sqlstate(&err).as_deref(), Some(UNIQUE_VIOLATION), "{err}");
    // A hash that is not SHA-256(prev_hash || event_json) is refused by the table itself.
    let err = sqlx::query(
        "INSERT INTO audit_events (prev_hash, hash, event_json, at, actor_kind, action, via) \
         VALUES (decode(repeat('ab', 32), 'hex'), decode(repeat('cd', 32), 'hex'), '{}', now(), 'system', 'settings_changed', 'system')",
    )
    .execute(&mut app)
    .await
    .expect_err("bad hash");
    assert_eq!(sqlstate(&err).as_deref(), Some(CHECK_VIOLATION), "{err}");
    let chain: Vec<(Vec<u8>, Vec<u8>)> = sqlx::query("SELECT prev_hash, hash FROM audit_events ORDER BY seq")
        .map(|r: sqlx::postgres::PgRow| (r.get(0), r.get(1)))
        .fetch_all(&mut app)
        .await
        .unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[1].0, chain[0].1);
}

#[tokio::test]
async fn approvals_reject_self_approval() {
    let db = db!();
    let mut app = db.app().await;
    for (oid, name) in [("alice", "Alice"), ("bob", "Bob")] {
        sqlx::query("INSERT INTO users (tid, oid, email, display_name, status, role) VALUES ('t', $1, $2, $2, 'active', 'admin')")
            .bind(oid)
            .bind(name)
            .execute(&mut app)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('p1', 'P1')")
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query("INSERT INTO swimlanes (id, project_id, display_name) VALUES ('sit1', 'p1', 'SIT 1')")
        .execute(&mut app)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, via, expires_at) \
         VALUES ('t', 'alice', 'edit', 'sit1', ARRAY['a.yml'], 'ui', now() + interval '72 hours') RETURNING id",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    let approve = "INSERT INTO approvals (proposal_id, decision, approver_tid, approver_oid) VALUES ($1, 'approved', 't', $2)";
    let err = sqlx::query(approve)
        .bind(id)
        .bind("alice")
        .execute(&mut app)
        .await
        .expect_err("self approval");
    assert_eq!(sqlstate(&err).as_deref(), Some(CHECK_VIOLATION), "{err}");
    sqlx::query(approve)
        .bind(id)
        .bind("bob")
        .execute(&mut app)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------------------------------------------
// 0004_write_additions.sql (prompt 06): notify proposals, the credential expiry, and the approvals hardening

/// `sql` without comments, string literals (their text) and dollar-quoted bodies, split into statements of upper-case words.
/// A body (`DO $$ ... $$`, `AS $$ ... $$`) is not one of the statement's words: it is checked on its own, below.
fn statements_of(sql: &str) -> Vec<Vec<String>> {
    let code = code_lines(sql).join("\n");
    // Dollar-quoted bodies first (a literal inside one could hold a `;`), then literals.
    let mut bodiless = String::new();
    let mut rest = code.as_str();
    while let Some(open) = rest.find("$$") {
        bodiless.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let close = after.find("$$").expect("a dollar-quoted body is closed");
        rest = &after[close + 2..];
    }
    bodiless.push_str(rest);
    let mut out = Vec::new();
    for statement in without_literals(&bodiless).split(';') {
        let words = sql_words(statement);
        if !words.is_empty() {
            out.push(words);
        }
    }
    out
}

/// `sql` with the text of every single-quoted literal removed.
fn without_literals(sql: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in sql.chars() {
        if c == '\'' {
            inside = !inside;
        } else if !inside {
            out.push(c);
        }
    }
    out
}

fn sql_words(sql: &str) -> Vec<String> {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_uppercase)
        .collect()
}

/// The first row-writing statement in `sql`, bodies included, if there is one: `INSERT INTO [schema.]t`, `UPDATE [ONLY]
/// [schema.]t [[AS] alias] SET` (or the `DO UPDATE SET` of an upsert), `DELETE FROM [ONLY] ...`, MERGE INTO, TRUNCATE or
/// COPY, or any `EXECUTE` that is not `EXECUTE FUNCTION` or `EXECUTE PROCEDURE` (a trigger's): dynamic SQL is a string,
/// and the text of strings is not read here, so it is refused whatever it holds. (`UPDATE` as a trigger event, in a
/// privilege list or as an `ON UPDATE` key action is not a write.)
fn row_writing_statement(sql: &str) -> Option<String> {
    let code = without_literals(&code_lines(sql).join("\n"));
    let words = sql_words(&code);
    words.iter().enumerate().find_map(|(i, w)| {
        let next = |n: usize| words.get(i + n).map(String::as_str);
        let writes = match w.as_str() {
            "TRUNCATE" | "COPY" => true,
            "INSERT" | "MERGE" => next(1) == Some("INTO"),
            "DELETE" => next(1) == Some("FROM"),
            "EXECUTE" => !matches!(next(1), Some("FUNCTION" | "PROCEDURE")),
            // After the optional ONLY come a name of up to two words (schema, table), an optional alias with or without
            // AS, and SET; an upsert's `DO UPDATE SET` has no name. A privilege (`UPDATE ON`, `UPDATE (a) ON`), a trigger
            // event (`UPDATE OF`, `UPDATE OR`) and a key action (`ON UPDATE`) end the search or are skipped.
            "UPDATE" if i > 0 && words[i - 1] == "ON" => false,
            "UPDATE" => {
                let from = i + 1 + usize::from(next(1) == Some("ONLY"));
                words[from.min(words.len())..]
                    .iter()
                    .take(5)
                    .take_while(|w| !matches!(w.as_str(), "ON" | "OF" | "OR" | "FROM" | "TO" | "FOR"))
                    .any(|w| w == "SET")
            }
            _ => false,
        };
        writes.then(|| words[i..words.len().min(i + 4)].join(" "))
    })
}

/// `0004_write_additions.sql` is exactly the statements it was reviewed with: no row is written or removed, no table is
/// created or dropped, nothing is renamed, and the audit tables are not touched. A new statement of any kind, in or out of
/// a DO block or a function body, fails here until a human adds it to the list on purpose.
#[test]
fn migration_0004_is_expand_only() {
    let sql = migration_file("0004_write_additions.sql");
    assert_eq!(
        row_writing_statement(&sql),
        None,
        "0004 changes the schema and no row: no INSERT, UPDATE, DELETE, MERGE, TRUNCATE, COPY or dynamic EXECUTE, in a DO \
         block or a function body either"
    );
    let allowed: [&str; 7] = [
        // The guard.
        "DO",
        // 1. Notify proposals: one statement, so no row is ever unchecked.
        "ALTER TABLE PROPOSALS DROP CONSTRAINT PROPOSALS_ACTION_CHECK ADD CONSTRAINT PROPOSALS_ACTION_CHECK CHECK ACTION IN",
        // 2. The token expiry.
        "ALTER TABLE GITHUB_CREDENTIALS ADD COLUMN EXPIRES_AT TIMESTAMPTZ",
        // 3. Approvals are written once.
        "REVOKE UPDATE DELETE ON APPROVALS FROM LANEKEEPER_APP",
        "ALTER TABLE APPROVALS DROP CONSTRAINT APPROVALS_PROPOSAL_ID_FKEY ADD CONSTRAINT APPROVALS_PROPOSAL_ID_FKEY FOREIGN KEY PROPOSAL_ID REFERENCES PROPOSALS ID ON DELETE RESTRICT",
        "CREATE FUNCTION PROPOSALS_AUTHOR_IMMUTABLE RETURNS TRIGGER LANGUAGE PLPGSQL SET SEARCH_PATH PG_CATALOG PG_TEMP AS",
        "CREATE TRIGGER PROPOSALS_AUTHOR_IMMUTABLE BEFORE UPDATE OF AUTHOR_TID AUTHOR_OID ON PROPOSALS FOR EACH ROW EXECUTE FUNCTION PROPOSALS_AUTHOR_IMMUTABLE",
    ];
    let found: Vec<String> = statements_of(&sql).iter().map(|w| w.join(" ")).collect();
    assert_eq!(
        found, allowed,
        "0004 has exactly the statements it was reviewed with, in this order (a new statement is a new review)"
    );
    // The words that no statement of an expand-only migration contains, wherever they are (a body, a comment aside).
    let code = without_literals(&code_lines(&sql).join("\n")).to_uppercase();
    for banned in [
        "DROP TABLE",
        "DROP COLUMN",
        "DROP TRIGGER",
        "DROP FUNCTION",
        "DROP INDEX",
        "RENAME",
        "GRANT ",
        "CASCADE",
        "AUDIT_EVENTS",
        "AUDIT_CHECKPOINTS",
        "CREATE TABLE",
        "SECURITY DEFINER",
        "ENABLE ",
        "DISABLE ",
    ] {
        assert!(
            !code.contains(banned),
            "0004 must not contain `{banned}` outside comments"
        );
    }
    assert!(
        code.contains("ON DELETE RESTRICT") && !code.contains("ON UPDATE"),
        "the approvals key restricts a delete of its proposal"
    );
}

/// The checks of `migration_0004_is_expand_only` have teeth: each of these changes to 0004 is refused by one of them.
#[test]
fn the_expand_only_checks_see_row_writes_and_new_statements() {
    let real = migration_file("0004_write_additions.sql");
    assert_eq!(row_writing_statement(&real), None);
    let base = statements_of(&real).len();
    for (sql, what) in [
        ("UPDATE proposals SET status = 'x';", "an UPDATE"),
        (
            "INSERT INTO approvals (decision) VALUES ('approved');",
            "an INSERT",
        ),
        ("DELETE FROM proposals;", "a DELETE"),
        ("TRUNCATE approvals;", "a TRUNCATE"),
        (
            "DO $$ BEGIN DELETE FROM proposals; END $$;",
            "a DELETE in a DO block",
        ),
        (
            "CREATE FUNCTION f() RETURNS void LANGUAGE sql AS $$ UPDATE users SET role = 'admin' $$;",
            "an UPDATE in a function body",
        ),
        // Dynamic SQL: the statement is a string, and the strings are not read, so any EXECUTE of one is refused.
        (
            "DO $$ BEGIN EXECUTE 'DELETE FROM proposals'; END $$;",
            "dynamic SQL in a DO block",
        ),
        (
            "DO $$ BEGIN EXECUTE format('DELETE FROM %I', 'proposals'); END $$;",
            "dynamic SQL built with format",
        ),
        (
            "DO $$ BEGIN EXECUTE 'UPDATE proposals SET status = $1' USING 'x'; END $$;",
            "dynamic SQL with USING",
        ),
        (
            "CREATE FUNCTION f() RETURNS void LANGUAGE plpgsql AS $$ BEGIN EXECUTE 'TRUNCATE approvals'; END $$;",
            "dynamic SQL in a function body",
        ),
        // The target of an UPDATE, DELETE or INSERT: ONLY, a schema, an alias, with or without AS.
        ("UPDATE ONLY proposals SET status = 'x';", "UPDATE ONLY"),
        (
            "UPDATE ONLY public.proposals SET status = 'x';",
            "UPDATE ONLY, qualified",
        ),
        ("UPDATE public.proposals SET status = 'x';", "a qualified UPDATE"),
        (
            "UPDATE public.proposals AS p SET status = 'x';",
            "a qualified UPDATE with AS",
        ),
        (
            "UPDATE public.proposals p SET status = 'x';",
            "a qualified UPDATE with an alias",
        ),
        (
            "UPDATE \"public\".\"proposals\" SET status = 'x';",
            "a quoted UPDATE target",
        ),
        (
            "DELETE FROM ONLY public.proposals;",
            "DELETE FROM ONLY, qualified",
        ),
        (
            "DELETE FROM public.proposals p WHERE p.id = 1;",
            "a qualified DELETE",
        ),
        (
            "INSERT INTO public.approvals (decision) VALUES ('approved');",
            "a qualified INSERT",
        ),
        (
            "INSERT INTO approvals (decision) VALUES ('x') ON CONFLICT DO UPDATE SET decision = 'y';",
            "the UPDATE of an upsert",
        ),
        (
            "WITH gone AS (DELETE FROM public.proposals RETURNING id) SELECT 1 FROM gone;",
            "a DELETE in a WITH",
        ),
    ] {
        assert!(
            row_writing_statement(&format!("{real}\n{sql}\n")).is_some(),
            "{what} must be seen"
        );
    }
    // Text that only looks like a row write is not one: the CHECK literal, the key action, the trigger event.
    assert_eq!(
        row_writing_statement(
            "ALTER TABLE t ADD CHECK (a IN ('delete', 'update t set'));\n-- UPDATE t SET x\nREVOKE UPDATE, DELETE ON t FROM r;"
        ),
        None
    );
    for sql in [
        "CREATE TRIGGER t BEFORE UPDATE OF a, b ON public.proposals FOR EACH ROW EXECUTE FUNCTION f();",
        "CREATE TRIGGER t AFTER UPDATE OR DELETE ON proposals FOR EACH ROW EXECUTE PROCEDURE f();",
        "CREATE TRIGGER t BEFORE UPDATE ON proposals FOR EACH ROW EXECUTE FUNCTION f();",
        "GRANT SELECT, UPDATE ON audit_verify_state TO r;",
        "GRANT UPDATE (a, b) ON t TO r;",
        "REVOKE UPDATE, DELETE ON ONLY public.approvals FROM r;",
        "ALTER TABLE a ADD FOREIGN KEY (b) REFERENCES c (id) ON DELETE RESTRICT ON UPDATE CASCADE;",
        "ALTER TABLE a ADD FOREIGN KEY (b) REFERENCES c (id) ON DELETE SET NULL ON UPDATE SET NULL;",
        "ALTER TABLE a ALTER COLUMN b SET NOT NULL;",
    ] {
        assert_eq!(row_writing_statement(sql), None, "{sql} writes no row");
    }
    // A new statement of any other kind changes the list.
    for sql in [
        "CREATE INDEX i ON proposals (status);",
        "ALTER TABLE proposals DROP COLUMN paths;",
        "DROP TABLE approvals;",
        "GRANT DELETE ON approvals TO lanekeeper_app;",
        "ALTER TABLE approvals DISABLE TRIGGER approvals_no_self_approval;",
    ] {
        assert_eq!(
            statements_of(&format!("{real}\n{sql}\n")).len(),
            base + 1,
            "{sql} is a statement of its own, so the exact list refuses it"
        );
    }
}

/// The migrator owns every object, so `0004` carries the guard of `0001`, `0002` and `0003`.
#[tokio::test]
async fn migration_0004_refuses_to_run_as_another_role() {
    let db = db!();
    let sql = migration_file("0004_write_additions.sql");
    for role in [APP, ("postgres", SUPERUSER_PASSWORD)] {
        let mut conn = if role.0 == "postgres" {
            db.admin().await
        } else {
            db.as_role(role).await
        };
        let err = sqlx::raw_sql(&sql)
            .execute(&mut conn)
            .await
            .expect_err("must refuse");
        assert_eq!(sqlstate(&err).as_deref(), Some("55000"), "{}: {err}", role.0);
        assert!(
            err.to_string().contains("lanekeeper_migrator"),
            "{}: {err}",
            role.0
        );
    }
}

/// 0004 changes exactly what it says: the action CHECK, the approvals key, the grants on `approvals`, one column and
/// one trigger with its function. Every other relation, trigger, constraint, column and function stays as 0003 left it,
/// the append-only audit tables included.
#[tokio::test]
async fn migration_0004_changes_only_what_it_names() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    db.migrate_upto(3).await;
    let snapshot = |sql: &'static str| async {
        let mut c = db.admin().await;
        sqlx::query_scalar::<_, String>(sql)
            .fetch_all(&mut c)
            .await
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<String>>()
    };
    let queries: [&'static str; 5] = [
        "SELECT c.relname::text || ' ' || c.relkind::text || ' ' || c.relowner::regrole::text || ' ' || coalesce(c.relacl::text, '-') \
         FROM pg_class c WHERE c.relnamespace = 'public'::regnamespace",
        "SELECT t.tgrelid::regclass::text || ' ' || pg_get_triggerdef(t.oid, true) || ' ' || t.tgenabled::text \
         FROM pg_trigger t WHERE NOT t.tgisinternal",
        "SELECT conrelid::regclass::text || ' ' || conname || ' ' || pg_get_constraintdef(oid) FROM pg_constraint \
         WHERE connamespace = 'public'::regnamespace",
        "SELECT attrelid::regclass::text || ' ' || attname || ' ' || format_type(atttypid, atttypmod) || ' ' || attnotnull::text \
         FROM pg_attribute WHERE attnum > 0 AND NOT attisdropped AND attrelid IN (SELECT oid FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relkind IN ('r', 'p'))",
        "SELECT p.proname || ' ' || p.proowner::regrole::text || ' ' || coalesce(p.proacl::text, '-') || ' ' || md5(p.prosrc) \
         FROM pg_proc p WHERE p.pronamespace = 'public'::regnamespace",
    ];
    let mut before = Vec::new();
    for q in queries {
        before.push(snapshot(q).await);
    }
    db.migrate_upto(4).await;
    let mut changed = Vec::new();
    for (q, b) in queries.iter().zip(&before) {
        let after = snapshot(q).await;
        let removed: Vec<String> = b.difference(&after).cloned().collect();
        let added: Vec<String> = after.difference(b).cloned().collect();
        changed.push((removed, added));
    }
    // Which names changed, whatever the rest of each line says.
    let names = |lines: &[String], n: usize| -> BTreeSet<String> {
        lines
            .iter()
            .map(|l| l.split_whitespace().take(n).collect::<Vec<_>>().join(" "))
            .collect()
    };
    let set = |v: &[&str]| -> BTreeSet<String> { v.iter().map(|s| (*s).to_owned()).collect() };
    // Relations: only `approvals` changes its ACL.
    assert_eq!(names(&changed[0].0, 1), set(&["approvals"]), "{:?}", changed[0]);
    assert_eq!(names(&changed[0].1, 1), set(&["approvals"]), "{:?}", changed[0]);
    // Triggers: one is added (on proposals), none is removed or changed.
    assert!(changed[1].0.is_empty(), "no trigger is removed: {:?}", changed[1]);
    assert_eq!(names(&changed[1].1, 1), set(&["proposals"]), "{:?}", changed[1]);
    assert_eq!(changed[1].1.len(), 1, "one new trigger: {:?}", changed[1]);
    // Constraints: the two that are replaced, by name.
    let keys = |lines: &[String]| -> BTreeSet<String> {
        lines
            .iter()
            .map(|l| l.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
            .collect()
    };
    let replaced = set(&[
        "proposals proposals_action_check",
        "approvals approvals_proposal_id_fkey",
    ]);
    assert_eq!(keys(&changed[2].0), replaced, "{:?}", changed[2]);
    assert_eq!(keys(&changed[2].1), replaced, "{:?}", changed[2]);
    // Columns: one is added.
    assert!(
        changed[3].0.is_empty(),
        "no column is removed or changed: {:?}",
        changed[3]
    );
    assert_eq!(
        changed[3].1,
        ["github_credentials expires_at timestamp with time zone false".to_owned()]
    );
    // Functions: one is added.
    assert!(changed[4].0.is_empty(), "no function changes: {:?}", changed[4]);
    assert_eq!(names(&changed[4].1, 1), set(&["proposals_author_immutable"]));
}

/// G8: a proposal can be for a notify, and still only for the actions the product has.
#[tokio::test]
async fn proposals_accept_notify_and_no_other_new_action() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    let insert = "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, via, expires_at) \
                  VALUES ('t', 'alice', $1, 'sit1', ARRAY['a.yml'], 'ui', now() + interval '72 hours')";
    for action in ["edit", "upload", "delete", "revert", "restart", "pr", "notify"] {
        sqlx::query(insert)
            .bind(action)
            .execute(&mut app)
            .await
            .unwrap_or_else(|e| panic!("{action}: {e}"));
    }
    for action in ["Notify", "notify ", "apply", ""] {
        let err = sqlx::query(insert)
            .bind(action)
            .execute(&mut app)
            .await
            .expect_err("unknown action");
        assert_eq!(
            sqlstate(&err).as_deref(),
            Some(CHECK_VIOLATION),
            "{action:?}: {err}"
        );
    }
}

/// G9: the expiry of a stored token is optional, and the app can write and clear it.
#[tokio::test]
async fn github_credentials_have_an_optional_expiry() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    let insert = "INSERT INTO github_credentials (user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext) \
                  VALUES ('t', 'alice', 'alice-gh', '\\x01', 'v1', '\\x000000000000000000000000', '\\x02') RETURNING id";
    let id: i64 = sqlx::query_scalar(insert).fetch_one(&mut app).await.unwrap();
    let expires: Option<String> =
        sqlx::query_scalar("SELECT expires_at::text FROM github_credentials WHERE id = $1")
            .bind(id)
            .fetch_one(&mut app)
            .await
            .unwrap();
    assert_eq!(expires, None, "a credential stored without an expiry has none");
    sqlx::query("UPDATE github_credentials SET expires_at = now() + interval '30 days' WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    let in_future: bool =
        sqlx::query_scalar("SELECT expires_at > now() FROM github_credentials WHERE id = $1")
            .bind(id)
            .fetch_one(&mut app)
            .await
            .unwrap();
    assert!(in_future);
    sqlx::query("UPDATE github_credentials SET expires_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
}

/// G10: an approval is written once. The app cannot change or remove one, and cannot remove one through its proposal.
#[tokio::test]
async fn the_app_cannot_change_or_remove_an_approval() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, via, expires_at) \
         VALUES ('t', 'alice', 'edit', 'sit1', ARRAY['a.yml'], 'ui', now() + interval '72 hours') RETURNING id",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    sqlx::query("INSERT INTO approvals (proposal_id, decision, approver_tid, approver_oid) VALUES ($1, 'approved', 't', 'bob')")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();

    // Re-pointing the approver (the way round the self-approval trigger) and deleting the approval both lack privilege.
    for stmt in [
        "UPDATE approvals SET approver_oid = 'alice' WHERE proposal_id = $1",
        "UPDATE approvals SET decision = 'rejected' WHERE proposal_id = $1",
        "DELETE FROM approvals WHERE proposal_id = $1",
    ] {
        let err = sqlx::query(stmt)
            .bind(id)
            .execute(&mut app)
            .await
            .expect_err("no privilege");
        assert_eq!(
            sqlstate(&err).as_deref(),
            Some(INSUFFICIENT_PRIVILEGE),
            "{stmt}: {err}"
        );
    }
    // Deleting the proposal would have taken the approval with it through the cascade, which checks no privilege.
    let err = sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .expect_err("the approval keeps its proposal");
    assert_eq!(sqlstate(&err).as_deref(), Some("23503"), "{err}");
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE proposal_id = $1")
        .bind(id)
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(kept, 1);

    // What the write path needs still works: a proposal without a decision can be removed, and its status moves.
    sqlx::query("UPDATE proposals SET status = 'approved', applied_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    let lone: i64 = sqlx::query_scalar(
        "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, via, expires_at) \
         VALUES ('t', 'alice', 'edit', 'sit1', ARRAY['b.yml'], 'ui', now() + interval '72 hours') RETURNING id",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(lone)
        .execute(&mut app)
        .await
        .unwrap();
}

/// G10, a consequence the README states: a swimlane that has a decided proposal cannot be deleted (SQLSTATE 23503), because
/// its proposals cascade and a decided proposal cannot go. A swimlane whose proposals nobody decided still can.
#[tokio::test]
async fn a_swimlane_with_a_decided_proposal_cannot_be_deleted() {
    let db = db!();
    let mut app = db.app().await;
    let mut owner = db.migrator().await;
    seed_proposal_world(&mut app).await;
    sqlx::query("INSERT INTO swimlanes (id, project_id, display_name) VALUES ('sit2', 'p1', 'SIT 2')")
        .execute(&mut app)
        .await
        .unwrap();
    let propose = |swimlane: &'static str| {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, via, expires_at) \
             VALUES ('t', 'alice', 'edit', $1, ARRAY['a.yml'], 'ui', now() + interval '72 hours') RETURNING id",
        )
        .bind(swimlane)
    };
    let decided = propose("sit1").fetch_one(&mut app).await.unwrap();
    sqlx::query("INSERT INTO approvals (proposal_id, decision, approver_tid, approver_oid) VALUES ($1, 'approved', 't', 'bob')")
        .bind(decided)
        .execute(&mut app)
        .await
        .unwrap();
    let undecided = propose("sit2").fetch_one(&mut app).await.unwrap();

    // The app and the owner alike: the key refuses, not a missing privilege.
    for conn in [&mut app, &mut owner] {
        let err = sqlx::query("DELETE FROM swimlanes WHERE id = 'sit1'")
            .execute(&mut *conn)
            .await
            .expect_err("a swimlane with a decided proposal stays");
        assert_eq!(sqlstate(&err).as_deref(), Some(FOREIGN_KEY_VIOLATION), "{err}");
    }
    let kept: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM swimlanes WHERE id = 'sit1') \
              + (SELECT count(*) FROM proposals WHERE id = $1) \
              + (SELECT count(*) FROM approvals WHERE proposal_id = $1)",
    )
    .bind(decided)
    .fetch_one(&mut owner)
    .await
    .unwrap();
    assert_eq!(
        kept, 3,
        "the swimlane, its proposal and the decision are all still there"
    );

    // Nobody decided its proposals: the swimlane goes, and they go with it.
    sqlx::query("DELETE FROM swimlanes WHERE id = 'sit2'")
        .execute(&mut app)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM proposals WHERE id = $1")
        .bind(undecided)
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// G10: the author of a proposal is fixed when it is written, for the app and for the owner alike.
#[tokio::test]
async fn a_proposals_author_cannot_change() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, via, expires_at) \
         VALUES ('t', 'alice', 'edit', 'sit1', ARRAY['a.yml'], 'ui', now() + interval '72 hours') RETURNING id",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    let mut owner = db.migrator().await;
    for conn in [&mut app, &mut owner] {
        for stmt in [
            "UPDATE proposals SET author_oid = 'bob' WHERE id = $1",
            "UPDATE proposals SET author_tid = 'other' WHERE id = $1",
            "UPDATE proposals SET author_tid = 't', author_oid = 'bob' WHERE id = $1",
        ] {
            let err = sqlx::query(stmt)
                .bind(id)
                .execute(&mut *conn)
                .await
                .expect_err("author is fixed");
            // The trigger runs before the foreign key is looked at, so a refusal is never the key's.
            assert_eq!(sqlstate(&err).as_deref(), Some(CHECK_VIOLATION), "{stmt}: {err}");
        }
    }
    // Setting it to what it is, and every other column, are fine.
    sqlx::query("UPDATE proposals SET author_oid = author_oid, status = 'rejected', params = '{\"x\":1}' WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    let author: String = sqlx::query_scalar("SELECT author_oid FROM proposals WHERE id = $1")
        .bind(id)
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(author, "alice");
}

/// Two users, a project and a swimlane: what a proposal, an approval and a stored token need to exist.
async fn seed_proposal_world(app: &mut PgConnection) {
    for (oid, name) in [("alice", "Alice"), ("bob", "Bob")] {
        sqlx::query("INSERT INTO users (tid, oid, email, display_name, status, role) VALUES ('t', $1, $2, $2, 'active', 'admin')")
            .bind(oid)
            .bind(name)
            .execute(&mut *app)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('p1', 'P1')")
        .execute(&mut *app)
        .await
        .unwrap();
    sqlx::query("INSERT INTO swimlanes (id, project_id, display_name) VALUES ('sit1', 'p1', 'SIT 1')")
        .execute(&mut *app)
        .await
        .unwrap();
}

#[tokio::test]
async fn sentinel_partition_function_is_the_only_ddl_path() {
    let db = db!();
    let mut app = db.app().await;
    let mut owner = db.migrator().await;

    // The function exists, runs as its owner, pins its search_path, and only the app may call it.
    let row = sqlx::query(
        "SELECT prosecdef, proconfig::text, pg_get_userbyid(proowner) FROM pg_proc WHERE proname = 'maintain_sentinel_partitions'",
    )
    .fetch_one(&mut owner)
    .await
    .unwrap();
    assert!(row.get::<bool, _>(0), "SECURITY DEFINER");
    let config: String = row.get(1);
    assert!(config.contains("search_path=pg_catalog, pg_temp"), "{config}");
    assert_eq!(row.get::<String, _>(2), "lanekeeper_migrator");
    let public_exec: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('public', 'maintain_sentinel_partitions(integer)', 'EXECUTE')",
    )
    .fetch_one(&mut owner)
    .await
    .unwrap();
    assert!(!public_exec, "EXECUTE is not granted to PUBLIC");
    let app_exec: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('lanekeeper_app', 'maintain_sentinel_partitions(integer)', 'EXECUTE')",
    )
    .fetch_one(&mut owner)
    .await
    .unwrap();
    assert!(app_exec);

    // The app has no other way to change partitions ...
    let prev = "to_char(date_trunc('month', now() AT TIME ZONE 'UTC') - interval '1 month', 'YYYY\"m\"MM')";
    let prev_name: String = sqlx::query_scalar(&format!("SELECT 'sentinel_records_y' || {prev}"))
        .fetch_one(&mut owner)
        .await
        .unwrap();
    for sql in [
        format!("DROP TABLE {prev_name}"),
        format!("ALTER TABLE sentinel_records DETACH PARTITION {prev_name}"),
        "CREATE TABLE sentinel_records_y2099m01 PARTITION OF sentinel_records FOR VALUES FROM ('2099-01-01+00') TO ('2099-02-01+00')".to_owned(),
        "DROP TABLE sentinel_records_default".to_owned(),
        "DELETE FROM sentinel_records".to_owned(),
        "UPDATE sentinel_records SET path = 'x'".to_owned(),
        "TRUNCATE sentinel_records".to_owned(),
    ] {
        expect_sqlstate(&mut app, &sql, INSUFFICIENT_PRIVILEGE).await;
    }

    // ... and the function validates its input.
    expect_sqlstate(&mut app, "SELECT * FROM maintain_sentinel_partitions(0)", "22023").await;
    expect_sqlstate(
        &mut app,
        "SELECT * FROM maintain_sentinel_partitions(100000)",
        "22023",
    )
    .await;

    // Records route to monthly partitions; an unexpected month lands in the default partition.
    sqlx::query("INSERT INTO sentinels (vm, export_root) VALUES ('nfs-sit1', '/export')")
        .execute(&mut owner)
        .await
        .unwrap();
    let insert = "INSERT INTO sentinel_records (sentinel_id, batch_seq, record_index, observed_at, path, operation, success) \
                  SELECT id, $1, 0, $2::timestamptz, 'a/b.yml', 'write', true FROM sentinels WHERE vm = 'nfs-sit1'";
    sqlx::query(insert)
        .bind(1_i64)
        .bind("2020-01-15 12:00:00+00")
        .execute(&mut app)
        .await
        .unwrap();
    let in_default: i64 = sqlx::query_scalar("SELECT count(*) FROM ONLY sentinel_records_default")
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(in_default, 1, "no partition for 2020-01 yet");

    // Pruning: a 2020 partition exists, holds a record, and is dropped (with the stale default row) by the function.
    sqlx::query("DELETE FROM ONLY sentinel_records_default")
        .execute(&mut owner)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE sentinel_records_y2020m01 PARTITION OF sentinel_records FOR VALUES FROM ('2020-01-01+00') TO ('2020-02-01+00')")
        .execute(&mut owner)
        .await
        .unwrap();
    sqlx::query(insert)
        .bind(2_i64)
        .bind("2020-01-15 12:00:00+00")
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query(insert)
        .bind(3_i64)
        .bind("2021-06-01 00:00:00+00")
        .execute(&mut app)
        .await
        .unwrap();
    let (created, dropped): (i32, i32) =
        sqlx::query("SELECT created, dropped FROM maintain_sentinel_partitions(90)")
            .map(|r: sqlx::postgres::PgRow| (r.get(0), r.get(1)))
            .fetch_one(&mut app)
            .await
            .unwrap();
    assert_eq!(dropped, 1, "the 2020-01 partition is past retention");
    assert_eq!(created, 0, "the current months already exist");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sentinel_records")
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(
        left, 0,
        "expired rows are gone, including the one in the default partition"
    );
    let gone: Option<String> = sqlx::query_scalar("SELECT to_regclass('sentinel_records_y2020m01')::text")
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(gone, None);

    // Creating: remove last month's partition; a record for that month lands in the default partition, then the
    // function creates the partition and moves the record into it.
    sqlx::query(&format!("DROP TABLE {prev_name}"))
        .execute(&mut owner)
        .await
        .unwrap();
    sqlx::query(&insert.replace(
        "$2::timestamptz",
        "(date_trunc('month', now() AT TIME ZONE 'UTC') - interval '1 month' + interval '2 days') AT TIME ZONE 'UTC'",
    ))
        .bind(4_i64)
        .execute(&mut app)
        .await
        .unwrap();
    let in_default: i64 = sqlx::query_scalar("SELECT count(*) FROM ONLY sentinel_records_default")
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(in_default, 1);
    // A temp table with a colliding name cannot hijack the function: it qualifies every name.
    sqlx::query("CREATE TEMP TABLE sentinel_records_default (observed_at timestamptz)")
        .execute(&mut app)
        .await
        .unwrap();
    let (created, _): (i32, i32) =
        sqlx::query("SELECT created, dropped FROM maintain_sentinel_partitions(90)")
            .map(|r: sqlx::postgres::PgRow| (r.get(0), r.get(1)))
            .fetch_one(&mut app)
            .await
            .unwrap();
    assert_eq!(created, 1);
    let in_default: i64 = sqlx::query_scalar("SELECT count(*) FROM ONLY public.sentinel_records_default")
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(in_default, 0, "the record moved out of the default partition");
    let moved: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {prev_name}"))
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(moved, 1);
}

/// Creates the monthly `sentinel_records` partition `months_back` months before the current UTC month, as the owner,
/// and has the app insert one record into it. Returns the partition name.
async fn sentinel_partition_with_record(
    owner: &mut PgConnection,
    app: &mut PgConnection,
    months_back: i32,
    batch_seq: i64,
) -> String {
    let month = format!("date_trunc('month', now() AT TIME ZONE 'UTC') - interval '{months_back} months'");
    let name: String = sqlx::query_scalar(&format!(
        "SELECT 'sentinel_records_y' || to_char({month}, 'YYYY\"m\"MM')"
    ))
    .fetch_one(&mut *owner)
    .await
    .unwrap();
    sqlx::raw_sql(&format!(
        "DO $$ DECLARE m timestamp := {month}; BEGIN \
           EXECUTE format('CREATE TABLE public.{name} PARTITION OF public.sentinel_records FOR VALUES FROM (%L) TO (%L)', \
                          m AT TIME ZONE 'UTC', (m + interval '1 month') AT TIME ZONE 'UTC'); END $$"
    ))
    .execute(&mut *owner)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO sentinel_records (sentinel_id, batch_seq, record_index, observed_at, path, operation, success) \
         SELECT id, $1, 0, ({month} + interval '2 days') AT TIME ZONE 'UTC', 'a/b.yml', 'write', true \
         FROM sentinels WHERE vm = 'nfs-sit1'"
    ))
    .bind(batch_seq)
    .execute(&mut *app)
    .await
    .unwrap();
    name
}

/// S9, S21: the definer function lets the app drop partitions, so it must never drop one that is still inside the
/// 90-day life, whatever the app passes. The floor is a constant in the function body.
#[tokio::test]
async fn sentinel_partition_function_refuses_short_retention() {
    let db = db!();
    let mut app = db.app().await;
    let mut owner = db.migrator().await;
    sqlx::query("INSERT INTO sentinels (vm, export_root) VALUES ('nfs-sit1', '/export')")
        .execute(&mut owner)
        .await
        .unwrap();
    // Six months back is long expired; two months back ended at most 62 days ago, so it is inside the life. Last
    // month's partition (made by the migration) is inside it too.
    let expired = sentinel_partition_with_record(&mut owner, &mut app, 6, 1).await;
    let recent = sentinel_partition_with_record(&mut owner, &mut app, 2, 2).await;
    let partitions = "SELECT count(*) FROM pg_inherits WHERE inhparent = 'public.sentinel_records'::regclass";
    let before_partitions: i64 = sqlx::query_scalar(partitions)
        .fetch_one(&mut owner)
        .await
        .unwrap();
    let before_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM sentinel_records")
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(before_rows, 2);

    // Anything below 90 days is refused (22023) and changes nothing.
    for keep_days in [1, 2, 30, 62, 89] {
        expect_sqlstate(
            &mut app,
            &format!("SELECT * FROM maintain_sentinel_partitions({keep_days})"),
            "22023",
        )
        .await;
    }
    expect_sqlstate(
        &mut app,
        "SELECT * FROM maintain_sentinel_partitions(NULL)",
        "22023",
    )
    .await;
    let after_partitions: i64 = sqlx::query_scalar(partitions)
        .fetch_one(&mut owner)
        .await
        .unwrap();
    let after_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM sentinel_records")
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(
        after_partitions, before_partitions,
        "a refused call drops nothing"
    );
    assert_eq!(after_rows, before_rows, "a refused call deletes nothing");

    // The legitimate call (90 days, and its default) drops only the partition past the floor.
    let (created, dropped): (i32, i32) =
        sqlx::query("SELECT created, dropped FROM maintain_sentinel_partitions(90)")
            .map(|r: sqlx::postgres::PgRow| (r.get(0), r.get(1)))
            .fetch_one(&mut app)
            .await
            .unwrap();
    assert_eq!((created, dropped), (0, 1));
    for (name, want_present) in [(expired, false), (recent, true)] {
        let found: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(&name)
            .fetch_one(&mut owner)
            .await
            .unwrap();
        assert_eq!(found.is_some(), want_present, "{name}");
    }
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sentinel_records")
        .fetch_one(&mut owner)
        .await
        .unwrap();
    assert_eq!(left, 1, "the record inside the 90-day life survives");

    // A longer retention is allowed and keeps the recent partition too.
    let (_, dropped): (i32, i32) =
        sqlx::query("SELECT created, dropped FROM maintain_sentinel_partitions(365)")
            .map(|r: sqlx::postgres::PgRow| (r.get(0), r.get(1)))
            .fetch_one(&mut app)
            .await
            .unwrap();
    assert_eq!(dropped, 0);
}

/// S9, S21: `lanekeeper_app` may DELETE from `sentinels`, so the foreign key from `sentinel_records` must not cascade,
/// or deleting a sentinel would erase its records inside their 90-day life.
#[tokio::test]
async fn app_cannot_erase_sentinel_records_by_deleting_the_sentinel() {
    let db = db!();
    let mut app = db.app().await;
    let mut owner = db.migrator().await;
    sqlx::query(
        "INSERT INTO sentinels (vm, export_root) VALUES ('nfs-sit1', '/export'), ('nfs-empty', '/export')",
    )
    .execute(&mut owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sentinel_records (sentinel_id, batch_seq, record_index, observed_at, path, operation, success) \
         SELECT id, 1, 0, now(), 'a/b.yml', 'write', true FROM sentinels WHERE vm = 'nfs-sit1'",
    )
    .execute(&mut app)
    .await
    .unwrap();

    let records = "SELECT count(*) FROM sentinel_records";
    let sentinels = "SELECT count(*) FROM sentinels";
    for sql in [
        "DELETE FROM sentinels",
        "DELETE FROM sentinels WHERE vm = 'nfs-sit1'",
    ] {
        let err = sqlx::query(sql)
            .execute(&mut app)
            .await
            .expect_err(&format!("expected failure: {sql}"));
        assert_eq!(
            sqlstate(&err).as_deref(),
            Some("23503"),
            "{sql}: a sentinel with records must be refused with foreign_key_violation: {err}"
        );
    }
    let kept: i64 = sqlx::query_scalar(records).fetch_one(&mut owner).await.unwrap();
    assert_eq!(kept, 1, "sentinel_records is unchanged");
    let kept: i64 = sqlx::query_scalar(sentinels).fetch_one(&mut owner).await.unwrap();
    assert_eq!(
        kept, 2,
        "no sentinel was removed, not even the one without records"
    );

    // A sentinel without records can still be removed: the guard is about records, not about sentinels.
    let removed = sqlx::query("DELETE FROM sentinels WHERE vm = 'nfs-empty'")
        .execute(&mut app)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(removed, 1);
    let kept: i64 = sqlx::query_scalar(records).fetch_one(&mut owner).await.unwrap();
    assert_eq!(kept, 1);
}

/// S9, S21: a foreign key whose delete (or update) action is CASCADE, SET NULL or SET DEFAULT changes the rows of the
/// child table without a privilege check on it. When the child, or the parent, is a protected table (no DELETE for
/// `lanekeeper_app`, or guarded by the append-only trigger), that is a way around the protection. This walks the
/// catalog so a later migration cannot add one.
#[tokio::test]
async fn no_cascading_foreign_key_touches_a_protected_table() {
    let db = db!();
    let mut owner = db.migrator().await;

    let protected: BTreeSet<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c \
         WHERE c.relnamespace = 'public'::regnamespace AND c.relkind IN ('r', 'p') AND c.relname <> '_sqlx_migrations' \
           AND (NOT has_table_privilege('lanekeeper_app', c.oid, 'DELETE') \
                OR EXISTS (SELECT 1 FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid \
                           WHERE t.tgrelid = c.oid AND NOT t.tgisinternal AND p.proname = 'audit_append_only'))",
    )
    .fetch_all(&mut owner)
    .await
    .unwrap()
    .into_iter()
    .collect();
    for t in ["audit_events", "audit_checkpoints", "sentinel_records"] {
        assert!(
            protected.contains(t),
            "{t} must count as protected: {protected:?}"
        );
    }
    assert!(
        !protected.contains("sentinels"),
        "sentinels is deletable by the app, so it is not protected: {protected:?}"
    );

    // Every foreign key in `public`, including the copies on partitions. confdeltype/confupdtype: a = no action,
    // r = restrict, c = cascade, n = set null, d = set default.
    let fks = sqlx::query(
        "SELECT conname::text, conrelid::regclass::text, confrelid::regclass::text, confdeltype::text, confupdtype::text \
         FROM pg_constraint WHERE contype = 'f' AND connamespace = 'public'::regnamespace ORDER BY 1, 2",
    )
    .fetch_all(&mut owner)
    .await
    .unwrap();
    let mut touching = 0;
    let mut violations = Vec::new();
    for row in &fks {
        let (name, child, parent): (String, String, String) = (row.get(0), row.get(1), row.get(2));
        let (on_delete, on_update): (String, String) = (row.get(3), row.get(4));
        if !protected.contains(&child) && !protected.contains(&parent) {
            continue;
        }
        touching += 1;
        let passive = |a: &str| a == "a" || a == "r";
        if !passive(&on_delete) || !passive(&on_update) {
            violations.push(format!(
                "{name}: {child} -> {parent} (on delete '{on_delete}', on update '{on_update}')"
            ));
        }
    }
    assert!(
        touching >= 2,
        "the walk must see the audit_checkpoints and sentinel_records keys, saw {touching}"
    );
    assert!(
        violations.is_empty(),
        "foreign keys that can change a protected table without a privilege check: {violations:#?}"
    );
}

#[tokio::test]
async fn idempotency_and_webhook_replay_keys_are_unique() {
    let db = db!();
    let mut app = db.app().await;
    // Webhook replay protection: a delivery id is accepted once.
    let delivery = "INSERT INTO webhook_deliveries (delivery_id, event) VALUES ($1, 'push')";
    sqlx::query(delivery).bind("d-1").execute(&mut app).await.unwrap();
    let err = sqlx::query(delivery)
        .bind("d-1")
        .execute(&mut app)
        .await
        .expect_err("replay");
    assert_eq!(sqlstate(&err).as_deref(), Some(UNIQUE_VIOLATION), "{err}");
    // Idempotency: one row per (user, key).
    sqlx::query(
        "INSERT INTO users (tid, oid, email, display_name) VALUES ('t', 'u', 'u@example.invalid', 'U')",
    )
    .execute(&mut app)
    .await
    .unwrap();
    let key = "INSERT INTO idempotency_keys (user_tid, user_oid, key, request_hash, expires_at) \
               VALUES ('t', 'u', 'k1', decode(repeat('11', 32), 'hex'), now() + interval '24 hours')";
    sqlx::query(key).execute(&mut app).await.unwrap();
    let err = sqlx::query(key)
        .execute(&mut app)
        .await
        .expect_err("duplicate key");
    assert_eq!(sqlstate(&err).as_deref(), Some(UNIQUE_VIOLATION), "{err}");
}

// ---------------------------------------------------------------------------------------------------------------
// 0003_registry.sql (prompt 05)

/// A project and a swimlane to hang rows on.
async fn seed_swimlane(conn: &mut PgConnection, id: &str) {
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('p1', 'P1') ON CONFLICT DO NOTHING")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO swimlanes (id, project_id, display_name) VALUES ($1, 'p1', $1)")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
}

/// 32 bytes as a SQL expression: `h(1)` is the hash made of `0x11` bytes.
fn h(n: u8) -> String {
    format!("decode(repeat('{n:02x}', 32), 'hex')")
}

/// `0003_registry.sql` is expand-only (the contract-change rule): it adds columns, indexes, tables and one grant, and widens
/// one CHECK. The single DROP is that CHECK, replaced in the same transaction by a wider one. Nothing is renamed, deleted,
/// truncated or re-typed, no trigger or function is added, the app gets no more than SELECT, INSERT, UPDATE and DELETE on the
/// new tables only, and the audit and sentinel tables are not named (A16: the `sentinel_records.operation` CHECK stays).
#[test]
fn migration_0003_is_additive_only() {
    let sql = migration_file("0003_registry.sql");
    let code = code_lines(&sql).join("\n").to_uppercase();
    for banned in [
        "RENAME",
        "TRUNCATE",
        "DELETE FROM",
        "INSERT INTO",
        "GRANT ALL",
        "DROP TABLE",
        "DROP COLUMN",
        "DROP INDEX",
        "DROP TYPE",
        "DROP FUNCTION",
        "DROP TRIGGER",
        "ALTER COLUMN",
        "ALTER DEFAULT",
        "SET NOT NULL",
        "CREATE TRIGGER",
        "CREATE FUNCTION",
        "AUDIT_EVENTS",
        "AUDIT_CHECKPOINTS",
        "SENTINEL_RECORDS",
        "ON DELETE SET",
    ] {
        assert!(
            !code.contains(banned),
            "0003 must not contain `{banned}` outside comments"
        );
    }
    assert_eq!(code.matches("DROP ").count(), 1, "one DROP");
    assert!(
        code.contains("DROP CONSTRAINT FINDINGS_KIND_CHECK"),
        "the one DROP is the findings.kind CHECK that is widened"
    );
    assert!(
        code.contains("ADD CONSTRAINT FINDINGS_KIND_CHECK"),
        "and it is added again, wider, in the same file (one transaction)"
    );
    // Every ALTER TABLE adds a column or replaces the one CHECK.
    for stmt in code
        .split(';')
        .map(str::trim)
        .filter(|s| s.starts_with("ALTER TABLE"))
    {
        let adds = stmt.contains("ADD COLUMN")
            || stmt.contains("ADD CONSTRAINT FINDINGS_KIND_CHECK")
            || stmt.contains("DROP CONSTRAINT FINDINGS_KIND_CHECK");
        assert!(adds, "an ALTER TABLE that is not an addition: {stmt}");
    }
    // Exactly one GRANT, SELECT/INSERT/UPDATE/DELETE on the new tables to the app, and it names every new table.
    assert_eq!(code.matches("GRANT ").count(), 1, "one grant");
    let grant = code
        .split(';')
        .map(str::trim)
        .find(|s| s.starts_with("GRANT "))
        .expect("the grant");
    let rest = grant
        .strip_prefix("GRANT SELECT, INSERT, UPDATE, DELETE ON")
        .unwrap_or_else(|| panic!("the grant is not SELECT, INSERT, UPDATE, DELETE: {grant}"));
    let (tables, to) = rest.split_once(" TO ").expect("TO");
    assert_eq!(to.trim(), "LANEKEEPER_APP");
    let granted: BTreeSet<String> = tables.split(',').map(|t| t.trim().to_lowercase()).collect();
    let created: BTreeSet<String> = REGISTRY_TABLES.iter().map(|t| (*t).to_owned()).collect();
    assert_eq!(
        granted, created,
        "the grant names exactly the tables the file creates"
    );
    assert_eq!(
        code.matches("CREATE TABLE ").count(),
        REGISTRY_TABLES.len(),
        "the file creates the registry tables and nothing else"
    );
}

/// The migrator owns every object, so `0003` carries the same guard as `0001` and `0002`.
#[tokio::test]
async fn migration_0003_refuses_to_run_as_another_role() {
    let db = db!();
    let sql = migration_file("0003_registry.sql");
    for role in [APP, ("postgres", SUPERUSER_PASSWORD)] {
        let mut conn = if role.0 == "postgres" {
            db.admin().await
        } else {
            db.as_role(role).await
        };
        let err = sqlx::raw_sql(&sql)
            .execute(&mut conn)
            .await
            .expect_err("must refuse");
        assert_eq!(sqlstate(&err).as_deref(), Some("55000"), "{}: {err}", role.0);
        assert!(
            err.to_string().contains("lanekeeper_migrator"),
            "{}: {err}",
            role.0
        );
    }
    let mut admin = db.admin().await;
    for t in REGISTRY_TABLES {
        let owner: String = sqlx::query_scalar("SELECT tableowner::text FROM pg_tables WHERE tablename = $1")
            .bind(t)
            .fetch_one(&mut admin)
            .await
            .unwrap();
        assert_eq!(owner, "lanekeeper_migrator", "{t}");
    }
}

/// What `migration_0003_changes_existing_objects_only_by_adding` compares before and after: (name, query), each query
/// returning one string per object.
const CATALOG: [(&str, &str); 7] = [
    (
        "relations",
        "SELECT c.relname::text || ' ' || c.relkind::text || ' ' || c.relowner::regrole::text || ' ' || coalesce(c.relacl::text, '-') \
         FROM pg_class c WHERE c.relnamespace = 'public'::regnamespace",
    ),
    (
        "triggers",
        "SELECT t.tgrelid::regclass::text || ' ' || pg_get_triggerdef(t.oid, true) || ' ' || t.tgenabled::text \
         FROM pg_trigger t WHERE NOT t.tgisinternal",
    ),
    (
        "constraints",
        "SELECT conrelid::regclass::text || ' ' || conname || ' ' || pg_get_constraintdef(oid) FROM pg_constraint \
         WHERE connamespace = 'public'::regnamespace AND conname <> 'findings_kind_check'",
    ),
    (
        "columns",
        "SELECT a.attrelid::regclass::text || '.' || a.attname || ' ' || format_type(a.atttypid, a.atttypmod) || ' ' \
                || a.attnotnull::text || ' ' || coalesce(nullif(a.attidentity::text, ''), '-') || ' ' \
                || coalesce(pg_get_expr(d.adbin, d.adrelid), '-') \
         FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attnum > 0 AND NOT a.attisdropped \
           AND a.attrelid IN (SELECT oid FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relkind IN ('r', 'p'))",
    ),
    (
        "indexes",
        "SELECT indexname::text || ' ' || indexdef FROM pg_indexes WHERE schemaname = 'public'",
    ),
    (
        "index_names",
        "SELECT tablename::text || ' ' || indexname::text FROM pg_indexes WHERE schemaname = 'public'",
    ),
    (
        "functions",
        "SELECT p.proname || ' ' || p.proowner::regrole::text || ' ' || coalesce(p.proacl::text, '-') || ' ' || md5(p.prosrc) \
         FROM pg_proc p WHERE p.pronamespace = 'public'::regnamespace",
    ),
];

type CatalogSnapshot = (BTreeMap<&'static str, BTreeSet<String>>, String, String);

/// The catalog as `CATALOG` reads it, the definition of the `findings.kind` CHECK, and the definition of the
/// `sentinel_records.operation` CHECK.
async fn catalog_snapshot(db: &TestDb) -> CatalogSnapshot {
    let mut out = BTreeMap::new();
    let mut c = db.admin().await;
    for (name, q) in CATALOG {
        let rows: BTreeSet<String> = sqlx::query_scalar::<_, String>(q)
            .fetch_all(&mut c)
            .await
            .unwrap()
            .into_iter()
            .collect();
        assert!(!rows.is_empty(), "the snapshot must see something: {name}");
        out.insert(name, rows);
    }
    let check = |table: &'static str, name: &'static str| {
        sqlx::query_scalar::<_, String>(
            "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = $2 AND conrelid = $1::regclass",
        )
        .bind(table)
        .bind(name)
    };
    let kind_check = check("findings", "findings_kind_check")
        .fetch_one(&mut c)
        .await
        .unwrap();
    let operation_check = check("sentinel_records", "sentinel_records_operation_check")
        .fetch_one(&mut c)
        .await
        .unwrap();
    (out, kind_check, operation_check)
}

/// 0003 only adds: everything 0002 left behind (relations with their owner and ACL, triggers, constraints, columns with
/// their types, nullability and defaults, indexes, functions) is still there exactly as it was. The additions are the ones
/// the plan lists, and the one CHECK that changed (`findings.kind`) keeps every old value.
#[tokio::test]
async fn migration_0003_changes_existing_objects_only_by_adding() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    db.migrate_upto(2).await;

    let (before, kinds_before, operation_before) = catalog_snapshot(&db).await;
    db.migrate_upto(3).await;
    let (after, kinds_after, operation_after) = catalog_snapshot(&db).await;

    for (name, _) in CATALOG {
        let gone: Vec<&String> = before[name].difference(&after[name]).collect();
        assert!(
            gone.is_empty(),
            "0003 changed or removed existing {name}: {gone:#?}"
        );
    }

    // The quoted values of a CHECK (kind = ANY (ARRAY['a'::text, 'b'::text])).
    let values =
        |def: &str| -> BTreeSet<String> { def.split('\'').skip(1).step_by(2).map(str::to_owned).collect() };
    let mut want = values(&kinds_before);
    assert_eq!(want.len(), 11, "0001 lists the 11 checks: {kinds_before}");
    want.insert("served_mismatch".to_owned());
    assert_eq!(
        values(&kinds_after),
        want,
        "findings.kind gains served_mismatch and loses nothing"
    );
    // A16: the sentinel operations CHECK is not relaxed.
    assert_eq!(operation_after, operation_before);
    assert_eq!(
        values(&operation_after),
        BTreeSet::from(["create", "write", "delete", "rename"].map(str::to_owned)),
        "no `attribute`"
    );

    // What 0003 adds to tables that already existed, and nothing else.
    let on_old_tables = |added: &str| {
        !REGISTRY_TABLES
            .iter()
            .any(|t| added.starts_with(&format!("{t}.")) || added.starts_with(&format!("{t} ")))
    };
    let added = |name: &str| -> BTreeSet<String> {
        after[name]
            .difference(&before[name])
            .filter(|s| on_old_tables(s))
            .cloned()
            .collect()
    };
    let set = |xs: &[&str]| -> BTreeSet<String> { xs.iter().map(|s| (*s).to_owned()).collect() };
    assert_eq!(
        added("columns"),
        set(&[
            "file_observations.initial boolean true - false",
            "file_observations.previous_hash sha256_hash false - -",
            "file_observations.previous_state text false - -",
            "file_state.seq bigint true a -",
            "file_state.nfs_drift_hash sha256_hash false - -",
            "file_state.observed_at timestamp with time zone false - -",
            "file_state.last_observation_id bigint false - -",
            "baselines.blob_hash sha256_hash false - -",
            "sync_windows.warned_at timestamp with time zone false - -",
            "sync_windows.processed_at timestamp with time zone false - -",
            "service_state.pending_files jsonb true - '[]'::jsonb",
            "settings_index.secret_rule text false - -",
            "swimlanes.last_notified_at timestamp with time zone false - -",
        ])
    );
    assert_eq!(
        added("index_names"),
        set(&[
            "file_observations file_observations_replay_key",
            "file_state file_state_seq_idx",
            "file_state file_state_prefix_idx",
            "file_state file_state_denied_hash_idx",
            "file_state file_state_current_hash_idx",
        ])
    );
    // New relations: the eight tables with their keys and indexes, and the identity sequences. Nothing is a view or a function.
    let new_relations: BTreeSet<String> = after["relations"]
        .difference(&before["relations"])
        .map(|s| s.split(' ').next().unwrap().to_owned())
        .collect();
    for t in REGISTRY_TABLES {
        assert!(new_relations.contains(*t), "{t} is new");
        assert!(
            new_relations.contains(&format!("{t}_pkey")),
            "{t} has a primary key"
        );
    }
    assert_eq!(after["functions"], before["functions"], "no function is added");
    assert_eq!(after["triggers"], before["triggers"], "no trigger is added");
}

/// Rows written under 0002 survive 0003, the new columns take their defaults, and `file_state.seq` is filled in for the
/// rows that already exist (the identity column is added to a table that has data).
#[tokio::test]
async fn migration_0003_keeps_existing_rows_and_fills_the_new_columns() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    db.migrate_upto(2).await;
    let mut app = db.app().await;
    seed_swimlane(&mut app, "sit1").await;
    let h1 = h(0x11);
    for sql in [
        "INSERT INTO blobs (hash, content, size) SELECT sha256(c), c, octet_length(c) FROM (SELECT convert_to('a: 1', 'UTF8') AS c) b".to_owned(),
        format!("INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at) VALUES \
                 ('sit1', 'a.yml', {h1}, 'scan', now() - interval '2 minutes'), ('sit1', 'a.yml', NULL, 'scan', now() - interval '1 minute')"),
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state) VALUES \
         ('sit1', 'a.yml', 'a.yml', 'base', 'structured', 'in_sync'), ('sit1', 'b.yml', 'b.yml', 'base', 'text', 'nfs_ahead'), \
         ('sit1', 'c.bin', 'c.bin', 'untracked', 'binary', 'untracked')".to_owned(),
        format!("INSERT INTO baselines (swimlane_id, path, hash, repo, commit_id) VALUES ('sit1', 'a.yml', {h1}, 'base', 'abcdef0123')"),
        "INSERT INTO findings (swimlane_id, kind, severity, message, fingerprint) VALUES ('sit1', 'drift', 'low', 'm', 'f1')".to_owned(),
        "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at) VALUES ('sit1', 'sync', 'uid-1', now())".to_owned(),
        "INSERT INTO services (swimlane_id, namespace, name) VALUES ('sit1', 'csp', 'tx-api')".to_owned(),
        "INSERT INTO service_state (service_id) SELECT id FROM services".to_owned(),
        "INSERT INTO settings_index (blob_hash, setting_path, value_text, value_type, start_line, end_line) \
         SELECT hash, 'a', '1', 'int', 1, 1 FROM blobs".to_owned(),
    ] {
        sqlx::query(&sql).execute(&mut app).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    drop(app);

    db.migrate_upto(3).await;
    let mut app = db.app().await;

    for (table, want) in [
        ("file_observations", 2_i64),
        ("file_state", 3),
        ("baselines", 1),
        ("findings", 1),
        ("sync_windows", 1),
        ("service_state", 1),
        ("settings_index", 1),
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut app)
            .await
            .unwrap();
        assert_eq!(n, want, "{table} keeps its rows");
    }
    // The new columns take their defaults on the old rows.
    let row = sqlx::query(
        "SELECT count(*) FILTER (WHERE initial) AS initial, count(previous_hash) AS ph, count(previous_state) AS ps FROM file_observations",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert_eq!(
        (
            row.get::<i64, _>("initial"),
            row.get::<i64, _>("ph"),
            row.get::<i64, _>("ps")
        ),
        (0, 0, 0),
        "an old observation is not an initial one and has no overwrite context"
    );
    let row = sqlx::query(
        "SELECT count(nfs_drift_hash) AS a, count(observed_at) AS b, count(last_observation_id) AS c FROM file_state",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert_eq!(
        (
            row.get::<i64, _>("a"),
            row.get::<i64, _>("b"),
            row.get::<i64, _>("c")
        ),
        (0, 0, 0),
        "the pipeline fills these"
    );
    let blob_hash: i64 = sqlx::query_scalar("SELECT count(blob_hash) FROM baselines")
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(blob_hash, 0);
    let (pending, secret_rule, warned, processed, notified): (
        serde_json::Value,
        Option<String>,
        Option<()>,
        Option<()>,
        Option<()>,
    ) = {
        let r = sqlx::query(
            "SELECT (SELECT pending_files FROM service_state) AS p, (SELECT secret_rule FROM settings_index) AS s, \
                    (SELECT warned_at IS NOT NULL FROM sync_windows) AS w, (SELECT processed_at IS NOT NULL FROM sync_windows) AS q, \
                    (SELECT last_notified_at IS NOT NULL FROM swimlanes) AS n",
        )
        .fetch_one(&mut app)
        .await
        .unwrap();
        let none_if_false = |c: &str| r.get::<bool, _>(c).then_some(());
        (
            r.get("p"),
            r.get("s"),
            none_if_false("w"),
            none_if_false("q"),
            none_if_false("n"),
        )
    };
    assert_eq!(pending, serde_json::json!([]));
    assert_eq!(
        (secret_rule, warned, processed, notified),
        (None, None, None, None)
    );

    // `seq` was filled for the rows that existed: distinct, positive; a new row continues after them.
    let seqs: Vec<i64> = sqlx::query_scalar("SELECT seq FROM file_state ORDER BY seq")
        .fetch_all(&mut app)
        .await
        .unwrap();
    assert_eq!(seqs.len(), 3);
    assert!(seqs.windows(2).all(|w| w[0] < w[1]) && seqs[0] > 0, "{seqs:?}");
    let next: i64 = sqlx::query_scalar(
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state) \
         VALUES ('sit1', 'd.yml', 'd.yml', 'base', 'text', 'in_sync') RETURNING seq",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert!(next > seqs[2], "{next} after {seqs:?}");
}

/// The twelve finding kinds are accepted, an unknown one is not.
#[tokio::test]
async fn findings_accept_served_mismatch_and_still_reject_unknown_kinds() {
    let db = db!();
    let mut app = db.app().await;
    seed_swimlane(&mut app, "sit1").await;
    let kinds = [
        "missed_base_changes",
        "redundant_tenant_file",
        "uneven_tenant_files",
        "orphan_file",
        "drift",
        "environment_host_mismatch",
        "overwritten_nfs_change",
        "duplicate_keys",
        "ambiguous_file_name",
        "unresolved_placeholder",
        "config_server_restart_required",
        "served_mismatch",
    ];
    for (i, kind) in kinds.iter().enumerate() {
        sqlx::query(
            "INSERT INTO findings (swimlane_id, kind, severity, message, fingerprint) VALUES ('sit1', $1, 'high', 'm', $2)",
        )
        .bind(kind)
        .bind(format!("fp-{i}"))
        .execute(&mut app)
        .await
        .unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
    expect_sqlstate(
        &mut app,
        "INSERT INTO findings (swimlane_id, kind, severity, message, fingerprint) VALUES ('sit1', 'served_mismatch_x', 'high', 'm', 'fp-x')",
        CHECK_VIOLATION,
    )
    .await;
}

/// `file_observations` gets a unique replay key: the same version of a file at the same instant is recorded once, whether
/// or not it has a hash (a deletion has none, and two deletions at one instant are one).
#[tokio::test]
async fn file_observations_replay_key_makes_a_replayed_version_a_conflict() {
    let db = db!();
    let mut app = db.app().await;
    seed_swimlane(&mut app, "sit1").await;
    let insert = |hash: String, at: &str| {
        format!(
            "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at) VALUES ('sit1', 'a.yml', {hash}, 'scan', '{at}')"
        )
    };
    let t1 = "2026-10-01 10:00:00+00";
    let t2 = "2026-10-01 10:00:01+00";
    sqlx::query(&insert(h(1), t1)).execute(&mut app).await.unwrap();
    expect_sqlstate(&mut app, &insert(h(1), t1), UNIQUE_VIOLATION).await;
    // The same instant with another content, and the same content at another instant, are different versions.
    sqlx::query(&insert(h(2), t1)).execute(&mut app).await.unwrap();
    sqlx::query(&insert(h(1), t2)).execute(&mut app).await.unwrap();
    // A deletion has no hash: NULLS NOT DISTINCT makes two of them at one instant one.
    sqlx::query(&insert("NULL".to_owned(), t1))
        .execute(&mut app)
        .await
        .unwrap();
    expect_sqlstate(&mut app, &insert("NULL".to_owned(), t1), UNIQUE_VIOLATION).await;
    // Replaying with the idempotent form the ingestion uses is a no-op.
    let done = sqlx::query(&format!("{} ON CONFLICT DO NOTHING", insert(h(1), t1)))
        .execute(&mut app)
        .await
        .unwrap();
    assert_eq!(done.rows_affected(), 0);
    // The previous state is a drift state.
    expect_sqlstate(
        &mut app,
        &format!(
            "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, previous_state) VALUES ('sit1', 'b.yml', {}, 'sync', now(), 'nope')",
            h(3)
        ),
        CHECK_VIOLATION,
    )
    .await;
    sqlx::query(&format!(
        "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at, previous_state, previous_hash, initial) \
         VALUES ('sit1', 'b.yml', {}, 'sync', now(), 'nfs_ahead', {}, true)",
        h(3),
        h(4)
    ))
    .execute(&mut app)
    .await
    .unwrap();
}

/// `file_state.seq` is the short keyset surrogate: assigned by the database, increasing, unique within a swimlane, and
/// not settable by the app.
#[tokio::test]
async fn file_state_seq_is_assigned_by_the_database_and_cannot_be_set() {
    let db = db!();
    let mut app = db.app().await;
    seed_swimlane(&mut app, "sit1").await;
    let insert = "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state) \
                  VALUES ('sit1', $1, $1, 'base', 'text', 'in_sync') RETURNING seq";
    let a: i64 = sqlx::query_scalar(insert)
        .bind("a.yml")
        .fetch_one(&mut app)
        .await
        .unwrap();
    let b: i64 = sqlx::query_scalar(insert)
        .bind("b.yml")
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert!(b > a, "{a} then {b}");
    // An upsert that updates an existing row keeps its seq.
    let again: i64 = sqlx::query_scalar(
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state) VALUES ('sit1', 'a.yml', 'a.yml', 'base', 'text', 'nfs_ahead') \
         ON CONFLICT (swimlane_id, path) DO UPDATE SET state = EXCLUDED.state RETURNING seq",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert_eq!(again, a, "an update keeps the surrogate");
    expect_sqlstate(
        &mut app,
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state, seq) VALUES ('sit1', 'c.yml', 'c.yml', 'base', 'text', 'in_sync', 1)",
        GENERATED_ALWAYS,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "UPDATE file_state SET seq = 99 WHERE path = 'a.yml'",
        GENERATED_ALWAYS,
    )
    .await;
    // Unique within the swimlane, by index, so a keyset on (swimlane_id, seq) is a total order.
    let mut admin = db.admin().await;
    let def: String =
        sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE indexname = 'file_state_seq_idx'")
            .fetch_one(&mut admin)
            .await
            .unwrap();
    assert!(
        def.contains("UNIQUE") && def.contains("(swimlane_id, seq)"),
        "{def}"
    );
}

/// The three indexes that serve the registry's file_state queries: prefix listing (pattern ops, so LIKE 'dir/%' can use
/// it under any collation), the denied-hash lookup of the blob policy, and the covering index for the text-search file list.
#[tokio::test]
async fn file_state_has_the_prefix_denied_and_current_hash_indexes() {
    let db = db!();
    let mut admin = db.admin().await;
    let mut defs = BTreeMap::new();
    for name in [
        "file_state_prefix_idx",
        "file_state_denied_hash_idx",
        "file_state_current_hash_idx",
    ] {
        let d: String = sqlx::query_scalar(
            "SELECT indexdef FROM pg_indexes WHERE schemaname = 'public' AND indexname = $1",
        )
        .bind(name)
        .fetch_one(&mut admin)
        .await
        .unwrap();
        defs.insert(name, d);
    }
    assert!(
        defs["file_state_prefix_idx"].contains("(swimlane_id, path text_pattern_ops)"),
        "{defs:#?}"
    );
    let denied = &defs["file_state_denied_hash_idx"];
    assert!(
        denied.contains("(nfs_hash)") && denied.contains("WHERE"),
        "{denied}"
    );
    assert!(denied.contains("denied"), "{denied}");
    let current = &defs["file_state_current_hash_idx"];
    assert!(current.contains("(swimlane_id, nfs_hash)"), "{current}");
    assert!(current.contains("INCLUDE (path, class)"), "{current}");
    assert!(current.contains("WHERE"), "{current}");
}

/// A listing of one folder is a prefix range, and the two prefix indexes (`file_state` and `effective_files`) can serve it.
/// This asks whether the index is usable for the query, not whether the planner finds it cheaper at the size of this fixture:
/// with 4,000 rows (about 80 of them match) a cost-based choice between an index and a sequential scan is not stable across
/// versions and settings. So sequential scans are switched off for the connection, and the plan must then be built on the index
/// if the index can serve the query at all (a `LIKE` prefix on a column without `text_pattern_ops` cannot, and fails here).
#[tokio::test]
async fn folder_listings_use_the_prefix_indexes() {
    let db = db!();
    let mut admin = db.admin().await;
    seed_swimlane(&mut admin, "sit1").await;
    for sql in [
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, state) \
         SELECT 'sit1', 'dir' || (i % 50) || '/file' || i || '.yml', 'dir' || (i % 50) || '/file' || i || '.yml', 'base', 'text', 'in_sync' \
         FROM generate_series(1, 4000) i",
        "INSERT INTO effective_files (swimlane_id, logical_file, hash) \
         SELECT 'sit1', 'dir' || (i % 50) || '/file' || i || '.yml', sha256(convert_to(i::text, 'UTF8')) FROM generate_series(1, 4000) i",
        "ANALYZE file_state",
        "ANALYZE effective_files",
    ] {
        sqlx::query(sql).execute(&mut admin).await.unwrap();
    }
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut admin)
        .await
        .unwrap();
    for (query, index) in [
        (
            "SELECT path FROM file_state WHERE swimlane_id = 'sit1' AND path LIKE 'dir7/%'",
            "file_state_prefix_idx",
        ),
        (
            "SELECT logical_file, hash FROM effective_files WHERE swimlane_id = 'sit1' AND logical_file LIKE 'dir7/%'",
            "effective_files_prefix_idx",
        ),
    ] {
        let plan: serde_json::Value = sqlx::query_scalar(&format!("EXPLAIN (FORMAT JSON) {query}"))
            .fetch_one(&mut admin)
            .await
            .unwrap();
        assert!(plan.to_string().contains(index), "{query}: {plan}");
    }
}

/// The new tables reject malformed rows (the CHECKs), keep their keys, hang off `swimlanes` with a cascade the app can
/// follow, and give the app the whole mutable set of privileges and no more (`every_table_has_a_grant_decision` also checks
/// the grants).
#[tokio::test]
async fn registry_tables_enforce_their_constraints_and_cascade_with_the_swimlane() {
    let db = db!();
    let mut app = db.app().await;
    seed_swimlane(&mut app, "sit1").await;
    let hash = h(7);

    // swimlane_registry: defaults, bounds, one row per swimlane, a known swimlane.
    sqlx::query("INSERT INTO swimlane_registry (swimlane_id) VALUES ('sit1')")
        .execute(&mut app)
        .await
        .unwrap();
    let row = sqlx::query(
        "SELECT first_inventory_at IS NULL AS no_inventory, needs_full_scan, version, state_counts, finding_count, rules_fingerprint IS NULL AS no_rules \
         FROM swimlane_registry",
    )
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert!(row.get::<bool, _>("no_inventory") && row.get::<bool, _>("no_rules"));
    assert!(!row.get::<bool, _>("needs_full_scan"));
    assert_eq!(row.get::<i64, _>("version"), 0);
    assert_eq!(
        row.get::<serde_json::Value, _>("state_counts"),
        serde_json::json!({})
    );
    assert_eq!(row.get::<i32, _>("finding_count"), 0);
    expect_sqlstate(
        &mut app,
        "INSERT INTO swimlane_registry (swimlane_id) VALUES ('sit1')",
        UNIQUE_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "INSERT INTO swimlane_registry (swimlane_id) VALUES ('nope')",
        FOREIGN_KEY_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "UPDATE swimlane_registry SET version = -1",
        CHECK_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "UPDATE swimlane_registry SET finding_count = -1",
        CHECK_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "UPDATE swimlane_registry SET state_counts = '[]'",
        CHECK_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "UPDATE swimlane_registry SET rules_fingerprint = decode('00', 'hex')",
        CHECK_VIOLATION,
    )
    .await;

    // swimlane_deployments: one row per (swimlane, tenant), commit ids of 7 to 64 characters.
    let deployment = |tenant: &str, commit: &str| {
        format!(
            "INSERT INTO swimlane_deployments (swimlane_id, tenant_id, tenant_commit, base_commit, source) VALUES ('sit1', '{tenant}', '{commit}', 'abcdef0', 'sync')"
        )
    };
    sqlx::query(&deployment("sit1", "0123456789abcdef"))
        .execute(&mut app)
        .await
        .unwrap();
    expect_sqlstate(
        &mut app,
        &deployment("sit1", "0123456789abcdef"),
        UNIQUE_VIOLATION,
    )
    .await;
    expect_sqlstate(&mut app, &deployment("sit2", "abcdef"), CHECK_VIOLATION).await;
    expect_sqlstate(&mut app, &deployment("", "abcdef0"), CHECK_VIOLATION).await;
    sqlx::query(&deployment("sit2", "abcdef0"))
        .execute(&mut app)
        .await
        .unwrap();

    // swimlane_folders: a folder is a non-empty directory path, first seen once.
    sqlx::query(
        "INSERT INTO swimlane_folders (swimlane_id, dir_path, initial) VALUES ('sit1', 'tx-api', true)",
    )
    .execute(&mut app)
    .await
    .unwrap();
    expect_sqlstate(
        &mut app,
        "INSERT INTO swimlane_folders (swimlane_id, dir_path) VALUES ('sit1', 'tx-api')",
        UNIQUE_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        "INSERT INTO swimlane_folders (swimlane_id, dir_path) VALUES ('sit1', '')",
        CHECK_VIOLATION,
    )
    .await;

    // release_hints: a hint per (service, key).
    let hint = |key: &str| {
        format!(
            "INSERT INTO release_hints (swimlane_id, namespace, service, key, value) VALUES ('sit1', 'csp', 'tx-api', '{key}', 'csp-tenant-data-sit1')"
        )
    };
    sqlx::query(&hint("helm.release"))
        .execute(&mut app)
        .await
        .unwrap();
    expect_sqlstate(&mut app, &hint("helm.release"), UNIQUE_VIOLATION).await;
    expect_sqlstate(&mut app, &hint(""), CHECK_VIOLATION).await;

    // effective_files: one effective hash per logical file; the hash is 32 bytes.
    let effective = |hash: &str| {
        format!(
            "INSERT INTO effective_files (swimlane_id, logical_file, hash) VALUES ('sit1', 'tx-api/core.yml', {hash})"
        )
    };
    expect_sqlstate(&mut app, &effective("decode('00', 'hex')"), CHECK_VIOLATION).await;
    sqlx::query(&effective(&hash)).execute(&mut app).await.unwrap();
    expect_sqlstate(&mut app, &effective(&hash), UNIQUE_VIOLATION).await;

    // adoptions: closed vocabularies, and one open adoption per swimlane and reason.
    let adoption = |reason: &str, status: &str, step: &str| {
        format!(
            "INSERT INTO adoptions (swimlane_id, reason, status, step) VALUES ('sit1', '{reason}', '{status}', '{step}') RETURNING id"
        )
    };
    expect_sqlstate(
        &mut app,
        &adoption("whim", "in_progress", "branch"),
        CHECK_VIOLATION,
    )
    .await;
    expect_sqlstate(&mut app, &adoption("initial", "done", "branch"), CHECK_VIOLATION).await;
    expect_sqlstate(
        &mut app,
        &adoption("initial", "in_progress", "lunch"),
        CHECK_VIOLATION,
    )
    .await;
    let first: i64 = sqlx::query_scalar(&adoption("initial", "in_progress", "branch"))
        .fetch_one(&mut app)
        .await
        .unwrap();
    expect_sqlstate(
        &mut app,
        &adoption("initial", "in_progress", "files"),
        UNIQUE_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        &adoption("initial", "review", "files"),
        UNIQUE_VIOLATION,
    )
    .await;
    // Another reason may be open at the same time; a finished adoption does not block a new one.
    let review: i64 = sqlx::query_scalar(&adoption("tenant_suffix_changed", "review", "branch"))
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_ne!(first, review);
    sqlx::query("UPDATE adoptions SET status = 'confirmed', confirmed_at = now() WHERE id = $1")
        .bind(first)
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query(&adoption("initial", "in_progress", "branch"))
        .fetch_one(&mut app)
        .await
        .unwrap();
    let defaults = sqlx::query(
        "SELECT ranking, hints, tenants, tenant_commits, secret_scan FROM adoptions WHERE id = $1",
    )
    .bind(review)
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert_eq!(
        defaults.get::<serde_json::Value, _>("ranking"),
        serde_json::json!([])
    );
    assert_eq!(
        defaults.get::<serde_json::Value, _>("hints"),
        serde_json::json!({})
    );
    assert_eq!(defaults.get::<Vec<String>, _>("tenants"), Vec::<String>::new());
    assert_eq!(
        defaults.get::<serde_json::Value, _>("tenant_commits"),
        serde_json::json!({})
    );
    assert_eq!(
        defaults.get::<serde_json::Value, _>("secret_scan"),
        serde_json::json!([])
    );
    expect_sqlstate(
        &mut app,
        &format!("UPDATE adoptions SET ranking = '{{}}' WHERE id = {review}"),
        CHECK_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        &format!("UPDATE adoptions SET secret_scan = '{{}}' WHERE id = {review}"),
        CHECK_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        &format!("UPDATE adoptions SET base_commit = 'abc' WHERE id = {review}"),
        CHECK_VIOLATION,
    )
    .await;
    // A user is named by both halves or by neither: a foreign key on two columns is not checked when one is NULL, so the
    // CHECK is what stops half an identity from being stored as attribution.
    for who in ["started_by", "confirmed_by"] {
        for half in ["tid", "oid"] {
            expect_sqlstate(
                &mut app,
                &format!("UPDATE adoptions SET {who}_{half} = 'half' WHERE id = {review}"),
                CHECK_VIOLATION,
            )
            .await;
        }
    }

    // adoption_resolutions: the three choices, one per (adoption, path); they go with their adoption.
    let resolution = |path: &str, choice: &str| {
        format!(
            "INSERT INTO adoption_resolutions (adoption_id, path, resolution, nfs_hash, git_hash) VALUES ({review}, '{path}', '{choice}', {hash}, NULL)"
        )
    };
    expect_sqlstate(&mut app, &resolution("a.yml", "whatever"), CHECK_VIOLATION).await;
    for (path, choice) in [
        ("a.yml", "nfs_is_right"),
        ("b.yml", "git_is_right"),
        ("c.yml", "intentional"),
    ] {
        sqlx::query(&resolution(path, choice))
            .execute(&mut app)
            .await
            .unwrap();
    }
    expect_sqlstate(&mut app, &resolution("a.yml", "git_is_right"), UNIQUE_VIOLATION).await;
    expect_sqlstate(
        &mut app,
        "INSERT INTO adoption_resolutions (adoption_id, path, resolution) VALUES (999999, 'a.yml', 'nfs_is_right')",
        FOREIGN_KEY_VIOLATION,
    )
    .await;
    expect_sqlstate(
        &mut app,
        &format!("UPDATE adoption_resolutions SET reason = repeat('x', 2001) WHERE adoption_id = {review} AND path = 'c.yml'"),
        CHECK_VIOLATION,
    )
    .await;
    for half in ["tid", "oid"] {
        expect_sqlstate(
            &mut app,
            &format!("UPDATE adoption_resolutions SET resolved_by_{half} = 'half' WHERE adoption_id = {review} AND path = 'c.yml'"),
            CHECK_VIOLATION,
        )
        .await;
    }

    // delta_staging: one staged message per (swimlane, seq).
    let staged = |seq: i64, part: i32| {
        format!(
            "INSERT INTO delta_staging (swimlane_id, seq, part, new_root, metadata) VALUES ('sit1', {seq}, {part}, {hash}, '{{\"entries\": []}}')"
        )
    };
    sqlx::query(&staged(10, 0)).execute(&mut app).await.unwrap();
    sqlx::query(&staged(11, 1)).execute(&mut app).await.unwrap();
    expect_sqlstate(&mut app, &staged(10, 0), UNIQUE_VIOLATION).await;
    expect_sqlstate(&mut app, &staged(-1, 0), CHECK_VIOLATION).await;
    expect_sqlstate(&mut app, &staged(12, -1), CHECK_VIOLATION).await;
    let no_root = sqlx::query(
        "INSERT INTO delta_staging (swimlane_id, seq, part, metadata) VALUES ('sit1', 13, 0, '{}')",
    )
    .execute(&mut app)
    .await
    .expect_err("a staged message names its new root");
    assert_eq!(sqlstate(&no_root).as_deref(), Some(NOT_NULL_VIOLATION));

    // Deleting the swimlane takes every row of the new tables with it, with the app's own privileges.
    for t in REGISTRY_TABLES {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t}"))
            .fetch_one(&mut app)
            .await
            .unwrap();
        assert!(n > 0, "{t} has a row to cascade");
    }
    sqlx::query("DELETE FROM swimlanes WHERE id = 'sit1'")
        .execute(&mut app)
        .await
        .unwrap();
    for t in REGISTRY_TABLES {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t}"))
            .fetch_one(&mut app)
            .await
            .unwrap();
        assert_eq!(n, 0, "{t} follows its swimlane");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// 0005_mcp_tokens.sql (prompt 07): personal tokens for the MCP server

/// `0005_mcp_tokens.sql` is exactly the statements it was reviewed with: one new table, its index, two grants, one function and
/// one trigger. No row is written, no existing object is altered, dropped or renamed, no cascade is declared, the audit tables are
/// not named, and the app is granted neither DELETE nor anything beyond SELECT, INSERT and two columns to UPDATE. A new statement of
/// any kind fails here until a human adds it to the list on purpose.
#[test]
fn migration_0005_is_expand_only() {
    let sql = migration_file("0005_mcp_tokens.sql");
    assert_eq!(
        row_writing_statement(&sql),
        None,
        "0005 changes the schema and no row: no INSERT, UPDATE, DELETE, MERGE, TRUNCATE, COPY or dynamic EXECUTE, in a DO block \
         or a function body either"
    );
    // A CREATE TABLE is listed by its head (its columns and constraints are proven against the catalog below); every other
    // statement is listed whole.
    let found: Vec<String> = statements_of(&sql)
        .iter()
        .map(|w| {
            let create_table = w.len() >= 2 && w[0] == "CREATE" && w[1] == "TABLE";
            w[..if create_table { 3 } else { w.len() }].join(" ")
        })
        .collect();
    let allowed: [&str; 7] = [
        // The guard.
        "DO",
        "CREATE TABLE MCP_TOKENS",
        "CREATE INDEX MCP_TOKENS_USER_IDX ON MCP_TOKENS USER_TID USER_OID ID DESC",
        "GRANT SELECT INSERT ON MCP_TOKENS TO LANEKEEPER_APP",
        "GRANT UPDATE REVOKED_AT LAST_USED_AT ON MCP_TOKENS TO LANEKEEPER_APP",
        "CREATE FUNCTION MCP_TOKENS_REVOCATION_IS_FINAL RETURNS TRIGGER LANGUAGE PLPGSQL SET SEARCH_PATH PG_CATALOG PG_TEMP AS",
        "CREATE TRIGGER MCP_TOKENS_REVOCATION_IS_FINAL BEFORE UPDATE OF REVOKED_AT ON MCP_TOKENS FOR EACH ROW EXECUTE FUNCTION MCP_TOKENS_REVOCATION_IS_FINAL",
    ];
    assert_eq!(
        found, allowed,
        "0005 has exactly the statements it was reviewed with, in this order (a new statement is a new review)"
    );
    let code = without_literals(&code_lines(&sql).join("\n")).to_uppercase();
    for banned in [
        "DROP ",
        "ALTER ",
        "RENAME",
        "CASCADE",
        "TRUNCATE",
        "AUDIT_EVENTS",
        "AUDIT_CHECKPOINTS",
        "SECURITY DEFINER",
        "ENABLE ",
        "DISABLE ",
        "ON UPDATE",
    ] {
        assert!(
            !code.contains(banned),
            "0005 must not contain `{banned}` outside comments"
        );
    }
    assert!(
        code.contains("ON DELETE RESTRICT"),
        "the key to users restricts: a cascade would delete tokens without a privilege check"
    );
    // What the app is granted: SELECT and INSERT on the table, UPDATE on two columns, nothing else, to the app only.
    for statement in statements_of(&sql).iter().filter(|w| w[0] == "GRANT") {
        let on = statement.iter().position(|w| w == "ON").unwrap();
        assert!(
            statement[1..on].iter().all(|w| matches!(
                w.as_str(),
                "SELECT" | "INSERT" | "UPDATE" | "REVOKED_AT" | "LAST_USED_AT"
            )),
            "{statement:?}: the app gets no DELETE, TRUNCATE, REFERENCES, TRIGGER or ALL on tokens"
        );
        assert_eq!(&statement[on..], ["ON", "MCP_TOKENS", "TO", "LANEKEEPER_APP"]);
    }
}

/// The migrator owns every object, so `0005` carries the guard of `0001` to `0004`.
#[tokio::test]
async fn migration_0005_refuses_to_run_as_another_role() {
    let db = db!();
    let sql = migration_file("0005_mcp_tokens.sql");
    for role in [APP, ("postgres", SUPERUSER_PASSWORD)] {
        let mut conn = if role.0 == "postgres" {
            db.admin().await
        } else {
            db.as_role(role).await
        };
        let err = sqlx::raw_sql(&sql)
            .execute(&mut conn)
            .await
            .expect_err("must refuse");
        assert_eq!(sqlstate(&err).as_deref(), Some("55000"), "{}: {err}", role.0);
        assert!(
            err.to_string().contains("lanekeeper_migrator"),
            "{}: {err}",
            role.0
        );
    }
}

/// 0005 adds one table with its indexes, key, constraints, one function and one trigger, and nothing else: every relation,
/// trigger, constraint, column and function that 0004 left is still there, unchanged (so the ACL of every existing table, the
/// audit tables included, is the same).
#[tokio::test]
async fn migration_0005_changes_nothing_that_exists() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    db.migrate_upto(4).await;
    let snapshot = |sql: &'static str| async {
        let mut c = db.admin().await;
        sqlx::query_scalar::<_, String>(sql)
            .fetch_all(&mut c)
            .await
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<String>>()
    };
    let queries: [&'static str; 5] = [
        "SELECT c.relname::text || ' ' || c.relkind::text || ' ' || c.relowner::regrole::text || ' ' || coalesce(c.relacl::text, '-') \
         FROM pg_class c WHERE c.relnamespace = 'public'::regnamespace",
        "SELECT t.tgrelid::regclass::text || ' ' || pg_get_triggerdef(t.oid, true) || ' ' || t.tgenabled::text \
         FROM pg_trigger t WHERE NOT t.tgisinternal",
        "SELECT conrelid::regclass::text || ' ' || conname || ' ' || pg_get_constraintdef(oid) FROM pg_constraint \
         WHERE connamespace = 'public'::regnamespace",
        "SELECT attrelid::regclass::text || ' ' || attname || ' ' || format_type(atttypid, atttypmod) || ' ' || attnotnull::text \
         FROM pg_attribute WHERE attnum > 0 AND NOT attisdropped AND attrelid IN (SELECT oid FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relkind IN ('r', 'p'))",
        "SELECT p.proname || ' ' || p.proowner::regrole::text || ' ' || coalesce(p.proacl::text, '-') || ' ' || md5(p.prosrc) \
         FROM pg_proc p WHERE p.pronamespace = 'public'::regnamespace",
    ];
    let mut before = Vec::new();
    for q in queries {
        before.push(snapshot(q).await);
    }
    db.migrate_upto(5).await;
    let mut added = Vec::new();
    for (q, b) in queries.iter().zip(&before) {
        let after = snapshot(q).await;
        assert!(
            b.difference(&after).next().is_none(),
            "0005 changed or removed something that existed: {:?}",
            b.difference(&after).collect::<Vec<_>>()
        );
        added.push(after.difference(b).cloned().collect::<Vec<String>>());
    }
    let first_words = |lines: &[String], n: usize| -> BTreeSet<String> {
        lines
            .iter()
            .map(|l| l.split_whitespace().take(n).collect::<Vec<_>>().join(" "))
            .collect()
    };
    let set = |v: &[&str]| -> BTreeSet<String> { v.iter().map(|s| (*s).to_owned()).collect() };
    assert_eq!(
        first_words(&added[0], 1),
        set(&[
            "mcp_tokens",
            "mcp_tokens_id_seq",
            "mcp_tokens_pkey",
            "mcp_tokens_token_hmac_key",
            "mcp_tokens_user_idx"
        ]),
        "{:?}",
        added[0]
    );
    assert_eq!(first_words(&added[1], 1), set(&["mcp_tokens"]), "{:?}", added[1]);
    assert_eq!(added[1].len(), 1, "one new trigger: {:?}", added[1]);
    assert!(
        first_words(&added[2], 1) == set(&["mcp_tokens"])
            && first_words(&added[3], 1) == set(&["mcp_tokens"]),
        "{:?} {:?}",
        added[2],
        added[3]
    );
    assert_eq!(
        first_words(&added[4], 1),
        set(&["mcp_tokens_revocation_is_final"]),
        "{:?}",
        added[4]
    );
}

/// A token row for `alice`, with a hash of 32 bytes `n`, that lives for 7 days. Returns its id.
async fn insert_token(conn: &mut PgConnection, oid: &str, n: u8) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "INSERT INTO mcp_tokens (user_tid, user_oid, token_hmac, label, expires_at) \
         VALUES ('t', $1, {}, 'laptop', now() + interval '7 days') RETURNING id",
        h(n)
    ))
    .bind(oid)
    .fetch_one(conn)
    .await
}

/// A token is a hash of 32 bytes that nobody else has, for a user who exists, with a label and a bounded life. The database
/// refuses what the application should never write, so a bug in the token store cannot make an immortal or empty token.
#[tokio::test]
async fn mcp_tokens_enforce_their_constraints() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;

    let id = insert_token(&mut app, "alice", 1).await.unwrap();
    assert!(id > 0);
    let row = sqlx::query(
        "SELECT revoked_at IS NULL, last_used_at IS NULL, created_at <= now() FROM mcp_tokens WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert!(row.get::<bool, _>(0) && row.get::<bool, _>(1) && row.get::<bool, _>(2));

    // The same hash twice: a hash identifies one token.
    let err = insert_token(&mut app, "bob", 1)
        .await
        .expect_err("duplicate hash");
    assert_eq!(sqlstate(&err).as_deref(), Some(UNIQUE_VIOLATION), "{err}");
    // Another user's token is a different row.
    insert_token(&mut app, "bob", 2).await.unwrap();

    let insert = |hmac: &str, label: &str, expires: &str, user: &str| {
        format!(
            "INSERT INTO mcp_tokens (user_tid, user_oid, token_hmac, label, expires_at) \
             VALUES ('t', '{user}', {hmac}, '{label}', {expires})"
        )
    };
    let week = "now() + interval '7 days'";
    for (what, sql, want) in [
        (
            "a hash of 31 bytes",
            insert("decode(repeat('aa', 31), 'hex')", "x", week, "alice"),
            CHECK_VIOLATION,
        ),
        (
            "a hash of 33 bytes",
            insert("decode(repeat('aa', 33), 'hex')", "x", week, "alice"),
            CHECK_VIOLATION,
        ),
        (
            "an empty hash",
            insert("''::bytea", "x", week, "alice"),
            CHECK_VIOLATION,
        ),
        ("no label", insert(&h(3), "", week, "alice"), CHECK_VIOLATION),
        (
            "a label of 65 characters",
            insert(&h(4), &"l".repeat(65), week, "alice"),
            CHECK_VIOLATION,
        ),
        (
            "a token that is already expired",
            insert(&h(5), "x", "now() - interval '1 second'", "alice"),
            CHECK_VIOLATION,
        ),
        (
            "a token with no end",
            insert(&h(6), "x", "'infinity'::timestamptz", "alice"),
            CHECK_VIOLATION,
        ),
        (
            "a token that lives 32 days",
            insert(&h(7), "x", "now() + interval '32 days'", "alice"),
            CHECK_VIOLATION,
        ),
        (
            "a user who does not exist",
            insert(&h(8), "x", week, "nobody"),
            FOREIGN_KEY_VIOLATION,
        ),
        (
            "no expiry",
            insert(&h(9), "x", "NULL", "alice"),
            NOT_NULL_VIOLATION,
        ),
    ] {
        let err = sqlx::query(&sql).execute(&mut app).await.expect_err(what);
        assert_eq!(sqlstate(&err).as_deref(), Some(want), "{what}: {err}");
    }
    // The longest life the table allows is 31 days, and 64 characters are a label.
    sqlx::query(&insert(
        &h(10),
        &"l".repeat(64),
        "now() + interval '31 days' - interval '1 minute'",
        "alice",
    ))
    .execute(&mut app)
    .await
    .unwrap();
    // The id is the database's.
    let err = sqlx::query(&format!(
        "INSERT INTO mcp_tokens (id, user_tid, user_oid, token_hmac, label, expires_at) VALUES (99, 't', 'alice', {}, 'x', now() + interval '1 day')",
        h(11)
    ))
    .execute(&mut app)
    .await
    .expect_err("id is generated always");
    assert_eq!(sqlstate(&err).as_deref(), Some(GENERATED_ALWAYS), "{err}");
}

/// The app writes a token once, reads it, revokes it and notes when it was last used; it can change nothing else about it and
/// can remove none (S7, S9: a token is evidence, and its owner, hash and life are fixed when it is written).
#[tokio::test]
async fn the_app_can_revoke_and_touch_a_token_and_change_nothing_else() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    let id = insert_token(&mut app, "alice", 1).await.unwrap();
    let mut admin = db.admin().await;

    // Privileges, as the catalog says them: the whole-table ones, and the columns that can be updated.
    for (privilege, want) in [
        ("SELECT", true),
        ("INSERT", true),
        ("UPDATE", false),
        ("DELETE", false),
        ("TRUNCATE", false),
        ("REFERENCES", false),
        ("TRIGGER", false),
    ] {
        let has: bool = sqlx::query_scalar("SELECT has_table_privilege('lanekeeper_app', 'mcp_tokens', $1)")
            .bind(privilege)
            .fetch_one(&mut admin)
            .await
            .unwrap();
        assert_eq!(has, want, "table privilege {privilege}");
    }
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT attname::text FROM pg_attribute WHERE attrelid = 'mcp_tokens'::regclass AND attnum > 0 AND NOT attisdropped \
         AND has_column_privilege('lanekeeper_app', 'mcp_tokens', attname, 'UPDATE') ORDER BY attnum",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap();
    assert_eq!(columns, ["revoked_at", "last_used_at"]);
    let (table, allowed) = COLUMN_UPDATES[0];
    assert_eq!(table, "mcp_tokens");
    assert_eq!(
        columns.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        allowed.iter().copied().collect::<BTreeSet<_>>(),
        "the list the grant test uses is the grant"
    );

    // What the token store does.
    sqlx::query("UPDATE mcp_tokens SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    sqlx::query("UPDATE mcp_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    let row =
        sqlx::query("SELECT revoked_at IS NOT NULL, last_used_at IS NOT NULL FROM mcp_tokens WHERE id = $1")
            .bind(id)
            .fetch_one(&mut app)
            .await
            .unwrap();
    assert!(row.get::<bool, _>(0) && row.get::<bool, _>(1));

    // What it cannot do: every other column, and removal.
    for stmt in [
        "UPDATE mcp_tokens SET expires_at = now() + interval '30 days' WHERE id = $1",
        "UPDATE mcp_tokens SET user_oid = 'bob' WHERE id = $1",
        "UPDATE mcp_tokens SET user_tid = 'other' WHERE id = $1",
        "UPDATE mcp_tokens SET token_hmac = decode(repeat('ee', 32), 'hex') WHERE id = $1",
        "UPDATE mcp_tokens SET label = 'renamed' WHERE id = $1",
        "UPDATE mcp_tokens SET created_at = now() WHERE id = $1",
        "UPDATE mcp_tokens SET revoked_at = now(), expires_at = now() + interval '30 days' WHERE id = $1",
        "DELETE FROM mcp_tokens WHERE id = $1",
    ] {
        let err = sqlx::query(stmt)
            .bind(id)
            .execute(&mut app)
            .await
            .expect_err("no privilege");
        assert_eq!(
            sqlstate(&err).as_deref(),
            Some(INSUFFICIENT_PRIVILEGE),
            "{stmt}: {err}"
        );
    }
    expect_sqlstate(&mut app, "TRUNCATE mcp_tokens", INSUFFICIENT_PRIVILEGE).await;
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_tokens")
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert_eq!(kept, 1);
}

/// A revoked token stays revoked: the one column that ends a token cannot be cleared or moved, for the app and for the owner.
/// Setting it to what it is, and noting a use, are still fine, so a repeated revocation is a no-op and not an error.
#[tokio::test]
async fn a_revoked_token_stays_revoked() {
    let db = db!();
    let mut app = db.app().await;
    let mut owner = db.migrator().await;
    seed_proposal_world(&mut app).await;
    let id = insert_token(&mut app, "alice", 1).await.unwrap();
    let live = insert_token(&mut app, "alice", 2).await.unwrap();

    // A live token can be revoked, once, at a time of the caller's choosing.
    sqlx::query("UPDATE mcp_tokens SET revoked_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    for conn in [&mut app, &mut owner] {
        for stmt in [
            "UPDATE mcp_tokens SET revoked_at = NULL WHERE id = $1",
            "UPDATE mcp_tokens SET revoked_at = now() + interval '1 hour' WHERE id = $1",
            "UPDATE mcp_tokens SET revoked_at = now() - interval '1 hour', last_used_at = now() WHERE id = $1",
        ] {
            let err = sqlx::query(stmt)
                .bind(id)
                .execute(&mut *conn)
                .await
                .expect_err("revocation is final");
            assert_eq!(sqlstate(&err).as_deref(), Some(CHECK_VIOLATION), "{stmt}: {err}");
        }
    }
    let still: bool = sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM mcp_tokens WHERE id = $1")
        .bind(id)
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert!(still);
    // The same value again, and a use, are not changes of the revocation.
    sqlx::query("UPDATE mcp_tokens SET revoked_at = revoked_at, last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    // The other token is untouched and can still be revoked.
    let other: bool = sqlx::query_scalar("SELECT revoked_at IS NULL FROM mcp_tokens WHERE id = $1")
        .bind(live)
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert!(other);
    sqlx::query("UPDATE mcp_tokens SET revoked_at = now() WHERE id = $1")
        .bind(live)
        .execute(&mut app)
        .await
        .unwrap();
}

/// A user who has a token (revoked or not) cannot be deleted: a cascade would remove evidence without a privilege check. A user
/// without one still can.
#[tokio::test]
async fn a_user_with_a_token_cannot_be_deleted() {
    let db = db!();
    let mut app = db.app().await;
    let mut owner = db.migrator().await;
    seed_proposal_world(&mut app).await;
    insert_token(&mut app, "alice", 1).await.unwrap();
    for conn in [&mut app, &mut owner] {
        let err = sqlx::query("DELETE FROM users WHERE oid = 'alice'")
            .execute(&mut *conn)
            .await
            .expect_err("the token keeps its user");
        assert_eq!(sqlstate(&err).as_deref(), Some(FOREIGN_KEY_VIOLATION), "{err}");
    }
    let kept: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM users WHERE oid = 'alice') + (SELECT count(*) FROM mcp_tokens)",
    )
    .fetch_one(&mut owner)
    .await
    .unwrap();
    assert_eq!(kept, 2);
    let gone = sqlx::query("DELETE FROM users WHERE oid = 'bob'")
        .execute(&mut app)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(gone, 1, "a user without tokens can go");
}

/// P12: the authentication lookup (by the hash) and a user's list (newest first) each have an index they can use. A table this
/// small would be read whole whatever the indexes, so the planner is told not to, and the plan must still be an index scan.
#[tokio::test]
async fn mcp_token_lookups_have_an_index() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    for n in 1..=3 {
        insert_token(&mut app, "alice", n).await.unwrap();
    }
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut app)
        .await
        .unwrap();
    for (sql, index) in [
        (
            format!("EXPLAIN (FORMAT JSON) SELECT id, user_tid, user_oid, expires_at, revoked_at FROM mcp_tokens WHERE token_hmac = {}", h(1)),
            "mcp_tokens_token_hmac_key",
        ),
        (
            "EXPLAIN (FORMAT JSON) SELECT id, label, created_at, expires_at, revoked_at, last_used_at FROM mcp_tokens \
             WHERE user_tid = 't' AND user_oid = 'alice' AND id < 100 ORDER BY id DESC LIMIT 50"
                .to_owned(),
            "mcp_tokens_user_idx",
        ),
    ] {
        let plan: serde_json::Value = sqlx::query_scalar(&sql).fetch_one(&mut app).await.unwrap();
        let text = plan.to_string();
        assert!(text.contains(index), "{sql}: {text}");
        assert!(!text.contains("Seq Scan"), "{sql}: {text}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// 0006_mcp_tokens_created_not_future.sql (prompt 07 follow-up): `created_at` is held to the moment of the insert

const MIGRATION_0006: &str = "0006_mcp_tokens_created_not_future.sql";

/// `0006` is exactly the two statements it was reviewed with: the guard, and one `ALTER TABLE mcp_tokens ADD CONSTRAINT`. No row is
/// written, nothing is created, dropped, renamed or granted, and the audit tables are not named. A new statement of any kind fails
/// here until a human adds it to the list on purpose.
#[test]
fn migration_0006_is_expand_only() {
    let sql = migration_file(MIGRATION_0006);
    assert_eq!(
        row_writing_statement(&sql),
        None,
        "0006 changes a constraint and no row: no INSERT, UPDATE, DELETE, MERGE, TRUNCATE, COPY or dynamic EXECUTE"
    );
    let found: Vec<String> = statements_of(&sql).iter().map(|w| w.join(" ")).collect();
    assert_eq!(
        found,
        [
            "DO",
            "ALTER TABLE MCP_TOKENS ADD CONSTRAINT MCP_TOKENS_CREATED_NOT_FUTURE CHECK CREATED_AT NOW INTERVAL"
        ],
        "0006 has exactly the statements it was reviewed with, in this order (a new statement is a new review)"
    );
    let code = without_literals(&code_lines(&sql).join("\n")).to_uppercase();
    for banned in [
        "CREATE ",
        "DROP ",
        "RENAME",
        "CASCADE",
        "TRUNCATE",
        "GRANT",
        "REVOKE",
        "AUDIT_EVENTS",
        "AUDIT_CHECKPOINTS",
        "SECURITY DEFINER",
        "ENABLE ",
        "DISABLE ",
    ] {
        assert!(
            !code.contains(banned),
            "0006 must not contain `{banned}` outside comments"
        );
    }
    // The constraint is the one the tests and the README name, with the bound they state.
    assert!(code.contains("CHECK (CREATED_AT <= NOW() + INTERVAL )"), "{code}");
    assert!(
        code_lines(&sql).join("\n").contains("interval '1 minute'"),
        "the slack for clock skew is a minute, as the README says"
    );
}

/// The migrator owns every object, so `0006` carries the guard of `0001` to `0005`.
#[tokio::test]
async fn migration_0006_refuses_to_run_as_another_role() {
    let db = db!();
    let sql = migration_file(MIGRATION_0006);
    for role in [APP, ("postgres", SUPERUSER_PASSWORD)] {
        let mut conn = if role.0 == "postgres" {
            db.admin().await
        } else {
            db.as_role(role).await
        };
        let err = sqlx::raw_sql(&sql)
            .execute(&mut conn)
            .await
            .expect_err("must refuse");
        assert_eq!(sqlstate(&err).as_deref(), Some("55000"), "{}: {err}", role.0);
        assert!(
            err.to_string().contains("lanekeeper_migrator"),
            "{}: {err}",
            role.0
        );
    }
}

/// 0006 adds one CHECK to `mcp_tokens` and changes nothing else: every relation (so every ACL, the grants of 0005 included),
/// trigger, column and function that 0005 left is still there and unchanged, and the only new object is the constraint.
#[tokio::test]
async fn migration_0006_adds_one_check_and_changes_nothing_else() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    db.migrate_upto(5).await;
    let snapshot = |sql: &'static str| async {
        let mut c = db.admin().await;
        sqlx::query_scalar::<_, String>(sql)
            .fetch_all(&mut c)
            .await
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<String>>()
    };
    let queries: [&'static str; 5] = [
        "SELECT c.relname::text || ' ' || c.relkind::text || ' ' || c.relowner::regrole::text || ' ' || coalesce(c.relacl::text, '-') \
         FROM pg_class c WHERE c.relnamespace = 'public'::regnamespace",
        "SELECT t.tgrelid::regclass::text || ' ' || pg_get_triggerdef(t.oid, true) || ' ' || t.tgenabled::text \
         FROM pg_trigger t WHERE NOT t.tgisinternal",
        "SELECT conrelid::regclass::text || ' ' || conname || ' ' || pg_get_constraintdef(oid) FROM pg_constraint \
         WHERE connamespace = 'public'::regnamespace",
        "SELECT attrelid::regclass::text || ' ' || attname || ' ' || format_type(atttypid, atttypmod) || ' ' || attnotnull::text \
         FROM pg_attribute WHERE attnum > 0 AND NOT attisdropped AND attrelid IN (SELECT oid FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relkind IN ('r', 'p'))",
        "SELECT p.proname || ' ' || p.proowner::regrole::text || ' ' || coalesce(p.proacl::text, '-') || ' ' || md5(p.prosrc) \
         FROM pg_proc p WHERE p.pronamespace = 'public'::regnamespace",
    ];
    let mut before = Vec::new();
    for q in queries {
        before.push(snapshot(q).await);
    }
    db.migrate_upto(6).await;
    let mut added = Vec::new();
    for (q, b) in queries.iter().zip(&before) {
        let after = snapshot(q).await;
        assert!(
            b.difference(&after).next().is_none(),
            "0006 changed or removed something that existed: {:?}",
            b.difference(&after).collect::<Vec<_>>()
        );
        added.push(after.difference(b).cloned().collect::<Vec<String>>());
    }
    for (i, what) in [
        (0, "relations"),
        (1, "triggers"),
        (3, "columns"),
        (4, "functions"),
    ] {
        assert!(added[i].is_empty(), "0006 adds no {what}: {:?}", added[i]);
    }
    assert_eq!(added[2].len(), 1, "one new constraint: {:?}", added[2]);
    assert!(
        added[2][0].starts_with(
            "mcp_tokens mcp_tokens_created_not_future CHECK ((created_at <= (now() + '00:01:00'::interval)))"
        ),
        "{:?}",
        added[2]
    );
}

/// What the constraint does. Each refused row satisfies `mcp_tokens_lifetime`, so the refusal is the new constraint's, and it
/// names itself. Back-dating only shortens the life the other bound allows, so it is accepted and cannot lengthen a life.
#[tokio::test]
async fn mcp_tokens_created_at_is_held_to_the_insert() {
    let db = db!();
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    let with_created = |hmac: String, created: &str, expires: &str| {
        format!(
            "INSERT INTO mcp_tokens (user_tid, user_oid, token_hmac, label, created_at, expires_at) \
             VALUES ('t', 'alice', {hmac}, 'x', {created}, {expires})"
        )
    };
    for (n, what, created, expires) in [
        (
            1,
            "a created_at ten years ahead with a 30-day life",
            "now() + interval '3683 days'",
            "now() + interval '3683 days' + interval '30 days'",
        ),
        (
            2,
            "a created_at two minutes ahead",
            "now() + interval '2 minutes'",
            "now() + interval '7 days'",
        ),
        (
            3,
            "a created_at an hour ahead",
            "now() + interval '1 hour'",
            "now() + interval '1 hour' + interval '1 day'",
        ),
    ] {
        let err = sqlx::query(&with_created(h(n), created, expires))
            .execute(&mut app)
            .await
            .expect_err(what);
        assert_eq!(sqlstate(&err).as_deref(), Some(CHECK_VIOLATION), "{what}: {err}");
        assert_eq!(
            err.as_database_error().and_then(|d| d.constraint()),
            Some("mcp_tokens_created_not_future"),
            "{what}: {err}"
        );
    }
    // What still goes in: the default, a back-dated `created_at`, and a few seconds ahead (the hub's clock may run ahead).
    for (n, what, created, expires) in [
        (
            4,
            "the default created_at",
            "DEFAULT",
            "now() + interval '7 days'",
        ),
        (
            5,
            "a created_at a day back",
            "now() - interval '1 day'",
            "now() + interval '7 days'",
        ),
        (
            6,
            "a created_at 30 seconds ahead",
            "now() + interval '30 seconds'",
            "now() + interval '7 days'",
        ),
    ] {
        sqlx::query(&with_created(h(n), created, expires))
            .execute(&mut app)
            .await
            .unwrap_or_else(|e| panic!("{what}: {e}"));
    }
    // A back-dated token still ends within 31 days of its own `created_at`, so back-dating cannot lengthen a life.
    let err = sqlx::query(&with_created(
        h(7),
        "now() - interval '1 day'",
        "now() + interval '31 days'",
    ))
    .execute(&mut app)
    .await
    .expect_err("31 days from now is 32 days after a created_at a day back");
    assert_eq!(
        err.as_database_error().and_then(|d| d.constraint()),
        Some("mcp_tokens_lifetime"),
        "{err}"
    );
    // The constraint is there under its name, and an update of the columns the app may change re-checks it and passes.
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT conname::text FROM pg_constraint WHERE conrelid = 'mcp_tokens'::regclass AND contype = 'c' ORDER BY 1",
    )
    .fetch_all(&mut app)
    .await
    .unwrap();
    assert!(
        names.contains(&"mcp_tokens_created_not_future".to_owned()),
        "{names:?}"
    );
    let touched =
        sqlx::query("UPDATE mcp_tokens SET last_used_at = now(), revoked_at = now() WHERE token_hmac = $1")
            .bind(vec![0x04_u8; 32])
            .execute(&mut app)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(touched, 1, "the default-created token can be touched and revoked");
}

/// The gap `0006` closes, and what it does about it. With `0005` alone the app can write a token that lives ten years (so the
/// "at most 31 days in the schema" of the `0005` header is not true there); `0006` refuses to run over such a row, loudly and
/// without changing the table, and once the row is gone it applies and the same insert is refused.
#[tokio::test]
async fn migration_0006_closes_the_gap_in_0005_and_refuses_a_bad_row() {
    let Some(db) = TestDb::bootstrap().await else {
        return;
    };
    db.migrate_upto(5).await;
    let mut app = db.app().await;
    seed_proposal_world(&mut app).await;
    let far = format!(
        "INSERT INTO mcp_tokens (user_tid, user_oid, token_hmac, label, created_at, expires_at) \
         VALUES ('t', 'alice', {}, 'far', now() + interval '3683 days', now() + interval '3683 days' + interval '30 days') RETURNING id",
        h(1)
    );
    let id: i64 = sqlx::query_scalar(&far).fetch_one(&mut app).await.unwrap();
    let lives: i64 = sqlx::query_scalar(
        "SELECT (extract(epoch FROM (expires_at - now())) / 86400)::bigint FROM mcp_tokens WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut app)
    .await
    .unwrap();
    assert!(lives > 3000, "with 0005 alone a token lives {lives} days");

    let sql = migration_file(MIGRATION_0006);
    let mut owner = db.migrator().await;
    let err = sqlx::raw_sql(&sql)
        .execute(&mut owner)
        .await
        .expect_err("0006 refuses to run over a row it would not accept");
    assert_eq!(sqlstate(&err).as_deref(), Some(CHECK_VIOLATION), "{err}");
    let constraints: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint WHERE conrelid = 'mcp_tokens'::regclass AND conname = 'mcp_tokens_created_not_future'",
    )
    .fetch_one(&mut owner)
    .await
    .unwrap();
    assert_eq!(constraints, 0, "the failed migration left no constraint behind");

    // A person looks at the row and removes it (the owner may; the app may not), and then 0006 applies.
    sqlx::query("DELETE FROM mcp_tokens WHERE id = $1")
        .bind(id)
        .execute(&mut owner)
        .await
        .unwrap();
    sqlx::raw_sql(&sql).execute(&mut owner).await.unwrap();
    let err = sqlx::query(&far)
        .fetch_one(&mut app)
        .await
        .expect_err("now refused");
    assert_eq!(
        err.as_database_error().and_then(|d| d.constraint()),
        Some("mcp_tokens_created_not_future"),
        "{err}"
    );
}
