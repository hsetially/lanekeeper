//! The relay: reads new outbox rows and hands them to this replica's subscribers (D80, P13).
//!
//! Every replica reads every row, so each replica's subscribers hear every event, and the id of the row orders them. A notification
//! (or the safety tick) only says "look"; what is delivered is what the table holds.
//!
//! **The cursor and its overlap window.** Ids come from a sequence, but transactions commit in any order: a transaction that
//! took id 7 can commit after one that took id 8, and a cursor that only moves forward would skip 7 forever. [`RelayState`]
//! keeps the ids below the cursor that it has not seen (the gaps) for a bounded time, and each poll asks for exactly those
//! again (`id = ANY(gaps)`) besides the rows after the cursor. A gap that stays empty for the window is a rolled-back
//! transaction and is forgotten. Each id is therefore read, and delivered, once; the cost of the window is one extra indexed
//! read while a gap is open, not a rescan of old rows.
//!
//! The state is a plain struct with no clock of its own and no database, so that the property tests can drive it through every
//! commit order.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::DomainEvent;
use sqlx::PgPool;
use tokio::sync::{Notify, broadcast, watch};
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};

use super::listen::ListenState;
use super::metrics::{EventMetrics, ResyncReason};
use super::outbox::{self, OutboxError, Row};
use super::source::{Relayed, wait_closed};
use super::wire::{self, MAX_EVENT_BYTES};

/// Which ids the relay has delivered, and which it is still waiting for.
#[derive(Debug, Clone)]
pub struct RelayState {
    /// The highest id seen. Ids above it have not been read.
    after: i64,
    /// Ids below `after` that were not there when a higher id was read, with the time they were first missed.
    gaps: BTreeMap<i64, Instant>,
    ttl: Duration,
    max_gaps: usize,
}

impl RelayState {
    /// A state that has seen every id up to `newest`. A gap is waited for at most `ttl`, and at most `max_gaps` are kept
    /// (the newest, when more appear).
    #[must_use]
    pub fn new(newest: i64, ttl: Duration, max_gaps: usize) -> Self {
        Self {
            after: newest,
            gaps: BTreeMap::new(),
            ttl,
            max_gaps,
        }
    }

    /// The cursor: the next read is `id > after()`.
    #[must_use]
    pub fn after(&self) -> i64 {
        self.after
    }

    /// The ids to read again, in ascending order.
    #[must_use]
    pub fn gaps(&self) -> Vec<i64> {
        self.gaps.keys().copied().collect()
    }

    /// Forget the gaps that have been open for the whole window.
    pub fn expire(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.gaps
            .retain(|_, first_missed| now.duration_since(*first_missed) < ttl);
    }

    /// Take the result of one poll: `fresh` are the ids found after the cursor, `late` the ids found among the gaps. Returns the
    /// ids to deliver, ascending, each of which is delivered by exactly one call. An id that is neither beyond the cursor nor
    /// an open gap is a duplicate and is dropped.
    pub fn observe(&mut self, fresh: &[i64], late: &[i64], now: Instant) -> Vec<i64> {
        let mut deliver = Vec::with_capacity(fresh.len() + late.len());
        for &id in late {
            if self.gaps.remove(&id).is_some() {
                deliver.push(id);
            }
        }
        let mut fresh = fresh.to_vec();
        fresh.sort_unstable();
        let bound = i64::try_from(self.max_gaps).unwrap_or(i64::MAX);
        for id in fresh {
            if id <= self.after {
                continue;
            }
            let first_missing = self.after + 1;
            if id > first_missing {
                // A jump of a million ids must not become a million entries: only the newest `max_gaps` can be kept.
                let from = first_missing.max(id.saturating_sub(bound));
                for missing in from..id {
                    self.gaps.insert(missing, now);
                }
            }
            self.after = id;
            deliver.push(id);
        }
        while self.gaps.len() > self.max_gaps {
            self.gaps.pop_first();
        }
        deliver.sort_unstable();
        deliver
    }

    /// Start again from `newest`, with no gaps: after a `Resync`, which makes every subscriber refetch, what was missed no
    /// longer matters.
    pub fn reset(&mut self, newest: i64) {
        self.after = newest;
        self.gaps.clear();
    }
}

/// Bounds and timings of the relay.
#[derive(Debug, Clone)]
pub struct RelaySettings {
    /// The most rows one read returns. A backlog is read in several batches, with a yield between them.
    pub batch: i64,
    /// The safety tick: the relay reads at least this often even if no notification arrives.
    pub tick: Duration,
    /// How long a missing id is waited for before it is taken for a rolled-back transaction. A transaction that writes an event
    /// and takes longer than this to commit is delivered to nobody (the row stays, and a replay finds it).
    pub gap_ttl: Duration,
    /// The most missing ids tracked at once.
    pub max_gaps: usize,
    /// The largest stored envelope the relay reads, in bytes. The database prints JSON with spaces, so this is above the cap
    /// that publishers keep to.
    pub read_cap_bytes: usize,
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self {
            batch: 200,
            tick: Duration::from_millis(250),
            gap_ttl: Duration::from_secs(10),
            max_gaps: 4096,
            read_cap_bytes: MAX_EVENT_BYTES * 2,
        }
    }
}

/// Reads this replica's view of the outbox and sends each row, once, to its subscribers.
pub struct OutboxRelay {
    pub pool: PgPool,
    pub sender: broadcast::Sender<Arc<Relayed>>,
    pub listen: Arc<ListenState>,
    pub wake: Arc<Notify>,
    pub metrics: EventMetrics,
    pub settings: RelaySettings,
    /// Set to true when the relay stops, which ends every live stream.
    pub closed: Arc<watch::Sender<bool>>,
}

impl std::fmt::Debug for OutboxRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxRelay").finish_non_exhaustive()
    }
}

impl OutboxRelay {
    /// The state a relay starts from: everything up to the newest row now has been seen. An event inserted after this returns
    /// has a higher id, so the relay delivers it.
    pub async fn initial_state(&self) -> Result<RelayState, OutboxError> {
        let newest = outbox::newest_id(&self.pool).await?;
        Ok(RelayState::new(
            newest,
            self.settings.gap_ttl,
            self.settings.max_gaps,
        ))
    }

    /// Run until `stop` turns true. `initial` is the state from [`Self::initial_state`], or `None` if the database could not be
    /// reached when the bus started (the relay then tries until it can).
    pub async fn run(self, initial: Option<RelayState>, mut stop: watch::Receiver<bool>) {
        let mut state = if let Some(state) = initial {
            state
        } else if let Some(state) = self.initial_state_when_reachable(&mut stop).await {
            state
        } else {
            self.closed.send_replace(true);
            return;
        };
        let mut generation = self.listen.generation();
        let mut pending_resync: Option<ResyncReason> = None;
        let mut outage_since: Option<Instant> = None;
        let mut ticker = tokio::time::interval(self.settings.tick);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                () = wait_closed(&mut stop) => break,
                () = self.wake.notified() => {}
                _ = ticker.tick() => {}
            }
            let seen = self.listen.generation();
            if seen != generation {
                generation = seen;
                pending_resync = Some(ResyncReason::Reconnect);
            }
            if let Some(reason) = pending_resync {
                if self.resync(reason, &mut state).await.is_err() {
                    self.metrics.relay_error();
                    continue;
                }
                pending_resync = None;
            }
            if self.poll(&mut state).await.is_ok() {
                if outage_since
                    .take()
                    .is_some_and(|since| since.elapsed() >= self.settings.gap_ttl)
                {
                    // The gaps that were open when reads began to fail have long expired, and a row that committed in the
                    // meantime may be below the cursor: everyone refetches.
                    pending_resync = Some(ResyncReason::Outage);
                    self.wake.notify_one();
                }
            } else {
                self.metrics.relay_error();
                outage_since.get_or_insert_with(Instant::now);
                warn!("the event relay could not read the outbox");
            }
        }
        self.closed.send_replace(true);
    }

    async fn initial_state_when_reachable(&self, stop: &mut watch::Receiver<bool>) -> Option<RelayState> {
        loop {
            if let Ok(state) = self.initial_state().await {
                return Some(state);
            }
            self.metrics.relay_error();
            tokio::select! {
                () = tokio::time::sleep(self.settings.tick * 4) => {}
                () = wait_closed(stop) => return None,
            }
        }
    }

    /// Everyone refetches: the cursor moves to the newest row and subscribers get a `Resync` after it.
    async fn resync(&self, reason: ResyncReason, state: &mut RelayState) -> Result<(), OutboxError> {
        let newest = outbox::newest_id(&self.pool).await?;
        state.reset(newest);
        self.metrics.resync(reason);
        // No receivers is fine: nobody is subscribed.
        let _ = self.sender.send(Arc::new(Relayed {
            id: None,
            event: DomainEvent::Resync,
        }));
        debug!(?reason, "subscribers told to resync");
        Ok(())
    }

    /// Read what is new, and what was missing, and deliver it.
    ///
    /// **The order of the two reads matters.** Rows after the cursor are read first, and the rows that were missing after them. A
    /// transaction that commits one event and then, once that returned, another has its two rows in id order, and a read that
    /// sees the second sees the first (one statement is one snapshot). Reading the missing ids first would let a gap commit
    /// between the two reads: the row after it would then be delivered before it, and one publisher's events would be heard out of
    /// the order it published them. This way, whatever the fresh read saw has its older rows either in the same read or in the gap
    /// read that follows, and `observe` hands both over in id order.
    async fn poll(&self, state: &mut RelayState) -> Result<(), OutboxError> {
        state.expire(Instant::now());
        let gaps = state.gaps();
        let mut first = true;
        loop {
            let new = outbox::read_after(&self.pool, state.after(), self.settings.batch).await?;
            let late = if first && !gaps.is_empty() {
                outbox::read_ids(&self.pool, &gaps).await?
            } else {
                Vec::new()
            };
            first = false;
            let full = i64::try_from(new.len()).is_ok_and(|n| n >= self.settings.batch);
            let new_ids: Vec<i64> = new.iter().map(|row| row.id.get()).collect();
            let late_ids: Vec<i64> = late.iter().map(|row| row.id.get()).collect();
            let deliver = state.observe(&new_ids, &late_ids, Instant::now());
            let by_id: HashMap<i64, &Row> = new
                .iter()
                .chain(late.iter())
                .map(|row| (row.id.get(), row))
                .collect();
            for id in deliver {
                if let Some(row) = by_id.get(&id) {
                    self.deliver(row);
                }
            }
            if !full {
                self.metrics.set_relay_lag(0);
                return Ok(());
            }
            // A full batch: more may be waiting. Say how far behind the relay is, and let the subscribers run.
            let newest = outbox::newest_id(&self.pool).await?;
            self.metrics.set_relay_lag((newest - state.after()).max(0));
            tokio::task::yield_now().await;
        }
    }

    fn deliver(&self, row: &Row) {
        let item = match wire::decode(&row.text, self.settings.read_cap_bytes) {
            Ok(event) => Relayed {
                id: Some(row.id),
                event,
            },
            Err(error) => {
                // Not an event this build can read (another version, or tampered with): subscribers refetch. The id and the
                // reason are logged, never the row.
                warn!(id = row.id.get(), %error, "an outbox row is not a readable event");
                self.metrics.resync(ResyncReason::Undecodable);
                Relayed {
                    id: Some(row.id),
                    event: DomainEvent::Resync,
                }
            }
        };
        self.metrics.relayed();
        let _ = self.sender.send(Arc::new(item));
    }
}
