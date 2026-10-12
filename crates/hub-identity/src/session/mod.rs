//! Server-side sessions (T3; S2).
//!
//! - [`cookie`]: the cookie names, the strict `Cookie` header parser, the random tokens and the `Set-Cookie` builders.
//! - [`store`]: the `sessions` table. The session id is 256 random bits; only its SHA-256 is stored.
//! - [`cache`]: the 30-second in-process cache that keeps a session lookup under 2 ms.
//! - [`touch`]: `last_seen_at`, written in batches off the request path.
//! - [`revocation`]: a role or status change ends the user's sessions on every replica within a second.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::task::JoinHandle;

use crate::clock::Clock;

pub mod cache;
pub mod cookie;
pub mod revocation;
pub mod store;
pub mod touch;

pub use cache::{CachedSession, SessionCache};
pub use cookie::{RandomToken, TokenHash};
pub use revocation::RevocationListener;
pub use store::{NewSession, SessionRow, SessionStore, SweepReport};
pub use touch::{FlushReport, LastSeenBatcher};

/// The limits that make a session (D40, S2) and its cache. The defaults are the product's; tests build others.
#[derive(Debug, Clone)]
pub struct SessionPolicy {
    /// A session unused for this long is over.
    pub idle: Duration,
    /// A session this old is over, however busy.
    pub absolute: Duration,
    /// How long the cache may serve a session without asking the database.
    pub cache_ttl: Duration,
    /// How many sessions the cache holds.
    pub cache_capacity: u64,
    /// `last_seen_at` is written at most this often per session.
    pub touch_interval: Duration,
    /// How many touches may wait for the next batch; more are dropped (the next request touches again).
    pub touch_queue: usize,
    /// How many rows one `last_seen_at` statement updates.
    pub touch_batch: usize,
    /// How often the queued touches are written.
    pub touch_flush: Duration,
    /// How often expired sessions are swept, and how many rows one sweep statement removes.
    pub sweep_every: Duration,
    pub sweep_batch: u32,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(8 * 3600),
            absolute: Duration::from_secs(12 * 3600),
            cache_ttl: Duration::from_secs(30),
            cache_capacity: 50_000,
            touch_interval: Duration::from_secs(60),
            touch_queue: 4096,
            touch_batch: 500,
            touch_flush: Duration::from_secs(5),
            sweep_every: Duration::from_secs(10 * 60),
            sweep_batch: 1_000,
        }
    }
}

/// Why a session operation failed. No variant carries a session id, a cookie, a user or a SQL message (S10, S21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The database failed or timed out. The cause is logged by SQLSTATE.
    #[error("session store is unavailable")]
    Unavailable,
    /// The system could not make random bytes.
    #[error("crypto provider failed")]
    Crypto,
}

/// A background task that stops when its owner is dropped.
#[derive(Debug)]
pub(crate) struct AbortOnDrop(JoinHandle<()>);

impl AbortOnDrop {
    pub(crate) fn spawn(task: impl std::future::Future<Output = ()> + Send + 'static) -> Self {
        Self(tokio::spawn(task))
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The sessions of one hub replica: the table, the cache in front of it, and the two background jobs (batched `last_seen_at`
/// writes, and the sweep of expired rows). Dropping it stops the jobs.
pub struct Sessions {
    store: SessionStore,
    cache: Arc<SessionCache>,
    touch: LastSeenBatcher,
    clock: Arc<dyn Clock>,
    idle_ms: i64,
    absolute: Duration,
    _sweeper: AbortOnDrop,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions").finish_non_exhaustive()
    }
}

impl Sessions {
    /// Needs a Tokio runtime: it starts the jobs.
    pub fn new(pool: PgPool, clock: Arc<dyn Clock>, policy: &SessionPolicy) -> Self {
        let store = SessionStore::new(pool.clone(), Arc::clone(&clock), policy);
        let cache = Arc::new(SessionCache::new(
            policy.cache_capacity,
            policy.cache_ttl,
            Arc::clone(&clock),
        ));
        let touch = LastSeenBatcher::spawn(pool, Arc::clone(&clock), policy);
        let sweeper = {
            let (store, every, batch) = (store.clone(), policy.sweep_every, policy.sweep_batch);
            AbortOnDrop::spawn(async move {
                let mut ticker = tokio::time::interval(every.max(Duration::from_secs(1)));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    match store.sweep_expired(batch).await {
                        Ok(report) if report.deleted > 0 => {
                            tracing::debug!(deleted = report.deleted, "expired sessions removed");
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "session sweep failed"),
                    }
                }
            })
        };
        Self {
            idle_ms: i64::try_from(policy.idle.as_millis()).unwrap_or(i64::MAX),
            absolute: policy.absolute,
            store,
            cache,
            touch,
            clock,
            _sweeper: sweeper,
        }
    }

    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    pub fn cache(&self) -> &Arc<SessionCache> {
        &self.cache
    }

    pub fn touch(&self) -> &LastSeenBatcher {
        &self.touch
    }

    /// How long a session lives at most: the `Max-Age` of its cookies.
    pub fn store_lifetime(&self) -> Duration {
        self.absolute
    }

    /// The live session a cookie value stands for, or `None`: not a token, not a session, over its 12 hours, or idle for
    /// 8. A live session is touched (its `last_seen_at` is queued for the next batch).
    ///
    /// The answer comes from the cache when it has the session younger than 30 seconds, and from one indexed read otherwise.
    /// A database failure is an error, never "no session": callers fail closed.
    pub async fn authenticate(&self, cookie_value: &str) -> Result<Option<Arc<CachedSession>>, SessionError> {
        Ok(self
            .authenticate_hashed(cookie_value)
            .await?
            .map(|(_, session)| session))
    }

    /// Like [`Sessions::authenticate`], and also the hash of the session id, which logout needs.
    pub async fn authenticate_hashed(
        &self,
        cookie_value: &str,
    ) -> Result<Option<(TokenHash, Arc<CachedSession>)>, SessionError> {
        let Some(token) = RandomToken::from_text(cookie_value) else {
            return Ok(None);
        };
        let hash = token.hash();
        let found = self
            .cache
            .get_or_load(hash, || async {
                let loaded_at = self.clock.now();
                Ok(self.store.lookup(&hash).await?.map(|row| CachedSession {
                    user: row.user,
                    role: row.role,
                    status: row.status,
                    csrf_hash: row.csrf_hash,
                    last_seen_at: row.last_seen_at,
                    expires_at: row.expires_at,
                    loaded_at,
                }))
            })
            .await?;
        let Some(session) = found else {
            return Ok(None);
        };
        let now = self.clock.now().unix_millis();
        if now >= session.expires_at.unix_millis()
            || now.saturating_sub(session.last_seen_at.unix_millis()) >= self.idle_ms
        {
            return Ok(None);
        }
        self.touch.touch(hash);
        Ok(Some((hash, session)))
    }
}
