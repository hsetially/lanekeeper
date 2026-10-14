//! Several audit or outbox events in one transaction (P15, D80, rule 8).
//!
//! A chunk of an apply moves up to `chunk_paths` paths and audits and publishes each one. `AuditLog::record` costs three round trips
//! an event (the chain's lock, the head of the chain, the insert) and `EventBus::publish_in_tx` one, so at the 50 paths of an agent
//! message they were 37 of the 67 ms a message took, and the registry applied 724 versions a second against the 1,000 of P15.
//! [`record_all`] and [`publish_all`] write the same events with `AuditLog::record_many` and `EventBus::publish_many_in_tx`: the same
//! rows, in the same order, one audit event for each path (rule 8 is untouched) and one outbox event for each `FileObserved`, in
//! three round trips for the audit and one for the outbox whatever the size.
//!
//! - **No events** write nothing, and do not look at the transaction.
//! - **One event** goes through `record` or `publish_in_tx`: a batch of one is no faster, and the port says to use the single call.
//! - **Several events**: the audit events are cut by [`ports::split_batches`] so that both limits of the port hold, the count
//!   ([`ports::MAX_AUDIT_BATCH`]) and the bytes of canonical JSON ([`ports::MAX_AUDIT_BATCH_BYTES`]), in order and without dropping
//!   one. Cutting encodes every event, which is CPU work, so it runs on the blocking pool and never on the async runtime (rule 4).
//!   The outbox events are cut by [`ports::MAX_EVENT_BATCH`], the only limit that bus has.
//!
//! The chain's lock is held from the first `record_many` to the end of the transaction, whatever the number of calls, so the caller
//! writes the audit events **last**, after everything slow, as before. A chunk of at most 200 events whose events are small is one
//! batch and one call; a bigger chunk (`chunk_paths` goes up to 1,000) or events with large diffs are a few batches, and the lock is
//! held for all of them, which is what the old loop of `record` calls did too.
//!
//! A port that refuses a batch for size after this cut it is a bug in the registry, never something a caller did: it is reported as
//! [`RegistryError::Internal`] (see `crate::error`).

use domain::DomainEvent;
use ports::{AuditEvent, AuditLog, EventBus, MAX_EVENT_BATCH, Tx, split_batches};

use crate::error::RegistryError;

/// Record `events` in the chain inside `tx`, in order: `record` for one, `record_many` for several. Nothing is recorded if one of the
/// events is one that `record` would refuse (too large a diff, an event that cannot be encoded): the cut looks at every event first.
pub(crate) async fn record_all(
    audit: &dyn AuditLog,
    tx: &mut Tx<'_>,
    mut events: Vec<AuditEvent>,
) -> Result<(), RegistryError> {
    match events.len() {
        0 => Ok(()),
        1 => {
            if let Some(event) = events.pop() {
                audit.record(tx, event).await?;
            }
            Ok(())
        }
        _ => {
            // CPU-bound (it canonicalises each event), so not on the async runtime.
            let batches = tokio::task::spawn_blocking(move || split_batches(events))
                .await
                .map_err(|_| RegistryError::off_runtime("audit_split"))??;
            for batch in batches {
                audit.record_many(tx, batch).await?;
            }
            Ok(())
        }
    }
}

/// Publish `events` in the outbox inside `tx`, in order: `publish_in_tx` for one, `publish_many_in_tx` for several, in batches of at
/// most [`MAX_EVENT_BATCH`].
pub(crate) async fn publish_all(
    bus: &dyn EventBus,
    tx: &mut Tx<'_>,
    mut events: Vec<DomainEvent>,
) -> Result<(), RegistryError> {
    match events.len() {
        0 => Ok(()),
        1 => {
            if let Some(event) = events.pop() {
                bus.publish_in_tx(tx, event).await?;
            }
            Ok(())
        }
        _ => {
            let mut rest = events.into_iter();
            loop {
                let batch: Vec<DomainEvent> = rest.by_ref().take(MAX_EVENT_BATCH).collect();
                if batch.is_empty() {
                    return Ok(());
                }
                bus.publish_many_in_tx(tx, batch).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use domain::{AuditId, DomainEvent, NfsPath, SwimlaneId, Timestamp};
    use futures::stream::BoxStream;
    use ports::fakes::FakeTxState;
    use ports::{
        AuditAction, AuditActor, AuditError, AuditVia, BusError, EventFilter, MAX_AUDIT_BATCH,
        MAX_AUDIT_BATCH_BYTES, canonical_size,
    };

    use super::*;

    #[derive(Default)]
    struct Audit {
        singles: AtomicUsize,
        /// The paths of each batch, in the order of the calls.
        batches: Mutex<Vec<Vec<String>>>,
        refuse_batches: bool,
    }

    fn path_of(e: &AuditEvent) -> String {
        e.path.as_ref().map(|p| p.as_str().to_owned()).unwrap_or_default()
    }

    #[async_trait]
    impl AuditLog for Audit {
        async fn record(&self, _: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
            self.singles.fetch_add(1, Ordering::SeqCst);
            self.batches.lock().unwrap().push(vec![path_of(&e)]);
            Ok(AuditId::from(1))
        }

        async fn record_many(
            &self,
            _: &mut Tx<'_>,
            events: Vec<AuditEvent>,
        ) -> Result<Vec<AuditId>, AuditError> {
            if self.refuse_batches {
                return Err(AuditError::BatchTooLarge);
            }
            self.batches
                .lock()
                .unwrap()
                .push(events.iter().map(path_of).collect());
            Ok(events.iter().map(|_| AuditId::from(1)).collect())
        }
    }

    #[derive(Default)]
    struct Bus {
        singles: AtomicUsize,
        batches: Mutex<Vec<usize>>,
        refuse_batches: bool,
    }

    #[async_trait]
    impl EventBus for Bus {
        async fn publish(&self, _: DomainEvent) -> Result<(), BusError> {
            Ok(())
        }

        async fn publish_in_tx(&self, _: &mut Tx<'_>, _: DomainEvent) -> Result<(), BusError> {
            self.singles.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn publish_many_in_tx(&self, _: &mut Tx<'_>, events: Vec<DomainEvent>) -> Result<(), BusError> {
            if self.refuse_batches {
                return Err(BusError::BatchTooLarge);
            }
            self.batches.lock().unwrap().push(events.len());
            Ok(())
        }

        fn subscribe(&self, _: EventFilter) -> BoxStream<'static, DomainEvent> {
            Box::pin(futures::stream::empty())
        }
    }

    fn audit_event(n: usize, diff_bytes: usize) -> AuditEvent {
        let mut e = AuditEvent::new(
            Timestamp::from_unix_millis(1_000),
            AuditActor::Agent {
                swimlane: SwimlaneId::parse("sit1").unwrap(),
            },
            AuditAction::FileChangeDetected,
            AuditVia::AgentDetected,
        );
        e.path = Some(NfsPath::parse(&format!("svc/f{n:04}.yml")).unwrap());
        // A control character costs six bytes in canonical JSON: the worst text an event can carry.
        e.diff = (diff_bytes > 0).then(|| "\u{1}".repeat(diff_bytes));
        e
    }

    fn event(n: usize) -> DomainEvent {
        DomainEvent::FileObserved {
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            path: NfsPath::parse(&format!("svc/f{n:04}.yml")).unwrap(),
            hash: ports::content_hash(n.to_string().as_bytes()),
            source: domain::ObservationSource::Scan,
        }
    }

    fn names(range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|n| format!("svc/f{n:04}.yml")).collect()
    }

    #[tokio::test]
    async fn no_events_write_nothing() {
        let (audit, bus) = (Audit::default(), Bus::default());
        let mut state = FakeTxState::new();
        let mut tx = Tx::in_memory(&mut state);
        record_all(&audit, &mut tx, Vec::new()).await.unwrap();
        publish_all(&bus, &mut tx, Vec::new()).await.unwrap();
        assert!(audit.batches.lock().unwrap().is_empty() && bus.batches.lock().unwrap().is_empty());
        assert_eq!(bus.singles.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn one_event_is_the_single_call_because_a_batch_of_one_is_no_faster() {
        let (audit, bus) = (Audit::default(), Bus::default());
        let mut state = FakeTxState::new();
        let mut tx = Tx::in_memory(&mut state);
        record_all(&audit, &mut tx, vec![audit_event(1, 0)])
            .await
            .unwrap();
        publish_all(&bus, &mut tx, vec![event(1)]).await.unwrap();
        assert_eq!(audit.singles.load(Ordering::SeqCst), 1);
        assert_eq!(bus.singles.load(Ordering::SeqCst), 1);
        assert!(bus.batches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_fifty_paths_of_a_message_are_one_batch_in_order() {
        let (audit, bus) = (Audit::default(), Bus::default());
        let mut state = FakeTxState::new();
        let mut tx = Tx::in_memory(&mut state);
        record_all(&audit, &mut tx, (0..50).map(|n| audit_event(n, 0)).collect())
            .await
            .unwrap();
        publish_all(&bus, &mut tx, (0..50).map(event).collect())
            .await
            .unwrap();
        assert_eq!(*audit.batches.lock().unwrap(), vec![names(0..50)]);
        assert_eq!(
            audit.singles.load(Ordering::SeqCst),
            0,
            "not one call for each event"
        );
        assert_eq!(*bus.batches.lock().unwrap(), vec![50]);
        assert_eq!(bus.singles.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn more_than_the_count_limit_is_cut_in_order_and_none_is_dropped() {
        let (audit, bus) = (Audit::default(), Bus::default());
        let mut state = FakeTxState::new();
        let mut tx = Tx::in_memory(&mut state);
        let n = 2 * MAX_AUDIT_BATCH + 50;
        record_all(&audit, &mut tx, (0..n).map(|i| audit_event(i, 0)).collect())
            .await
            .unwrap();
        publish_all(&bus, &mut tx, (0..n).map(event).collect())
            .await
            .unwrap();
        assert_eq!(
            *audit.batches.lock().unwrap(),
            vec![names(0..200), names(200..400), names(400..450)]
        );
        assert_eq!(*bus.batches.lock().unwrap(), vec![200, 200, 50]);
    }

    #[tokio::test]
    async fn events_with_large_diffs_are_cut_by_bytes_and_each_batch_holds_the_budget() {
        // Three events of 64 KiB of control characters: about 393 KB each in canonical JSON, so two fit the 1 MiB budget and the
        // third starts the next batch. Count alone would have sent all three at once.
        let events: Vec<AuditEvent> = (0..3)
            .map(|n| audit_event(n, AuditEvent::MAX_DIFF_BYTES))
            .collect();
        let size = canonical_size(&events[0]).unwrap();
        assert!(2 * size <= MAX_AUDIT_BATCH_BYTES && 3 * size > MAX_AUDIT_BATCH_BYTES);
        let audit = Audit::default();
        let mut state = FakeTxState::new();
        let mut tx = Tx::in_memory(&mut state);
        record_all(&audit, &mut tx, events).await.unwrap();
        assert_eq!(*audit.batches.lock().unwrap(), vec![names(0..2), names(2..3)]);
    }

    #[tokio::test]
    async fn an_event_that_cannot_be_recorded_stops_everything_before_any_call() {
        let audit = Audit::default();
        let mut state = FakeTxState::new();
        let mut tx = Tx::in_memory(&mut state);
        let mut events: Vec<AuditEvent> = (0..5).map(|n| audit_event(n, 0)).collect();
        events[3].diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
        assert_eq!(
            record_all(&audit, &mut tx, events).await,
            Err(RegistryError::Audit(AuditError::TooLarge))
        );
        assert!(
            audit.batches.lock().unwrap().is_empty(),
            "the bad event is found by the cut, before the first batch is written"
        );
    }

    #[tokio::test]
    async fn a_port_that_refuses_a_batch_for_size_is_an_internal_error() {
        let audit = Audit {
            refuse_batches: true,
            ..Audit::default()
        };
        let bus = Bus {
            refuse_batches: true,
            ..Bus::default()
        };
        let mut state = FakeTxState::new();
        let mut tx = Tx::in_memory(&mut state);
        assert_eq!(
            record_all(&audit, &mut tx, (0..3).map(|n| audit_event(n, 0)).collect()).await,
            Err(RegistryError::Internal)
        );
        assert_eq!(
            publish_all(&bus, &mut tx, (0..3).map(event).collect()).await,
            Err(RegistryError::Internal)
        );
    }
}
