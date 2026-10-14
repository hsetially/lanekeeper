//! `PgAuditLog::record_many` over a real Postgres, as `lanekeeper_app` (S9, P15): one lock, one head read and one insert, and the
//! chain it writes is the chain `record` writes. The registry's replay ingestion audits every path of a message of 50 versions
//! in one transaction, and the three round trips of `record` per event were 43 % of its time.
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

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use domain::{AuditId, Timestamp};
use hub_identity::audit::verify::NightlyOutcome;
use hub_identity::audit::{CHAIN_LOCK_KEY, PgAuditLog};
use ports::{AuditError, AuditEvent, AuditLog, MAX_AUDIT_BATCH, Tx, genesis_hash, verify_chain};
use sqlx::PgPool;
use support::audit_rig::{AuditRig, bare_event, event, full_event};
use support::log_capture;
use support::pg::TestDb;
use support::pg_tx::{PgAuditProbe, PgTxFactory, committed};

/// `n` events with distinct times, alternating between a bare event and one with every optional field.
fn events(from: i64, n: usize) -> Vec<AuditEvent> {
    (0..n as i64)
        .map(|i| {
            if i % 2 == 0 {
                bare_event(from + i)
            } else {
                full_event(from + i)
            }
        })
        .collect()
}

async fn many(pool: &PgPool, batch: Vec<AuditEvent>) -> Vec<AuditId> {
    let mut tx = pool.begin().await.unwrap();
    let ids = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), batch)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    ids
}

async fn single(pool: &PgPool, e: AuditEvent) -> AuditId {
    let mut tx = pool.begin().await.unwrap();
    let id = PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), e)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

/// A connection that holds the chain lock until it is rolled back, as another replica's audited transaction does.
async fn hold_chain_lock(pool: &PgPool) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CHAIN_LOCK_KEY)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder
}

async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(pool)
        .await
        .unwrap()
}

// -------------------------------------------------------------------------------------------------------------------
// The port's contract

#[tokio::test]
async fn ports_conformance_batch_suite_passes() {
    let db = db!();
    let pool = db.app_pool(4).await;
    let probe = PgAuditProbe {
        log: PgAuditLog::new(),
        pool: pool.clone(),
    };
    ports::conformance::audit_log_batch(&probe, &PgTxFactory(pool)).await;
}

// -------------------------------------------------------------------------------------------------------------------
// The point of the method: three round trips for the batch, not for every event

/// The statements `sqlx` logged since `from`, one per line of the capture.
fn statements(capture: &log_capture::Capture, from: usize) -> Vec<String> {
    capture.text()[from..]
        .lines()
        .filter(|l| l.contains("sqlx::query"))
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn a_batch_is_one_lock_one_head_read_and_one_insert() {
    let (capture, _guard) = log_capture::install();
    let db = db!();
    let pool = db.app_pool(4).await;
    single(&pool, event(0)).await;

    for n in [1_usize, 2, 50, MAX_AUDIT_BATCH] {
        let mut tx = pool.begin().await.unwrap();
        let from = capture.text().len();
        let ids = PgAuditLog::new()
            .record_many(&mut Tx::from_pg(&mut tx), events(1_000 * n as i64, n))
            .await
            .unwrap();
        let seen = statements(&capture, from);
        tx.commit().await.unwrap();
        assert_eq!(ids.len(), n);
        assert_eq!(
            seen.len(),
            3,
            "{n} events took {} statements, not a lock, a head read and an insert: {seen:#?}",
            seen.len()
        );
        assert!(
            seen[0].contains("pg_advisory_xact_lock"),
            "the lock first: {}",
            seen[0]
        );
        assert!(
            seen[1].contains("SELECT hash FROM audit_events"),
            "then the head: {}",
            seen[1]
        );
        assert!(
            seen[2].contains("INSERT INTO audit_events"),
            "then the rows: {}",
            seen[2]
        );
    }
}

// -------------------------------------------------------------------------------------------------------------------
// The chain is the chain `record` writes (S9)

#[tokio::test]
async fn a_chain_written_by_record_many_passes_the_nightly_verification() {
    let rig = AuditRig::new(db!()).await;
    let mut total = 0;
    for (i, n) in [1_usize, 7, 50, MAX_AUDIT_BATCH, 3].into_iter().enumerate() {
        many(&rig.pool, events(10_000 * i as i64, n)).await;
        total += n;
    }
    let chain = committed(&rig.pool).await;
    assert_eq!(chain.len(), total);
    assert_eq!(chain[0].prev_hash, genesis_hash());
    assert_eq!(verify_chain(&chain), Ok(()));

    // A signed checkpoint of the head of a chain written this way, and then the whole nightly run: links, hashes, the columns that
    // copy the event, and the checkpoint.
    rig.checkpoint_job().run_once().await.unwrap();
    let NightlyOutcome::Clean(report) = rig.nightly().run_once().await.unwrap() else {
        panic!("the chain written by record_many does not verify");
    };
    assert_eq!(report.events_checked, total as u64);
    assert_eq!(report.checkpoints_checked, 1);
}

#[tokio::test]
async fn a_chain_written_by_record_alone_still_verifies() {
    // The other half of "same chain": `record` after the refactor, checked by the same verification.
    let rig = AuditRig::new(db!()).await;
    for n in 0..12 {
        single(&rig.pool, if n % 2 == 0 { bare_event(n) } else { full_event(n) }).await;
    }
    let NightlyOutcome::Clean(report) = rig.nightly().run_once().await.unwrap() else {
        panic!("the chain written by record does not verify");
    };
    assert_eq!(report.events_checked, 12);
}

#[tokio::test]
async fn a_mixed_chain_verifies() {
    let rig = AuditRig::new(db!()).await;
    let pool = &rig.pool;

    // In one transaction: record, many, record, many.
    let mut tx = pool.begin().await.unwrap();
    let log = PgAuditLog::new();
    log.record(&mut Tx::from_pg(&mut tx), event(1)).await.unwrap();
    log.record_many(&mut Tx::from_pg(&mut tx), events(10, 5))
        .await
        .unwrap();
    log.record(&mut Tx::from_pg(&mut tx), event(2)).await.unwrap();
    log.record_many(&mut Tx::from_pg(&mut tx), events(20, 3))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // Across transactions.
    many(pool, events(30, 4)).await;
    single(pool, event(3)).await;
    many(pool, events(40, 1)).await;

    let chain = committed(pool).await;
    assert_eq!(chain.len(), 1 + 5 + 1 + 3 + 4 + 1 + 1);
    assert_eq!(verify_chain(&chain), Ok(()));
    let ats: Vec<i64> = chain
        .iter()
        .map(|e| e.event.at.unix_millis() - 1_791_676_800_000)
        .collect();
    assert_eq!(
        ats,
        [1, 10, 11, 12, 13, 14, 2, 20, 21, 22, 30, 31, 32, 33, 3, 40],
        "input order, everywhere"
    );
    let NightlyOutcome::Clean(report) = rig.nightly().run_once().await.unwrap() else {
        panic!("the mixed chain does not verify");
    };
    assert_eq!(report.events_checked, chain.len() as u64);
}

#[tokio::test]
async fn the_rows_of_a_batch_are_the_rows_record_writes() {
    // Every column, for events that exercise every column: written once by `record` and once, the same events, by `record_many`.
    let db = db!();
    let pool = db.app_pool(4).await;
    let mut unicode = full_event(5);
    unicode.diff = Some("é".repeat(AuditEvent::MAX_DIFF_BYTES / 2));
    let mut odd = full_event(6);
    odd.diff = Some("a\u{0}b\n\"quoted\"\t\\ \u{1F600}".to_owned());
    let mut no_actor = full_event(7);
    if let Some(a) = no_actor.attribution.as_mut() {
        a.actor = None;
    }
    let dates = [
        -210_866_803_200_000,
        -1,
        0,
        1,
        1_791_676_800_123,
        253_402_300_799_999,
        8_900_000_000_000_123,
    ];
    let mut batch = vec![bare_event(1), full_event(2), unicode, odd, no_actor];
    for ms in dates {
        let mut e = event(0);
        e.at = Timestamp::from_unix_millis(ms);
        batch.push(e);
    }
    for e in &batch {
        single(&pool, e.clone()).await;
    }
    many(&pool, batch.clone()).await;

    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT to_jsonb(t) - 'seq' - 'prev_hash' - 'hash' - 'recorded_at' FROM audit_events t ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let k = batch.len();
    assert_eq!(rows.len(), 2 * k);
    for i in 0..k {
        assert_eq!(
            rows[i],
            rows[k + i],
            "event {i}: the columns differ between record and record_many"
        );
    }
    // `at` is exact to the microsecond for every date the column holds, as for `record`.
    let at: Vec<i64> = sqlx::query_scalar(
        "SELECT (extract(epoch FROM at) * 1000000)::bigint FROM audit_events ORDER BY seq OFFSET $1",
    )
    .bind((2 * k - dates.len()) as i64)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(at, dates.map(|ms| ms * 1000));
    assert_eq!(verify_chain(&committed(&pool).await), Ok(()));
}

#[tokio::test]
async fn a_batch_of_the_largest_events_fits_the_limit() {
    // The worst case that the limit is justified with: 200 events, each with the largest diff.
    let db = db!();
    let pool = db.app_pool(4).await;
    let big: Vec<AuditEvent> = (0..MAX_AUDIT_BATCH as i64)
        .map(|i| {
            let mut e = full_event(i);
            e.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES));
            e
        })
        .collect();
    let started = Instant::now();
    let ids = many(&pool, big).await;
    assert_eq!(ids.len(), MAX_AUDIT_BATCH);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "12.8 MiB in one statement took {:?}",
        started.elapsed()
    );
    assert_eq!(verify_chain(&committed(&pool).await), Ok(()));
}

// -------------------------------------------------------------------------------------------------------------------
// One total order under concurrency

/// Which transaction wrote an event: the writer and round are in the time of the event.
const BASE: i64 = 1_791_676_800_000;

fn tagged(writer: i64, round: i64, i: i64) -> AuditEvent {
    let mut e = bare_event(0);
    e.at = Timestamp::from_unix_millis(BASE + writer * 1_000_000 + round * 1_000 + i);
    e
}

fn decode(e: &AuditEvent) -> (i64, i64, i64) {
    let n = e.at.unix_millis() - BASE;
    (n / 1_000_000, (n / 1_000) % 1_000, n % 1_000)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_record_many_and_record_callers_give_one_strict_total_order() {
    let rig = AuditRig::new(db!()).await;
    // Two hub replicas: two pools on one database.
    let pools = [rig.db.app_pool(8).await, rig.db.app_pool(8).await];

    // Phase 1: everything commits. No gap in the sequence either.
    let mut tasks = Vec::new();
    let mut expected = 0_usize;
    for writer in 0..12_i64 {
        let pool = pools[usize::try_from(writer % 2).unwrap()].clone();
        let sizes: Vec<usize> = (0..6_i64)
            .map(|round| {
                if (writer + round) % 3 == 0 {
                    0
                } else {
                    1 + ((writer * 7 + round * 3) % 40) as usize
                }
            })
            .collect();
        expected += sizes.iter().map(|s| (*s).max(1)).sum::<usize>();
        tasks.push(tokio::spawn(async move {
            for (round, size) in sizes.into_iter().enumerate() {
                let round = round as i64;
                let mut tx = pool.begin().await.unwrap();
                let log = PgAuditLog::new();
                if size == 0 {
                    log.record(&mut Tx::from_pg(&mut tx), tagged(writer, round, 0))
                        .await
                        .unwrap();
                } else {
                    let batch = (0..size as i64).map(|i| tagged(writer, round, i)).collect();
                    log.record_many(&mut Tx::from_pg(&mut tx), batch).await.unwrap();
                }
                tokio::task::yield_now().await;
                tx.commit().await.unwrap();
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let chain = committed(&pools[0]).await;
    assert_eq!(chain.len(), expected);
    assert_eq!(verify_chain(&chain), Ok(()), "one linear chain, no fork");
    assert_eq!(chain[0].id.get(), 1);
    assert!(
        chain.windows(2).all(|w| w[1].id.get() == w[0].id.get() + 1),
        "the sequence has no gap and runs in chain order"
    );
    assert_batches_are_whole(&chain);

    // Phase 2: some transactions roll back after a batch, some are refused for one bad event, some wait on each other.
    let mut tasks = Vec::new();
    for writer in 20..32_i64 {
        let pool = pools[usize::try_from(writer % 2).unwrap()].clone();
        tasks.push(tokio::spawn(async move {
            let mut committed_events = 0_usize;
            for round in 0..5_i64 {
                let mut tx = pool.begin().await.unwrap();
                let log = PgAuditLog::new();
                let size = 1 + ((writer + round * 5) % 30) as usize;
                let mut batch: Vec<AuditEvent> = (0..size as i64).map(|i| tagged(writer, round, i)).collect();
                match (writer + round) % 4 {
                    0 => {
                        log.record_many(&mut Tx::from_pg(&mut tx), batch).await.unwrap();
                        tokio::task::yield_now().await;
                        tx.rollback().await.unwrap();
                    }
                    1 => {
                        let last = batch.len() - 1;
                        batch[last / 2].diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
                        let refused = log.record_many(&mut Tx::from_pg(&mut tx), batch).await;
                        assert_eq!(refused, Err(AuditError::TooLarge));
                        tx.rollback().await.unwrap();
                    }
                    _ => {
                        log.record_many(&mut Tx::from_pg(&mut tx), batch).await.unwrap();
                        tx.commit().await.unwrap();
                        committed_events += size;
                    }
                }
            }
            committed_events
        }));
    }
    let mut added = 0;
    for t in tasks {
        added += t.await.unwrap();
    }
    let chain = committed(&pools[0]).await;
    assert_eq!(chain.len(), expected + added, "exactly the committed events");
    assert_eq!(verify_chain(&chain), Ok(()), "still one linear chain");
    let prevs: std::collections::BTreeSet<_> = chain.iter().map(|e| e.prev_hash).collect();
    assert_eq!(prevs.len(), chain.len(), "no two events share a predecessor");
    assert!(
        chain.windows(2).all(|w| w[1].id > w[0].id),
        "the sequence runs in chain order"
    );
    assert_batches_are_whole(&chain);

    // And the production verification agrees.
    let NightlyOutcome::Clean(report) = rig.nightly().run_once().await.unwrap() else {
        panic!("the chain written by concurrent batches does not verify");
    };
    assert_eq!(report.events_checked, chain.len() as u64);
}

/// The events of one transaction are next to each other in the chain, in input order: another writer's event never falls inside a batch.
fn assert_batches_are_whole(chain: &[ports::AuditEntry]) {
    let mut places: BTreeMap<(i64, i64), Vec<(usize, i64)>> = BTreeMap::new();
    for (pos, entry) in chain.iter().enumerate() {
        let (writer, round, i) = decode(&entry.event);
        places.entry((writer, round)).or_default().push((pos, i));
    }
    for ((writer, round), at) in places {
        for pair in at.windows(2) {
            assert_eq!(
                pair[1].0,
                pair[0].0 + 1,
                "writer {writer} round {round}: a foreign event inside the batch"
            );
            assert_eq!(
                pair[1].1,
                pair[0].1 + 1,
                "writer {writer} round {round}: out of input order"
            );
        }
    }
}

// -------------------------------------------------------------------------------------------------------------------
// Failure leaves nothing behind

#[tokio::test]
async fn an_event_that_cannot_be_stored_is_refused_before_the_lock_and_nothing_is_written() {
    let db = db!();
    let pool = db.app_pool(4).await;
    single(&pool, event(0)).await;
    let holder = hold_chain_lock(&pool).await;

    let mut batch = events(100, 4);
    batch[2].diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
    let mut tx = pool.begin().await.unwrap();
    let started = Instant::now();
    let result = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), batch)
        .await;
    assert_eq!(result, Err(AuditError::TooLarge));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "waited for the lock: {:?}",
        started.elapsed()
    );
    // The transaction is untouched, and committing it keeps nothing of the batch.
    sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    holder.rollback().await.unwrap();
    assert_eq!(count(&pool).await, 1);
}

#[tokio::test]
async fn a_database_error_in_the_middle_of_a_batch_leaves_nothing_and_the_chain_goes_on() {
    let db = db!();
    let pool = db.app_pool(4).await;
    single(&pool, event(0)).await;
    let head_before = committed(&pool).await.pop().unwrap().hash;

    // A time that no `timestamptz` holds: it passes the checks of the port and the table refuses it, for `record` and for a batch.
    let mut out_of_range = event(1);
    out_of_range.at = Timestamp::from_unix_millis(i64::MAX / 2);
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        PgAuditLog::new()
            .record(&mut Tx::from_pg(&mut tx), out_of_range.clone())
            .await,
        Err(AuditError::Unavailable),
        "the single record refuses it"
    );
    drop(tx);

    let mut batch = events(100, 4);
    batch[2] = out_of_range;
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        PgAuditLog::new()
            .record_many(&mut Tx::from_pg(&mut tx), batch)
            .await,
        Err(AuditError::Unavailable),
        "so does the batch, as a whole"
    );
    // The transaction is aborted: the action does not go ahead without its events.
    assert!(sqlx::query("SELECT 1").execute(&mut *tx).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(count(&pool).await, 1, "not one event of the failed batch");

    // The next batch chains to the old head.
    many(&pool, events(200, 3)).await;
    let chain = committed(&pool).await;
    assert_eq!(chain.len(), 4);
    assert_eq!(chain[1].prev_hash, head_before);
    assert_eq!(verify_chain(&chain), Ok(()));
}

#[tokio::test]
async fn a_rolled_back_batch_leaves_no_row_and_the_next_batch_chains_correctly() {
    let db = db!();
    let pool = db.app_pool(4).await;
    many(&pool, events(0, 3)).await;
    let mut tx = pool.begin().await.unwrap();
    PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), events(100, 50))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    many(&pool, events(200, 2)).await;
    let chain = committed(&pool).await;
    assert_eq!(chain.len(), 5);
    assert_eq!(
        chain[3].prev_hash, chain[2].hash,
        "the rolled-back batch is not in the chain"
    );
    assert_eq!(verify_chain(&chain), Ok(()));
}

// -------------------------------------------------------------------------------------------------------------------
// The lock

#[tokio::test]
async fn an_empty_batch_takes_no_lock_and_does_not_look_at_the_transaction() {
    let db = db!();
    let pool = db.app_pool(4).await;

    // Another replica holds the lock; an empty batch does not wait for it, whatever its timeout.
    let holder = hold_chain_lock(&pool).await;
    let log = PgAuditLog::new().with_lock_timeout(Duration::from_millis(300));
    let mut tx = pool.begin().await.unwrap();
    let started = Instant::now();
    assert_eq!(
        log.record_many(&mut Tx::from_pg(&mut tx), Vec::new()).await,
        Ok(Vec::new())
    );
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "{:?}",
        started.elapsed()
    );
    // And the transaction is not poisoned by it.
    sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
    tx.rollback().await.unwrap();
    holder.rollback().await.unwrap();

    // It does not take the lock: a second connection can take it while the first transaction is still open.
    let mut first = pool.begin().await.unwrap();
    PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut first), Vec::new())
        .await
        .unwrap();
    let mut second = pool.begin().await.unwrap();
    let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(CHAIN_LOCK_KEY)
        .fetch_one(&mut *second)
        .await
        .unwrap();
    assert!(free, "an empty batch holds the chain lock");
    second.rollback().await.unwrap();
    first.rollback().await.unwrap();

    // Nor does it look at a transaction of another store.
    let mut state = ports::fakes::FakeTxState::default();
    assert_eq!(
        PgAuditLog::new()
            .record_many(&mut Tx::in_memory(&mut state), Vec::new())
            .await,
        Ok(Vec::new())
    );
}

#[tokio::test]
async fn an_oversized_batch_is_refused_before_the_lock() {
    let db = db!();
    let pool = db.app_pool(4).await;
    let holder = hold_chain_lock(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let started = Instant::now();
    let result = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), events(0, MAX_AUDIT_BATCH + 1))
        .await;
    assert_eq!(result, Err(AuditError::BatchTooLarge));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "waited for the lock: {:?}",
        started.elapsed()
    );
    tx.rollback().await.unwrap();
    holder.rollback().await.unwrap();
    assert_eq!(count(&pool).await, 0);
}

#[tokio::test]
async fn a_batch_waits_for_the_lock_and_gives_up_like_record_does() {
    let db = db!();
    let pool = db.app_pool(4).await;
    let holder = hold_chain_lock(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let started = Instant::now();
    let log = PgAuditLog::new().with_lock_timeout(Duration::from_millis(300));
    let result = log.record_many(&mut Tx::from_pg(&mut tx), events(0, 5)).await;
    assert_eq!(result, Err(AuditError::Unavailable));
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_secs(3),
        "{waited:?}"
    );
    assert!(
        sqlx::query("SELECT 1").execute(&mut *tx).await.is_err(),
        "the transaction cannot commit"
    );
    drop(tx);
    holder.rollback().await.unwrap();
    assert_eq!(count(&pool).await, 0);
}

#[tokio::test]
async fn the_lock_timeout_of_a_batch_ends_with_its_transaction() {
    // One connection: "the next statement" is on the same backend. `SET LOCAL` must not leave the timeout on the pooled connection.
    let db = db!();
    let pool = db.app_pool(1).await;
    let default: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    let log = PgAuditLog::new().with_lock_timeout(Duration::from_millis(1500));
    let mut tx = pool.begin().await.unwrap();
    log.record_many(&mut Tx::from_pg(&mut tx), events(0, 3))
        .await
        .unwrap();
    let inside: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(inside, "1500ms");
    tx.commit().await.unwrap();
    let after: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, default);
}

#[tokio::test]
async fn a_transaction_of_another_store_is_refused_for_a_batch() {
    let mut state = ports::fakes::FakeTxState::default();
    let result = PgAuditLog::new()
        .record_many(&mut Tx::in_memory(&mut state), events(0, 2))
        .await;
    assert_eq!(result, Err(AuditError::WrongTx));
}

// -------------------------------------------------------------------------------------------------------------------
// S10, S21

#[tokio::test]
async fn a_failing_batch_logs_its_stage_and_never_its_content() {
    let (capture, _guard) = log_capture::install();
    let db = db!();
    let pool = db.app_pool(4).await;
    let secret = "SECRET-DIFF-CONTENT-4711";
    let mut batch = events(0, 4);
    for e in &mut batch {
        e.diff = Some(format!("+token: {secret}\n"));
    }
    many(&pool, batch.clone()).await;

    // The table refuses the third event, after the lock.
    batch[2].at = Timestamp::from_unix_millis(i64::MAX / 2);
    let mut tx = pool.begin().await.unwrap();
    let failed = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), batch.clone())
        .await
        .unwrap_err();
    drop(tx);
    // And one the port refuses for its size.
    batch[1].diff = Some(format!("{secret}{}", "x".repeat(AuditEvent::MAX_DIFF_BYTES)));
    let mut tx = pool.begin().await.unwrap();
    let too_large = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), batch)
        .await
        .unwrap_err();
    let text = format!("{failed} {failed:?} {too_large} {too_large:?} {}", capture.text());
    assert!(!text.contains(secret), "event content leaked: {text}");
    assert!(
        capture.text().contains("audit record failed"),
        "the failure is logged by stage: {}",
        capture.text()
    );
}

// -------------------------------------------------------------------------------------------------------------------
// Where the time goes: not a test of speed, a report of it. `cargo test --release -p hub-identity --test audit_batch -- --ignored --nocapture`

#[tokio::test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
async fn report_the_time_of_a_batch_against_the_same_events_one_by_one() {
    let db: TestDb = db!();
    let pool = db.app_pool(4).await;
    // A chain with history, as in service.
    for n in 0..200 {
        single(&pool, event(n)).await;
    }
    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    println!(
        "\nevents | record x n (ms) | record_many (ms) | speed-up   (one transaction: BEGIN, the audit, COMMIT; median of 15)"
    );
    for n in [1_usize, 10, 50, 100, MAX_AUDIT_BATCH] {
        let (mut one_by_one, mut batched) = (Vec::new(), Vec::new());
        for round in 0..15_i64 {
            let base = 1_000_000 + round * 10_000;
            let started = Instant::now();
            let mut tx = pool.begin().await.unwrap();
            for e in events(base, n) {
                PgAuditLog::new()
                    .record(&mut Tx::from_pg(&mut tx), e)
                    .await
                    .unwrap();
            }
            tx.commit().await.unwrap();
            one_by_one.push(started.elapsed());

            let started = Instant::now();
            let mut tx = pool.begin().await.unwrap();
            PgAuditLog::new()
                .record_many(&mut Tx::from_pg(&mut tx), events(base + 5_000, n))
                .await
                .unwrap();
            tx.commit().await.unwrap();
            batched.push(started.elapsed());
        }
        let (a, b) = (median(one_by_one), median(batched));
        println!(
            "{n:>6} | {:>15.2} | {:>16.2} | {:>8.1}x",
            a.as_secs_f64() * 1000.0,
            b.as_secs_f64() * 1000.0,
            a.as_secs_f64() / b.as_secs_f64()
        );
        if n >= 10 {
            assert!(
                b < a,
                "a batch of {n} is not faster than {n} calls: {b:?} against {a:?}"
            );
        }
    }
}
