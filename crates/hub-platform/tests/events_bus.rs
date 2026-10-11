//! T5: the event bus over the Postgres outbox, three replicas on one real database (D80, P13).
//!
//! Every event is a row in `outbox`; its trigger sends `NOTIFY` at commit; each replica listens on one connection and its relay reads
//! the rows after its cursor. The tests run three replicas, each with its own pool, listener, relay and subscribers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::time::{Duration, Instant};

use domain::{DomainEvent, NfsPath, ProposalId, ProposalStatus, SwimlaneId};
use futures::StreamExt;
use hub_platform::events::bus::BusSettings;
use hub_platform::events::metrics::{PublishOutcome, ResyncReason};
use hub_platform::events::relay::RelaySettings;
use hub_platform::events::source::Replay;
use hub_platform::events::sse::SseSettings;
use hub_platform::events::wire::EventId;
use ports::{EventBus, EventFilter, Tx};
use sqlx::Row;
use support::event_cluster::{EventCluster, wait_for};

const WITHIN: Duration = Duration::from_secs(5);

fn proposal(n: i64) -> DomainEvent {
    DomainEvent::ProposalChanged {
        id: ProposalId::from(n),
        status: ProposalStatus::Pending,
    }
}

fn findings(s: &str) -> DomainEvent {
    DomainEvent::FindingsChanged {
        swimlane: SwimlaneId::parse(s).unwrap(),
    }
}

async fn next<S: futures::Stream<Item = DomainEvent> + Unpin>(s: &mut S) -> DomainEvent {
    tokio::time::timeout(WITHIN, s.next())
        .await
        .expect("an event arrives")
        .expect("the stream is open")
}

async fn quiet<S: futures::Stream<Item = DomainEvent> + Unpin>(s: &mut S, why: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(300), s.next())
            .await
            .is_err(),
        "unexpected event: {why}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_hubs_event_reaches_every_subscriber_within_1s() {
    let Some(cluster) = EventCluster::start(3, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let mut subscribers: Vec<_> = cluster
        .replicas
        .iter()
        .map(|r| r.bus.subscribe(EventFilter::all()))
        .collect();

    // A plain publish on replica 0.
    let started = Instant::now();
    cluster.replicas[0].bus.publish(proposal(1)).await.unwrap();
    for s in &mut subscribers {
        assert_eq!(next(s).await, proposal(1));
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );

    // An event inside a transaction on replica 2, delivered at commit.
    let started = Instant::now();
    let mut tx = cluster.replicas[2].pool.begin().await.unwrap();
    cluster.replicas[2]
        .bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(2))
        .await
        .unwrap();
    for s in &mut subscribers {
        quiet(s, "the transaction is still open").await;
    }
    let committing = Instant::now();
    tx.commit().await.unwrap();
    for s in &mut subscribers {
        assert_eq!(next(s).await, proposal(2));
    }
    assert!(
        committing.elapsed() < Duration::from_secs(1),
        "{:?}",
        committing.elapsed()
    );
    let _ = started;
}

/// Ids are given out in insert order but rows commit in any order, so two replicas may hear two events that committed out of id
/// order in different orders (each is delivered when the replica first sees it). What holds everywhere: every event is heard
/// once, and the events of one publisher, which commit one after the other, are heard in the order they were published.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_replica_hears_every_event_once_and_each_publishers_events_in_order() {
    let Some(cluster) = EventCluster::start(3, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let mut subscribers: Vec<_> = cluster
        .replicas
        .iter()
        .map(|r| r.bus.subscribe(EventFilter::all()))
        .collect();
    // Publishers on all three replicas at once.
    let mut tasks = Vec::new();
    for (i, replica) in cluster.replicas.iter().enumerate() {
        let bus = replica.bus.clone();
        tasks.push(tokio::spawn(async move {
            for n in 0..20 {
                bus.publish(proposal(i64::try_from(i).unwrap() * 100 + n))
                    .await
                    .unwrap();
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    for s in &mut subscribers {
        let mut heard: Vec<i64> = Vec::new();
        for _ in 0..60 {
            match next(s).await {
                DomainEvent::ProposalChanged { id, .. } => heard.push(id.get()),
                other => panic!("{other:?}"),
            }
        }
        quiet(s, "nothing is delivered twice").await;
        let mut sorted = heard.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 60, "every event once: {heard:?}");
        for publisher in 0..3_i64 {
            let own: Vec<i64> = heard.iter().copied().filter(|n| n / 100 == publisher).collect();
            let mut in_order = own.clone();
            in_order.sort_unstable();
            assert_eq!(own, in_order, "publisher {publisher}'s events keep their order");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_consumer_gets_resync_instead_of_blocking_the_bus() {
    // A channel of 8 and a relay that reads 4 rows at a time: a subscriber that is running always has a whole batch of slack, so
    // only the one that is not polled at all can fall behind.
    let settings = BusSettings {
        capacity: 8,
        relay: RelaySettings {
            batch: 4,
            ..RelaySettings::default()
        },
        ..BusSettings::default()
    };
    let Some(cluster) = EventCluster::start(2, settings, SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let b = &cluster.replicas[1];
    // `slow` is not polled while events flow; `fast` is polled by its own task.
    let mut slow = b.bus.subscribe(EventFilter::all());
    let mut fast = b.bus.subscribe(EventFilter::all());
    let fast_task = tokio::spawn(async move {
        let mut got = Vec::new();
        while got.len() < 40 {
            match tokio::time::timeout(WITHIN, fast.next()).await {
                Ok(Some(e)) => got.push(e),
                _ => break,
            }
        }
        got
    });

    let started = Instant::now();
    for n in 0..40 {
        a.bus.publish(proposal(n)).await.unwrap();
    }
    let got = fast_task.await.unwrap();
    assert_eq!(got.len(), 40, "a slow subscriber does not hold anyone else up");
    assert!(!got.contains(&DomainEvent::Resync), "the fast subscriber kept up");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );

    assert_eq!(
        next(&mut slow).await,
        DomainEvent::Resync,
        "the lagging subscriber is told to refetch before anything else"
    );
    // It then catches up to the newest events.
    let mut last = DomainEvent::Resync;
    for _ in 0..9 {
        last = next(&mut slow).await;
        if last == proposal(39) {
            break;
        }
    }
    assert_eq!(last, proposal(39));
    assert_eq!(b.metrics.resyncs_total(ResyncReason::Lagged), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_listener_connection_gives_every_subscriber_a_resync_and_events_flow_again() {
    let Some(cluster) = EventCluster::start(2, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let victim = &cluster.replicas[0];
    let other = &cluster.replicas[1];
    let mut on_victim = victim.bus.subscribe(EventFilter::all());
    let mut on_other = other.bus.subscribe(EventFilter::all());
    wait_for("both listeners to be connected", WITHIN, async || {
        victim.bus.listener_connected() && other.bus.listener_connected()
    })
    .await;

    // Kill the LISTEN backends of this database: both replicas lose their connection.
    let mut admin = cluster.db.admin().await;
    let killed: i64 = sqlx::query(
        "SELECT count(pg_terminate_backend(pid)) AS n FROM pg_stat_activity \
         WHERE datname = current_database() AND usename = 'lanekeeper_app' AND query ILIKE 'listen%'",
    )
    .fetch_one(&mut admin)
    .await
    .unwrap()
    .get("n");
    assert_eq!(killed, 2, "one LISTEN connection per replica");

    assert_eq!(next(&mut on_victim).await, DomainEvent::Resync);
    assert_eq!(next(&mut on_other).await, DomainEvent::Resync);
    assert!(victim.metrics.resyncs_total(ResyncReason::Reconnect) >= 1);
    wait_for("the listeners to reconnect", WITHIN, async || {
        victim.bus.listener_connected() && other.bus.listener_connected()
    })
    .await;

    victim.bus.publish(findings("sit1")).await.unwrap();
    assert_eq!(next(&mut on_victim).await, findings("sit1"));
    assert_eq!(next(&mut on_other).await, findings("sit1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_over_the_size_cap_is_stored_as_a_resync() {
    let Some(cluster) = EventCluster::start(2, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let b = &cluster.replicas[1];
    let mut sub = b.bus.subscribe(EventFilter::all());
    let paths: Vec<NfsPath> = (0..400)
        .map(|i| NfsPath::parse(&format!("service-{i}/config-file-{i}.yml")).unwrap())
        .collect();
    a.bus
        .publish(DomainEvent::DriftChanged {
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            paths,
        })
        .await
        .unwrap();
    assert_eq!(next(&mut sub).await, DomainEvent::Resync);
    assert_eq!(a.metrics.published_total(PublishOutcome::StoredAsResync), 1);
    let stored: String = sqlx::query_scalar("SELECT event::text FROM outbox ORDER BY id DESC LIMIT 1")
        .fetch_one(&a.pool)
        .await
        .unwrap();
    assert!(stored.len() < 100, "no path is stored: {stored}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_that_is_not_an_event_becomes_a_resync_for_subscribers() {
    let Some(cluster) = EventCluster::start(1, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let mut sub = a.bus.subscribe(EventFilter::all());
    sqlx::query("INSERT INTO outbox (event) VALUES ('{\"v\": 9, \"event\": {}}'::jsonb)")
        .execute(&a.pool)
        .await
        .unwrap();
    assert_eq!(next(&mut sub).await, DomainEvent::Resync);
    assert_eq!(a.metrics.resyncs_total(ResyncReason::Undecodable), 1);
    a.bus.publish(proposal(1)).await.unwrap();
    assert_eq!(next(&mut sub).await, proposal(1), "and the relay goes on");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_reads_the_log_by_event_id_within_the_window() {
    let Some(cluster) = EventCluster::start(2, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let source = cluster.replicas[1].bus.source();
    for n in 1..=4 {
        a.bus.publish(proposal(n)).await.unwrap();
    }
    let rows: Vec<i64> = sqlx::query_scalar("SELECT id FROM outbox ORDER BY id")
        .fetch_all(&a.pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);
    let id = |n: usize| EventId::new(rows[n]).unwrap();

    // Everything after the first event, on another replica than the one that published.
    let Replay::Events(events) = source.replay(id(0), &EventFilter::all(), 100).await.unwrap() else {
        panic!("expected events");
    };
    assert_eq!(
        events.iter().map(|(i, _)| i.get()).collect::<Vec<_>>(),
        vec![rows[1], rows[2], rows[3]]
    );
    assert_eq!(events[0].1, proposal(2));
    // A filter applies.
    let Replay::Events(none) = source
        .replay(
            id(0),
            &EventFilter::for_swimlane(SwimlaneId::parse("sit1").unwrap()),
            100,
        )
        .await
        .unwrap()
    else {
        panic!("expected events");
    };
    assert_eq!(none.len(), 3, "events with no swimlane pass a swimlane filter");
    // The newest id has nothing after it.
    assert_eq!(
        source.replay(id(3), &EventFilter::all(), 100).await.unwrap(),
        Replay::Events(vec![])
    );
    // More rows than the limit: the client refetches.
    assert_eq!(
        source.replay(id(0), &EventFilter::all(), 2).await.unwrap(),
        Replay::Resync
    );
    // An id that does not exist (never sent, or already pruned).
    assert_eq!(
        source
            .replay(EventId::new(rows[3] + 1000).unwrap(), &EventFilter::all(), 100)
            .await
            .unwrap(),
        Replay::Resync
    );
    // An id whose row is older than the 5 minute window.
    let mut admin = cluster.db.admin().await;
    sqlx::query("UPDATE outbox SET created_at = now() - interval '6 minutes' WHERE id = $1")
        .bind(rows[1])
        .execute(&mut admin)
        .await
        .unwrap();
    assert_eq!(
        source.replay(id(1), &EventFilter::all(), 100).await.unwrap(),
        Replay::Resync,
        "older than the window"
    );
    assert!(matches!(
        source.replay(id(2), &EventFilter::all(), 100).await.unwrap(),
        Replay::Events(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ports_event_bus_semantics_hold_for_the_outbox_bus_on_one_replica() {
    // The pieces of `ports::conformance::event_bus` that need no `TxFactory`: order, filters, no history for a new subscriber.
    let Some(cluster) = EventCluster::start(1, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let bus = &cluster.replicas[0].bus;
    let mut all = bus.subscribe(EventFilter::all());
    bus.publish(findings("sit1")).await.unwrap();
    bus.publish(proposal(1)).await.unwrap();
    assert_eq!(next(&mut all).await, findings("sit1"));
    assert_eq!(next(&mut all).await, proposal(1));

    let mut only_sit1 = bus.subscribe(EventFilter::for_swimlane(SwimlaneId::parse("sit1").unwrap()));
    bus.publish(findings("sit2")).await.unwrap();
    bus.publish(findings("sit1")).await.unwrap();
    assert_eq!(next(&mut only_sit1).await, findings("sit1"));
    quiet(&mut only_sit1, "sit2 is filtered out").await;

    let mut late = bus.subscribe(EventFilter::all());
    quiet(&mut late, "history is not replayed").await;
}
