use std::time::Duration;

use domain::{DomainEvent, ProposalId, ProposalStatus, ServiceRef};
use futures::StreamExt;

use super::sample::{swimlane, user_id};
use crate::{BusError, EventBus, EventFilter, EventKind, MAX_EVENT_BATCH, TxFactory};

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
/// - `publish_many_in_tx` ([`event_bus_batch`]) behaves like `publish_in_tx` called once per event.
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

    // Internal events are delivered like any other (the session cache subscribes for them); hiding them from
    // browsers is the SSE edge's job, through `DomainEvent::is_internal`.
    let revoked = DomainEvent::UserSessionsRevoked { user: user_id(1) };
    let mut everything = bus.subscribe(EventFilter::all());
    let mut in_sit1 = bus.subscribe(EventFilter::for_swimlane(swimlane("sit1")));
    let mut only_revoked = bus.subscribe(EventFilter {
        swimlanes: None,
        kinds: [EventKind::UserSessionsRevoked].into(),
    });
    bus.publish(revoked.clone()).await.unwrap();
    assert_eq!(next(&mut everything).await, revoked, "an unfiltered subscriber");
    assert_eq!(
        next(&mut in_sit1).await,
        revoked,
        "an internal event has no swimlane, so it passes a swimlane filter"
    );
    assert_eq!(next(&mut only_revoked).await, revoked, "its own kind filter");

    // `PickupChanged` (D85, prompt 05) belongs to a swimlane: a swimlane filter limits it, and its own kind finds it.
    let pickup = |s: &str| DomainEvent::PickupChanged {
        swimlane: swimlane(s),
        services: vec![ServiceRef::new("csp", "tx-infinity-api").expect("a service")],
    };
    let mut pickups_in_sit1 = bus.subscribe(EventFilter {
        swimlanes: Some([swimlane("sit1")].into()),
        kinds: [EventKind::PickupChanged].into(),
    });
    bus.publish(pickup("sit2")).await.unwrap();
    bus.publish(pickup("sit1")).await.unwrap();
    assert_eq!(
        next(&mut pickups_in_sit1).await,
        pickup("sit1"),
        "sit2 is filtered out"
    );
    assert_quiet(&mut pickups_in_sit1, "only sit1 was asked for").await;

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

    event_bus_batch(bus, txf).await;
}

/// Read `s` up to the next `marker`, and return what came before it.
async fn until<S: futures::Stream<Item = DomainEvent> + Unpin>(
    s: &mut S,
    marker: &DomainEvent,
) -> Vec<DomainEvent> {
    let mut seen = Vec::new();
    loop {
        let e = next(s).await;
        if &e == marker {
            return seen;
        }
        seen.push(e);
        assert!(
            seen.len() <= MAX_EVENT_BATCH * 2,
            "the marker event never arrived after {} others",
            seen.len()
        );
    }
}

/// `EventBus::publish_many_in_tx`, against `publish_in_tx`. Every implementation must pass it, whether it batches or keeps the
/// default.
///
/// - The events of a batch are delivered after the commit, never before, in input order, and in order with the single
///   `publish_in_tx` calls of the same transaction.
/// - A rollback delivers none of them.
/// - Filters and the swimlane rule apply to each event of a batch exactly as to the same events published one at a time,
///   internal events included.
/// - An empty batch is a no-op; [`MAX_EVENT_BATCH`] events are accepted and one more is `BatchTooLarge`, publishing nothing and
///   leaving the transaction usable.
pub async fn event_bus_batch<B: EventBus + ?Sized>(bus: &B, txf: &dyn TxFactory) {
    // Every section ends with a marker event that passes every filter used here, so "nothing else came" is checked against an event
    // that must arrive after the section's events, and not against a quiet period.
    let marker = |n: i64| proposal(10_000 + n);

    // Order, and nothing before the commit.
    let mut all = bus.subscribe(EventFilter::all());
    let batch = vec![proposal(1000), findings("sit1"), proposal(1001)];
    let mut tx = txf.begin().await.unwrap();
    bus.publish_many_in_tx(&mut tx.tx(), batch.clone()).await.unwrap();
    assert_quiet(&mut all, "a batch published inside an open transaction").await;
    tx.commit().await.unwrap();
    for e in &batch {
        assert_eq!(
            &next(&mut all).await,
            e,
            "delivered after the commit, in input order"
        );
    }
    assert_quiet(&mut all, "nothing beyond the batch").await;

    // In order with single publishes of the same transaction.
    let mut tx = txf.begin().await.unwrap();
    bus.publish_in_tx(&mut tx.tx(), proposal(1010)).await.unwrap();
    bus.publish_many_in_tx(&mut tx.tx(), vec![proposal(1011), proposal(1012)])
        .await
        .unwrap();
    bus.publish_in_tx(&mut tx.tx(), proposal(1013)).await.unwrap();
    bus.publish_many_in_tx(&mut tx.tx(), vec![proposal(1014)])
        .await
        .unwrap();
    bus.publish_in_tx(&mut tx.tx(), marker(1)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        until(&mut all, &marker(1)).await,
        (1010..=1014).map(proposal).collect::<Vec<_>>(),
        "single and batched publishes keep the order they were made in"
    );

    // A rollback delivers none of it.
    let mut tx = txf.begin().await.unwrap();
    bus.publish_many_in_tx(&mut tx.tx(), vec![proposal(1020), proposal(1021)])
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let mut tx = txf.begin().await.unwrap();
    bus.publish_in_tx(&mut tx.tx(), marker(2)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        until(&mut all, &marker(2)).await,
        Vec::<DomainEvent>::new(),
        "a rolled-back batch is never delivered"
    );

    // The same rules as `publish_in_tx`: what each kind of subscriber hears of a batch is what it hears of the same events one at
    // a time, internal events and events of another swimlane included.
    let events = vec![
        findings("sit2"),
        findings("sit1"),
        proposal(1030),
        DomainEvent::UserSessionsRevoked { user: user_id(2) },
        DomainEvent::PickupChanged {
            swimlane: swimlane("sit1"),
            services: vec![ServiceRef::new("csp", "tx-infinity-api").expect("a service")],
        },
        DomainEvent::PickupChanged {
            swimlane: swimlane("sit2"),
            services: vec![ServiceRef::new("csp", "tx-infinity-api").expect("a service")],
        },
    ];
    let filters = [
        EventFilter::all(),
        EventFilter::for_swimlane(swimlane("sit1")),
        EventFilter {
            swimlanes: None,
            kinds: [EventKind::ProposalChanged, EventKind::UserSessionsRevoked].into(),
        },
    ];
    for (i, f) in filters.iter().enumerate() {
        let expected: Vec<DomainEvent> = events.iter().filter(|e| f.matches(e)).cloned().collect();
        assert!(
            expected.len() >= 2,
            "filter {i} keeps something and the test means something"
        );
        let mut sub = bus.subscribe(f.clone());

        let mut tx = txf.begin().await.unwrap();
        bus.publish_many_in_tx(&mut tx.tx(), events.clone())
            .await
            .unwrap();
        bus.publish_in_tx(&mut tx.tx(), marker(10 + i as i64))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let batched = until(&mut sub, &marker(10 + i as i64)).await;

        let mut tx = txf.begin().await.unwrap();
        for e in &events {
            bus.publish_in_tx(&mut tx.tx(), e.clone()).await.unwrap();
        }
        bus.publish_in_tx(&mut tx.tx(), marker(20 + i as i64))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let single = until(&mut sub, &marker(20 + i as i64)).await;

        assert_eq!(batched, expected, "filter {i}: what a batch delivers");
        assert_eq!(
            batched, single,
            "filter {i}: the same as one publish_in_tx per event"
        );
    }

    // A subscriber of its own for what follows: `all` has the events of the filter section waiting.
    let mut all = bus.subscribe(EventFilter::all());

    // Empty: a successful no-op.
    let mut tx = txf.begin().await.unwrap();
    assert_eq!(bus.publish_many_in_tx(&mut tx.tx(), Vec::new()).await, Ok(()));
    bus.publish_in_tx(&mut tx.tx(), marker(30)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        until(&mut all, &marker(30)).await,
        Vec::<DomainEvent>::new(),
        "an empty batch publishes nothing"
    );

    // The limit. One more than the limit is refused whole, and the transaction stays usable.
    let mut tx = txf.begin().await.unwrap();
    let too_many = (0..=MAX_EVENT_BATCH as i64)
        .map(|i| proposal(50_000 + i))
        .collect();
    assert_eq!(
        bus.publish_many_in_tx(&mut tx.tx(), too_many).await,
        Err(BusError::BatchTooLarge),
        "refused, not truncated"
    );
    bus.publish_in_tx(&mut tx.tx(), marker(31)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        until(&mut all, &marker(31)).await,
        Vec::<DomainEvent>::new(),
        "nothing of a refused batch is published"
    );

    // Exactly the limit is accepted. A subscriber with a small buffer may lag on so many events, and is then told to resync; what it
    // does hear is in order, and with no lag it hears all of them.
    let mut tx = txf.begin().await.unwrap();
    let full: Vec<DomainEvent> = (0..MAX_EVENT_BATCH as i64)
        .map(|i| proposal(60_000 + i))
        .collect();
    bus.publish_many_in_tx(&mut tx.tx(), full.clone()).await.unwrap();
    bus.publish_in_tx(&mut tx.tx(), marker(32)).await.unwrap();
    tx.commit().await.unwrap();
    let heard = until(&mut all, &marker(32)).await;
    let proposals: Vec<&DomainEvent> = heard.iter().filter(|e| **e != DomainEvent::Resync).collect();
    let in_order = full.iter().filter(|e| proposals.contains(e));
    assert!(
        proposals.iter().copied().eq(in_order),
        "what arrives of a full batch is in input order"
    );
    if proposals.len() == heard.len() {
        assert_eq!(heard, full, "no lag, so every event of a full batch arrives");
    }
}
