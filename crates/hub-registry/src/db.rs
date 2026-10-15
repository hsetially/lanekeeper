//! `Db`: the registry's door to Postgres (rule 5, S21).
//!
//! - **Every call has a timeout.** A statement that runs past it is dropped on the client side, and each transaction also sets
//!   `statement_timeout` for the server (`SET LOCAL`), so a statement that outlives the client is cancelled too.
//! - **No driver text escapes.** A failure becomes a [`RegistryError`] and a log line with a fixed word (the operation, and
//!   `pool_timed_out`, `io`, `database` or `other`). A driver's own message can name the host, the role or the SQL, so it is
//!   never logged.
//! - **Queries are runtime-checked with binds** (plan A39). No SQL is built from input.

use std::future::Future;
use std::time::Duration;

use sqlx::postgres::PgPool;
use sqlx::{Postgres, Transaction};
use tracing::warn;

use crate::error::RegistryError;

/// A fixed word for why a call failed. A driver's own message is never logged.
fn kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::PoolTimedOut => "pool_timed_out",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "io",
        sqlx::Error::Database(_) => "database",
        sqlx::Error::RowNotFound => "row_not_found",
        sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "decode",
        _ => "other",
    }
}

/// An open transaction that owns its connection.
pub type Txn = Transaction<'static, Postgres>;

#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
    timeout: Duration,
}

impl Db {
    pub fn new(pool: PgPool, timeout: Duration) -> Self {
        Self { pool, timeout }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Run one database future with the call timeout. `op` is a fixed word for the log.
    pub async fn run<T, F>(&self, op: &'static str, call: F) -> Result<T, RegistryError>
    where
        F: Future<Output = Result<T, sqlx::Error>>,
    {
        run_within(self.timeout, op, call).await
    }

    /// Start a transaction whose statements the server cancels after the call timeout.
    pub async fn begin(&self) -> Result<Txn, RegistryError> {
        let mut tx = self.run("begin", self.pool.begin()).await?;
        let millis = i64::try_from(self.timeout.as_millis()).unwrap_or(i64::MAX);
        self.run(
            "begin",
            sqlx::query("SELECT set_config('statement_timeout', $1, true)")
                .bind(millis.to_string())
                .execute(&mut *tx),
        )
        .await?;
        Ok(tx)
    }

    /// Commit, with the call timeout.
    pub async fn commit(&self, tx: Txn) -> Result<(), RegistryError> {
        self.run("commit", tx.commit()).await
    }
}

/// Run one database future with a timeout of the caller's choosing, logging a failure as a fixed word (the operation, and
/// `pool_timed_out`, `io`, `database` or `other`: never the driver's own text, S21). [`Db::run`] is this with the registry's call
/// timeout; a place whose bound is its own (a registration inside a transaction that is the caller's) uses it directly.
pub(crate) async fn run_within<T, F>(timeout: Duration, op: &'static str, call: F) -> Result<T, RegistryError>
where
    F: Future<Output = Result<T, sqlx::Error>>,
{
    match tokio::time::timeout(timeout, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            warn!(op, why = kind(&error), "database call failed");
            Err(RegistryError::Database)
        }
        Err(_) => {
            warn!(op, why = "timed_out", "database call failed");
            Err(RegistryError::Timeout)
        }
    }
}

/// `timestamptz` from a bound Unix-millisecond value. Whole milliseconds, exact: the replay key of `file_observations` and the
/// "older than stored" test compare these values, so they must not round. The interval is parsed from text, which is integer
/// arithmetic; `$n::bigint * interval '1 millisecond'` multiplies in floating point and is off by microseconds from the year
/// 2255 on (the same reason the audit log builds its `at` this way), and an `observed_at` may name any year up to 9999.
pub(crate) fn ts(param: &str) -> String {
    format!("(timestamptz 'epoch' + ({param}::bigint::text || ' milliseconds')::interval)")
}

/// Unix milliseconds of a `timestamptz` column, exact for values written by [`ts`].
pub(crate) fn ms(column: &str) -> String {
    format!("(EXTRACT(EPOCH FROM {column}) * 1000)::bigint")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_helpers_name_their_parameter_and_column() {
        assert!(ts("$3").contains("$3::bigint"));
        assert!(
            !ts("$3").contains("* interval"),
            "integer arithmetic only: a float multiply is off from the year 2255"
        );
        assert!(ms("observed_at").contains("observed_at"));
    }

    #[test]
    fn a_failure_is_a_fixed_word() {
        assert_eq!(kind(&sqlx::Error::PoolTimedOut), "pool_timed_out");
        assert_eq!(kind(&sqlx::Error::PoolClosed), "pool_closed");
        assert_eq!(kind(&sqlx::Error::RowNotFound), "row_not_found");
        assert_eq!(kind(&sqlx::Error::WorkerCrashed), "other");
    }
}
