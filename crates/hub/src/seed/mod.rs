//! `hub seed` (plan 18, T7, G24; S5, S9, S10, rule 8): registers projects, swimlanes, tenants and sentinels from a file, so that
//! an agent or a sentinel whose identity was seeded can join (until a swimlane has an `agent_service_account`, `Join` always fails).
//!
//! **Why a command.** The rows that say which Google service account may join as which swimlane's agent are written by nobody else:
//! no route and no job inserts `projects`, `swimlanes` or `sentinels` (the admin operations that would are not built). A person or a
//! Job runs this with a file that lives outside the repository (rule 12) and is reviewed like any change of who may join.
//!
//! **The guarantees**, which `tests/seed.rs` holds against a real Postgres:
//!
//! - **A running system is not disturbed.** A row that already holds what the file says is not written and writes no audit event;
//!   a field the file does not carry is never touched; nothing is ever deleted (what the database has and the file does not list is
//!   reported); a second run of the same file changes nothing. `--dry-run` reads in a read-only transaction and prints exactly
//!   what a run would change.
//! - **All or nothing.** The file is validated whole ([`file::parse`]) and compared with the database whole ([`plan::plan`]) before
//!   the first write. The writes and their audit events are one transaction, and the events are one `record_many` (one audit chain
//!   lock acquisition), so there is exactly one event per changed row, in the same transaction as the change (rule 8, S9), or neither.
//!   A run changes at most [`MAX_CHANGES`] rows, so that the chain lock is held briefly.
//! - **The least role.** It runs as `lanekeeper_app` only, which the code checks before connecting and again once connected; the
//!   migrator and the superuser are refused.
//! - **Nothing of the file in an error, a log or the report** beyond ids and column names (S10). The audit `diff` of a change does
//!   hold what a column was and became, because that is the record of who may join as what.

pub mod apply;
pub mod error;
pub mod file;
pub mod plan;

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use hub_identity::audit::PgAuditLog;
use hub_platform::clock::{Clock, SystemClock};
use ports::{AuditLog, Tx};
use tracing::{error, info};

use crate::cli::SeedArgs;
use crate::config::SeedSettings;
use crate::db;
use crate::process::{install_crypto_provider, say};
use crate::telemetry;

pub use error::{FieldPath, MAX_CHANGES, MAX_FILE_BYTES, Place, Reason, SeedError};
pub use file::SeedFile;
pub use plan::{Plan, State};

/// How much longer than the whole run a single statement may take on the server: the run's own deadline reports first, and this is
/// the backstop for a statement the server keeps executing after the client has gone.
const STATEMENT_GRACE: Duration = Duration::from_secs(10);

/// A seed file, read and validated, with the SHA-256 of its bytes (which goes into every audit event it causes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub file: SeedFile,
    pub sha256: String,
}

/// Validate `bytes` as a seed file.
pub fn from_bytes(bytes: &[u8]) -> Result<Loaded, SeedError> {
    let file = file::parse(bytes)?;
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
    Ok(Loaded {
        file,
        sha256: hex::encode(digest.as_ref()),
    })
}

/// Read a seed file from disk, at most [`MAX_FILE_BYTES`] of a regular file, and validate it. Blocking: the caller is not in the
/// async runtime (the command reads its file before it starts one).
pub fn load(path: &Path) -> Result<Loaded, SeedError> {
    let meta = std::fs::metadata(path).map_err(|_| SeedError::Read)?;
    if !meta.is_file() {
        return Err(SeedError::Read);
    }
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Err(SeedError::TooLarge);
    }
    let mut bytes = Vec::new();
    // One byte more than the limit, so that a file that grew since `metadata` is seen to be too large and not cut.
    File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| SeedError::Read)?;
    from_bytes(&bytes)
}

/// What to do with the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Read and report; the transaction is read-only, so nothing can be written.
    DryRun,
    /// Write the changes and their audit events.
    Apply,
}

/// What a run found, and whether it wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub plan: Plan,
    /// Rows were written and committed: `Mode::Apply` and at least one change.
    pub applied: bool,
    pub dry_run: bool,
}

impl Report {
    /// The text for a person (see [`Plan::render`]).
    #[must_use]
    pub fn render(&self) -> String {
        self.plan.render(self.dry_run)
    }
}

/// The role in the URL must be the application role, before any connection exists.
pub fn check_role(role: &str) -> Result<(), SeedError> {
    if role == apply::APP_ROLE {
        Ok(())
    } else {
        Err(SeedError::WrongRole)
    }
}

/// Run a seed: connect as `lanekeeper_app`, compare the file with the database, and (unless `mode` is a dry run) write the
/// differences and their audit events. The whole run is bounded by the settings' time limit.
pub async fn run(
    settings: &SeedSettings,
    loaded: &Loaded,
    mode: Mode,
    clock: &dyn Clock,
) -> Result<Report, SeedError> {
    let options = db::connect_options_for(
        &settings.url,
        &settings.ca_file,
        settings.timeout + STATEMENT_GRACE,
    )
    .map_err(|_| SeedError::Url)?;
    // Before a connection exists: a Job pointed at the migrator, the superuser or anyone else is refused without a login.
    check_role(options.username())?;
    let limit = settings.timeout;
    tokio::time::timeout(limit, execute(&options, loaded, mode, clock))
        .await
        .map_err(|_| SeedError::TimedOut(limit.as_secs()))?
}

async fn execute(
    options: &db::PoolOptions,
    loaded: &Loaded,
    mode: Mode,
    clock: &dyn Clock,
) -> Result<Report, SeedError> {
    let mut conn = db::connect_one(options).await.map_err(|_| SeedError::Connect)?;
    check_role(&apply::current_role(&mut conn).await?)?;

    let dry_run = mode == Mode::DryRun;
    let mut tx = apply::begin(&mut conn, dry_run).await?;
    // Read inside the transaction, after the seed lock: what the plan compares is what the writes will find.
    let state = apply::load_state(&mut tx, &loaded.file).await?;
    let plan = plan::plan(&loaded.file, &state)?;

    if dry_run || plan.changes.is_empty() {
        // Nothing to write. Rolled back, not committed: a run that changes nothing leaves nothing, not even a transaction id.
        return Ok(Report {
            plan,
            applied: false,
            dry_run,
        });
    }

    let events = plan.events(clock.now(), &loaded.sha256)?;
    let bytes = events
        .iter()
        .try_fold(0_usize, |sum, e| ports::canonical_size(e).map(|n| sum + n))
        .map_err(|_| SeedError::Audit)?;
    if bytes > ports::MAX_AUDIT_BATCH_BYTES {
        return Err(SeedError::AuditTooLarge);
    }
    for change in &plan.changes {
        apply::write(&mut tx, change).await?;
    }
    // Last, as the audit log asks: it takes the chain lock, which is held to the commit.
    PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), events)
        .await
        .map_err(|_| SeedError::Audit)?;
    tx.commit().await.map_err(|e| apply::database(&e))?;
    Ok(Report {
        plan,
        applied: true,
        dry_run,
    })
}

/// `hub seed` as a process: read the settings and the file, run, print the report, and say how it ended with the exit code. 0: the
/// database matches the file (whether or not this run changed anything, and for a dry run: whatever it would change). 1: the run
/// could not be completed. 2: a setting or the file was refused.
pub fn entry(args: &SeedArgs, lookup: impl Fn(&str) -> Option<String>) -> ExitCode {
    let settings = match SeedSettings::from_lookup(lookup) {
        Ok(settings) => settings,
        Err(error) => {
            say(&error.to_string());
            return ExitCode::from(2);
        }
    };
    let loaded = match load(&args.file) {
        Ok(loaded) => loaded,
        Err(error) => {
            say(&error.to_string());
            return ExitCode::from(error.exit_code());
        }
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
    let mode = if args.dry_run { Mode::DryRun } else { Mode::Apply };
    match runtime.block_on(run(&settings, &loaded, mode, &SystemClock)) {
        Ok(report) => {
            print_report(&report.render());
            info!(
                dry_run = report.dry_run,
                applied = report.applied,
                changes = report.plan.changes.len(),
                projects_unchanged = report.plan.unchanged.projects,
                swimlanes_unchanged = report.plan.unchanged.swimlanes,
                sentinels_unchanged = report.plan.unchanged.sentinels,
                file_sha256 = %loaded.sha256,
                "seed finished"
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            error!(%failure, "the seed was not applied");
            ExitCode::from(failure.exit_code())
        }
    }
}

/// The report goes to standard output as text, for the person or the Job log; the structured line after it goes through `tracing`.
#[allow(clippy::print_stdout)]
fn print_report(text: &str) {
    print!("{text}");
}
