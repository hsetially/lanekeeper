//! Leases: at most one holder of a named job across hub replicas (D63).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    #[error("lease name is not valid")]
    InvalidName,
    #[error("lease ttl must be more than zero and at most one hour")]
    InvalidTtl,
    /// The lease expired and was taken, or was released. The holder must stop its work.
    #[error("lease was lost")]
    Lost,
    #[error("lease store is unavailable")]
    Unavailable,
}

/// Whether `name` is acceptable to [`Leases::try_acquire`]: 1-128 characters from `[a-z0-9._:-]`.
pub fn is_valid_lease_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b':' | b'-'))
}

pub const MAX_LEASE_TTL: Duration = Duration::from_secs(3600);

/// The store behind a [`LeaseGuard`]. Implemented by lease stores, not by callers.
#[async_trait]
pub trait LeaseBackend: Send + Sync + 'static {
    /// Extend the lease held with `token`. Returns the new expiry, or [`LeaseError::Lost`].
    async fn renew(&self, name: &str, token: u64, ttl: Duration) -> Result<Instant, LeaseError>;
    /// Give the lease up so that others can take it at once.
    async fn release(&self, name: &str, token: u64) -> Result<(), LeaseError>;
}

/// Proof that this replica holds a lease until [`LeaseGuard::expires_at`].
///
/// A lease is advisory time-bound ownership, not a lock: call [`LeaseGuard::is_valid`] before each unit of
/// work, [`LeaseGuard::renew`] on long jobs, and stop on [`LeaseError::Lost`]. Dropping the guard does not
/// release the lease (a destructor cannot await); it simply runs out. Call [`LeaseGuard::release`] when done.
pub struct LeaseGuard {
    name: String,
    token: u64,
    expires_at: Instant,
    backend: Arc<dyn LeaseBackend>,
}

impl LeaseGuard {
    /// For lease stores to build a guard after a successful acquire.
    pub fn new(name: String, token: u64, expires_at: Instant, backend: Arc<dyn LeaseBackend>) -> Self {
        Self {
            name,
            token,
            expires_at,
            backend,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    /// True while the local clock is before the expiry the store granted.
    pub fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at
    }

    /// Push the expiry out to now + `ttl`.
    pub async fn renew(&mut self, ttl: Duration) -> Result<(), LeaseError> {
        check_ttl(ttl)?;
        self.expires_at = self.backend.renew(&self.name, self.token, ttl).await?;
        Ok(())
    }

    /// Release the lease so that another replica can acquire it immediately.
    pub async fn release(self) -> Result<(), LeaseError> {
        self.backend.release(&self.name, self.token).await
    }
}

impl fmt::Debug for LeaseGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseGuard")
            .field("name", &self.name)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Validate a requested ttl: more than zero and at most [`MAX_LEASE_TTL`].
pub fn check_ttl(ttl: Duration) -> Result<(), LeaseError> {
    if ttl.is_zero() || ttl > MAX_LEASE_TTL {
        return Err(LeaseError::InvalidTtl);
    }
    Ok(())
}

#[async_trait]
pub trait Leases: Send + Sync + 'static {
    /// `Ok(None)` when another holder has an unexpired lease on `name`.
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError>;
}
