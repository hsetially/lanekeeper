//! The settings of `hub migrate` (plan 18, T2, G4; S6, S7, S16): the migrator's credential, the CA that signed the server, and
//! a limit on how long the whole run may take.
//!
//! These are not `HubSettings`. The migration Job carries the migrator credential and nothing else of the hub's, and the
//! serving hub's pod carries none of these three variables (`tests/hardening.rs` holds the pod to that), so the one process
//! that could change the schema is not the one that faces users.
//!
//! The credential is a file (`LK_MIGRATOR_URL_FILE`), never a variable and never a Secret Manager name: a URL with a password
//! in the environment shows in `/proc/<pid>/environ` and in a crash dump. The CA file is the one the hub's own pool uses
//! (`LK_DB_CA_FILE`), because the same server signs for both.

use std::path::PathBuf;
use std::time::Duration;

use domain::Secret;

use super::ConfigError;
use super::reader::Reader;

/// How long a run may take unless `LK_MIGRATE_TIMEOUT_SECS` says otherwise. Ten minutes: the first run creates the whole schema
/// in a few seconds, and a later one that backfills a large table needs the room. The Job's own deadline is the outer bound.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// The variables only the migration Job sets. The hub pod must not carry any of them.
pub const JOB_ONLY_VARIABLES: [&str; 2] = ["LK_MIGRATOR_URL_FILE", "LK_MIGRATE_TIMEOUT_SECS"];

/// What `hub migrate` needs.
#[derive(Debug)]
pub struct MigratorSettings {
    /// The connection URL of `lanekeeper_migrator`. Holds the password: redacted in `Debug`, zeroized on drop.
    pub url: Secret<String>,
    /// The CA that signed the database server's certificate. Required: there is no "trust the system" mode.
    pub ca_file: PathBuf,
    /// The longest the whole run (connecting, waiting for the lock, applying) may take.
    pub timeout: Duration,
}

impl MigratorSettings {
    /// Read the settings through `lookup`, which is `std::env::var` in `main` and a map in tests. The file named by
    /// `LK_MIGRATOR_URL_FILE` is read from disk.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Self::from_parts(lookup, |path| std::fs::read_to_string(path))
    }

    /// Like [`MigratorSettings::from_lookup`], with the file reader given (a map in tests and in the fuzz target).
    pub fn from_parts(
        lookup: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&std::path::Path) -> std::io::Result<String>,
    ) -> Result<Self, ConfigError> {
        let reader = Reader::new(&lookup, &read_file);
        Self::read(&reader)
    }

    /// Every variable is checked before the file is read, so a Job that is missing two things hears about the first.
    fn read(reader: &Reader<'_>) -> Result<Self, ConfigError> {
        let url_file = reader
            .parse("LK_MIGRATOR_URL_FILE")?
            .ok_or(ConfigError::Missing("LK_MIGRATOR_URL_FILE"))?;
        let ca_file = reader
            .parse("LK_DB_CA_FILE")?
            .map(PathBuf::from)
            .ok_or(ConfigError::Missing("LK_DB_CA_FILE"))?;
        let timeout =
            Duration::from_secs(reader.number("LK_MIGRATE_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS, 1, 7200)?);
        let url = reader.file_text("LK_MIGRATOR_URL_FILE", &url_file)?;
        Ok(Self {
            url: Secret::new(url),
            ca_file,
            timeout,
        })
    }
}
