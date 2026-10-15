//! T10: the transactional outbox (D80), on a real Postgres 16 as the app role.
//!
//! An event published inside a transaction is a row written on that transaction's connection, so it commits or rolls back with the
//! change; the table's trigger notifies at commit; the relay of every replica reads the row once and hands it to its subscribers.
//! These tests drive that path with real transactions on separate connections, commit orders the database chooses, and three
//! replicas (each with its own pool, `LISTEN` connection and relay). The relay's cursor on its own, without a database, is in
//! `events_relay_state.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use domain::{DomainEvent, ProposalId, ProposalStatus};
use futures::StreamExt;
use hub_platform::events::bus::BusSettings;
use hub_platform::events::metrics::EventMetrics;
use hub_platform::events::relay::RelaySettings;
use hub_platform::events::source::Replay;
use hub_platform::events::sse::SseSettings;
use hub_platform::events::wire::{self, EventId};
use hub_platform::jobs::outbox_prune::{
    DEFAULT_RETENTION, MIN_RETENTION, OutboxPrune, PRUNE_JOB, PruneConfigError, Pruned,
};
use hub_platform::jobs::{AcquireOutcome, JobContext, JobError, JobMetrics, JobOutcome, JobRunner};
use ports::fakes::{FakeLeases, FakeTxState};
use ports::{BusError, EventBus, EventFilter, Tx};
use sqlx::PgPool;
use support::event_cluster::{EventCluster, wait_for};
use support::pg::TestDb;
use support::pg_tx::PgTxFactory;

const WITHIN: Duration = Duration::from_secs(5);

fn proposal(n: i64) -> DomainEvent {
    DomainEvent::ProposalChanged {
        id: ProposalId::from(n),
        status: ProposalStatus::Pending,
    }
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

/// Nothing arrives for long enough that the relay polled more than ten times (and a notification would have been handled).
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

// ------------------------------------------------------------------------------------------- rollback and commit

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rolled_back_transaction_emits_no_event() {
    let Some(cluster) = EventCluster::start(2, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let (a, b) = (&cluster.replicas[0], &cluster.replicas[1]);
    let mut on_a = a.bus.subscribe(EventFilter::all());
    let mut on_b = b.bus.subscribe(EventFilter::all());

    // A change and its event in one transaction, which is then rolled back.
    let mut tx = a.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('rolled-back', 'Rolled back')")
        .execute(&mut *tx)
        .await
        .unwrap();
    a.bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(1))
        .await
        .unwrap();
    // Not visible to anyone else before the commit, and so notified to nobody.
    assert!(
        rows(&b.pool).await.is_empty(),
        "the row is invisible before the commit"
    );
    tx.rollback().await.unwrap();

    quiet(&mut on_a, "the transaction was rolled back (publisher's replica)").await;
    quiet(&mut on_b, "the transaction was rolled back (another replica)").await;
    assert!(
        rows(&a.pool).await.is_empty(),
        "the event row went with the change"
    );
    assert_eq!(
        cluster
            .db
            .count(&a.pool, "SELECT count(*) FROM projects WHERE id = 'rolled-back'")
            .await,
        0
    );
    assert_eq!(a.metrics.relayed_total(), 0);
    assert_eq!(b.metrics.relayed_total(), 0);

    // The bus is still alive, and the next event is the next thing anyone hears.
    b.bus.publish(proposal(2)).await.unwrap();
    assert_eq!(next(&mut on_a).await, proposal(2));
    assert_eq!(next(&mut on_b).await, proposal(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_commit_or_a_dropped_transaction_emits_no_event() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let mut sub = a.bus.subscribe(EventFilter::all());

    // The caller's error path drops the transaction without finishing it: sqlx rolls it back.
    {
        let mut tx = a.pool.begin().await.unwrap();
        a.bus
            .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(1))
            .await
            .unwrap();
    }
    quiet(&mut sub, "the transaction was dropped").await;
    assert!(rows(&a.pool).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_event_is_delivered_exactly_once_per_subscriber() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let mut subscribers: Vec<_> = (0..3).map(|_| a.bus.subscribe(EventFilter::all())).collect();

    // Two events and a change in one transaction.
    let mut tx = a.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('committed', 'Committed')")
        .execute(&mut *tx)
        .await
        .unwrap();
    a.bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(1))
        .await
        .unwrap();
    a.bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(2))
        .await
        .unwrap();
    for s in &mut subscribers {
        quiet(s, "the transaction is still open").await;
    }
    tx.commit().await.unwrap();

    for s in &mut subscribers {
        assert_eq!(next(s).await, proposal(1));
        assert_eq!(next(s).await, proposal(2), "in the order they were written");
    }
    for s in &mut subscribers {
        quiet(s, "each event is delivered once, however often the relay polls").await;
    }
    assert_eq!(a.metrics.relayed_total(), 2, "the relay read each row once");
    assert_eq!(rows(&a.pool).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_hubs_committed_event_delivered_once_per_subscriber() {
    let Some(cluster) = EventCluster::start(3, fast_relay(), SseSettings::default()).await else {
        return;
    };
    // Two subscribers on each replica.
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

    // Committed on replica 1, heard on all three.
    let publisher = &cluster.replicas[1];
    let mut tx = publisher.pool.begin().await.unwrap();
    publisher
        .bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(7))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for s in &mut subscribers {
        assert_eq!(next(s).await, proposal(7));
    }
    for s in &mut subscribers {
        quiet(s, "once per subscriber, on every replica").await;
    }
    for r in &cluster.replicas {
        assert_eq!(r.metrics.relayed_total(), 1, "{}: one row read once", r.name);
    }
}

// ---------------------------------------------------------------------------------------------- commit order

/// Ids come from a sequence in the order the inserts run, but commits happen in any order. A transaction that took the lower id and
/// commits after one with a higher id must still be delivered: a cursor that only moves forward would skip it for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_commit_with_lower_id_is_not_skipped() {
    let Some(cluster) = EventCluster::start(2, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let (a, b) = (&cluster.replicas[0], &cluster.replicas[1]);
    let mut subscribers = vec![
        a.bus.subscribe(EventFilter::all()),
        b.bus.subscribe(EventFilter::all()),
    ];

    // `first` takes the lower id and stays open; `second` takes the higher id and commits.
    let mut first = a.pool.begin().await.unwrap();
    a.bus
        .publish_in_tx(&mut Tx::from_pg(&mut first), proposal(1))
        .await
        .unwrap();
    let mut second = b.pool.begin().await.unwrap();
    b.bus
        .publish_in_tx(&mut Tx::from_pg(&mut second), proposal(2))
        .await
        .unwrap();
    second.commit().await.unwrap();
    for s in &mut subscribers {
        assert_eq!(
            next(s).await,
            proposal(2),
            "the committed one, ahead of the open one"
        );
    }
    // Several polls pass with the gap open and nothing in it.
    for s in &mut subscribers {
        quiet(s, "the lower id has not committed").await;
    }

    first.commit().await.unwrap();
    for s in &mut subscribers {
        assert_eq!(
            next(s).await,
            proposal(1),
            "the lower id committed late and is delivered"
        );
    }
    for s in &mut subscribers {
        quiet(s, "and only once").await;
    }
    let stored = rows(&a.pool).await;
    assert_eq!(stored.len(), 2);
    let decoded: Vec<DomainEvent> = stored
        .iter()
        .map(|(_, text)| wire::decode(text, wire::MAX_EVENT_BYTES * 2).unwrap())
        .collect();
    assert_eq!(
        decoded,
        vec![proposal(1), proposal(2)],
        "the first insert has the lower id, though it committed last"
    );
    for r in &cluster.replicas {
        assert_eq!(r.metrics.relayed_total(), 2, "{}: each row read once", r.name);
    }
}

/// Forty transactions on two replicas, each held open for a random few milliseconds between its insert and its end, so that
/// they commit in an order unrelated to their ids; every fourth is rolled back. Every subscriber hears each committed event
/// once, and no rolled-back one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commits_in_any_order_are_each_heard_once_and_rollbacks_never() {
    let Some(cluster) = EventCluster::start(2, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let mut subscribers: Vec<_> = cluster
        .replicas
        .iter()
        .map(|r| r.bus.subscribe(EventFilter::all()))
        .collect();

    let mut tasks = Vec::new();
    for n in 0..40_i64 {
        let replica = &cluster.replicas[usize::try_from(n % 2).unwrap()];
        let (pool, bus) = (replica.pool.clone(), replica.bus.clone());
        tasks.push(tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            bus.publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(n))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(fastrand::u64(0..30))).await;
            if n % 4 == 0 {
                tx.rollback().await.unwrap();
                None
            } else {
                tx.commit().await.unwrap();
                Some(n)
            }
        }));
    }
    let mut committed = BTreeSet::new();
    for t in tasks {
        if let Some(n) = t.await.unwrap() {
            committed.insert(n);
        }
    }
    assert_eq!(committed.len(), 30);

    for s in &mut subscribers {
        let mut heard = Vec::new();
        for _ in 0..committed.len() {
            match next(s).await {
                DomainEvent::ProposalChanged { id, .. } => heard.push(id.get()),
                other => panic!("unexpected {other:?}"),
            }
        }
        quiet(s, "no duplicate and no rolled-back event follows").await;
        let unique: BTreeSet<i64> = heard.iter().copied().collect();
        assert_eq!(unique.len(), heard.len(), "no event twice: {heard:?}");
        assert_eq!(unique, committed, "exactly the committed events");
    }
}

// ------------------------------------------------------------------------------------------------ wrong kind of Tx

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_in_tx_refuses_a_transaction_that_is_not_postgres() {
    let Some(cluster) = EventCluster::start(1, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let a = &cluster.replicas[0];
    let mut state = FakeTxState::new();
    assert_eq!(
        a.bus
            .publish_in_tx(&mut Tx::in_memory(&mut state), proposal(1))
            .await,
        Err(BusError::WrongTx),
        "a wiring bug fails loudly and writes nothing"
    );
    assert!(rows(&a.pool).await.is_empty());
}

// ------------------------------------------------------------------------------------------------- conformance

/// `ports::conformance::event_bus` is the suite every `EventBus` must pass; this runs it against the outbox bus, with
/// transactions on a real Postgres.
///
/// The suite's lag section publishes a burst and then expects the first thing a never-polled subscriber reads to be a `Resync`.
/// That holds for a bus that delivers while `publish` runs. The outbox relay delivers a moment after the insert, so this harness
/// lets each plain `publish` return only once this replica's relay has read the row; nothing else about the bus is changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ports_conformance_event_bus_holds_for_the_outbox_bus_with_a_postgres_tx_factory() {
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
    let bus = Settled {
        inner: replica.bus.clone(),
        metrics: replica.metrics.clone(),
    };
    let txf = PgTxFactory::new(replica.pool.clone());
    ports::conformance::event_bus(&bus, &txf, BUFFER).await;
}

/// The outbox bus with `publish` that waits for this replica's relay to have read as many rows as were written.
struct Settled {
    inner: hub_platform::events::bus::PgEventBus,
    metrics: EventMetrics,
}

#[async_trait::async_trait]
impl EventBus for Settled {
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError> {
        let before = self.metrics.relayed_total();
        self.inner.publish(e).await?;
        let deadline = tokio::time::Instant::now() + WITHIN;
        while self.metrics.relayed_total() <= before {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the relay did not read the row"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        self.inner.publish_in_tx(tx, e).await
    }

    /// Forwarded, so that the conformance suite tests the outbox bus's batching and not the default.
    async fn publish_many_in_tx(&self, tx: &mut Tx<'_>, events: Vec<DomainEvent>) -> Result<(), BusError> {
        self.inner.publish_many_in_tx(tx, events).await
    }

    fn subscribe(&self, f: EventFilter) -> futures::stream::BoxStream<'static, DomainEvent> {
        self.inner.subscribe(f)
    }
}

// ------------------------------------------------------------------------------------------------------ the prune

const OLD_ROWS: &str = "INSERT INTO outbox (event, created_at) \
     SELECT '{}'::jsonb, now() - interval '61 minutes' - g * interval '1 second' FROM generate_series(1, $1) g";
const YOUNG_ROWS: &str = "INSERT INTO outbox (event, created_at) \
     SELECT '{}'::jsonb, now() - interval '59 minutes' + g * interval '1 second' FROM generate_series(1, $1) g";

async fn seed(pool: &PgPool, old: i32, young: i32) {
    sqlx::query(OLD_ROWS).bind(old).execute(pool).await.unwrap();
    sqlx::query(YOUNG_ROWS).bind(young).execute(pool).await.unwrap();
}

fn prune_job(pool: PgPool) -> (OutboxPrune, EventMetrics) {
    let metrics = EventMetrics::default();
    (OutboxPrune::new(pool, metrics.clone()), metrics)
}

fn held() -> (JobContext, tokio::sync::watch::Sender<tokio::time::Instant>) {
    JobContext::held_until_told_otherwise(PRUNE_JOB)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbox_prune_deletes_rows_older_than_1h_in_batches() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    // 25 rows 61 minutes old or more, and 7 that are 59 minutes old or less.
    seed(&pool, 25, 7).await;
    let (job, metrics) = prune_job(pool.clone());
    let job = job.with_batch(10).unwrap();
    let (ctx, _hold) = held();

    let done = job.prune(&ctx).await.unwrap();
    assert_eq!(done, Pruned { rows: 25, batches: 3 }, "10, 10 and 5");
    assert_eq!(db.count(&pool, "SELECT count(*) FROM outbox").await, 7);
    assert_eq!(
        db.count(
            &pool,
            "SELECT count(*) FROM outbox WHERE created_at < now() - interval '1 hour'"
        )
        .await,
        0,
        "nothing older than an hour is left, and nothing younger was touched"
    );
    assert_eq!(metrics.pruned_total(), 25);

    // Idempotent: a second run finds nothing, in one statement.
    assert_eq!(job.prune(&ctx).await.unwrap(), Pruned { rows: 0, batches: 1 });
    assert_eq!(DEFAULT_RETENTION, Duration::from_secs(3600));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbox_prune_does_at_most_its_batch_limit_per_run_and_the_next_run_goes_on() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    seed(&pool, 25, 0).await;
    let (job, _) = prune_job(pool.clone());
    let job = job.with_batch(10).unwrap().with_max_batches(2).unwrap();
    let (ctx, _hold) = held();

    assert_eq!(job.prune(&ctx).await.unwrap(), Pruned { rows: 20, batches: 2 });
    assert_eq!(db.count(&pool, "SELECT count(*) FROM outbox").await, 5);
    assert_eq!(job.prune(&ctx).await.unwrap(), Pruned { rows: 5, batches: 1 });
    assert_eq!(db.count(&pool, "SELECT count(*) FROM outbox").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbox_prune_stops_when_the_lease_is_lost() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    seed(&pool, 25, 0).await;
    let (job, _) = prune_job(pool.clone());
    let job = job.with_batch(10).unwrap();
    let (ctx, hold) = held();
    hold.send_replace(tokio::time::Instant::now() - Duration::from_secs(1));

    assert_eq!(job.prune(&ctx).await, Err(JobError::LeaseLost));
    assert_eq!(
        db.count(&pool, "SELECT count(*) FROM outbox").await,
        25,
        "nothing is deleted without the lease"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbox_prune_refuses_a_retention_the_replay_window_could_outlive() {
    let Some(db) = TestDb::ready().await else { return };
    let pool = db.app_pool().await;
    let (job, _) = prune_job(pool);
    // SSE replay reads the log for five minutes and the relay waits ten seconds for a late commit: rows must outlive both.
    assert!(MIN_RETENTION >= Duration::from_secs(10 * 60));
    assert_eq!(
        job.clone().with_retention(Duration::from_secs(60)).err(),
        Some(PruneConfigError::RetentionTooShort)
    );
    assert!(job.clone().with_retention(MIN_RETENTION).is_ok());
    assert_eq!(job.clone().with_batch(0).err(), Some(PruneConfigError::BadLimit));
    assert_eq!(job.with_max_batches(0).err(), Some(PruneConfigError::BadLimit));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbox_prune_leaves_the_live_relay_and_replay_working() {
    let Some(cluster) = EventCluster::start(2, fast_relay(), SseSettings::default()).await else {
        return;
    };
    let (a, b) = (&cluster.replicas[0], &cluster.replicas[1]);
    let mut on_a = a.bus.subscribe(EventFilter::all());
    let mut on_b = b.bus.subscribe(EventFilter::all());
    for n in 1..=3 {
        a.bus.publish(proposal(n)).await.unwrap();
    }
    for _ in 1..=3 {
        next(&mut on_a).await;
        next(&mut on_b).await;
    }
    let old: Vec<i64> = rows(&a.pool).await.into_iter().map(|(id, _)| id).collect();
    // The three events are now two hours old.
    let mut admin = cluster.db.admin().await;
    sqlx::query("UPDATE outbox SET created_at = now() - interval '2 hours'")
        .execute(&mut admin)
        .await
        .unwrap();

    let (job, _) = prune_job(a.pool.clone());
    let (ctx, _hold) = held();
    assert_eq!(job.prune(&ctx).await.unwrap().rows, 3);

    // New events still reach every subscriber, once.
    b.bus.publish(proposal(4)).await.unwrap();
    assert_eq!(next(&mut on_a).await, proposal(4));
    assert_eq!(next(&mut on_b).await, proposal(4));
    quiet(&mut on_a, "once").await;
    quiet(&mut on_b, "once").await;

    // A client that last saw a pruned event is told to refetch; one that saw the new event is not.
    let source = b.bus.source();
    let pruned = EventId::new(old[0]).unwrap();
    assert_eq!(
        source.replay(pruned, &EventFilter::all(), 100).await.unwrap(),
        Replay::Resync
    );
    let newest = EventId::new(rows(&a.pool).await[0].0).unwrap();
    assert_eq!(
        source.replay(newest, &EventFilter::all(), 100).await.unwrap(),
        Replay::Events(vec![])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_prune_job_runs_under_the_leased_runner_on_one_replica() {
    let Some(db) = TestDb::ready().await else { return };
    seed(&db.app_pool().await, 12, 3).await;
    let leases = FakeLeases::new();
    let job_metrics: Vec<JobMetrics> = (0..2).map(|_| JobMetrics::default()).collect();
    let mut handles = Vec::new();
    let mut event_metrics = Vec::new();
    for jm in &job_metrics {
        let pool = db.app_pool().await;
        let (job, em) = prune_job(pool);
        event_metrics.push(em);
        let mut runner = JobRunner::new(Arc::new(leases.clone()), jm.clone());
        runner
            .register(
                OutboxPrune::spec_with(Duration::from_millis(100), Duration::from_millis(900)).unwrap(),
                Arc::new(job),
            )
            .unwrap();
        handles.push(runner.start());
    }
    let pool = db.app_pool().await;
    wait_for("the old rows to be pruned", Duration::from_secs(10), async || {
        db.count(&pool, "SELECT count(*) FROM outbox").await == 3
    })
    .await;

    let holders = job_metrics
        .iter()
        .filter(|m| m.acquires_total(PRUNE_JOB, AcquireOutcome::Acquired) > 0)
        .count();
    assert_eq!(holders, 1, "one replica holds the lease and prunes");
    let pruned: u64 = event_metrics.iter().map(EventMetrics::pruned_total).sum();
    assert_eq!(pruned, 12);
    assert!(
        job_metrics
            .iter()
            .any(|m| m.runs_total(PRUNE_JOB, JobOutcome::Ok) >= 1)
    );
    for h in handles {
        h.shutdown().await;
    }
}

#[test]
fn the_default_prune_spec_is_a_valid_job_spec_named_for_its_lease() {
    let spec = OutboxPrune::spec().unwrap();
    assert_eq!(spec.name(), PRUNE_JOB);
}
