//! The users behind the sessions, in memory (plan 18, T4, G18; S1, S2, P7).
//!
//! The platform's guard asks for a whole [`User`] on every request: the e-mail and the name go to the audit trail of a write, and
//! `Users::get` is one indexed read. At 100 users and the request rates of the design that read would be most of the 2 ms that
//! authentication may add, and all of it avoidable, so the users are kept: at most [`USER_CACHE_ENTRIES`] of them, each for at
//! most [`USER_CACHE_TTL`], which is the session cache's own.
//!
//! **What the cache may be wrong about.** The profile: e-mail, name, the GitHub login, whether approval is required. Never the
//! authorization: the role and the status the guard decides on come from the session the request carries, which identity's cache
//! already evicts on a role or status change (`CookieAuth` takes them from there). This cache is evicted by the same event
//! (`UserSessionsRevoked`, published in the transaction of the change), so a profile is stale for a moment at worst; the long-lived
//! stream's `still_valid` reads the role from here, and that is why the eviction matters.
//!
//! **Fail closed, like the session cache.** An eviction bumps an epoch, and a load that sees another epoch when it finishes does
//! not keep its answer. `Resync` (the listener lagged) flushes everything. When the event stream ends the cache is switched off,
//! so every request reads the database, until the listener has subscribed again. It subscribes before it flushes and switches the
//! cache back on, so nothing slips between.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use domain::{DomainEvent, User, UserId};
use futures::StreamExt;
use futures::stream::BoxStream;
use ports::{AuthError, EventBus, EventFilter, EventKind};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// How many users are kept. Design scale is 100 web users; the session cache holds 50,000 sessions, and this holds as many
/// distinct users as that could name.
pub const USER_CACHE_ENTRIES: u64 = 50_000;
/// How long a user is kept: the session cache's time to live.
pub const USER_CACHE_TTL: Duration = Duration::from_secs(30);

/// The first and the longest wait before subscribing again after the stream ended. Full jitter between zero and the wait.
const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_LONGEST: Duration = Duration::from_secs(5);

/// Why a single-flight load did not produce an entry.
#[derive(Debug, Clone, Copy)]
enum LoadFailure {
    /// There is no such user. Not cached: the next request asks again.
    Missing,
    Failed(AuthError),
}

/// A user and the moment it was read. The age is judged here, on tokio's clock, so that a test can move time; moka's own time to
/// live (real time) stays on as the bound on how long memory is held.
#[derive(Clone)]
struct Held {
    user: Arc<User>,
    loaded: Instant,
}

pub struct UserCache {
    entries: moka::future::Cache<UserId, Held>,
    ttl: Duration,
    epoch: AtomicU64,
    enabled: AtomicBool,
}

impl std::fmt::Debug for UserCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserCache")
            .field("enabled", &self.is_enabled())
            .finish_non_exhaustive()
    }
}

impl UserCache {
    /// `capacity` users at most, each for `ttl` at most.
    #[must_use]
    pub fn new(capacity: u64, ttl: Duration) -> Self {
        Self {
            entries: moka::future::Cache::builder()
                .max_capacity(capacity)
                // Every entry weighs one: they are all about the same size.
                .weigher(|_key, _value| 1)
                .time_to_live(ttl)
                .support_invalidation_closures()
                .build(),
            ttl,
            epoch: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
        }
    }

    /// The user, from memory or from `load`. Concurrent calls for one user share one load. `Ok(None)` means there is no such
    /// user. While the cache is off, `load` runs every time.
    pub async fn get_or_load<F, Fut>(&self, id: &UserId, load: F) -> Result<Option<Arc<User>>, AuthError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<User>, AuthError>> + Send,
    {
        if !self.is_enabled() {
            return Ok(load().await?.map(Arc::new));
        }
        if let Some(hit) = self.entries.get(id).await {
            if hit.loaded.elapsed() < self.ttl {
                return Ok(Some(hit.user));
            }
            // Too old: forget it, so that the load below runs and does not just hand the same entry back.
            self.entries.invalidate(id).await;
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        let loaded = self
            .entries
            .try_get_with(id.clone(), async {
                match load().await {
                    Ok(Some(user)) => Ok(Held {
                        user: Arc::new(user),
                        loaded: Instant::now(),
                    }),
                    Ok(None) => Err(LoadFailure::Missing),
                    Err(error) => Err(LoadFailure::Failed(error)),
                }
            })
            .await;
        match loaded {
            Ok(held) => {
                // An eviction, a flush or a switch-off happened while the row was being read: the row may be older than the change,
                // so the request that read it uses it and it is not kept.
                if self.epoch.load(Ordering::Acquire) != epoch || !self.is_enabled() {
                    self.entries.invalidate(id).await;
                }
                Ok(Some(held.user))
            }
            Err(failure) => match *failure {
                LoadFailure::Missing => Ok(None),
                LoadFailure::Failed(error) => Err(error),
            },
        }
    }

    /// Forget `user`, on this replica, now.
    pub fn evict_user(&self, user: &UserId) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let user = user.clone();
        // The predicate is applied lazily, but an entry it matches is never returned once this call has returned.
        if self
            .entries
            .invalidate_entries_if(move |key, _| *key == user)
            .is_err()
        {
            self.entries.invalidate_all();
        }
    }

    /// Forget every user.
    pub fn flush(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.entries.invalidate_all();
    }

    /// Switch the cache off (every call loads) or back on. Turning it off also empties it.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Release);
        if !on {
            self.flush();
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// How many users are held now (after pending maintenance), for tests and the gauge.
    pub async fn held(&self) -> u64 {
        self.entries.run_pending_tasks().await;
        self.entries.entry_count()
    }
}

type Events = BoxStream<'static, DomainEvent>;

fn subscribe(bus: &dyn EventBus) -> Events {
    bus.subscribe(EventFilter {
        swimlanes: None,
        // `Resync` passes every filter, so a lagging listener still hears that it lagged.
        kinds: [EventKind::UserSessionsRevoked].into_iter().collect(),
    })
}

/// Keep `cache` in step with the bus until `closing` turns true. Subscribes now, before the task starts, so that an event
/// committed right after this returns is not missed.
pub fn spawn_listener(
    bus: Arc<dyn EventBus>,
    cache: Arc<UserCache>,
    closing: watch::Receiver<bool>,
) -> JoinHandle<()> {
    let first = subscribe(bus.as_ref());
    tokio::spawn(run(bus, cache, first, closing))
}

async fn run(
    bus: Arc<dyn EventBus>,
    cache: Arc<UserCache>,
    first: Events,
    mut closing: watch::Receiver<bool>,
) {
    let mut events = Some(first);
    let mut wait = RETRY_FIRST;
    loop {
        // The first stream was subscribed before the cache was shared, so every change since is queued in it and nothing is lost:
        // the cache is on and holds only what it may. A later stream follows a gap in which changes were missed, so what was held
        // from before it is dropped, and only after the new subscription exists, so that nothing slips between.
        let mut stream = if let Some(stream) = events.take() {
            stream
        } else {
            let pause = Duration::from_secs_f64(wait.as_secs_f64() * fastrand::f64());
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                _ = closing.wait_for(|stopping| *stopping) => return,
            }
            wait = (wait * 2).min(RETRY_LONGEST);
            let stream = subscribe(bus.as_ref());
            cache.flush();
            cache.set_enabled(true);
            stream
        };
        loop {
            let event = tokio::select! {
                event = stream.next() => event,
                _ = closing.wait_for(|stopping| *stopping) => return,
            };
            let Some(event) = event else { break };
            wait = RETRY_FIRST;
            match event {
                DomainEvent::UserSessionsRevoked { user } => cache.evict_user(&user),
                DomainEvent::Resync => cache.flush(),
                _ => {}
            }
        }
        // The bus is gone or shut down: nothing tells this replica about revocations now.
        cache.set_enabled(false);
        tracing::warn!("session revocation stream ended; the user cache is off until it is back");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;
    use domain::{Guid, Role, ShortText, UserStatus};
    use ports::{BusError, Tx};
    use tokio::sync::{mpsc, oneshot};

    use super::*;

    fn id(n: u32) -> UserId {
        UserId::new(
            Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        )
    }

    fn user(n: u32) -> User {
        User {
            id: id(n),
            email: ShortText::parse(&format!("user{n}@example.test")).unwrap(),
            display_name: ShortText::parse(&format!("User {n}")).unwrap(),
            role: Some(Role::Viewer),
            status: UserStatus::Active,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        }
    }

    fn cache() -> UserCache {
        UserCache::new(100, Duration::from_secs(30))
    }

    /// Loads user `n` and counts the loads.
    async fn load(cache: &UserCache, n: u32, loads: &AtomicUsize) -> Option<Arc<User>> {
        cache
            .get_or_load(&id(n), || async {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(Some(user(n)))
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_loaded_user_is_served_from_memory_until_it_is_evicted() {
        let (cache, loads) = (cache(), AtomicUsize::new(0));
        for _ in 0..3 {
            assert_eq!(load(&cache, 1, &loads).await.unwrap().id, id(1));
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1, "one load for three requests");
        cache.evict_user(&id(1));
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            2,
            "an eviction is a reload, at once"
        );
        // And only that user's.
        load(&cache, 2, &loads).await.unwrap();
        cache.evict_user(&id(1));
        load(&cache, 2, &loads).await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_user_that_does_not_exist_is_not_cached_and_a_failure_is_passed_on() {
        let cache = cache();
        let asked = AtomicUsize::new(0);
        for _ in 0..2 {
            let found = cache
                .get_or_load(&id(7), || async {
                    asked.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                })
                .await
                .unwrap();
            assert!(found.is_none());
        }
        assert_eq!(asked.load(Ordering::SeqCst), 2, "the next request asks again");
        let failed = cache
            .get_or_load(&id(7), || async { Err(AuthError::Unavailable) })
            .await;
        assert_eq!(failed.unwrap_err(), AuthError::Unavailable);
        assert_eq!(cache.held().await, 0);
    }

    #[tokio::test]
    async fn a_load_that_began_before_an_eviction_is_used_once_and_not_kept() {
        let (cache, loads) = (Arc::new(cache()), Arc::new(AtomicUsize::new(0)));
        let (release, released) = oneshot::channel::<()>();
        let slow = {
            let (cache, loads) = (Arc::clone(&cache), Arc::clone(&loads));
            tokio::spawn(async move {
                cache
                    .get_or_load(&id(1), || async {
                        loads.fetch_add(1, Ordering::SeqCst);
                        let _ = released.await;
                        Ok(Some(user(1)))
                    })
                    .await
            })
        };
        // The slow load is in flight; a role change evicts the user; then the old row arrives.
        while loads.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        cache.evict_user(&id(1));
        release.send(()).unwrap();
        assert!(
            slow.await.unwrap().unwrap().is_some(),
            "the request that read it still gets its answer"
        );
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            2,
            "the row read before the change was not kept"
        );
    }

    #[tokio::test]
    async fn concurrent_requests_for_one_user_share_one_load() {
        let (cache, loads) = (Arc::new(cache()), Arc::new(AtomicUsize::new(0)));
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let (cache, loads) = (Arc::clone(&cache), Arc::clone(&loads));
            tasks.push(tokio::spawn(async move {
                cache
                    .get_or_load(&id(1), || async {
                        loads.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        Ok(Some(user(1)))
                    })
                    .await
            }));
        }
        for task in tasks {
            assert!(task.await.unwrap().unwrap().is_some());
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_cache_that_is_off_loads_every_time_and_keeps_nothing() {
        let (cache, loads) = (cache(), AtomicUsize::new(0));
        load(&cache, 1, &loads).await.unwrap();
        cache.set_enabled(false);
        assert_eq!(cache.held().await, 0, "switching off forgets what was held");
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 3);
        assert_eq!(cache.held().await, 0);
        cache.set_enabled(true);
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn the_cache_is_bounded() {
        let cache = UserCache::new(10, Duration::from_secs(30));
        let loads = AtomicUsize::new(0);
        for n in 0..200 {
            load(&cache, n, &loads).await.unwrap();
        }
        assert!(cache.held().await <= 10, "{}", cache.held().await);
    }

    #[tokio::test(start_paused = true)]
    async fn a_user_is_kept_for_the_time_to_live_and_no_longer() {
        let (cache, loads) = (cache(), AtomicUsize::new(0));
        load(&cache, 1, &loads).await.unwrap();
        tokio::time::sleep(Duration::from_secs(29)).await;
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_secs(2)).await;
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    /// A bus whose streams the test controls: each `feed()` is the stream of one `subscribe`.
    #[derive(Default)]
    struct ScriptedBus {
        feeds: Mutex<Vec<mpsc::Receiver<DomainEvent>>>,
        subscriptions: AtomicUsize,
    }

    impl ScriptedBus {
        fn feed(&self) -> mpsc::Sender<DomainEvent> {
            let (tx, rx) = mpsc::channel(16);
            self.feeds.lock().unwrap().push(rx);
            tx
        }
    }

    #[async_trait]
    impl EventBus for ScriptedBus {
        async fn publish(&self, _e: DomainEvent) -> Result<(), BusError> {
            Err(BusError::Unavailable)
        }

        async fn publish_in_tx(&self, _tx: &mut Tx<'_>, _e: DomainEvent) -> Result<(), BusError> {
            Err(BusError::Unavailable)
        }

        fn subscribe(&self, _f: EventFilter) -> Events {
            self.subscriptions.fetch_add(1, Ordering::SeqCst);
            let mut feeds = self.feeds.lock().unwrap();
            if feeds.is_empty() {
                return futures::stream::empty().boxed();
            }
            let rx = feeds.remove(0);
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) }).boxed()
        }
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_revocation_event_evicts_that_user_on_this_replica() {
        let bus = Arc::new(ScriptedBus::default());
        let feed = bus.feed();
        let (cache, loads) = (Arc::new(cache()), AtomicUsize::new(0));
        let (_stop, closing) = watch::channel(false);
        let _listener = spawn_listener(bus.clone(), Arc::clone(&cache), closing);
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 2, &loads).await.unwrap();
        feed.send(DomainEvent::UserSessionsRevoked { user: id(1) })
            .await
            .unwrap();
        settle().await;
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 2, &loads).await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            3,
            "user 1 was reloaded, user 2 was not"
        );
    }

    #[tokio::test]
    async fn a_resync_flushes_everyone() {
        let bus = Arc::new(ScriptedBus::default());
        let feed = bus.feed();
        let (cache, loads) = (Arc::new(cache()), AtomicUsize::new(0));
        let (_stop, closing) = watch::channel(false);
        let _listener = spawn_listener(bus.clone(), Arc::clone(&cache), closing);
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 2, &loads).await.unwrap();
        feed.send(DomainEvent::Resync).await.unwrap();
        settle().await;
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 2, &loads).await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn when_the_stream_ends_the_cache_is_off_until_the_listener_has_subscribed_again() {
        let bus = Arc::new(ScriptedBus::default());
        let first = bus.feed();
        let (cache, loads) = (Arc::new(cache()), AtomicUsize::new(0));
        let (_stop, closing) = watch::channel(false);
        let _listener = spawn_listener(bus.clone(), Arc::clone(&cache), closing);
        settle().await;
        assert!(cache.is_enabled());
        load(&cache, 1, &loads).await.unwrap();
        // The bus drops the stream; nothing now says when a role changes.
        drop(first);
        settle().await;
        assert!(
            !cache.is_enabled(),
            "nothing tells this replica about revocations"
        );
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            3,
            "every request reads the database"
        );
        // The second subscription is a stream that stays open: the cache comes back on, empty.
        let _second = bus.feed();
        tokio::time::sleep(Duration::from_secs(10)).await;
        settle().await;
        assert!(cache.is_enabled());
        assert_eq!(bus.subscriptions.load(Ordering::SeqCst), 2);
        load(&cache, 1, &loads).await.unwrap();
        load(&cache, 1, &loads).await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn the_listener_stops_when_the_hub_closes() {
        let bus = Arc::new(ScriptedBus::default());
        let _feed = bus.feed();
        let (stop, closing) = watch::channel(false);
        let listener = spawn_listener(bus, Arc::new(cache()), closing);
        settle().await;
        assert!(!listener.is_finished());
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), listener)
            .await
            .expect("the listener stops on the closing signal")
            .unwrap();
    }
}
