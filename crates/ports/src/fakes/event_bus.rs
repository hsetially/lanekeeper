use async_trait::async_trait;
use domain::DomainEvent;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use super::tx::Pending;
use crate::{BusError, EventBus, EventFilter, Tx};

pub(crate) struct BusInner {
    sender: broadcast::Sender<DomainEvent>,
}

impl BusInner {
    pub(crate) fn fan_out(&self, e: DomainEvent) {
        // No receivers is fine: nobody is subscribed.
        let _ = self.sender.send(e);
    }
}

/// An in-process bus over a bounded broadcast channel. A subscriber that falls more than `buffer` events
/// behind loses the oldest ones and sees [`DomainEvent::Resync`] first.
#[derive(Clone)]
pub struct FakeEventBus {
    inner: std::sync::Arc<BusInner>,
}

impl std::fmt::Debug for FakeEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeEventBus").finish_non_exhaustive()
    }
}

impl FakeEventBus {
    /// `buffer` is the number of events each subscriber may lag behind (at least 1).
    pub fn new(buffer: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer.max(1));
        Self {
            inner: std::sync::Arc::new(BusInner { sender }),
        }
    }
}

impl Default for FakeEventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[async_trait]
impl EventBus for FakeEventBus {
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError> {
        self.inner.fan_out(e);
        Ok(())
    }

    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        let state = tx.mem().map_err(|_| BusError::WrongTx)?;
        state.push(Pending::Publish(self.inner.clone(), Box::new(e)));
        Ok(())
    }

    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent> {
        // Subscribe now, not on first poll, so that events published before the first poll are not lost.
        let rx = self.inner.sender.subscribe();
        futures::stream::unfold((rx, f), |(mut rx, f)| async move {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        if f.matches(&e) {
                            return Some((e, (rx, f)));
                        }
                    }
                    Err(RecvError::Lagged(_)) => return Some((DomainEvent::Resync, (rx, f))),
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .boxed()
    }
}
