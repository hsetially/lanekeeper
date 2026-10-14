//! `EventBus::publish_many_in_tx` over the Postgres outbox (D80, P15), on a real Postgres 16 as the app role.
//!
//! The registry's replay ingestion publishes the events of a message of 50 versions in one transaction; one insert per event was 12 %
//! of its time. A batch is one `INSERT` of as many outbox rows as events, in the order of the batch, in the caller's transaction, so
//! it commits or rolls back with the change, and the rows are ordinary outbox rows: the relay reads each once, in id order.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::print_stdout
)]

mod support;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use domain::{DomainEvent, NfsPath, ProposalId, ProposalStatus, SwimlaneId};
use futures::StreamExt;
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::{EventMetrics, PublishOutcome};
use hub_platform::events::relay::RelaySettings;
use hub_platform::events::sse::SseSettings;
use hub_platform::events::wire;
use ports::fakes::FakeTxState;
use ports::{BusError, EventBus, EventFilter, MAX_EVENT_BATCH, Tx};
use sqlx::PgPool;
use support::event_cluster::EventCluster;
use support::log_capture::LogCapture;
use support::pg::TestDb;
use support::pg_tx::PgTxFactory;

const WITHIN: Duration = Duration::from_secs(5);

fn proposal(n: i64) -> DomainEvent {
    DomainEvent::ProposalChanged {
        id: ProposalId::from(n),
        status: ProposalStatus::Pending,
    }
}

fn proposals(from: i64, n: usize) -> Vec<DomainEvent> {
    (0..n as i64).map(|i| proposal(from + i)).collect()
}

/// A relay that polls often, so that "nothing more arrives" is checked over many polls and not one.
fn fast_relay() -> BusSettings {
    BusSettings {
        relay: RelaySettings {
            tick: Duration::from_millis(50),
            ..RelaySettings::default()
        },
        ..BusSettings::default()
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
        tokio::time::timeout(Duration::from_millis(600), s.next())
            .await
            .is_err(),
        "unexpected event: {why}"
    );
}

async fn rows(pool: &PgPool) -> Vec<(i64, String)> {
    sqlx::query_as("SELECT id, event::text FROM outbox ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

fn decode(text: &str) -> DomainEvent {
    wire::decode(text, wire::MAX_EVENT_BYTES * 2).unwrap()
}

// -------------------------------------------------------------------------------------------------------------------
// The port's contract

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ports_conformance_batch_suite_holds_for_the_outbox_bus() {
    const BUFFER: usize = 8;
    let settings = BusSettings {
        capacity: BUFFER,
        relay: RelaySettings {
            batch: 4,
            tick: Duration::from_millis(50),
            ..RelaySettings::default()
        },
        ..BusSettings::default()
    };
    let Some(cluster) = EventCluster::start(1, settings, SseSettings::default()).await else {
        return;
    };
    let replica = &cluster.replicas[0];
    let txf = PgTxFactory::new(replica.pool.clone());
    ports::conformance::event_bus_batch(&replica.bus, &txf).await;
}

// -------------------------------------------------------------------------------------------------------------------
// The point of the method: one insert for the batch, not one for every event

#[tokio::test]
async fn a_batch_is_one_insert() {
    let (capture, _guard) = LogCapture::install();
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let bus = PgEventBus::new(pool.clone(), BusSettings::default(), EventMetrics::default());

    for n in [1_usize, 2, 50, MAX_EVENT_BATCH] {
        let mut tx = pool.begin().await.unwrap();
        let before = capture.text().len();
        bus.publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(0, n))
            .await
            .unwrap();
        let statements: Vec<String> = capture.text()[before..]
            .lines()
            .filter(|l| l.contains("sqlx::query"))
            .map(str::to_owned)
            .collect();
        tx.commit().await.unwrap();
        assert_eq!(
            statements.len(),
            1,
            "{n} events took {} statements: {statements:#?}",
            statements.len()
        );
        assert!(statements[0].contains("INSERT INTO outbox"), "{}", statements[0]);
    }
    assert_eq!(rows(&pool).await.len(), 1 + 2 + 50 + MAX_EVENT_BATCH);
}

// -------------------------------------------------------------------------------------------------------------------
// The rows: order, bounds, atomicity

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rows_follow_the_order_of_the_calls_in_the_transaction() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let mut tx = a.pool.begin().await.unwrap();
    let mut tx_ref = Tx::from_pg(&mut tx);
    a.bus.publish_in_tx(&mut tx_ref, proposal(1)).await.unwrap();
    a.bus
        .publish_many_in_tx(&mut tx_ref, proposals(2, 3))
        .await
        .unwrap();
    a.bus.publish_in_tx(&mut tx_ref, proposal(5)).await.unwrap();
    a.bus
        .publish_many_in_tx(&mut tx_ref, proposals(6, 2))
        .await
        .unwrap();
    a.bus.publish_in_tx(&mut tx_ref, proposal(8)).await.unwrap();
    tx.commit().await.unwrap();

    let stored = rows(&a.pool).await;
    assert!(
        stored.windows(2).all(|w| w[0].0 < w[1].0),
        "ids increase: {stored:?}"
    );
    assert_eq!(
        stored.iter().map(|(_, text)| decode(text)).collect::<Vec<_>>(),
        proposals(1, 8),
        "by id, the events are in the order they were published in"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_over_the_size_cap_is_stored_as_a_resync_at_its_place_as_for_a_single_publish() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let big = DomainEvent::DriftChanged {
        swimlane: SwimlaneId::parse("sit1").unwrap(),
        paths: (0..400)
            .map(|i| NfsPath::parse(&format!("service-{i}/config-file-{i}.yml")).unwrap())
            .collect(),
    };
    let mut sub = a.bus.subscribe(EventFilter::all());

    let mut tx = a.pool.begin().await.unwrap();
    a.bus
        .publish_many_in_tx(
            &mut Tx::from_pg(&mut tx),
            vec![proposal(1), big.clone(), proposal(2)],
        )
        .await
        .unwrap();
    a.bus.publish_in_tx(&mut Tx::from_pg(&mut tx), big).await.unwrap();
    tx.commit().await.unwrap();

    let stored = rows(&a.pool).await;
    assert_eq!(
        stored.iter().map(|(_, text)| decode(text)).collect::<Vec<_>>(),
        [proposal(1), DomainEvent::Resync, proposal(2), DomainEvent::Resync],
        "the big event is a Resync where it was, in the batch and published singly; its neighbours are intact"
    );
    assert_eq!(
        stored[1].1, stored[3].1,
        "the same stored text as `publish_in_tx` gives"
    );
    assert!(stored[1].1.len() < 100, "no path is stored: {}", stored[1].1);
    for want in [proposal(1), DomainEvent::Resync, proposal(2), DomainEvent::Resync] {
        assert_eq!(next(&mut sub).await, want);
    }
    assert_eq!(a.metrics.published_total(PublishOutcome::Stored), 2);
    assert_eq!(a.metrics.published_total(PublishOutcome::StoredAsResync), 2);
    assert_eq!(a.metrics.published_total(PublishOutcome::Failed), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rolled_back_batch_emits_nothing_and_leaves_no_row() {
    let Some(cluster) = EventCluster::start(2, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let (a, b) = (&cluster.replicas[0], &cluster.replicas[1]);
    let mut on_a = a.bus.subscribe(EventFilter::all());
    let mut on_b = b.bus.subscribe(EventFilter::all());

    let mut tx = a.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('rolled-back', 'Rolled back')")
        .execute(&mut *tx)
        .await
        .unwrap();
    a.bus
        .publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(1, 50))
        .await
        .unwrap();
    assert!(rows(&b.pool).await.is_empty(), "invisible before the commit");
    tx.rollback().await.unwrap();

    quiet(&mut on_a, "rolled back (the publisher's replica)").await;
    quiet(&mut on_b, "rolled back (another replica)").await;
    assert!(rows(&a.pool).await.is_empty(), "the rows went with the change");
    assert_eq!(a.metrics.relayed_total(), 0);
    assert_eq!(b.metrics.relayed_total(), 0);
    b.bus.publish(proposal(99)).await.unwrap();
    assert_eq!(next(&mut on_a).await, proposal(99), "the next thing anyone hears");
    assert_eq!(next(&mut on_b).await, proposal(99));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_transaction_emits_nothing() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let mut sub = a.bus.subscribe(EventFilter::all());
    {
        let mut tx = a.pool.begin().await.unwrap();
        a.bus
            .publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(1, 5))
            .await
            .unwrap();
    }
    quiet(&mut sub, "the transaction was dropped").await;
    assert!(rows(&a.pool).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_database_error_fails_the_whole_batch_and_writes_nothing() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    // Someone holds the table: the insert cannot get its lock within the transaction's timeout, so the statement fails.
    let mut admin = cluster.db.admin().await;
    sqlx::query("BEGIN").execute(&mut admin).await.unwrap();
    sqlx::query("LOCK TABLE outbox IN ACCESS EXCLUSIVE MODE")
        .execute(&mut admin)
        .await
        .unwrap();

    let mut tx = a.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '200ms'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let result = a
        .bus
        .publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(1, 5))
        .await;
    assert_eq!(result, Err(BusError::Unavailable));
    assert_eq!(
        a.metrics.published_total(PublishOutcome::Failed),
        5,
        "none of the five was written"
    );
    assert_eq!(a.metrics.published_total(PublishOutcome::Stored), 0);
    // The transaction is aborted, as after a failed `publish_in_tx`.
    assert!(sqlx::query("SELECT 1").execute(&mut *tx).await.is_err());
    tx.rollback().await.unwrap();
    sqlx::query("ROLLBACK").execute(&mut admin).await.unwrap();
    assert!(rows(&a.pool).await.is_empty());
}

// -------------------------------------------------------------------------------------------------------------------
// Delivery

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_batch_is_delivered_once_per_subscriber_on_every_replica_in_order() {
    let Some(cluster) = EventCluster::start(3, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let mut subscribers: Vec<_> = cluster
        .replicas
        .iter()
        .flat_map(|r| {
            [
                r.bus.subscribe(EventFilter::all()),
                r.bus.subscribe(EventFilter::all()),
            ]
        })
        .collect();

    // A batch, with a change and a single publish in one transaction, committed on replica 1.
    let publisher = &cluster.replicas[1];
    let mut tx = publisher.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('batch', 'Batch')")
        .execute(&mut *tx)
        .await
        .unwrap();
    publisher
        .bus
        .publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(1, 30))
        .await
        .unwrap();
    publisher
        .bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(31))
        .await
        .unwrap();
    for s in &mut subscribers {
        quiet(s, "the transaction is still open").await;
    }
    tx.commit().await.unwrap();

    for s in &mut subscribers {
        for want in proposals(1, 31) {
            assert_eq!(next(s).await, want, "in the order of the batch");
        }
    }
    for s in &mut subscribers {
        quiet(s, "each event once, however many notifications the rows sent").await;
    }
    for r in &cluster.replicas {
        assert_eq!(r.metrics.relayed_total(), 31, "{}: each row read once", r.name);
    }
}

/// Batches of 1 to 40 events from writers on two replicas, committing in an order that has nothing to do with their ids, some of them
/// rolled back. Every subscriber hears the events of each committed batch in the order of the batch, each once, and nothing from a
/// rolled-back one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_batches_are_each_heard_once_in_order_and_rollbacks_never() {
    let Some(cluster) = EventCluster::start(2, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let mut subscribers: Vec<_> = cluster
        .replicas
        .iter()
        .map(|r| r.bus.subscribe(EventFilter::all()))
        .collect();

    let mut tasks = Vec::new();
    for writer in 0..16_i64 {
        let replica = &cluster.replicas[usize::try_from(writer % 2).unwrap()];
        let (pool, bus) = (replica.pool.clone(), replica.bus.clone());
        tasks.push(tokio::spawn(async move {
            let mut committed = Vec::new();
            for round in 0..4_i64 {
                let size = 1 + ((writer * 5 + round * 11) % 40) as usize;
                let base = writer * 10_000 + round * 100;
                let mut tx = pool.begin().await.unwrap();
                bus.publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(base, size))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(fastrand::u64(0..20))).await;
                if (writer + round) % 4 == 0 {
                    tx.rollback().await.unwrap();
                } else {
                    tx.commit().await.unwrap();
                    committed.push((base, size));
                }
            }
            committed
        }));
    }
    let mut batches = Vec::new();
    for t in tasks {
        batches.extend(t.await.unwrap());
    }
    let total: usize = batches.iter().map(|(_, n)| *n).sum();

    // What is stored: the rows of each batch increase in the order of the batch, whatever else is between them.
    let stored: Vec<(i64, i64)> = rows(&cluster.replicas[0].pool)
        .await
        .iter()
        .map(|(id, text)| {
            let DomainEvent::ProposalChanged { id: event, .. } = decode(text) else {
                panic!("unexpected row");
            };
            (*id, event.get())
        })
        .collect();
    assert_eq!(
        stored.len(),
        total,
        "a row for each committed event, none for a rolled-back one"
    );
    let expected: BTreeSet<i64> = batches
        .iter()
        .flat_map(|(base, n)| (0..*n as i64).map(move |i| base + i))
        .collect();
    assert_eq!(stored.iter().map(|(_, e)| *e).collect::<BTreeSet<_>>(), expected);
    // A batch whose rows no other row falls between: one statement drew its ids one after the other.
    let mut contiguous = Vec::new();
    for (base, n) in &batches {
        let ids: Vec<i64> = stored
            .iter()
            .filter(|(_, e)| (*base..*base + *n as i64).contains(e))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(
            ids.iter().copied().collect::<BTreeSet<_>>().len(),
            *n,
            "the batch at {base}: a row for each event"
        );
        let in_input_order: Vec<i64> = (0..*n as i64)
            .map(|i| stored.iter().find(|(_, e)| *e == base + i).unwrap().0)
            .collect();
        assert!(
            in_input_order.windows(2).all(|w| w[0] < w[1]),
            "the batch at {base}: ids increase in the order of the batch"
        );
        let (lowest, highest) = (in_input_order[0], in_input_order[n - 1]);
        if highest - lowest + 1 == *n as i64 {
            contiguous.push((*base, *n));
        }
    }

    for s in &mut subscribers {
        let mut heard = Vec::new();
        for _ in 0..total {
            let DomainEvent::ProposalChanged { id, .. } = next(s).await else {
                panic!("unexpected event");
            };
            heard.push(id.get());
        }
        quiet(s, "no duplicate and no rolled-back event follows").await;
        let unique: BTreeSet<i64> = heard.iter().copied().collect();
        assert_eq!(unique.len(), heard.len(), "no event twice");
        assert_eq!(unique, expected, "exactly the committed events");
        // A batch with consecutive ids became visible in one piece and is read in id order.
        for (base, n) in &contiguous {
            let of_batch: Vec<i64> = heard
                .iter()
                .copied()
                .filter(|id| (*base..*base + *n as i64).contains(id))
                .collect();
            assert_eq!(
                of_batch,
                (0..*n as i64).map(|i| base + i).collect::<Vec<_>>(),
                "the batch at {base}, in its order"
            );
        }
        assert!(
            !contiguous.is_empty(),
            "most batches are consecutive, so this checks something"
        );
    }
}

// -------------------------------------------------------------------------------------------------------------------
// Before the database

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_oversized_or_foreign_batch_writes_nothing() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];

    // Empty: a no-op, whatever the transaction is.
    let mut state = FakeTxState::new();
    assert_eq!(
        a.bus
            .publish_many_in_tx(&mut Tx::in_memory(&mut state), Vec::new())
            .await,
        Ok(())
    );
    // More than the limit: refused whole, before the database is touched, so even a transaction of another store gets this answer.
    assert_eq!(
        a.bus
            .publish_many_in_tx(&mut Tx::in_memory(&mut state), proposals(0, MAX_EVENT_BATCH + 1))
            .await,
        Err(BusError::BatchTooLarge)
    );
    let mut tx = a.pool.begin().await.unwrap();
    assert_eq!(
        a.bus
            .publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(0, MAX_EVENT_BATCH + 1))
            .await,
        Err(BusError::BatchTooLarge)
    );
    // The transaction is untouched.
    sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    // A transaction of another store is a wiring bug, and writes nothing.
    assert_eq!(
        a.bus
            .publish_many_in_tx(&mut Tx::in_memory(&mut state), proposals(0, 3))
            .await,
        Err(BusError::WrongTx)
    );
    assert!(rows(&a.pool).await.is_empty());
    assert_eq!(a.metrics.published_total(PublishOutcome::Stored), 0);
    assert_eq!(a.metrics.published_total(PublishOutcome::Failed), 0);

    // The limit itself is accepted and stored in full.
    let mut tx = a.pool.begin().await.unwrap();
    a.bus
        .publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(0, MAX_EVENT_BATCH))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rows(&a.pool).await.len(), MAX_EVENT_BATCH);
    assert_eq!(
        a.metrics.published_total(PublishOutcome::Stored),
        MAX_EVENT_BATCH as u64
    );
}

// -------------------------------------------------------------------------------------------------------------------
// Where the time goes: not a test of speed, a report of it.
// `cargo test --release -p hub-platform --test outbox_batch -- --ignored --nocapture`

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
async fn report_the_time_of_a_batch_against_the_same_events_one_by_one() {
    let Some(cluster) = EventCluster::start(1, BusSettings::default(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    println!(
        "\nevents | publish_in_tx x n (ms) | publish_many_in_tx (ms) | speed-up   (one transaction: BEGIN, the publishes, COMMIT; median of 15)"
    );
    for n in [1_usize, 10, 50, 100, MAX_EVENT_BATCH] {
        let (mut one_by_one, mut batched) = (Vec::new(), Vec::new());
        for round in 0..15_i64 {
            let started = Instant::now();
            let mut tx = a.pool.begin().await.unwrap();
            for e in proposals(round * 1_000, n) {
                a.bus.publish_in_tx(&mut Tx::from_pg(&mut tx), e).await.unwrap();
            }
            tx.commit().await.unwrap();
            one_by_one.push(started.elapsed());

            let started = Instant::now();
            let mut tx = a.pool.begin().await.unwrap();
            a.bus
                .publish_many_in_tx(&mut Tx::from_pg(&mut tx), proposals(round * 1_000 + 500, n))
                .await
                .unwrap();
            tx.commit().await.unwrap();
            batched.push(started.elapsed());
        }
        let (x, y) = (median(one_by_one), median(batched));
        println!(
            "{n:>6} | {:>22.2} | {:>23.2} | {:>8.1}x",
            x.as_secs_f64() * 1000.0,
            y.as_secs_f64() * 1000.0,
            x.as_secs_f64() / y.as_secs_f64()
        );
        if n >= 10 {
            assert!(
                y < x,
                "a batch of {n} is not faster than {n} calls: {y:?} against {x:?}"
            );
        }
    }
}
