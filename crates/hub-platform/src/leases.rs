//! Leases on Postgres (D62, D63): at most one replica holds a named job at a time.
//!
//! One row per name in `leases`:
//!
//! - **Acquire** is one statement, `INSERT ... ON CONFLICT (name) DO UPDATE ... WHERE leases.expires_at < now()`. Two replicas
//!   that race for a free name both reach the row, the database serialises them on it, and the second one re-reads the row the
//!   first one wrote, finds it unexpired and updates nothing: zero rows back means somebody else holds it.
//! - **The fence** is `leases.fence`. A takeover (or a re-acquire after release) adds one; renewal never changes it. A guard
//!   remembers the fence it was given, and renews and releases only `WHERE holder = $ AND fence = $ AND expires_at > now()`, so
//!   a holder that stalled past its expiry cannot touch the lease of whoever took over, even if that is the same replica.
//! - **Release** does not delete the row: it moves the expiry into the past. A `DELETE` would let the next acquire start again
//!   at fence 1 while a stale guard still holds 1.
//! - **The clock** is the database's. A replica's own clock decides nothing but the guard's local `is_valid`, and the expiry
//!   the guard keeps is measured from before the request was sent, so it is never later than the row's.
//!
//! A lease is advisory, time-bound ownership and not a lock (see [`ports::LeaseGuard`]): work that writes must check
//! `is_valid()` first, and [`crate::jobs`] does so for the jobs it runs.
//!
//! Every call has a timeout (rule 5). A store that does not answer is [`LeaseError::Unavailable`], never a lease.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ports::{LeaseBackend, LeaseError, LeaseGuard, Leases, check_ttl, is_valid_lease_name};
use sqlx::PgPool;
use tokio::time::Instant;
use tracing::warn;

use crate::ca::profile::PeerName;

/// One database call.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// The statements, in one place so that the tests and the reader see the same text.
mod sql {
    pub const ACQUIRE: &str = "INSERT INTO leases (name, holder, expires_at) \
         VALUES ($1, $2, now() + make_interval(secs => $3::float8)) \
         ON CONFLICT (name) DO UPDATE SET \
           holder = excluded.holder, \
           expires_at = excluded.expires_at, \
           fence = leases.fence + 1, \
           acquired_at = now() \
         WHERE leases.expires_at < now() \
         RETURNING fence";

    pub const RENEW: &str = "UPDATE leases SET expires_at = now() + make_interval(secs => $4::float8) \
         WHERE name = $1 AND holder = $2 AND fence = $3 AND expires_at > now()";

    pub const RELEASE: &str = "UPDATE leases SET expires_at = now() - interval '1 second' \
         WHERE name = $1 AND holder = $2 AND fence = $3 AND expires_at > now()";
}

async fn within<T>(call: impl std::future::Future<Output = Result<T, sqlx::Error>>) -> Result<T, LeaseError> {
    match tokio::time::timeout(QUERY_TIMEOUT, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            warn!(code = "lease_store_error", %error, "the lease store refused a call");
            Err(LeaseError::Unavailable)
        }
        Err(_) => {
            warn!(
                code = "lease_store_timeout",
                "the lease store did not answer in time"
            );
            Err(LeaseError::Unavailable)
        }
    }
}

/// What the guards this store hands out call to renew and release.
#[derive(Debug)]
pub struct PgLeaseBackend {
    pool: PgPool,
    holder: String,
}

#[async_trait]
impl LeaseBackend for PgLeaseBackend {
    async fn renew(&self, name: &str, token: u64, ttl: Duration) -> Result<Instant, LeaseError> {
        check_ttl(ttl)?;
        let fence = fence_from_token(token)?;
        // Measured before the request goes out: the row's expiry is later than this, never earlier.
        let started = Instant::now();
        let done = within(
            sqlx::query(sql::RENEW)
                .bind(name)
                .bind(&self.holder)
                .bind(fence)
                .bind(ttl.as_secs_f64())
                .execute(&self.pool),
        )
        .await?;
        if done.rows_affected() == 0 {
            return Err(LeaseError::Lost);
        }
        Ok(started + ttl)
    }

    async fn release(&self, name: &str, token: u64) -> Result<(), LeaseError> {
        let fence = fence_from_token(token)?;
        let done = within(
            sqlx::query(sql::RELEASE)
                .bind(name)
                .bind(&self.holder)
                .bind(fence)
                .execute(&self.pool),
        )
        .await?;
        if done.rows_affected() == 0 {
            return Err(LeaseError::Lost);
        }
        Ok(())
    }
}

/// A fence that cannot be a database value is a lease that cannot be ours.
fn fence_from_token(token: u64) -> Result<i64, LeaseError> {
    i64::try_from(token).map_err(|_| LeaseError::Lost)
}

/// The `Leases` of one replica. Cloning shares the pool.
#[derive(Debug, Clone)]
pub struct PgLeases {
    backend: Arc<PgLeaseBackend>,
}

impl PgLeases {
    /// `holder` names this replica in the `holder` column; it is what an operator reads to see who runs a job.
    #[must_use]
    pub fn new(pool: PgPool, holder: &PeerName) -> Self {
        Self {
            backend: Arc::new(PgLeaseBackend {
                pool,
                holder: holder.to_string(),
            }),
        }
    }
}

#[async_trait]
impl Leases for PgLeases {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError> {
        if !is_valid_lease_name(name) {
            return Err(LeaseError::InvalidName);
        }
        check_ttl(ttl)?;
        // Measured before the request goes out, so the guard never outlives the row.
        let started = Instant::now();
        let fence: Option<i64> = within(
            sqlx::query_scalar(sql::ACQUIRE)
                .bind(name)
                .bind(&self.backend.holder)
                .bind(ttl.as_secs_f64())
                .fetch_optional(&self.backend.pool),
        )
        .await?;
        let Some(fence) = fence else {
            return Ok(None);
        };
        // Fences start at 1 and only grow. Anything else is a row nobody here wrote.
        let token = u64::try_from(fence)
            .ok()
            .filter(|f| *f >= 1)
            .ok_or(LeaseError::Unavailable)?;
        Ok(Some(LeaseGuard::new(
            name.to_owned(),
            token,
            started + ttl,
            self.backend.clone(),
        )))
    }
}
