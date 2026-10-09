use std::time::Duration;

use domain::{DomainEvent, ProposalId, ProposalStatus};
use futures::StreamExt;

use super::sample::swimlane;
use crate::{EventBus, EventFilter, EventKind, TxFactory};

const WAIT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(50);

fn findings(s: &str) -> DomainEvent {
    DomainEvent::FindingsChanged {
        swimlane: swimlane(s),
    }
}

fn proposal(n: i64) -> DomainEvent {
    DomainEvent::ProposalChanged {
        id: ProposalId::from(n),
        status: ProposalStatus::Pending,
    }
}

async fn next<S: futures::Stream<Item = DomainEvent> + Unpin>(s: &mut S) -> DomainEvent {
    tokio::time::timeout(WAIT, s.next())
        .await
        .expect("an event arrives")
        .expect("the stream is open")
}

async fn assert_quiet<S: futures::Stream<Item = DomainEvent> + Unpin>(s: &mut S, why: &str) {
    assert!(
        tokio::time::timeout(QUIET, s.next()).await.is_err(),
        "unexpected event: {why}"
    );
}

/// `buffer` is the number of events a subscriber may lag behind before it is cut off.
///
/// - Subscribers see events published after they subscribed (even before their first poll), in order.
/// - Filters limit by swimlane and kind; events with no swimlane pass a swimlane filter.
/// - `publish_in_tx` delivers only after the transaction commits, never on rollback.
/// - A subscriber that lags receives `Resync` before any later event, and then catches up.
pub async fn event_bus<B: EventBus + ?Sized>(bus: &B, txf: &dyn TxFactory, buffer: usize) {
    // Order, and subscribing before the first poll.
    let mut all = bus.subscribe(EventFilter::all());
    bus.publish(findings("sit1")).await.unwrap();
    bus.publish(proposal(1)).await.unwrap();
    assert_eq!(next(&mut all).await, findings("sit1"));
    assert_eq!(next(&mut all).await, proposal(1));

    // Filters.
    let mut only_sit1 = bus.subscribe(EventFilter::for_swimlane(swimlane("sit1")));
    let mut only_proposals = bus.subscribe(EventFilter {
        swimlanes: None,
        kinds: [EventKind::ProposalChanged].into(),
    });
    bus.publish(findings("sit2")).await.unwrap();
    bus.publish(findings("sit1")).await.unwrap();
    bus.publish(proposal(2)).await.unwrap();
    assert_eq!(
        next(&mut only_sit1).await,
        findings("sit1"),
        "sit2 is filtered out"
    );
    assert_eq!(
        next(&mut only_sit1).await,
        proposal(2),
        "events without a swimlane pass a swimlane filter"
    );
    assert_eq!(next(&mut only_proposals).await, proposal(2), "kind filter");
    assert_quiet(&mut only_proposals, "only proposals were asked for").await;

    // A new subscriber does not see the past.
    let mut late = bus.subscribe(EventFilter::all());
    assert_quiet(&mut late, "history is not replayed").await;

    // Outbox: parked until commit, dropped on rollback.
    let mut tx = txf.begin().await.unwrap();
    bus.publish_in_tx(&mut tx.tx(), findings("sit3")).await.unwrap();
    assert_quiet(&mut late, "published inside an open transaction").await;
    tx.commit().await.unwrap();
    assert_eq!(next(&mut late).await, findings("sit3"), "delivered after commit");

    let mut tx = txf.begin().await.unwrap();
    bus.publish_in_tx(&mut tx.tx(), findings("sit4")).await.unwrap();
    tx.rollback().await.unwrap();
    assert_quiet(&mut late, "rolled back").await;

    // Lag: never polled while more than `buffer` events are published.
    let mut slow = bus.subscribe(EventFilter::all());
    let burst = buffer * 2 + 2;
    for i in 0..burst {
        bus.publish(proposal(100 + i as i64)).await.unwrap();
    }
    assert_eq!(
        next(&mut slow).await,
        DomainEvent::Resync,
        "a lagging subscriber is told to resync first"
    );
    let last = proposal(100 + burst as i64 - 1);
    let mut seen_last = false;
    for _ in 0..burst {
        let e = next(&mut slow).await;
        assert_ne!(e, DomainEvent::Resync, "one Resync per lag");
        if e == last {
            seen_last = true;
            break;
        }
    }
    assert!(
        seen_last,
        "after Resync the subscriber catches up to the newest event"
    );
}
