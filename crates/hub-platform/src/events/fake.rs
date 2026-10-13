//! An in-memory [`EventSource`] for tests of the SSE edge (`fakes` feature). Not cryptography, not a database: it keeps a log
//! with ids, a broadcast channel for live events, and a switch for "the window has passed".

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use domain::DomainEvent;
use futures::stream::BoxStream;
use ports::{BusError, EventFilter};
use tokio::sync::{broadcast, watch};

use super::source::{EventSource, Relayed, Replay, relayed_stream};
use super::wire::EventId;

type Hook = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    last: i64,
    log: Vec<(EventId, DomainEvent)>,
    /// Ids up to and including this one are older than the replay window.
    expired_through: i64,
    before_replay: Option<Hook>,
}

pub struct FakeEventSource {
    state: Mutex<State>,
    sender: broadcast::Sender<Arc<Relayed>>,
    closed: watch::Sender<bool>,
}

impl std::fmt::Debug for FakeEventSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeEventSource").finish_non_exhaustive()
    }
}

impl FakeEventSource {
    /// `capacity` is how far a live subscriber may fall behind before it gets a `Resync`.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            sender: broadcast::channel(capacity.max(1)).0,
            closed: watch::channel(false).0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Store the event under the next id and send it to live subscribers.
    ///
    /// # Panics
    ///
    /// Never in practice: the id counter does not reach `i64::MAX`.
    #[allow(clippy::expect_used)]
    pub fn publish(&self, event: DomainEvent) -> EventId {
        let id = {
            let mut state = self.lock();
            state.last += 1;
            let id = EventId::new(state.last).expect("the counter starts at 1");
            state.log.push((id, event.clone()));
            id
        };
        let _ = self.sender.send(Arc::new(Relayed { id: Some(id), event }));
        id
    }

    /// Send an event to live subscribers without storing it, as the relay does for the `Resync` it makes up.
    pub fn publish_unstored(&self, event: DomainEvent) {
        let _ = self.sender.send(Arc::new(Relayed { id: None, event }));
    }

    /// Everything up to and including `id` is now older than the replay window.
    pub fn expire_through(&self, id: EventId) {
        let mut state = self.lock();
        state.expired_through = state.expired_through.max(id.get());
    }

    /// Run `hook` once, just before the next replay returns: events published in it are both in that replay and live, as
    /// when an event commits between the subscription and the replay's query.
    pub fn before_next_replay(&self, hook: impl FnOnce() + Send + 'static) {
        self.lock().before_replay = Some(Box::new(hook));
    }

    /// How many live subscribers there are.
    #[must_use]
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }

    /// End every live stream, as a bus that shut down does.
    pub fn close(&self) {
        self.closed.send_replace(true);
    }
}

#[async_trait]
impl EventSource for FakeEventSource {
    fn subscribe(&self, filter: EventFilter) -> BoxStream<'static, Arc<Relayed>> {
        relayed_stream(self.sender.subscribe(), filter, self.closed.subscribe(), || {})
    }

    async fn replay(&self, after: EventId, filter: &EventFilter, limit: usize) -> Result<Replay, BusError> {
        let hook = self.lock().before_replay.take();
        if let Some(hook) = hook {
            hook();
        }
        let state = self.lock();
        if after.get() > state.last || after.get() <= state.expired_through {
            return Ok(Replay::Resync);
        }
        let matching: Vec<(EventId, DomainEvent)> = state
            .log
            .iter()
            .filter(|(id, event)| *id > after && filter.matches(event))
            .cloned()
            .collect();
        if matching.len() > limit {
            return Ok(Replay::Resync);
        }
        Ok(Replay::Events(matching))
    }
}
