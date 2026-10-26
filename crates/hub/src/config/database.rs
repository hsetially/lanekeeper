//! Where Postgres is and how it is reached (S6, S7): TLS with full verification against the cluster's own CA.

use std::path::PathBuf;
use std::time::Duration;

use domain::Secret;

use super::ConfigError;
use super::reader::Reader;

#[derive(Debug)]
pub struct DatabaseSettings {
    pub url: Secret<String>,
    /// The CA that signed the server's certificate (`CloudNativePG`'s own CA). Required: there is no "trust the system" mode.
    pub ca_file: PathBuf,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub statement_timeout: Duration,
}

impl DatabaseSettings {
    /// `None` when no URL is set. The URL holds a password, so it is read from a mounted file (`LK_DATABASE_URL_FILE`) and is
    /// never in the process environment, where a child process, a crash dump or `/proc/<pid>/environ` could show it. The plain
    /// variable exists for local development and is refused when the file variable is also set.
    pub(super) fn read(reader: &Reader<'_>) -> Result<Option<Self>, ConfigError> {
        let url = match (
            reader.parse("LK_DATABASE_URL_FILE")?,
            reader.parse("LK_DATABASE_URL")?,
        ) {
            (Some(_), Some(_)) => return Err(ConfigError::Invalid("LK_DATABASE_URL")),
            (Some(path), None) => Some(reader.file_text("LK_DATABASE_URL_FILE", &path)?),
            (None, url) => url,
        };
        let Some(url) = url else { return Ok(None) };
        Ok(Some(Self {
            url: Secret::new(url),
            ca_file: reader
                .parse("LK_DB_CA_FILE")?
                .map(PathBuf::from)
                .ok_or(ConfigError::Missing("LK_DB_CA_FILE"))?,
            max_connections: reader.count("LK_DB_MAX_CONNECTIONS", 16, 1, 512)?,
            acquire_timeout: Duration::from_secs(reader.number("LK_DB_ACQUIRE_TIMEOUT_SECS", 5, 1, 60)?),
            statement_timeout: Duration::from_secs(reader.number(
                "LK_DB_STATEMENT_TIMEOUT_SECS",
                30,
                1,
                600,
            )?),
        }))
    }
}
