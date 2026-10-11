//! One agent's open `Connect` stream, as the rest of the hub sees it: a bounded way to send it commands.
//!
//! Everything about it is bounded (rule 5): the queue of messages waiting to be written to the agent holds 64, and no more
//! than 16 requests may be waiting for the agent's answer at once. A queue that stays full means the agent stopped reading;
//! the stream is closed rather than the hub's memory growing.

use std::sync::Arc;
use std::time::Duration;

use domain::SwimlaneId;
use proto::convert::ToAgent;
use proto::pb;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;

/// Messages waiting to be written to one agent.
pub const OUTBOUND_QUEUE: usize = 64;
/// Requests one agent may have waiting for an answer.
pub const MAX_IN_FLIGHT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// The queue is full (or stayed full for the whole wait): the agent is not keeping up.
    #[error("the agent is not reading")]
    Full,
    /// The stream has ended.
    #[error("the stream has ended")]
    Closed,
}

/// The agent already has [`MAX_IN_FLIGHT`] requests waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the agent has too many requests in flight")]
pub struct Busy;

/// A claim on one of the agent's in-flight slots, released when dropped.
#[derive(Debug)]
pub struct InFlight(#[allow(dead_code)] OwnedSemaphorePermit);

#[derive(Debug)]
pub struct ConnectedAgent {
    swimlane: SwimlaneId,
    outbound: mpsc::Sender<Result<pb::HubMessage, Status>>,
    in_flight: Arc<Semaphore>,
    closed: watch::Sender<bool>,
}

impl ConnectedAgent {
    /// A new stream for `swimlane`: the handle the hub keeps, and the stream of messages to write to the agent.
    #[must_use]
    pub fn new(swimlane: SwimlaneId) -> (Arc<Self>, ReceiverStream<Result<pb::HubMessage, Status>>) {
        let (outbound, queue) = mpsc::channel(OUTBOUND_QUEUE);
        let agent = Arc::new(Self {
            swimlane,
            outbound,
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
            closed: watch::channel(false).0,
        });
        (agent, ReceiverStream::new(queue))
    }

    #[must_use]
    pub fn swimlane(&self) -> &SwimlaneId {
        &self.swimlane
    }

    /// Queue `message` for the agent without waiting. [`SendError::Full`] when the queue has no room.
    pub fn try_send(&self, message: ToAgent) -> Result<(), SendError> {
        self.outbound
            .try_send(Ok(message.into_proto()))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => SendError::Full,
                mpsc::error::TrySendError::Closed(_) => SendError::Closed,
            })
    }

    /// Queue `message`, waiting up to `wait` for room.
    pub async fn send(&self, message: ToAgent, wait: Duration) -> Result<(), SendError> {
        match tokio::time::timeout(wait, self.outbound.send(Ok(message.into_proto()))).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(SendError::Closed),
            Err(_elapsed) => Err(SendError::Full),
        }
    }

    /// Claim an in-flight slot for a request that will wait for the agent's answer.
    pub fn reserve_in_flight(&self) -> Result<InFlight, Busy> {
        Arc::clone(&self.in_flight)
            .try_acquire_owned()
            .map(InFlight)
            .map_err(|_| Busy)
    }

    /// How many in-flight slots are free.
    #[must_use]
    pub fn free_in_flight(&self) -> usize {
        self.in_flight.available_permits()
    }

    /// Ask the stream to end. The handler notices, tells the agent to reconnect, and stops.
    pub fn close(&self) {
        self.closed.send_replace(true);
    }

    /// A receiver that sees `true` once [`ConnectedAgent::close`] was called.
    #[must_use]
    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.subscribe()
    }

    /// Whether the stream has ended on the writing side.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.outbound.is_closed() || *self.closed.borrow()
    }

    /// Put a final status on the stream, if there is room for it, and say nothing if there is not.
    pub(crate) fn finish(&self, status: Status) {
        let _ = self.outbound.try_send(Err(status));
    }
}
