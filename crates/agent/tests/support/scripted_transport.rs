//! A hub transport that is not gRPC: the test holds both ends of each connection.
//!
//! It shows that the session runs over anything that implements `HubTransport` (the WebSocket story), and it lets a test
//! stall the hub by not reading, which the real network cannot do on demand.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt;
use std::sync::{Arc, Mutex};

use agent::identity::ClientIdentity;
use agent::transport::{Connection, HubTransport, TransportError};
use async_trait::async_trait;
use proto::convert::ToAgent;
use proto::pb;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;

/// The hub's end of one connection.
pub struct HubEnd {
    /// What the agent sent. Not reading it stalls the agent's writer, as a hub that has stopped reading would.
    pub from_agent: mpsc::Receiver<pb::AgentMessage>,
    to_agent: mpsc::Sender<Result<pb::HubMessage, TransportError>>,
}

impl HubEnd {
    pub async fn send(&self, message: ToAgent) {
        self.to_agent.send(Ok(message.into_proto())).await.unwrap();
    }

    /// Break the stream with an error.
    pub async fn fail(&self) {
        self.to_agent.send(Err(TransportError::Stream)).await.unwrap();
    }
}

#[derive(Default)]
pub struct ScriptedTransport {
    ends: Mutex<Vec<Option<HubEnd>>>,
    opened: watch::Sender<usize>,
}

impl fmt::Debug for ScriptedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScriptedTransport")
    }
}

impl ScriptedTransport {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Wait for the `n`-th connection (from 1) and take the hub's end of it.
    pub async fn accept(&self, n: usize) -> HubEnd {
        let mut opened = self.opened.subscribe();
        opened.wait_for(|count| *count >= n).await.unwrap();
        self.ends.lock().unwrap()[n - 1].take().expect("already taken")
    }

    pub fn opened(&self) -> usize {
        *self.opened.borrow()
    }
}

#[async_trait]
impl HubTransport for ScriptedTransport {
    async fn connect(
        &self,
        _identity: &ClientIdentity,
        first: pb::AgentMessage,
    ) -> Result<Connection, TransportError> {
        // One slot, like the real transport's wire queue: the first message fills it until the hub reads.
        let (outbound, from_agent) = mpsc::channel(1);
        outbound.try_send(first).unwrap();
        let (to_agent, inbound) = mpsc::channel(16);
        {
            let mut ends = self.ends.lock().unwrap();
            ends.push(Some(HubEnd { from_agent, to_agent }));
            self.opened.send_replace(ends.len());
        }
        Ok(Connection {
            outbound,
            inbound: Box::pin(ReceiverStream::new(inbound)),
        })
    }
}
