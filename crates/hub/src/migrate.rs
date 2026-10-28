//! `hub migrate` (plan 18, T2; G4, G29; S6, S7, S10, S16): applies `db/migrations` as `lanekeeper_migrator` and exits.
//!
//! **Why a command and not a step of starting.** The serving hub runs as `lanekeeper_app`, which can write rows and change no
//! table. Migrating at start would put a credential that can change the schema into a pod that faces users (S16), and with
//! three replicas starting together it would make them race for it. So the migrations are a Job that runs before the hub rolls,
//! and the hub only checks, at start and in `/readyz`, that the schema it needs is there (`crate::schema`).
//!
//! **What is embedded.** The migration files are compiled in, because the image carries a binary and the web app and nothing
//! else. [`EMBEDDED`] is the table of them; `tests/migrate.rs` fails if it differs from `db/migrations/` by a file, a name or a
//! byte, so a migration that is added and not listed fails CI instead of being silently left out of production.
//!
//! **What it refuses.** Any role but `lanekeeper_migrator`, before it connects (the role in the URL) and again once connected
//! (the role the session says it is); the constant is in the code, not in a setting. A database that has a migration this binary
//! does not know (a newer image already migrated it); a migration that was changed after it was applied; a half-applied one.
//! Each is a typed [`MigrateError`], and none carries a host, a user, a password or a word of the driver's: a failed migration is
//! reported as its version and the five-character SQLSTATE.
//!
//! **What it does.** Connects once with `verify-full` TLS (the same rules as the hub's pool), takes sqlx's advisory lock so that
//! two Jobs at once cannot both apply a migration, applies what is missing, each in its own transaction, and reports exactly
//! the versions it applied. Running it again applies nothing and rewrites nothing. The whole run is bounded
//! (`LK_MIGRATE_TIMEOUT_SECS`), and so is each statement a little longer than that, so a migration cannot go on after the Job
//! has given up on it.
//!
//! **What it does not do** (G29). It writes no audit event: `audit_events` is created by the first migration, so the first run has
//! no chain to append to, and the migrator is not the role the audit log is written as. `_sqlx_migrations` is the record of what
//! was applied, when and with which checksum, and the run prints the versions. That a schema change leaves no row in the audit
//! chain is a residual risk in `docs/threat-model.md`.

use std::borrow::Cow;
use std::process::ExitCode;
use std::time::Duration;

use futures::future::BoxFuture;
use sqlx::migrate::{
    Migrate, MigrateError as SqlxMigrateError, Migration, MigrationSource, MigrationType, Migrator,
};
use tracing::{error, info};

use crate::config::{ConfigError, MigratorSettings};
use crate::db;
use crate::process::{install_crypto_provider, say};
use crate::telemetry;

/// The one role that may migrate. A constant, not a setting: a Job that is pointed at another role is refused, not obeyed.
pub const MIGRATOR_ROLE: &str = "lanekeeper_migrator";

/// How much longer than the whole run a single statement may take on the server. The run's own deadline reports first; this
/// is the backstop for a statement the server keeps executing after the client has gone.
const STATEMENT_GRACE: Duration = Duration::from_secs(10);

/// One migration file, as compiled in.
#[derive(Debug, Clone, Copy)]
pub struct Embedded {
    /// The number in the file name (`0003_registry.sql` is 3).
    pub version: i64,
    /// The rest of the file name with underscores as spaces, as sqlx derives it (`registry`).
    pub description: &'static str,
    /// The file's text, byte for byte.
    pub sql: &'static str,
}

/// Every migration in `db/migrations`, in order. Adding a file there means adding a line here and a marker in `crate::schema`;
/// `tests/migrate.rs` fails until both are done.
pub const EMBEDDED: [Embedded; 8] = [
    Embedded {
        version: 1,
        description: "init",
        sql: include_str!("../../../db/migrations/0001_init.sql"),
    },
    Embedded {
        version: 2,
        description: "audit verify state",
        sql: include_str!("../../../db/migrations/0002_audit_verify_state.sql"),
    },
    Embedded {
        version: 3,
        description: "registry",
        sql: include_str!("../../../db/migrations/0003_registry.sql"),
    },
    Embedded {
        version: 4,
        description: "write additions",
        sql: include_str!("../../../db/migrations/0004_write_additions.sql"),
    },
    Embedded {
        version: 5,
        description: "mcp tokens",
        sql: include_str!("../../../db/migrations/0005_mcp_tokens.sql"),
    },
    Embedded {
        version: 6,
        description: "mcp tokens created not future",
        sql: include_str!("../../../db/migrations/0006_mcp_tokens_created_not_future.sql"),
    },
    Embedded {
        version: 7,
        description: "tool write registrations",
        sql: include_str!("../../../db/migrations/0007_tool_write_registrations.sql"),
    },
    Embedded {
        version: 8,
        description: "blob touch",
        sql: include_str!("../../../db/migrations/0008_blob_touch.sql"),
    },
];

/// The embedded migrations as sqlx's own type, built the way sqlx builds one from a file (so the checksums are the ones the
/// files have: SHA-384 of the text).
#[must_use]
pub fn embedded() -> Vec<Migration> {
    EMBEDDED
        .iter()
        .map(|m| {
            Migration::new(
                m.version,
                Cow::Borrowed(m.description),
                MigrationType::Simple,
                Cow::Borrowed(m.sql),
                m.sql.starts_with("-- no-transaction"),
            )
        })
        .collect()
}

/// Why a run did not complete. Every message is fixed text, a version, or a SQLSTATE; none holds a host, a user, a password or
/// anything the driver said.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrateError {
    #[error("the migrator URL (LK_MIGRATOR_URL_FILE) is not a valid database URL")]
    Url,
    #[error("refusing to run: only the role lanekeeper_migrator may apply migrations")]
    WrongRole,
    #[error(
        "could not connect to the database as the migrator (the server, its certificate and the CA are checked)"
    )]
    Connect,
    #[error(
        "the database holds migration {0}, which this hub does not know: a newer image has already migrated it, so run \
         the migration job of that image, not this one"
    )]
    DatabaseIsNewer(i64),
    #[error(
        "migration {0} was changed after it was applied to this database; a merged migration is never edited"
    )]
    Changed(i64),
    #[error("migration {0} was left half applied; fix the database and remove its row from _sqlx_migrations")]
    Dirty(i64),
    #[error("migration {0} failed (SQLSTATE {1}) and was rolled back")]
    Failed(i64, String),
    #[error("the database refused a statement of the migrator (SQLSTATE {0})")]
    Database(String),
    #[error("the migrations did not finish within {0} seconds (LK_MIGRATE_TIMEOUT_SECS)")]
    TimedOut(u64),
}

/// A migration this run applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub version: i64,
    pub description: String,
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The migrations this run applied, in order. Empty when the schema was already complete.
    pub applied: Vec<Applied>,
    /// The highest version in the database once the run is over.
    pub latest: i64,
}

/// The session's role must be the migrator's. Checked once the connection exists, after the check on the URL: a pooler or a
/// default role could log in as someone other than the URL says.
pub fn check_role(current: &str) -> Result<(), MigrateError> {
    if current == MIGRATOR_ROLE {
        Ok(())
    } else {
        Err(MigrateError::WrongRole)
    }
}

/// The SQLSTATE of a database error: five characters of `0-9A-Z`, or `?????` for anything else (a driver error that is not the
/// server's, or a code that is not shaped like one). Nothing else of the error is ever shown.
fn sqlstate(error: &sqlx::Error) -> String {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .filter(|code| code.len() == 5 && code.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()))
        .map_or_else(|| "?????".to_owned(), std::borrow::Cow::into_owned)
}

fn map_error(error: &SqlxMigrateError) -> MigrateError {
    match error {
        SqlxMigrateError::VersionMissing(version) => MigrateError::DatabaseIsNewer(*version),
        SqlxMigrateError::VersionMismatch(version) => MigrateError::Changed(*version),
        SqlxMigrateError::Dirty(version) => MigrateError::Dirty(*version),
        SqlxMigrateError::ExecuteMigration(error, version) => MigrateError::Failed(*version, sqlstate(error)),
        SqlxMigrateError::Execute(error) => MigrateError::Database(sqlstate(error)),
        // The other variants are about reading migration files and reverting; none can happen with a table compiled in.
        _ => MigrateError::Database("?????".to_owned()),
    }
}

/// A table of migrations as sqlx's source type, so that its `Migrator` takes them without anyone reaching into its fields.
#[derive(Debug)]
struct Table(Vec<Migration>);

impl<'s> MigrationSource<'s> for Table {
    fn resolve(self) -> BoxFuture<'s, Result<Vec<Migration>, Box<dyn std::error::Error + Send + Sync>>> {
        Box::pin(async move { Ok(self.0) })
    }
}

/// Apply the embedded migrations. See the module's notes.
pub async fn run(settings: &MigratorSettings) -> Result<Report, MigrateError> {
    run_set(settings, &embedded()).await
}

/// Apply `migrations` (in tests: a prefix of the embedded ones, or the embedded ones and one more). [`run`] is this with the
/// real set.
pub async fn run_set(settings: &MigratorSettings, migrations: &[Migration]) -> Result<Report, MigrateError> {
    let options = db::connect_options_for(
        &settings.url,
        &settings.ca_file,
        settings.timeout + STATEMENT_GRACE,
    )
    .map_err(|_| MigrateError::Url)?;
    // Before a connection exists: a Job pointed at the app role, the superuser or anyone else is refused without a login.
    check_role(options.username())?;
    let limit = settings.timeout;
    tokio::time::timeout(limit, apply(&options, migrations))
        .await
        .map_err(|_| MigrateError::TimedOut(limit.as_secs()))?
}

async fn apply(options: &db::PoolOptions, migrations: &[Migration]) -> Result<Report, MigrateError> {
    let mut conn = db::connect_one(options)
        .await
        .map_err(|_| MigrateError::Connect)?;
    let current: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut conn)
        .await
        .map_err(|e| MigrateError::Database(sqlstate(&e)))?;
    check_role(&current)?;

    // The migrator's own lock is taken here and not inside `Migrator::run`, so that "what was there before" and "what is there
    // after" are read under it: when two Jobs run together, each reports what it applied and the two reports do not overlap.
    let mut table = Migrator::new(Table(migrations.to_vec()))
        .await
        .map_err(|e| map_error(&e))?;
    table.set_locking(false);
    conn.lock().await.map_err(|e| map_error(&e))?;
    conn.ensure_migrations_table().await.map_err(|e| map_error(&e))?;
    let before = versions(&mut conn).await?;
    table.run(&mut conn).await.map_err(|e| map_error(&e))?;
    let after = versions(&mut conn).await?;
    // Best effort: closing the session releases the lock whether or not this succeeds.
    let _ = conn.unlock().await;

    let applied = migrations
        .iter()
        .filter(|m| after.contains(&m.version) && !before.contains(&m.version))
        .map(|m| Applied {
            version: m.version,
            description: m.description.to_string(),
        })
        .collect();
    Ok(Report {
        applied,
        latest: after.last().copied().unwrap_or(0),
    })
}

/// The versions recorded as applied, ascending.
async fn versions(conn: &mut sqlx::PgConnection) -> Result<Vec<i64>, MigrateError> {
    let mut versions: Vec<i64> = conn
        .list_applied_migrations()
        .await
        .map_err(|e| map_error(&e))?
        .into_iter()
        .map(|m| m.version)
        .collect();
    versions.sort_unstable();
    Ok(versions)
}

/// `hub migrate` as a process: read its settings, run, print what happened, and say how it ended with the exit code. 0: the
/// schema is complete (whether or not this run did anything). 1: it could not be completed. 2: a setting was refused.
pub fn entry(lookup: impl Fn(&str) -> Option<String>) -> ExitCode {
    let settings = match MigratorSettings::from_lookup(lookup) {
        Ok(settings) => settings,
        Err(error) => return refuse(&error),
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        say("the async runtime could not be created");
        return ExitCode::from(1);
    };
    let _inside = runtime.enter();
    // The database leg is TLS, and the binary links two crypto providers: choose one before anything asks for "the" provider.
    install_crypto_provider();
    let _telemetry = match telemetry::init_stdout() {
        Ok(telemetry) => telemetry,
        Err(error) => {
            say(&error.to_string());
            return ExitCode::from(1);
        }
    };
    match runtime.block_on(run(&settings)) {
        Ok(report) => {
            for applied in &report.applied {
                info!(
                    version = applied.version,
                    description = %applied.description,
                    "migration applied"
                );
            }
            if report.applied.is_empty() {
                info!(
                    version = report.latest,
                    "nothing to apply; the schema is complete"
                );
            } else {
                info!(
                    applied = report.applied.len(),
                    version = report.latest,
                    "the schema is up to date"
                );
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            error!(%failure, "the migrations were not applied");
            ExitCode::from(1)
        }
    }
}

fn refuse(error: &ConfigError) -> ExitCode {
    say(&error.to_string());
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_in_order_without_gaps_and_names_follow_the_files() {
        let versions: Vec<i64> = EMBEDDED.iter().map(|m| m.version).collect();
        assert_eq!(versions, (1..=8).collect::<Vec<i64>>());
        for m in &EMBEDDED {
            assert!(!m.description.is_empty() && !m.description.contains('_'), "{m:?}");
            assert!(m.sql.len() > 100, "migration {} is suspiciously short", m.version);
        }
    }

    #[test]
    fn a_failed_migration_is_its_version_and_its_code_and_nothing_the_driver_said() {
        // A database error of any other shape than a SQLSTATE is not shown.
        let io = sqlx::Error::Io(std::io::Error::other("connect to 10.1.2.3 as secret_user failed"));
        let mapped = map_error(&SqlxMigrateError::ExecuteMigration(io, 4));
        assert_eq!(mapped, MigrateError::Failed(4, "?????".to_owned()));
        let shown = format!("{mapped} {mapped:?}");
        assert!(
            !shown.contains("10.1.2.3") && !shown.contains("secret_user"),
            "{shown}"
        );
    }

    #[test]
    fn each_sqlx_error_has_its_own_typed_answer() {
        assert_eq!(
            map_error(&SqlxMigrateError::VersionMissing(9)),
            MigrateError::DatabaseIsNewer(9)
        );
        assert_eq!(
            map_error(&SqlxMigrateError::VersionMismatch(3)),
            MigrateError::Changed(3)
        );
        assert_eq!(map_error(&SqlxMigrateError::Dirty(5)), MigrateError::Dirty(5));
        assert_eq!(
            map_error(&SqlxMigrateError::Execute(sqlx::Error::PoolClosed)),
            MigrateError::Database("?????".to_owned())
        );
        assert_eq!(
            map_error(&SqlxMigrateError::VersionNotPresent(2)),
            MigrateError::Database("?????".to_owned())
        );
    }

    #[test]
    fn every_message_is_fixed_text_and_a_number() {
        for error in [
            MigrateError::Url,
            MigrateError::WrongRole,
            MigrateError::Connect,
            MigrateError::DatabaseIsNewer(9),
            MigrateError::Changed(3),
            MigrateError::Dirty(5),
            MigrateError::Failed(4, "22012".to_owned()),
            MigrateError::Database("42501".to_owned()),
            MigrateError::TimedOut(600),
        ] {
            let shown = error.to_string();
            assert!(!shown.is_empty() && shown.len() < 250, "{shown}");
            assert!(!shown.contains("postgres://") && !shown.contains('@'), "{shown}");
        }
    }

    #[test]
    fn the_role_check_is_exact() {
        assert_eq!(check_role(MIGRATOR_ROLE), Ok(()));
        assert_eq!(check_role("lanekeeper_app"), Err(MigrateError::WrongRole));
        assert_eq!(check_role(""), Err(MigrateError::WrongRole));
    }

    #[test]
    fn a_url_that_asks_for_less_tls_is_overridden_and_the_role_is_the_urls() {
        let settings = MigratorSettings {
            url: domain::Secret::new(
                "postgres://lanekeeper_migrator:pw@db.example/lanekeeper?sslmode=disable".to_owned(),
            ),
            ca_file: "/ca.pem".into(),
            timeout: Duration::from_secs(60),
        };
        let options = db::connect_options_for(&settings.url, &settings.ca_file, settings.timeout).unwrap();
        assert!(matches!(
            options.ssl_mode(),
            sqlx::postgres::PgSslMode::VerifyFull
        ));
        assert_eq!(options.username(), MIGRATOR_ROLE);
        assert!(!format!("{options:?}").contains("pw"));
    }

    #[tokio::test]
    async fn a_url_that_is_not_a_url_is_a_typed_error_before_any_connection() {
        let settings = MigratorSettings {
            url: domain::Secret::new("not a url at all".to_owned()),
            ca_file: "/ca.pem".into(),
            timeout: Duration::from_secs(5),
        };
        assert_eq!(run(&settings).await.unwrap_err(), MigrateError::Url);
    }
}
