//! A role or status change ends the user's sessions on every replica (T3; S2).
//!
//! The change deletes the user's `sessions` rows and publishes `UserSessionsRevoked` through `EventBus::publish_in_tx`, in the
//! same transaction as the change and its audit event (D80): nothing is fanned out for a change that rolls back. The rows are
//! gone for every replica at once; what each replica may still hold is a cached copy, for at most 30 seconds. The
//! [`RevocationListener`] closes that gap: each replica runs one, subscribed to the event, and it evicts the user's cached
//! sessions when the event arrives (within a second of the commit).
//!
//! The listener fails closed:
//!
//! - `Resync` means it lagged and may have missed events: the whole cache is flushed.
//! - If the event stream ends, the cache is switched off, so every request reads the database, until the listener has
//!   subscribed again. It subscribes before it flushes and switches the cache back on, so nothing slips between.

use std::sync::Arc;
use std::time::Duration;

use domain::{DomainEvent, UserId};
use futures::StreamExt;
use ports::{BusError, EventBus, EventFilter, EventKind, Tx};
use tokio::sync::watch;

use super::AbortOnDrop;
use super::cache::SessionCache;

/// The first and the longest wait before subscribing again after the stream ended. Full jitter between zero and the wait.
const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_LONGEST: Duration = Duration::from_secs(5);

/// Publish the revocation of every session of `user`, in the caller's transaction. Call it with the deletion of the rows.
pub async fn publish_revoked(bus: &dyn EventBus, tx: &mut Tx<'_>, user: &UserId) -> Result<(), BusError> {
    bus.publish_in_tx(tx, DomainEvent::UserSessionsRevoked { user: user.clone() })
        .await
}

/// What the listener has done so far. Counters only; tests wait on them instead of sleeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ListenerState {
    /// Times it subscribed (the first one included).
    pub subscriptions: u64,
    /// Revocation events applied.
    pub events: u64,
    /// Times the whole cache was flushed because of `Resync`.
    pub resyncs: u64,
    /// Times the event stream ended.
    pub stream_ends: u64,
}

pub struct RevocationListener {
    state: watch::Receiver<ListenerState>,
    _task: AbortOnDrop,
}

impl std::fmt::Debug for RevocationListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevocationListener")
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl RevocationListener {
    /// Subscribe now and keep `cache` in step with the bus until the listener is dropped. Needs a Tokio runtime.
    pub fn spawn(bus: Arc<dyn EventBus>, cache: Arc<SessionCache>) -> Self {
        let (tx, state) = watch::channel(ListenerState::default());
        // Subscribe before the task starts, so an event committed right after `spawn` returns is not missed.
        let first = subscribe(bus.as_ref());
        let task = AbortOnDrop::spawn(run(bus, cache, first, tx));
        Self { state, _task: task }
    }

    pub fn state(&self) -> ListenerState {
        *self.state.borrow()
    }

    /// Wait until the state satisfies `done`. Callers bound the wait with a timeout.
    pub async fn wait_for(&self, mut done: impl FnMut(&ListenerState) -> bool) -> ListenerState {
        let mut rx = self.state.clone();
        loop {
            let current = *rx.borrow_and_update();
            if done(&current) {
                return current;
            }
            if rx.changed().await.is_err() {
                // The task is gone; the last state is all there will be.
                return *rx.borrow();
            }
        }
    }
}

type Events = futures::stream::BoxStream<'static, DomainEvent>;

fn subscribe(bus: &dyn EventBus) -> Events {
    bus.subscribe(EventFilter {
        swimlanes: None,
        // `Resync` passes every filter, so a lagging listener still hears that it lagged.
        kinds: [EventKind::UserSessionsRevoked].into_iter().collect(),
    })
}

async fn run(
    bus: Arc<dyn EventBus>,
    cache: Arc<SessionCache>,
    first: Events,
    state: watch::Sender<ListenerState>,
) {
    let mut events = Some(first);
    let mut wait = RETRY_FIRST;
    loop {
        let mut stream = if let Some(s) = events.take() {
            s
        } else {
            // Subscribe first; only then flush and switch the cache back on.
            tokio::time::sleep(Duration::from_secs_f64(wait.as_secs_f64() * fastrand::f64())).await;
            wait = (wait * 2).min(RETRY_LONGEST);
            subscribe(bus.as_ref())
        };
        cache.flush();
        cache.set_enabled(true);
        state.send_modify(|s| s.subscriptions += 1);
        while let Some(event) = stream.next().await {
            wait = RETRY_FIRST;
            match event {
                DomainEvent::UserSessionsRevoked { user } => {
                    cache.evict_user(&user);
                    state.send_modify(|s| s.events += 1);
                }
                DomainEvent::Resync => {
                    cache.flush();
                    state.send_modify(|s| s.resyncs += 1);
                }
                _ => {}
            }
        }
        // The bus is gone or shut down: nothing tells this replica about revocations now.
        cache.set_enabled(false);
        tracing::warn!("session revocation stream ended; the session cache is off until it is back");
        state.send_modify(|s| s.stream_ends += 1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use domain::{Guid, Role, Timestamp, UserStatus};
    use tokio::sync::mpsc;

    use super::*;
    use crate::clock::{Clock, ManualClock};
    use crate::session::cache::CachedSession;
    use crate::session::cookie::TokenHash;

    const TID: &str = "72f988bf-86f1-41af-91ab-2d7cd011db47";

    fn user(n: u32) -> UserId {
        UserId::new(
            Guid::parse(TID).unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        )
    }

    /// A bus whose streams the test controls: it hands out the receiver of the next `feed()` call.
    #[derive(Default)]
    struct ScriptedBus {
        feeds: Mutex<Vec<mpsc::Receiver<DomainEvent>>>,
        filters: Mutex<Vec<EventFilter>>,
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

        fn subscribe(&self, f: EventFilter) -> Events {
            self.filters.lock().unwrap().push(f);
            let mut feeds = self.feeds.lock().unwrap();
            if feeds.is_empty() {
                return futures::stream::empty().boxed();
            }
            let rx = feeds.remove(0);
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) }).boxed()
        }
    }

    fn cache_with_clock() -> (Arc<ManualClock>, Arc<SessionCache>) {
        let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(1_791_676_800_000)));
        let cache = Arc::new(SessionCache::new(
            100,
            Duration::from_secs(30),
            Arc::clone(&clock) as Arc<dyn Clock>,
        ));
        (clock, cache)
    }

    async fn warm(cache: &SessionCache, clock: &ManualClock, key: u8, who: u32) {
        cache
            .get_or_load(TokenHash::from_bytes([key; 32]), || {
                let now = clock.now();
                async move {
                    Ok(Some(CachedSession {
                        user: user(who),
                        role: Some(Role::Viewer),
                        status: UserStatus::Active,
                        csrf_hash: TokenHash::from_bytes([0; 32]),
                        last_seen_at: now,
                        expires_at: Timestamp::from_unix_millis(now.unix_millis() + 3_600_000),
                        loaded_at: now,
                    }))
                }
            })
            .await
            .unwrap();
    }

    /// Whether `key` is served from the cache (true) or would be loaded again (false).
    async fn cached(cache: &SessionCache, key: u8) -> bool {
        let mut loaded = false;
        cache
            .get_or_load(TokenHash::from_bytes([key; 32]), || {
                loaded = true;
                std::future::ready(Ok(None))
            })
            .await
            .unwrap();
        !loaded
    }

    async fn until(l: &RevocationListener, f: impl FnMut(&ListenerState) -> bool) -> ListenerState {
        tokio::time::timeout(Duration::from_secs(5), l.wait_for(f))
            .await
            .expect("the listener reached the state in time")
    }

    #[tokio::test]
    async fn a_revocation_event_evicts_that_user_and_only_that_user() {
        let (clock, cache) = cache_with_clock();
        let bus = Arc::new(ScriptedBus::default());
        let feed = bus.feed();
        let listener = RevocationListener::spawn(Arc::clone(&bus) as Arc<dyn EventBus>, Arc::clone(&cache));
        // Its first subscription flushes the cache; wait for that before warming it.
        until(&listener, |s| s.subscriptions == 1).await;
        warm(&cache, &clock, 1, 1).await;
        warm(&cache, &clock, 2, 2).await;
        feed.send(DomainEvent::UserSessionsRevoked { user: user(1) })
            .await
            .unwrap();
        until(&listener, |s| s.events == 1).await;
        assert!(!cached(&cache, 1).await, "user 1's session is evicted");
        assert!(cached(&cache, 2).await, "user 2's stays");
        // It asked the bus for revocations only.
        let filters = bus.filters.lock().unwrap();
        assert_eq!(
            filters[0].kinds,
            [EventKind::UserSessionsRevoked].into_iter().collect()
        );
    }

    #[tokio::test]
    async fn resync_flushes_cache() {
        let (clock, cache) = cache_with_clock();
        let bus = Arc::new(ScriptedBus::default());
        let feed = bus.feed();
        let listener = RevocationListener::spawn(Arc::clone(&bus) as Arc<dyn EventBus>, Arc::clone(&cache));
        // Its first subscription flushes the cache; wait for that before warming it.
        until(&listener, |s| s.subscriptions == 1).await;
        warm(&cache, &clock, 1, 1).await;
        warm(&cache, &clock, 2, 2).await;
        feed.send(DomainEvent::Resync).await.unwrap();
        let state = until(&listener, |s| s.resyncs == 1).await;
        assert_eq!(state.events, 0);
        assert!(
            !cached(&cache, 1).await && !cached(&cache, 2).await,
            "after a lag nothing can be trusted"
        );
    }

    #[tokio::test]
    async fn other_events_change_nothing() {
        let (clock, cache) = cache_with_clock();
        let bus = Arc::new(ScriptedBus::default());
        let feed = bus.feed();
        let listener = RevocationListener::spawn(Arc::clone(&bus) as Arc<dyn EventBus>, Arc::clone(&cache));
        // Its first subscription flushes the cache; wait for that before warming it.
        until(&listener, |s| s.subscriptions == 1).await;
        warm(&cache, &clock, 1, 1).await;
        feed.send(DomainEvent::AccessRequestCreated { user: user(1) })
            .await
            .unwrap();
        feed.send(DomainEvent::UserSessionsRevoked { user: user(9) })
            .await
            .unwrap();
        until(&listener, |s| s.events == 1).await;
        assert!(cached(&cache, 1).await);
    }

    #[tokio::test(start_paused = true)]
    async fn event_stream_end_disables_cache() {
        let (clock, cache) = cache_with_clock();
        let bus = Arc::new(ScriptedBus::default());
        let first = bus.feed();
        let listener = RevocationListener::spawn(Arc::clone(&bus) as Arc<dyn EventBus>, Arc::clone(&cache));
        until(&listener, |s| s.subscriptions == 1).await;
        warm(&cache, &clock, 1, 1).await;
        assert!(cache.is_enabled() && cached(&cache, 1).await);

        // The bus shuts down: the stream ends. Nothing can tell this replica about revocations, so it stops caching.
        drop(first);
        until(&listener, |s| s.stream_ends == 1).await;
        assert!(!cache.is_enabled(), "no event stream, no cache");
        assert!(!cached(&cache, 1).await, "the old entry is gone");
        let mut loads = 0;
        for _ in 0..3 {
            cache
                .get_or_load(TokenHash::from_bytes([1; 32]), || {
                    loads += 1;
                    std::future::ready(Ok(None))
                })
                .await
                .unwrap();
        }
        assert_eq!(
            loads, 3,
            "every request reads the database while the stream is down"
        );

        // The bus is back (the paused clock lets the retry wait pass): the listener subscribes again, flushes, and turns
        // the cache on.
        let second = bus.feed();
        until(&listener, |s| s.subscriptions == 2).await;
        assert!(cache.is_enabled());
        warm(&cache, &clock, 1, 1).await;
        assert!(cached(&cache, 1).await);
        second
            .send(DomainEvent::UserSessionsRevoked { user: user(1) })
            .await
            .unwrap();
        until(&listener, |s| s.events == 1).await;
        assert!(!cached(&cache, 1).await, "and it listens again");
    }

    #[tokio::test]
    async fn a_listener_that_is_dropped_stops_listening() {
        let (clock, cache) = cache_with_clock();
        let bus = Arc::new(ScriptedBus::default());
        let feed = bus.feed();
        let listener = RevocationListener::spawn(Arc::clone(&bus) as Arc<dyn EventBus>, Arc::clone(&cache));
        until(&listener, |s| s.subscriptions == 1).await;
        drop(listener);
        warm(&cache, &clock, 1, 1).await;
        // The task is aborted, so its receiver is dropped and the feed is closed.
        tokio::task::yield_now().await;
        for _ in 0..50 {
            if feed.is_closed() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(feed.is_closed());
    }
}
