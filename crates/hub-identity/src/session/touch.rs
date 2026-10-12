//! `last_seen_at`, written in batches off the request path (T3; S2).
//!
//! A request that finds a session in use asks for a touch. The batcher remembers when it last accepted one for that session
//! and ignores asks inside the touch interval (a minute), puts the rest in a bounded queue, and a background task writes the
//! queue with one statement per batch. If the queue is full the touch is dropped: the next request asks again. A request never
//! waits for, or fails because of, a touch.
//!
//! The idle limit (8 hours) is judged against the stored `last_seen_at`, which therefore lags real activity by up to the touch
//! interval plus the flush interval, and the session cache adds up to 30 seconds. That is about two minutes of slack on an
//! eight-hour limit, and it is stated in the threat model.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use domain::Timestamp;
use sqlx::PgPool;
use tokio::sync::mpsc;

use super::cookie::TokenHash;
use super::store::db_err;
use super::{AbortOnDrop, SessionError, SessionPolicy};
use crate::clock::Clock;

/// A write that takes longer than this is abandoned; its touches are lost, and the next requests ask again.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(10);

/// What one flush wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlushReport {
    /// Sessions in the batches.
    pub sessions: usize,
    /// `UPDATE` statements run.
    pub statements: usize,
}

type Touch = (TokenHash, Timestamp);

/// The receiving side and what it writes with, shared with the background task.
struct Writer {
    pool: PgPool,
    rx: tokio::sync::Mutex<mpsc::Receiver<Touch>>,
    batch: usize,
    /// The most queued touches one flush takes, so a flush cannot run for ever behind a busy producer.
    queue: usize,
}

pub struct LastSeenBatcher {
    writer: Arc<Writer>,
    tx: mpsc::Sender<Touch>,
    clock: Arc<dyn Clock>,
    interval_ms: i64,
    /// When each recently touched session was last accepted, in wall-clock milliseconds. Bounded by `remember_cap`.
    remembered: Mutex<HashMap<TokenHash, i64>>,
    remember_cap: usize,
    dropped: AtomicU64,
    _task: Option<AbortOnDrop>,
}

impl std::fmt::Debug for LastSeenBatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LastSeenBatcher")
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

impl LastSeenBatcher {
    /// A batcher whose queue is written every `policy.touch_flush` by a task it owns. Needs a Tokio runtime.
    pub fn spawn(pool: PgPool, clock: Arc<dyn Clock>, policy: &SessionPolicy) -> Self {
        Self::build(pool, clock, policy, true)
    }

    /// A batcher that is written only by [`LastSeenBatcher::flush_now`].
    pub fn without_task(pool: PgPool, clock: Arc<dyn Clock>, policy: &SessionPolicy) -> Self {
        Self::build(pool, clock, policy, false)
    }

    fn build(pool: PgPool, clock: Arc<dyn Clock>, policy: &SessionPolicy, with_task: bool) -> Self {
        let queue = policy.touch_queue.max(1);
        let (tx, rx) = mpsc::channel(queue);
        let writer = Arc::new(Writer {
            pool,
            rx: tokio::sync::Mutex::new(rx),
            batch: policy.touch_batch.max(1),
            queue,
        });
        let task = with_task.then(|| {
            let (writer, every) = (Arc::clone(&writer), policy.touch_flush);
            AbortOnDrop::spawn(async move {
                let mut ticker = tokio::time::interval(every.max(Duration::from_millis(10)));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // The first tick is immediate; there is nothing to write yet.
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    match tokio::time::timeout(FLUSH_TIMEOUT, writer.flush()).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => tracing::warn!(error = %e, "last-seen batch was not written"),
                        Err(_) => tracing::warn!("last-seen batch timed out"),
                    }
                }
            })
        });
        Self {
            writer,
            tx,
            clock,
            interval_ms: i64::try_from(policy.touch_interval.as_millis()).unwrap_or(i64::MAX),
            remembered: Mutex::new(HashMap::new()),
            remember_cap: queue.saturating_mul(2),
            dropped: AtomicU64::new(0),
            _task: task,
        }
    }

    /// Ask for `hash`'s `last_seen_at` to be set to now. Never waits and never fails: a touch that cannot be queued is
    /// counted and dropped.
    pub fn touch(&self, hash: TokenHash) {
        let now = self.clock.now();
        {
            let mut seen = self.remembered.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(last) = seen.get(&hash) {
                // A clock that moved back is not a reason to hold the touch for ever: only a recent past suppresses it.
                if (0..self.interval_ms).contains(&now.unix_millis().saturating_sub(*last)) {
                    return;
                }
            }
            if seen.len() >= self.remember_cap && !seen.contains_key(&hash) {
                let cutoff = now.unix_millis().saturating_sub(self.interval_ms);
                seen.retain(|_, at| *at > cutoff);
                if seen.len() >= self.remember_cap {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
            if self.tx.try_send((hash, now)).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            seen.insert(hash, now.unix_millis());
        }
    }

    /// Write what is queued, now. The background task calls this; tests call it to avoid waiting.
    pub async fn flush_now(&self) -> Result<FlushReport, SessionError> {
        self.writer.flush().await
    }

    /// How many touches were dropped because the queue or the memory of recent touches was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Writer {
    async fn flush(&self) -> Result<FlushReport, SessionError> {
        let mut rx = self.rx.lock().await;
        let mut report = FlushReport::default();
        let mut taken = 0usize;
        while taken < self.queue {
            let mut ids: Vec<Vec<u8>> = Vec::with_capacity(self.batch);
            let mut times: Vec<i64> = Vec::with_capacity(self.batch);
            while ids.len() < self.batch && taken < self.queue {
                let Ok((hash, at)) = rx.try_recv() else { break };
                taken += 1;
                ids.push(hash.as_bytes().to_vec());
                times.push(at.unix_millis());
            }
            if ids.is_empty() {
                break;
            }
            // Never backwards: a late write of an older touch changes nothing.
            sqlx::query(
                "UPDATE sessions s SET last_seen_at = to_timestamp(t.ms / 1000.0) \
                 FROM unnest($1::bytea[], $2::bigint[]) AS t(h, ms) \
                 WHERE s.session_hash = t.h AND s.last_seen_at < to_timestamp(t.ms / 1000.0)",
            )
            .bind(&ids)
            .bind(&times)
            .execute(&self.pool)
            .await
            .map_err(|e| db_err("write last seen", &e))?;
            report.sessions += ids.len();
            report.statements += 1;
        }
        Ok(report)
    }
}
