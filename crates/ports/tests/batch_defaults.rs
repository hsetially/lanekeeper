//! The default `AuditLog::record_many` and `EventBus::publish_many_in_tx`: an implementation that only has the single-event
//! method (every implementor that existed before the batch methods) keeps working, and gets the documented behaviour.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_wrap
)]

use std::sync::Mutex;

use async_trait::async_trait;
use domain::{AuditId, DomainEvent, ProposalId, ProposalStatus, Timestamp};
use futures::stream::BoxStream;
use ports::conformance::sample::{audit_events_totalling, user_id, worst_case_audit_event};
use ports::fakes::FakeTxState;
use ports::{
    AuditAction, AuditActor, AuditError, AuditEvent, AuditLog, AuditVia, BusError, EventBus, EventFilter,
    MAX_AUDIT_BATCH, MAX_AUDIT_BATCH_BYTES, MAX_EVENT_BATCH, Tx,
};

fn audit_event(n: i64) -> AuditEvent {
    AuditEvent::new(
        Timestamp::from_unix_millis(n),
        AuditActor::User { user: user_id(1) },
        AuditAction::SettingsChanged,
        AuditVia::Ui,
    )
}

fn proposal(n: i64) -> DomainEvent {
    DomainEvent::ProposalChanged {
        id: ProposalId::from(n),
        status: ProposalStatus::Pending,
    }
}

/// An audit log with only `record`: it notes the `at` of every event it is given and fails on a chosen one.
#[derive(Default)]
struct SingleAudit {
    seen: Mutex<Vec<i64>>,
    fail_at: Option<i64>,
}

#[async_trait]
impl AuditLog for SingleAudit {
    async fn record(&self, _tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        let at = e.at.unix_millis();
        if self.fail_at == Some(at) {
            return Err(AuditError::Unavailable);
        }
        let mut seen = self.seen.lock().unwrap();
        seen.push(at);
        Ok(AuditId::from(i64::try_from(seen.len()).unwrap() * 10))
    }
}

/// A bus with only `publish_in_tx`.
#[derive(Default)]
struct SingleBus {
    seen: Mutex<Vec<DomainEvent>>,
    fail_on: Option<DomainEvent>,
}

#[async_trait]
impl EventBus for SingleBus {
    async fn publish(&self, _e: DomainEvent) -> Result<(), BusError> {
        Err(BusError::Unavailable)
    }

    async fn publish_in_tx(&self, _tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        if self.fail_on.as_ref() == Some(&e) {
            return Err(BusError::Unavailable);
        }
        self.seen.lock().unwrap().push(e);
        Ok(())
    }

    fn subscribe(&self, _f: EventFilter) -> BoxStream<'static, DomainEvent> {
        Box::pin(futures::stream::empty())
    }
}

#[tokio::test]
async fn default_record_many_records_each_event_in_order_and_returns_the_ids_in_order() {
    let log = SingleAudit::default();
    let mut state = FakeTxState::new();
    let ids = log
        .record_many(&mut Tx::in_memory(&mut state), (1..=4).map(audit_event).collect())
        .await
        .unwrap();
    assert_eq!(
        *log.seen.lock().unwrap(),
        [1, 2, 3, 4],
        "one call of `record` per event, in order"
    );
    assert_eq!(
        ids,
        [10, 20, 30, 40].map(AuditId::from),
        "the ids are the ones `record` returned, in input order"
    );
}

#[tokio::test]
async fn default_record_many_of_nothing_calls_nothing() {
    let log = SingleAudit::default();
    let mut state = FakeTxState::new();
    assert_eq!(
        log.record_many(&mut Tx::in_memory(&mut state), Vec::new()).await,
        Ok(Vec::new())
    );
    assert!(log.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn default_record_many_accepts_the_limit_and_refuses_one_more_without_recording() {
    let log = SingleAudit::default();
    let mut state = FakeTxState::new();
    let at_limit = (0..MAX_AUDIT_BATCH as i64).map(audit_event).collect();
    let ids = log
        .record_many(&mut Tx::in_memory(&mut state), at_limit)
        .await
        .unwrap();
    assert_eq!(ids.len(), MAX_AUDIT_BATCH);

    let log = SingleAudit::default();
    let over = (0..=MAX_AUDIT_BATCH as i64).map(audit_event).collect();
    assert_eq!(
        log.record_many(&mut Tx::in_memory(&mut state), over).await,
        Err(AuditError::BatchTooLarge)
    );
    assert!(
        log.seen.lock().unwrap().is_empty(),
        "refused whole: not one event was recorded"
    );
}

#[tokio::test]
async fn default_record_many_refuses_an_oversized_diff_before_recording_anything() {
    let log = SingleAudit::default();
    let mut state = FakeTxState::new();
    let mut events: Vec<AuditEvent> = (1..=4).map(audit_event).collect();
    events[2].diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
    assert_eq!(
        log.record_many(&mut Tx::in_memory(&mut state), events).await,
        Err(AuditError::TooLarge)
    );
    assert!(
        log.seen.lock().unwrap().is_empty(),
        "the two events before the bad one were not recorded either"
    );
}

#[tokio::test]
async fn default_record_many_stops_at_the_first_error_and_reports_it() {
    let log = SingleAudit {
        fail_at: Some(3),
        ..SingleAudit::default()
    };
    let mut state = FakeTxState::new();
    assert_eq!(
        log.record_many(&mut Tx::in_memory(&mut state), (1..=5).map(audit_event).collect())
            .await,
        Err(AuditError::Unavailable)
    );
    assert_eq!(
        *log.seen.lock().unwrap(),
        [1, 2],
        "nothing after the failing event is attempted"
    );
}

#[tokio::test]
async fn default_publish_many_in_tx_publishes_each_event_in_order() {
    let bus = SingleBus::default();
    let mut state = FakeTxState::new();
    bus.publish_many_in_tx(&mut Tx::in_memory(&mut state), (1..=4).map(proposal).collect())
        .await
        .unwrap();
    assert_eq!(
        *bus.seen.lock().unwrap(),
        (1..=4).map(proposal).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn default_publish_many_in_tx_of_nothing_publishes_nothing() {
    let bus = SingleBus::default();
    let mut state = FakeTxState::new();
    assert_eq!(
        bus.publish_many_in_tx(&mut Tx::in_memory(&mut state), Vec::new())
            .await,
        Ok(())
    );
    assert!(bus.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn default_publish_many_in_tx_accepts_the_limit_and_refuses_one_more_without_publishing() {
    let bus = SingleBus::default();
    let mut state = FakeTxState::new();
    let at_limit = (0..MAX_EVENT_BATCH as i64).map(proposal).collect();
    bus.publish_many_in_tx(&mut Tx::in_memory(&mut state), at_limit)
        .await
        .unwrap();
    assert_eq!(bus.seen.lock().unwrap().len(), MAX_EVENT_BATCH);

    let bus = SingleBus::default();
    let over = (0..=MAX_EVENT_BATCH as i64).map(proposal).collect();
    assert_eq!(
        bus.publish_many_in_tx(&mut Tx::in_memory(&mut state), over).await,
        Err(BusError::BatchTooLarge)
    );
    assert!(
        bus.seen.lock().unwrap().is_empty(),
        "refused whole: not one event was published"
    );
}

#[tokio::test]
async fn default_publish_many_in_tx_stops_at_the_first_error_and_reports_it() {
    let bus = SingleBus {
        fail_on: Some(proposal(3)),
        ..SingleBus::default()
    };
    let mut state = FakeTxState::new();
    assert_eq!(
        bus.publish_many_in_tx(&mut Tx::in_memory(&mut state), (1..=5).map(proposal).collect())
            .await,
        Err(BusError::Unavailable)
    );
    assert_eq!(*bus.seen.lock().unwrap(), [proposal(1), proposal(2)]);
}

#[test]
fn the_batch_limits_are_what_the_docs_say() {
    assert_eq!(MAX_AUDIT_BATCH, 200);
    assert_eq!(MAX_EVENT_BATCH, 200);
}

#[tokio::test]
async fn default_record_many_refuses_a_batch_over_the_byte_budget_before_recording_anything() {
    let log = SingleAudit::default();
    let mut state = FakeTxState::new();
    // Small events first, then enough of the largest to pass the budget: three events at the end, 1.2 MB, 192 KiB of diff.
    let mut events: Vec<AuditEvent> = (1..=4).map(audit_event).collect();
    events.extend((0..3).map(worst_case_audit_event));
    assert_eq!(
        log.record_many(&mut Tx::in_memory(&mut state), events).await,
        Err(AuditError::BatchTooLarge)
    );
    assert!(
        log.seen.lock().unwrap().is_empty(),
        "the four small events before the one that passed the budget were not recorded either"
    );
}

#[tokio::test]
async fn default_record_many_takes_the_byte_budget_to_the_byte_and_the_largest_event_alone() {
    let mut state = FakeTxState::new();
    let log = SingleAudit::default();
    let at_budget = audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES);
    let ids = log
        .record_many(&mut Tx::in_memory(&mut state), at_budget)
        .await
        .unwrap();
    assert_eq!(ids.len(), 3);

    let log = SingleAudit::default();
    let over = audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES + 1);
    assert_eq!(
        log.record_many(&mut Tx::in_memory(&mut state), over).await,
        Err(AuditError::BatchTooLarge)
    );
    assert!(log.seen.lock().unwrap().is_empty());

    // The invariant behind the budget: what `record` accepts, `record_many` accepts alone.
    let log = SingleAudit::default();
    let ids = log
        .record_many(&mut Tx::in_memory(&mut state), vec![worst_case_audit_event(0)])
        .await
        .unwrap();
    assert_eq!(ids.len(), 1);
}
