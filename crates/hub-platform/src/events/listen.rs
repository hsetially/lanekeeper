//! This replica's one `LISTEN` connection (D80, P13).
//!
//! A notification says only "there is a new row": the relay reads the rows. The connection is dedicated (taken out of the pool, so
//! a busy pool cannot starve it). When it is lost, it is made again with exponential backoff and full jitter, and every time it
//! has been made again the generation counter goes up, which tells the relay that notifications may have been missed and every
//! subscriber must refetch. The relay's 250 ms tick means a connection that fails without saying so costs latency, not events.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::{Notify, watch};
use tracing::{debug, warn};

use super::metrics::EventMetrics;
use super::source::wait_closed;

/// The channel the outbox trigger notifies on.
pub const CHANNEL: &str = "lanekeeper_outbox";

/// The longest a connection attempt (and the `LISTEN`) may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_BASE: Duration = Duration::from_millis(100);
const BACKOFF_CAP: Duration = Duration::from_secs(5);

/// What the listener tells the relay and `/readyz`.
#[derive(Debug, Default)]
pub struct ListenState {
    connected: AtomicBool,
    /// Counts the times the connection was made again after it had been lost.
    generation: AtomicU64,
}

impl ListenState {
    /// Whether the `LISTEN` connection is up now.
    #[must_use]
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

pub struct OutboxListener {
    pub pool: PgPool,
    pub state: Arc<ListenState>,
    /// Woken by every notification (and by a reconnect). It keeps at most one pending wake, so a burst is one poll.
    pub wake: Arc<Notify>,
    pub metrics: EventMetrics,
}

impl std::fmt::Debug for OutboxListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxListener")
            .field("connected", &self.state.connected())
            .finish_non_exhaustive()
    }
}

enum Pumped {
    Stopped,
    Lost,
}

impl OutboxListener {
    fn set_connected(&self, connected: bool) {
        self.state.connected.store(connected, Ordering::SeqCst);
        self.metrics.set_listener_connected(connected);
    }

    fn lost_notifications(&self) {
        self.state.generation.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_one();
    }

    async fn connect(&self) -> Result<PgListener, sqlx::Error> {
        let attempt = async {
            let mut listener = PgListener::connect_with(&self.pool).await?;
            listener.listen(CHANNEL).await?;
            Ok(listener)
        };
        tokio::time::timeout(CONNECT_TIMEOUT, attempt)
            .await
            .map_err(|_| sqlx::Error::PoolTimedOut)?
    }

    /// Run until `stop` turns true (or its sender is dropped).
    pub async fn run(self, mut stop: watch::Receiver<bool>) {
        let mut failures = 0_u32;
        let mut ever_connected = false;
        loop {
            if *stop.borrow() {
                break;
            }
            match self.connect().await {
                Ok(listener) => {
                    failures = 0;
                    self.set_connected(true);
                    if ever_connected {
                        // Made again after a loss: whatever was notified in between is gone.
                        self.lost_notifications();
                    }
                    ever_connected = true;
                    debug!("outbox listener connected");
                    let pumped = self.pump(listener, &mut stop).await;
                    self.set_connected(false);
                    if matches!(pumped, Pumped::Stopped) {
                        break;
                    }
                    warn!("outbox listener lost its connection");
                }
                Err(_) => {
                    warn!(failures, "outbox listener could not connect");
                }
            }
            failures = failures.saturating_add(1);
            tokio::select! {
                () = tokio::time::sleep(backoff(failures)) => {}
                () = wait_closed(&mut stop) => break,
            }
        }
        self.set_connected(false);
    }

    async fn pump(&self, mut listener: PgListener, stop: &mut watch::Receiver<bool>) -> Pumped {
        loop {
            tokio::select! {
                biased;
                () = wait_closed(stop) => return Pumped::Stopped,
                received = listener.try_recv() => match received {
                    Ok(Some(_)) => self.wake.notify_one(),
                    // The driver made the connection again by itself: notifications in between were lost.
                    Ok(None) => self.lost_notifications(),
                    Err(_) => return Pumped::Lost,
                },
            }
        }
    }
}

/// Exponential backoff with full jitter: a random wait between zero and the cap for this attempt.
fn backoff(failures: u32) -> Duration {
    let ceiling = BACKOFF_BASE
        .saturating_mul(2_u32.saturating_pow(failures.min(8)))
        .min(BACKOFF_CAP);
    let millis = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
    Duration::from_millis(fastrand::u64(0..=millis))
}
