//! The agent streams held by this replica (D62).
//!
//! A [`LocalStream`] is one open `Connect` stream as the gateway uses it: the bounded way to write to the agent
//! ([`ConnectedAgent`]), the table of requests waiting for its answers ([`Pending`]) and the epoch of the
//! `agent_connections` row that registered it. [`LocalStreams`] is the replica's table of them, one per swimlane.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use domain::{AgentReply, HubCommand, OpResult, RequestId, SwimlaneId};
use ports::GatewayError;
use proto::convert::ToAgent;
use tokio::time::Instant;

use super::correlation::{Accepts, Delivery, Pending, RegisterError};
use crate::stream::{ConnectedAgent, MAX_IN_FLIGHT, SendError};

/// The generation of an agent's registration: it grows by one every time the agent connects, anywhere (D62). The highest
/// epoch is the live stream; a stream whose epoch is lower has been replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(i64);

impl Epoch {
    #[must_use]
    pub fn new(epoch: i64) -> Self {
        Self(epoch)
    }

    #[must_use]
    pub fn get(self) -> i64 {
        self.0
    }

    /// The epoch as it travels between replicas. Epochs start at 1, so a negative one (which the database would never hold)
    /// becomes 0, which no stream has.
    #[must_use]
    pub fn to_wire(self) -> u64 {
        u64::try_from(self.0).unwrap_or(0)
    }

    /// `None` for a value that is not an epoch this database could hold.
    #[must_use]
    pub fn from_wire(epoch: u64) -> Option<Self> {
        i64::try_from(epoch).ok().map(Self)
    }
}

/// The id a command that has no request id is answered with: the agent replies to those with scan deltas, not with a message.
const ACK_ID: &str = "ack";

/// One open agent stream.
#[derive(Debug)]
pub struct LocalStream {
    agent: Arc<ConnectedAgent>,
    epoch: Epoch,
    pending: Arc<Pending>,
}

impl LocalStream {
    #[must_use]
    pub fn new(agent: Arc<ConnectedAgent>, epoch: Epoch) -> Arc<Self> {
        Arc::new(Self {
            agent,
            epoch,
            pending: Pending::new(MAX_IN_FLIGHT),
        })
    }

    #[must_use]
    pub fn agent(&self) -> &Arc<ConnectedAgent> {
        &self.agent
    }

    #[must_use]
    pub fn swimlane(&self) -> &SwimlaneId {
        self.agent.swimlane()
    }

    #[must_use]
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// Requests waiting for the agent.
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// Write `command` to the agent and wait for its answer, at most `timeout`.
    ///
    /// A command with a request id takes an in-flight slot (16 per agent, [`GatewayError::Busy`] above that) and waits for
    /// the reply that carries the same id. `RequestDelta` and `RequestFullScan` have no id and no reply message (the agent
    /// answers with scan deltas, which go to the ingestion sink): they are written and acknowledged at once.
    pub async fn request(&self, command: HubCommand, timeout: Duration) -> Result<AgentReply, GatewayError> {
        if timeout.is_zero() {
            return Err(GatewayError::Timeout);
        }
        if self.agent.is_closed() {
            return Err(GatewayError::Closed);
        }
        let deadline = Instant::now() + timeout;
        let Some((id, accepts)) = Accepts::for_command(&command) else {
            return self.write_unanswered(command);
        };
        let _slot = self.agent.reserve_in_flight().map_err(|_| GatewayError::Busy)?;
        let ticket = self.pending.register(id, accepts).map_err(|error| match error {
            RegisterError::Closed => GatewayError::Closed,
            // A second request on the same id could not be told apart from the first one's answer.
            RegisterError::Duplicate | RegisterError::Full => GatewayError::Busy,
        })?;
        self.agent
            .send(
                ToAgent::Command(command),
                deadline.saturating_duration_since(Instant::now()),
            )
            .await
            .map_err(|error| match error {
                SendError::Full => GatewayError::Busy,
                SendError::Closed => GatewayError::Closed,
            })?;
        ticket
            .wait(deadline.saturating_duration_since(Instant::now()))
            .await
    }

    fn write_unanswered(&self, command: HubCommand) -> Result<AgentReply, GatewayError> {
        self.agent
            .try_send(ToAgent::Command(command))
            .map_err(|error| match error {
                SendError::Full => GatewayError::Busy,
                SendError::Closed => GatewayError::Closed,
            })?;
        let request_id = RequestId::parse(ACK_ID).map_err(|_| GatewayError::Unavailable)?;
        Ok(AgentReply::Op(OpResult {
            request_id,
            ok: true,
            error: None,
            current_hash: None,
        }))
    }

    /// Give the agent's reply to the request waiting for it. False when nothing was waiting (it is dropped).
    pub fn complete(&self, reply: AgentReply) -> bool {
        self.pending.complete(reply) == Delivery::Delivered
    }

    /// End the stream: every request in flight fails with [`GatewayError::Closed`], later ones fail at once, and the stream's
    /// handler is told to close and to tell the agent to reconnect.
    pub fn close(&self) {
        self.pending.close();
        self.agent.close();
    }
}

/// The table is full: a replica holds at most the streams it was sized for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("too many agent streams on this replica")]
pub struct TableFull;

/// This replica's agent streams, one per swimlane.
#[derive(Debug)]
pub struct LocalStreams {
    capacity: usize,
    streams: Mutex<HashMap<SwimlaneId, Arc<LocalStream>>>,
}

impl LocalStreams {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            streams: Mutex::new(HashMap::new()),
        }
    }

    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<SwimlaneId, Arc<LocalStream>>> {
        self.streams.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hold `stream` as its swimlane's stream and return the one it replaces, which the caller closes. A swimlane that already
    /// has a stream needs no room; a new one is refused when the table is full.
    pub fn insert(&self, stream: Arc<LocalStream>) -> Result<Option<Arc<LocalStream>>, TableFull> {
        let mut table = self.table();
        if !table.contains_key(stream.swimlane()) && table.len() >= self.capacity {
            return Err(TableFull);
        }
        Ok(table.insert(stream.swimlane().clone(), stream))
    }

    #[must_use]
    pub fn get(&self, swimlane: &SwimlaneId) -> Option<Arc<LocalStream>> {
        self.table().get(swimlane).cloned()
    }

    /// Stop holding `stream`, unless it has been replaced already: a handler that ends late must not remove its successor.
    pub fn remove_if(&self, stream: &Arc<LocalStream>) -> bool {
        let mut table = self.table();
        match table.get(stream.swimlane()) {
            Some(held) if Arc::ptr_eq(held, stream) => {
                table.remove(stream.swimlane());
                true
            }
            _ => false,
        }
    }

    /// Every swimlane held and the epoch it was registered with.
    #[must_use]
    pub fn snapshot(&self) -> Vec<(SwimlaneId, Epoch)> {
        self.table()
            .iter()
            .map(|(swimlane, stream)| (swimlane.clone(), stream.epoch()))
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.table().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
