//! Request ids to waiting callers, for one agent stream (D62, rule 5).
//!
//! Every stream has its own [`Pending`] table, so a reply can only complete a request that was sent on the stream it arrived
//! on: an agent cannot answer, or break, another swimlane's request by guessing or repeating an id. The table is bounded,
//! refuses a second waiter for an id that is already waited on, and is closed when the stream ends, which completes every
//! waiter with [`GatewayError::Closed`] and makes later registrations fail instead of waiting for an answer that cannot come.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use domain::{AgentReply, HubCommand, RequestId};
use ports::GatewayError;
use tokio::sync::oneshot;

/// The kind of an agent's reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplyKind {
    Op,
    File,
    Served,
    Notify,
    Cluster,
}

impl ReplyKind {
    fn of(reply: &AgentReply) -> Self {
        match reply {
            AgentReply::Op(_) => Self::Op,
            AgentReply::File { .. } => Self::File,
            AgentReply::Served { .. } => Self::Served,
            AgentReply::Notify { .. } => Self::Notify,
            AgentReply::Cluster { .. } => Self::Cluster,
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// The reply kinds a command can be answered with. Any command may be answered by an operation result (a refusal or an
/// I/O failure on the agent's side); the success answer is specific to the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepts(u8);

impl Accepts {
    fn of(kinds: &[ReplyKind]) -> Self {
        Self(kinds.iter().fold(0, |bits, kind| bits | kind.bit()))
    }

    /// The request id of `command` and the replies that may complete it. `None` for a command that has no request id and no
    /// reply (`RequestDelta`, `RequestFullScan`): the agent answers those with scan deltas.
    #[must_use]
    pub fn for_command(command: &HubCommand) -> Option<(RequestId, Self)> {
        match command {
            HubCommand::RequestDelta { .. } | HubCommand::RequestFullScan => None,
            HubCommand::ReadFile { request_id, .. } => {
                Some((request_id.clone(), Self::of(&[ReplyKind::Op, ReplyKind::File])))
            }
            HubCommand::WriteFile { request_id, .. }
            | HubCommand::DeleteFile { request_id, .. }
            | HubCommand::RestartDeployment { request_id, .. } => {
                Some((request_id.clone(), Self::of(&[ReplyKind::Op])))
            }
            HubCommand::RequestClusterReport { request_id } => {
                Some((request_id.clone(), Self::of(&[ReplyKind::Op, ReplyKind::Cluster])))
            }
            HubCommand::NotifyConfigServer { request_id, .. } => {
                Some((request_id.clone(), Self::of(&[ReplyKind::Op, ReplyKind::Notify])))
            }
            HubCommand::FetchServed { request_id, .. } => {
                Some((request_id.clone(), Self::of(&[ReplyKind::Op, ReplyKind::Served])))
            }
        }
    }

    /// Whether `reply` is a kind of answer the command accepts.
    #[must_use]
    pub fn allows(self, reply: &AgentReply) -> bool {
        self.0 & ReplyKind::of(reply).bit() != 0
    }
}

/// The request id an agent reply answers.
#[must_use]
pub fn reply_request_id(reply: &AgentReply) -> &RequestId {
    match reply {
        AgentReply::Op(op) => &op.request_id,
        AgentReply::File { request_id, .. }
        | AgentReply::Served { request_id, .. }
        | AgentReply::Notify { request_id, .. }
        | AgentReply::Cluster { request_id, .. } => request_id,
    }
}

/// Why a request could not be registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    /// The stream has ended: nothing will ever answer.
    #[error("the stream has closed")]
    Closed,
    /// Another caller is already waiting for this request id on this stream.
    #[error("the request id is already waiting for an answer")]
    Duplicate,
    /// The table is full.
    #[error("too many requests are waiting")]
    Full,
}

/// What became of an agent's reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// A caller was waiting and has the reply.
    Delivered,
    /// Nobody was waiting for it (unknown id, already answered, timed out, or the wrong kind of reply): it is dropped.
    Unmatched,
}

struct Waiter {
    token: u64,
    accepts: Accepts,
    reply: oneshot::Sender<Result<AgentReply, GatewayError>>,
}

#[derive(Default)]
struct State {
    closed: bool,
    waiting: HashMap<RequestId, Waiter>,
}

/// The requests waiting for an answer on one stream.
pub struct Pending {
    capacity: usize,
    state: Mutex<State>,
    next_token: AtomicU64,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl Pending {
    #[must_use]
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            state: Mutex::new(State::default()),
            next_token: AtomicU64::new(0),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // The table holds no invariant a panicking thread could leave half-true: a poisoned lock is still usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start waiting for the answer to `id`. The returned ticket removes the entry when it is dropped, so a caller that gives
    /// up (a timeout, a dropped future) never leaves an id behind.
    pub fn register(self: &Arc<Self>, id: RequestId, accepts: Accepts) -> Result<Ticket, RegisterError> {
        let (reply, receiver) = oneshot::channel();
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        {
            let mut state = self.state();
            if state.closed {
                return Err(RegisterError::Closed);
            }
            if state.waiting.contains_key(&id) {
                return Err(RegisterError::Duplicate);
            }
            if state.waiting.len() >= self.capacity {
                return Err(RegisterError::Full);
            }
            state.waiting.insert(
                id.clone(),
                Waiter {
                    token,
                    accepts,
                    reply,
                },
            );
        }
        Ok(Ticket {
            pending: Arc::clone(self),
            id,
            token,
            receiver,
        })
    }

    /// Hand an agent's reply to the caller waiting for it, if there is one and the reply is a kind it can accept.
    pub fn complete(&self, reply: AgentReply) -> Delivery {
        let waiter = {
            let mut state = self.state();
            let id = reply_request_id(&reply);
            match state.waiting.get(id) {
                Some(waiter) if waiter.accepts.allows(&reply) => state.waiting.remove(id),
                _ => None,
            }
        };
        match waiter {
            // The caller may have stopped waiting between the lookup and now: the reply is then dropped, which is right.
            Some(waiter) => {
                let _ = waiter.reply.send(Ok(reply));
                Delivery::Delivered
            }
            None => Delivery::Unmatched,
        }
    }

    /// The stream ended: every waiter gets [`GatewayError::Closed`] and no new one is accepted.
    pub fn close(&self) {
        let waiters: Vec<Waiter> = {
            let mut state = self.state();
            state.closed = true;
            state.waiting.drain().map(|(_, waiter)| waiter).collect()
        };
        for waiter in waiters {
            let _ = waiter.reply.send(Err(GatewayError::Closed));
        }
    }

    /// How many requests are waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state().waiting.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn forget(&self, id: &RequestId, token: u64) {
        let mut state = self.state();
        if state.waiting.get(id).is_some_and(|waiter| waiter.token == token) {
            state.waiting.remove(id);
        }
    }
}

/// A claim on the answer to one request.
#[derive(Debug)]
pub struct Ticket {
    pending: Arc<Pending>,
    id: RequestId,
    token: u64,
    receiver: oneshot::Receiver<Result<AgentReply, GatewayError>>,
}

impl Ticket {
    /// Wait for the answer, at most `limit`.
    pub async fn wait(mut self, limit: Duration) -> Result<AgentReply, GatewayError> {
        match tokio::time::timeout(limit, &mut self.receiver).await {
            Ok(Ok(result)) => result,
            // The table dropped the sender without an answer: it is gone.
            Ok(Err(_closed)) => Err(GatewayError::Closed),
            Err(_elapsed) => Err(GatewayError::Timeout),
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.pending.forget(&self.id, self.token);
    }
}
