//! The Postgres pool (T1, S6): TLS with full verification against the cluster's CA, bounded, with a statement timeout.
//!
//! There is no way to connect without TLS or without verifying the server's certificate and name: the pool is always built
//! with `verify-full` and the CA file from the settings, whatever the URL says. A URL that asks for something weaker is
//! overridden, not obeyed. Errors carry a code only: a connection error from a driver can name the host and the user.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use domain::Secret;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions, PgSslMode};
use sqlx::{ConnectOptions, Connection, PgPool};

use crate::config::DatabaseSettings;
use crate::ops::ReadyCheck;

/// Why the pool could not be built. None of these carries the URL, a host or a driver message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DbError {
    #[error("the database URL is not valid")]
    Url,
    #[error("could not connect to the database")]
    Connect,
}

/// The connection options, with TLS forced on. sqlx's own `Debug` for them prints the password, so they are wrapped: this
/// type prints the host, port, database and TLS mode, and nothing that is a credential.
pub struct PoolOptions {
    inner: PgConnectOptions,
}

impl PoolOptions {
    #[must_use]
    pub fn ssl_mode(&self) -> PgSslMode {
        self.inner.get_ssl_mode()
    }

    #[must_use]
    pub fn host(&self) -> &str {
        self.inner.get_host()
    }

    /// The role the URL logs in as. It is not a secret, and `hub migrate` refuses any but its own before it connects.
    #[must_use]
    pub fn username(&self) -> &str {
        self.inner.get_username()
    }
}

impl std::fmt::Debug for PoolOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolOptions")
            .field("host", &self.inner.get_host())
            .field("port", &self.inner.get_port())
            .field("database", &self.inner.get_database())
            .field("ssl_mode", &self.inner.get_ssl_mode())
            .finish_non_exhaustive()
    }
}

/// Build the options. Separate from [`connect`] so a test can check them without a server.
pub fn connect_options(settings: &DatabaseSettings) -> Result<PoolOptions, DbError> {
    connect_options_for(&settings.url, &settings.ca_file, settings.statement_timeout)
}

/// The same options for a URL and CA that are not the hub's own pool's: `hub migrate` connects as another role to the same
/// server, and gets exactly the same TLS rules (`verify-full` against the given CA, whatever the URL says).
pub fn connect_options_for(
    url: &Secret<String>,
    ca_file: &Path,
    statement_timeout: Duration,
) -> Result<PoolOptions, DbError> {
    let timeout_ms = statement_timeout.as_millis().to_string();
    let options = PgConnectOptions::from_str(url.expose())
        .map_err(|_| DbError::Url)?
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert(ca_file)
        .options([("statement_timeout", timeout_ms.as_str())])
        .log_statements(tracing::log::LevelFilter::Off);
    Ok(PoolOptions { inner: options })
}

/// One connection, no pool: for a command that runs once and exits. The caller bounds how long it may take.
pub async fn connect_one(options: &PoolOptions) -> Result<PgConnection, DbError> {
    PgConnection::connect_with(&options.inner)
        .await
        .map_err(|_| DbError::Connect)
}

/// Build the pool and check that one connection works.
pub async fn connect(settings: &DatabaseSettings) -> Result<PgPool, DbError> {
    let options = connect_options(settings)?.inner;
    PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .acquire_timeout(settings.acquire_timeout)
        .idle_timeout(Some(Duration::from_secs(300)))
        .max_lifetime(Some(Duration::from_secs(1800)))
        .connect_with(options)
        .await
        .map_err(|_| DbError::Connect)
}

/// Readiness: Postgres answers a trivial query.
#[derive(Debug, Clone)]
pub struct PostgresReady(pub PgPool);

#[async_trait]
impl ReadyCheck for PostgresReady {
    async fn ready(&self) -> bool {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.0)
            .await
            .is_ok()
    }
}
