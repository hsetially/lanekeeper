//! The queue between the agent's components and the stream to the hub, bounded in messages and in bytes (rule 5, P4).
//!
//! A message is at most 4 MiB, so a count alone would let a few dozen of them fill the 64 MiB the whole agent may use.
//! Every queued message therefore holds permits from a byte budget as well as a slot in the channel, and gives both back
//! when the writer takes it. A full queue makes [`Outbox::send`] wait, which is the back-pressure the scanner needs, and
//! makes [`Outbox::try_send`] fail, which is what a heartbeat wants: a late heartbeat is worth nothing.
//!
//! One outbox belongs to one connection. When the connection ends the receiver is dropped, every waiting sender gets
//! [`OutboxError::Closed`], and the next connection starts with an empty queue, so nothing stale is sent to the hub.

use std::fmt;
use std::sync::Arc;

use prost::Message;
use proto::convert::FromAgent;
use proto::pb;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError, mpsc};

use super::wire;

/// The byte budget is counted in blocks of this size, so it fits a semaphore.
const BLOCK: usize = 1024;

/// How much may wait in the outbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxLimits {
    /// Most messages waiting.
    pub messages: usize,
    /// Most encoded bytes waiting.
    pub bytes: usize,
}

impl Default for OutboxLimits {
    /// 64 messages and 8 MiB: two maximum-size deltas, or a long run of small messages.
    fn default() -> Self {
        Self {
            messages: 64,
            bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    /// There is no room right now ([`Outbox::try_send`] only).
    #[error("the outbox is full")]
    Full,
    /// The connection this outbox belonged to is gone.
    #[error("the connection is closed")]
    Closed,
    /// The message could never be sent: it is bigger than the wire allows (4 MiB) or than the whole byte budget. Waiting
    /// would not help, and sending it would only make the hub close the stream.
    #[error("the message is too large to send")]
    TooLarge,
}

struct Budget {
    permits: Arc<Semaphore>,
    total: u32,
}

/// A message waiting to be sent, with the room it takes. Dropping it gives the room back.
pub struct Queued {
    message: pb::AgentMessage,
    permit: OwnedSemaphorePermit,
}

impl Queued {
    /// The message, and the permit that keeps its room reserved until the caller has handed the message on.
    pub fn into_parts(self) -> (pb::AgentMessage, OwnedSemaphorePermit) {
        (self.message, self.permit)
    }
}

impl fmt::Debug for Queued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queued")
            .field("encoded_len", &self.message.encoded_len())
            .field("blocks", &self.permit.num_permits())
            .finish()
    }
}

/// The sending half. Cheap to clone; all clones share one queue and one budget.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<Queued>,
    budget: Arc<Budget>,
}

impl fmt::Debug for Outbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outbox")
            .field("closed", &self.tx.is_closed())
            .field("free_blocks", &self.budget.permits.available_permits())
            .finish()
    }
}

/// The receiving half, owned by the writer of the connection.
pub struct OutboxReceiver {
    rx: mpsc::Receiver<Queued>,
    budget: Arc<Budget>,
}

impl fmt::Debug for OutboxReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OutboxReceiver")
    }
}

/// A new outbox. A `messages` or `bytes` of zero is raised to one message or one block.
pub fn channel(limits: OutboxLimits) -> (Outbox, OutboxReceiver) {
    let total = u32::try_from(limits.bytes.div_ceil(BLOCK).max(1))
        .unwrap_or(u32::MAX)
        .min(u32::try_from(Semaphore::MAX_PERMITS).unwrap_or(u32::MAX));
    let budget = Arc::new(Budget {
        permits: Arc::new(Semaphore::new(total as usize)),
        total,
    });
    let (tx, rx) = mpsc::channel(limits.messages.max(1));
    (
        Outbox {
            tx,
            budget: Arc::clone(&budget),
        },
        OutboxReceiver { rx, budget },
    )
}

impl Outbox {
    /// Queue `message`, waiting for room. Dropping the future cancels the send and gives the room back.
    pub async fn send(&self, message: FromAgent) -> Result<(), OutboxError> {
        let message = wire::encode(message);
        let blocks = self.blocks(&message)?;
        let permit = Arc::clone(&self.budget.permits)
            .acquire_many_owned(blocks)
            .await
            .map_err(|_| OutboxError::Closed)?;
        self.tx
            .send(Queued { message, permit })
            .await
            .map_err(|_| OutboxError::Closed)
    }

    /// Queue `message` if there is room right now.
    pub fn try_send(&self, message: FromAgent) -> Result<(), OutboxError> {
        let message = wire::encode(message);
        let blocks = self.blocks(&message)?;
        let permit = Arc::clone(&self.budget.permits)
            .try_acquire_many_owned(blocks)
            .map_err(|e| match e {
                TryAcquireError::NoPermits => OutboxError::Full,
                TryAcquireError::Closed => OutboxError::Closed,
            })?;
        self.tx.try_send(Queued { message, permit }).map_err(|e| match e {
            TrySendError::Full(_) => OutboxError::Full,
            TrySendError::Closed(_) => OutboxError::Closed,
        })
    }

    /// True once the connection is gone.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// True when nothing is waiting and nothing is being handed on. Used to pick a quiet moment to reconnect.
    pub fn is_idle(&self) -> bool {
        self.budget.permits.available_permits() == self.budget.total as usize
    }

    fn blocks(&self, message: &pb::AgentMessage) -> Result<u32, OutboxError> {
        let len = message.encoded_len();
        let blocks = u32::try_from(len.div_ceil(BLOCK).max(1)).unwrap_or(u32::MAX);
        if len > proto::limits::MAX_MESSAGE_BYTES || blocks > self.budget.total {
            return Err(OutboxError::TooLarge);
        }
        Ok(blocks)
    }
}

impl OutboxReceiver {
    /// The next message, in the order it was queued; `None` once every sender is gone and the queue is empty.
    pub async fn recv(&mut self) -> Option<Queued> {
        self.rx.recv().await
    }
}

impl Drop for OutboxReceiver {
    /// Senders waiting for budget must not wait for a writer that no longer exists.
    fn drop(&mut self) {
        self.budget.permits.close();
        self.rx.close();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use domain::{AgentReply, ContentHash, NfsPath, RequestId};

    use super::*;

    fn small(n: usize) -> FromAgent {
        file(n, 8)
    }

    /// A reply that carries `len` bytes of file content.
    fn file(n: usize, len: usize) -> FromAgent {
        FromAgent::Reply(AgentReply::File {
            request_id: RequestId::parse(&format!("req-{n}")).unwrap(),
            path: NfsPath::parse("a/b.yml").unwrap(),
            hash: ContentHash::from_bytes([7; 32]),
            bytes: Bytes::from(vec![0_u8; len]),
        })
    }

    fn id_of(queued: Queued) -> String {
        let (message, _permit) = queued.into_parts();
        match message.kind {
            Some(pb::agent_message::Kind::FileContent(f)) => f.request_id,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn outbound_queue_is_bounded() {
        let (outbox, mut rx) = channel(OutboxLimits {
            messages: 4,
            bytes: 1024 * 1024,
        });
        for n in 0..4 {
            outbox.try_send(small(n)).unwrap();
        }
        assert_eq!(outbox.try_send(small(4)), Err(OutboxError::Full));
        // Taking one message out, and finishing with it, makes room for exactly one more.
        let first = rx.rx.try_recv().unwrap();
        assert_eq!(id_of(first), "req-0");
        outbox.try_send(small(5)).unwrap();
        assert_eq!(outbox.try_send(small(6)), Err(OutboxError::Full));
    }

    #[test]
    fn the_byte_budget_bounds_the_queue_even_with_few_messages() {
        let (outbox, mut rx) = channel(OutboxLimits::default());
        let three_mib = 3 * 1024 * 1024;
        outbox.try_send(file(1, three_mib)).unwrap();
        outbox.try_send(file(2, three_mib)).unwrap();
        // Two messages are 6 MiB of the 8 MiB. A third would be 9.
        assert_eq!(outbox.try_send(file(3, three_mib)), Err(OutboxError::Full));
        // Small messages still fit in what is left.
        outbox.try_send(small(4)).unwrap();
        // Handing a big one on frees its bytes, and only then.
        let (first, permit) = rx.rx.try_recv().unwrap().into_parts();
        assert!(first.encoded_len() > three_mib);
        assert_eq!(outbox.try_send(file(5, three_mib)), Err(OutboxError::Full));
        drop(permit);
        outbox.try_send(file(5, three_mib)).unwrap();
    }

    #[test]
    fn a_message_bigger_than_the_whole_budget_is_refused_at_once() {
        let (outbox, _rx) = channel(OutboxLimits::default());
        assert_eq!(
            outbox.try_send(file(1, 9 * 1024 * 1024)),
            Err(OutboxError::TooLarge)
        );
    }

    #[test]
    fn a_message_the_wire_would_refuse_never_enters_the_queue() {
        // The budget is large enough, so it is the 4 MiB message limit that refuses it, here and not as a dead stream.
        let (outbox, _rx) = channel(OutboxLimits {
            messages: 4,
            bytes: 64 * 1024 * 1024,
        });
        let over = proto::limits::MAX_MESSAGE_BYTES + 1;
        assert_eq!(outbox.try_send(file(1, over)), Err(OutboxError::TooLarge));
        outbox.try_send(file(2, 3 * 1024 * 1024)).unwrap();
    }

    #[test]
    fn zero_limits_are_raised_not_obeyed() {
        let (outbox, _rx) = channel(OutboxLimits {
            messages: 0,
            bytes: 0,
        });
        // One block holds a small message; nothing divides by zero.
        outbox.try_send(small(1)).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_outbox_makes_send_wait_and_room_wakes_it() {
        let (outbox, mut rx) = channel(OutboxLimits {
            messages: 1,
            bytes: 4096,
        });
        outbox.send(small(1)).await.unwrap();
        let waiting = {
            let outbox = outbox.clone();
            tokio::spawn(async move { outbox.send(small(2)).await })
        };
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!waiting.is_finished(), "send must wait, not fail and not drop");
        assert_eq!(id_of(rx.recv().await.unwrap()), "req-1");
        waiting.await.unwrap().unwrap();
        assert_eq!(id_of(rx.recv().await.unwrap()), "req-2");
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_send_gives_its_room_back() {
        let (outbox, mut rx) = channel(OutboxLimits {
            messages: 1,
            bytes: 2048,
        });
        outbox.send(small(1)).await.unwrap();
        // The second send waits for a slot; the timeout cancels it.
        let cancelled = tokio::time::timeout(Duration::from_secs(5), outbox.send(small(2))).await;
        assert!(cancelled.is_err());
        // Nothing stays reserved for it: once the first message is taken and finished, the whole budget is free.
        assert_eq!(id_of(rx.recv().await.unwrap()), "req-1");
        assert!(outbox.is_idle(), "the cancelled send left room reserved");
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_receiver_releases_every_waiting_sender() {
        let (outbox, rx) = channel(OutboxLimits {
            messages: 1,
            bytes: 2048,
        });
        outbox.send(small(1)).await.unwrap();
        let waiting = {
            let outbox = outbox.clone();
            tokio::spawn(async move { outbox.send(small(2)).await })
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!waiting.is_finished());
        drop(rx);
        assert_eq!(waiting.await.unwrap(), Err(OutboxError::Closed));
        assert_eq!(outbox.try_send(small(3)), Err(OutboxError::Closed));
        assert!(outbox.is_closed());
    }

    #[tokio::test]
    async fn messages_keep_their_order_and_the_budget_comes_back() {
        let (outbox, mut rx) = channel(OutboxLimits::default());
        for n in 0..10 {
            outbox.send(small(n)).await.unwrap();
        }
        assert!(!outbox.is_idle());
        for n in 0..10 {
            assert_eq!(id_of(rx.recv().await.unwrap()), format!("req-{n}"));
        }
        assert!(
            outbox.is_idle(),
            "every block is back once the writer is done with the messages"
        );
    }
}
