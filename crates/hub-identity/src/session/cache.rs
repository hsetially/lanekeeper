//! The in-process session cache (T3; S2, P7): at most 50,000 entries, each for at most 30 seconds, with one database load
//! per missing session however many requests ask at once.
//!
//! The cache holds the session row and the user's role and status, never the CSRF token (only its hash). It may be wrong for
//! 30 seconds, and three things shorten that:
//!
//! - **Eviction by user.** A role or status change publishes `UserSessionsRevoked`; every replica's listener calls
//!   [`SessionCache::evict_user`] (see `revocation`).
//! - **Flush.** `Resync` (the listener lagged) flushes everything.
//! - **Off.** When the event stream ends the cache is switched off ([`SessionCache::set_enabled`]) and every request reads
//!   the database, until the listener has subscribed again.
//!
//! A load that started before an eviction must not put its answer in the cache afterwards. Every eviction bumps an epoch;
//! a load that sees a different epoch when it finishes removes its own entry (the request that did the load still uses it
//! once, as if it had run just before the change).
//!
//! The 30-second limit is compared on the wall clock the rest of the crate uses (`Clock`), so tests move it; moka's own
//! real-time limit is the same 30 seconds and bounds memory regardless.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use domain::{Role, Timestamp, UserId, UserStatus};

use super::SessionError;
use super::cookie::TokenHash;
use crate::clock::Clock;

/// What the middleware needs to know about a session. A snapshot, at most [`SessionCache`]'s time-to-live old.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedSession {
    pub user: UserId,
    pub role: Option<Role>,
    pub status: UserStatus,
    /// The hash of the CSRF token. The token itself is never stored.
    pub csrf_hash: TokenHash,
    /// From the database at load time. The idle limit is judged against it.
    pub last_seen_at: Timestamp,
    pub expires_at: Timestamp,
    /// The wall-clock time of the load, for the time-to-live check.
    pub loaded_at: Timestamp,
}

pub struct SessionCache {
    entries: moka::future::Cache<TokenHash, Arc<CachedSession>>,
    epoch: AtomicU64,
    enabled: AtomicBool,
    ttl_ms: i64,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for SessionCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCache")
            .field("enabled", &self.is_enabled())
            .finish_non_exhaustive()
    }
}

/// Why a single-flight load did not produce an entry.
#[derive(Debug, Clone, Copy)]
enum LoadFailure {
    /// There is no such session. Not cached: the next request asks again.
    Missing,
    Failed(SessionError),
}

impl SessionCache {
    /// `capacity` entries at most; each lives `ttl` at most.
    pub fn new(capacity: u64, ttl: Duration, clock: Arc<dyn Clock>) -> Self {
        let entries = moka::future::Cache::builder()
            .max_capacity(capacity)
            // Every entry weighs one: they are all the same size.
            .weigher(|_key, _value| 1)
            .time_to_live(ttl)
            .support_invalidation_closures()
            .build();
        Self {
            entries,
            epoch: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
            ttl_ms: i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX),
            clock,
        }
    }

    /// The entry for `key`, loading it with `load` when it is missing or too old. Concurrent calls for one key share one
    /// load. `Ok(None)` means there is no such session. While the cache is off, `load` runs every time.
    pub async fn get_or_load<F, Fut>(
        &self,
        key: TokenHash,
        load: F,
    ) -> Result<Option<Arc<CachedSession>>, SessionError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<CachedSession>, SessionError>> + Send,
    {
        if !self.is_enabled() {
            return Ok(load().await?.map(Arc::new));
        }
        if let Some(hit) = self.entries.get(&key).await {
            if self.fresh(&hit) {
                return Ok(Some(hit));
            }
            self.entries.invalidate(&key).await;
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        let loaded = self
            .entries
            .try_get_with(key, async {
                match load().await {
                    Ok(Some(session)) => Ok(Arc::new(session)),
                    Ok(None) => Err(LoadFailure::Missing),
                    Err(e) => Err(LoadFailure::Failed(e)),
                }
            })
            .await;
        match loaded {
            Ok(session) => {
                // An eviction, a flush or a switch-off happened while the row was being read: the row may be older than the
                // change, so it is used by the request that read it and not kept.
                if self.epoch.load(Ordering::Acquire) != epoch || !self.is_enabled() {
                    self.entries.invalidate(&key).await;
                }
                Ok(Some(session))
            }
            Err(failure) => match *failure {
                LoadFailure::Missing => Ok(None),
                LoadFailure::Failed(e) => Err(e),
            },
        }
    }

    /// Forget every entry of `user`, on this replica, now.
    pub fn evict_user(&self, user: &UserId) {
        self.bump_epoch();
        let user = user.clone();
        // The closure is applied lazily, but an entry it matches is never returned once this call has returned.
        if self
            .entries
            .invalidate_entries_if(move |_, s| s.user == user)
            .is_err()
        {
            self.entries.invalidate_all();
        }
    }

    /// Forget everything.
    pub fn flush(&self) {
        self.bump_epoch();
        self.entries.invalidate_all();
    }

    /// Switch the cache on or off. Turning it off also empties it.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Release);
        if !on {
            self.flush();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Apply pending housekeeping now. Tests call it before they look at the size.
    pub async fn sync(&self) {
        self.entries.run_pending_tasks().await;
    }

    /// The number of entries, after [`SessionCache::sync`].
    pub fn entry_count(&self) -> u64 {
        self.entries.entry_count()
    }

    /// Younger than the time-to-live, and not from the future (a wall clock that moved back is not trusted).
    fn fresh(&self, entry: &CachedSession) -> bool {
        let age = self
            .clock
            .now()
            .unix_millis()
            .saturating_sub(entry.loaded_at.unix_millis());
        (0..self.ttl_ms).contains(&age)
    }

    fn bump_epoch(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use domain::Guid;

    use super::*;
    use crate::clock::ManualClock;

    const TID: &str = "72f988bf-86f1-41af-91ab-2d7cd011db47";
    const START: i64 = 1_791_676_800_000;

    fn user(n: u32) -> UserId {
        UserId::new(
            Guid::parse(TID).unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        )
    }

    fn key(n: u8) -> TokenHash {
        TokenHash::from_bytes([n; 32])
    }

    fn entry(n: u32, clock: &ManualClock) -> CachedSession {
        CachedSession {
            user: user(n),
            role: Some(Role::Viewer),
            status: UserStatus::Active,
            csrf_hash: TokenHash::from_bytes([9; 32]),
            last_seen_at: clock.now(),
            expires_at: Timestamp::from_unix_millis(clock.now().unix_millis() + 12 * 3_600_000),
            loaded_at: clock.now(),
        }
    }

    fn cache(capacity: u64) -> (Arc<ManualClock>, SessionCache) {
        let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(START)));
        let cache = SessionCache::new(
            capacity,
            Duration::from_secs(30),
            Arc::clone(&clock) as Arc<dyn Clock>,
        );
        (clock, cache)
    }

    /// A loader that counts its calls and answers with a session of user `n`.
    fn loader<'a>(
        calls: &'a AtomicUsize,
        clock: &'a ManualClock,
        n: u32,
    ) -> impl FnOnce() -> std::future::Ready<Result<Option<CachedSession>, SessionError>> + 'a {
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(Some(entry(n, clock))))
        }
    }

    #[tokio::test]
    async fn a_loaded_entry_is_served_again_without_a_load() {
        let (clock, cache) = cache(100);
        let calls = AtomicUsize::new(0);
        let a = cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap()
            .unwrap();
        let b = cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn an_entry_older_than_30_seconds_is_loaded_again() {
        let (clock, cache) = cache(100);
        let calls = AtomicUsize::new(0);
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        clock.advance_secs(29);
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "29 s: still fresh");
        clock.advance_secs(1);
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "30 s: reloaded");
        // A wall clock that moved backwards is not trusted either.
        clock.set(Timestamp::from_unix_millis(START - 60_000));
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_missing_session_is_not_cached() {
        let (_clock, cache) = cache(100);
        let calls = AtomicUsize::new(0);
        for _ in 0..3 {
            let got = cache
                .get_or_load(key(1), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(Ok(None))
                })
                .await
                .unwrap();
            assert!(got.is_none());
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "an unknown cookie is looked up each time, and never remembered"
        );
        cache.sync().await;
        assert_eq!(cache.entry_count(), 0);
    }

    #[tokio::test]
    async fn a_failed_load_is_an_error_and_is_not_cached() {
        let (clock, cache) = cache(100);
        let err = cache
            .get_or_load(key(1), || std::future::ready(Err(SessionError::Unavailable)))
            .await
            .unwrap_err();
        assert_eq!(err, SessionError::Unavailable);
        let calls = AtomicUsize::new(0);
        assert!(
            cache
                .get_or_load(key(1), loader(&calls, &clock, 1))
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cache_miss_is_single_flight() {
        let (clock, cache) = cache(100);
        let cache = Arc::new(cache);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..64 {
            let (cache, calls, clock) = (Arc::clone(&cache), Arc::clone(&calls), Arc::clone(&clock));
            tasks.push(tokio::spawn(async move {
                cache
                    .get_or_load(key(7), || async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        // Long enough for every other task to arrive while this load is in flight.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Ok(Some(entry(7, &clock)))
                    })
                    .await
            }));
        }
        for t in tasks {
            assert!(t.await.unwrap().unwrap().is_some());
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "64 concurrent misses made one database load"
        );
    }

    #[tokio::test]
    async fn cache_is_bounded() {
        let (clock, cache) = cache(100);
        let calls = AtomicUsize::new(0);
        for n in 0..1_000u32 {
            let k = TokenHash::from_bytes({
                let mut b = [0u8; 32];
                b[..4].copy_from_slice(&n.to_be_bytes());
                b
            });
            cache.get_or_load(k, loader(&calls, &clock, n)).await.unwrap();
        }
        cache.sync().await;
        assert!(cache.entry_count() <= 100, "{} entries", cache.entry_count());
    }

    #[tokio::test]
    async fn evicting_a_user_removes_only_that_users_entries() {
        let (clock, cache) = cache(100);
        let calls = AtomicUsize::new(0);
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        cache
            .get_or_load(key(2), loader(&calls, &clock, 1))
            .await
            .unwrap();
        cache
            .get_or_load(key(3), loader(&calls, &clock, 2))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        cache.evict_user(&user(1));
        cache
            .get_or_load(key(3), loader(&calls, &clock, 2))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3, "the other user's entry stays");
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        cache
            .get_or_load(key(2), loader(&calls, &clock, 1))
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            5,
            "both sessions of the evicted user are loaded again"
        );
    }

    #[tokio::test]
    async fn flush_removes_everything() {
        let (clock, cache) = cache(100);
        let calls = AtomicUsize::new(0);
        for n in 1..=3 {
            cache
                .get_or_load(key(n), loader(&calls, &clock, u32::from(n)))
                .await
                .unwrap();
        }
        cache.flush();
        for n in 1..=3 {
            cache
                .get_or_load(key(n), loader(&calls, &clock, u32::from(n)))
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn a_switched_off_cache_loads_every_time_and_keeps_nothing() {
        let (clock, cache) = cache(100);
        let calls = AtomicUsize::new(0);
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        cache.set_enabled(false);
        assert!(!cache.is_enabled());
        for _ in 0..3 {
            cache
                .get_or_load(key(1), loader(&calls, &clock, 1))
                .await
                .unwrap();
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "the old entry was dropped and nothing was kept while off"
        );
        cache.sync().await;
        assert_eq!(cache.entry_count(), 0);
        cache.set_enabled(true);
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn a_load_that_raced_an_eviction_is_not_kept() {
        let (clock, cache) = cache(100);
        let cache = Arc::new(cache);
        let calls = AtomicUsize::new(0);
        let started = Arc::new(tokio::sync::Notify::new());
        let go = Arc::new(tokio::sync::Notify::new());
        let slow = {
            let (cache, started, go, clock) = (
                Arc::clone(&cache),
                Arc::clone(&started),
                Arc::clone(&go),
                Arc::clone(&clock),
            );
            tokio::spawn(async move {
                cache
                    .get_or_load(key(1), || async move {
                        // The row was read before the change; the change is processed before the answer is stored.
                        started.notify_one();
                        go.notified().await;
                        Ok(Some(entry(1, &clock)))
                    })
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("the load started");
        cache.evict_user(&user(1));
        go.notify_one();
        assert!(
            slow.await.unwrap().unwrap().is_some(),
            "the request that loaded it can still use it once"
        );
        cache.sync().await;
        cache
            .get_or_load(key(1), loader(&calls, &clock, 1))
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the stale answer was not kept: the next request loads again"
        );
    }
}
