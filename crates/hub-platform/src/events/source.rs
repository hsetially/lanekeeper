//! Where the SSE endpoint gets its events: live ones with their ids, and a replay of the recent past.
//!
//! [`PgEventBus`](super::bus::PgEventBus) is the real source. The trait exists so that the SSE edge (filters, visibility,
//! heartbeats, limits, replay) is tested without a database, and so that the endpoint never sees how events arrive.

use std::sync::Arc;

use async_trait::async_trait;
use domain::DomainEvent;
use futures::StreamExt;
use futures::stream::BoxStream;
use ports::{BusError, EventFilter};
use tokio::sync::{broadcast, watch};

use super::wire::EventId;

/// One event as relayed: its id in the outbox, and what happened. `id` is `None` for an event the relay made up itself
/// ([`DomainEvent::Resync`] after a lag or a reconnect): it was never stored, so a client cannot resume from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relayed {
    pub id: Option<EventId>,
    pub event: DomainEvent,
}

/// What a replay found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replay {
    /// Every matching event after the id, in id order.
    Events(Vec<(EventId, DomainEvent)>),
    /// The id is older than the replay window, or unknown, or more events matched than one replay may carry: the client
    /// cannot be brought up to date from the log and must refetch.
    Resync,
}

#[async_trait]
pub trait EventSource: Send + Sync + 'static {
    /// Live events that match `filter`, starting from now. A subscriber that falls behind gets a [`DomainEvent::Resync`] in place
    /// of what it missed. The subscription exists when this returns, before the first poll of the stream.
    fn subscribe(&self, filter: EventFilter) -> BoxStream<'static, Arc<Relayed>>;

    /// The events after `after` that match `filter`, at most `limit`, if the log still holds them all.
    async fn replay(&self, after: EventId, filter: &EventFilter, limit: usize) -> Result<Replay, BusError>;
}

/// Resolves when `closed` turns true or its sender is gone. The guard that `wait_for` returns is dropped here, so the
/// future that calls this stays `Send`.
pub(crate) async fn wait_closed(closed: &mut watch::Receiver<bool>) {
    let _ = closed.wait_for(|c| *c).await.is_ok();
}

enum Step {
    Item(Arc<Relayed>),
    Lagged,
    Skip,
    End,
}

/// A stream over a broadcast receiver: filtered, `Resync` for a lag (`on_lag` runs once per lag, for the metric), and ended when
/// the channel closes or `closed` turns true.
pub(crate) fn relayed_stream(
    rx: broadcast::Receiver<Arc<Relayed>>,
    filter: EventFilter,
    closed: watch::Receiver<bool>,
    on_lag: impl Fn() + Send + 'static,
) -> BoxStream<'static, Arc<Relayed>> {
    futures::stream::unfold(
        (rx, filter, closed, on_lag),
        |(mut rx, filter, mut closed, on_lag)| async move {
            loop {
                let step = tokio::select! {
                    received = rx.recv() => match received {
                        Ok(item) if filter.matches(&item.event) => Step::Item(item),
                        Ok(_) => Step::Skip,
                        Err(broadcast::error::RecvError::Lagged(_)) => Step::Lagged,
                        Err(broadcast::error::RecvError::Closed) => Step::End,
                    },
                    () = wait_closed(&mut closed) => Step::End,
                };
                match step {
                    Step::Item(item) => return Some((item, (rx, filter, closed, on_lag))),
                    Step::Lagged => {
                        on_lag();
                        let resync = Arc::new(Relayed {
                            id: None,
                            event: DomainEvent::Resync,
                        });
                        return Some((resync, (rx, filter, closed, on_lag)));
                    }
                    Step::Skip => {}
                    Step::End => return None,
                }
            }
        },
    )
    .boxed()
}
