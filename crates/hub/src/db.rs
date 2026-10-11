//! The Postgres pool (T1, S6): TLS with full verification against the cluster's CA, bounded, with a statement timeout.
//!
//! There is no way to connect without TLS or without verifying the server's certificate and name: the pool is always built
//! with `verify-full` and the CA file from the settings, whatever the URL says. A URL that asks for something weaker is
//! overridden, not obeyed. Errors carry a code only: a connection error from a driver can name the host and the user.

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::ConnectOptions;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};

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
    let timeout_ms = settings.statement_timeout.as_millis().to_string();
    let options = PgConnectOptions::from_str(settings.url.expose())
        .map_err(|_| DbError::Url)?
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert(&settings.ca_file)
        .options([("statement_timeout", timeout_ms.as_str())])
        .log_statements(tracing::log::LevelFilter::Off);
    Ok(PoolOptions { inner: options })
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
