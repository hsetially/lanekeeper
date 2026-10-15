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
use ports::conformance::sample::{
    audit_events_totalling, storable_worst_case_audit_event as worst_case_audit_event,
};
use ports::{
    AuditError, AuditEvent, AuditLog, MAX_AUDIT_BATCH, MAX_AUDIT_BATCH_BYTES, MAX_AUDIT_EVENT_BYTES, Tx,
    genesis_hash, verify_chain,
};
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

// -------------------------------------------------------------------------------------------------------------------
// The bound on the work under the lock is bytes, not events. A control character is six bytes of canonical JSON, a diff is
// untrusted text of up to 64 KiB, so one event is up to 398,673 bytes and 200 of them were about 76 MiB. These tests use the worst case:
// events of control characters, not plain ones.

/// The bytes of canonical JSON that the table holds for the events of the batch (what the budget counts).
async fn stored_bytes(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT coalesce(sum(octet_length(event_json)), 0)::bigint FROM audit_events")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_largest_event_that_can_exist_is_accepted_alone_by_record_many_and_by_record() {
    // The invariant behind the budget: what `record` accepts, `record_many` accepts alone.
    let db = db!();
    let pool = db.app_pool(4).await;
    let by_batch = worst_case_audit_event(1);
    let by_record = worst_case_audit_event(2);
    assert_eq!(many(&pool, vec![by_batch.clone()]).await.len(), 1);
    single(&pool, by_record.clone()).await;
    let chain = committed(&pool).await;
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].event, by_batch);
    assert_eq!(chain[1].event, by_record);
    assert_eq!(verify_chain(&chain), Ok(()));
    // Both are stored as 398,669 bytes (the constant's 398,673, less four: the table holds no time as long as `i64::MIN`), and the table accepts them (its limit is 1 MiB per event).
    let sizes: Vec<i64> =
        sqlx::query_scalar("SELECT octet_length(event_json)::bigint FROM audit_events ORDER BY seq")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(sizes, [MAX_AUDIT_EVENT_BYTES as i64 - 4; 2]);
}

#[tokio::test]
async fn a_batch_may_be_exactly_the_byte_budget_and_not_one_byte_more() {
    let db = db!();
    let pool = db.app_pool(4).await;
    // Exactly the budget, in three events and in as many as the count allows; the size is checked on what the table holds.
    for (n, what) in [
        (3, "three events"),
        (MAX_AUDIT_BATCH, "as many events as the count limit allows"),
    ] {
        let batch = audit_events_totalling(n, MAX_AUDIT_BATCH_BYTES);
        let before = stored_bytes(&pool).await;
        assert_eq!(many(&pool, batch).await.len(), n, "{what}");
        assert_eq!(
            stored_bytes(&pool).await - before,
            MAX_AUDIT_BATCH_BYTES as i64,
            "{what}: the table holds exactly the budget"
        );
    }
    assert_eq!(verify_chain(&committed(&pool).await), Ok(()));

    // One byte more is refused whole, in a transaction that stays usable, and nothing is written.
    let rows = count(&pool).await;
    for n in [3, MAX_AUDIT_BATCH] {
        let mut tx = pool.begin().await.unwrap();
        let result = PgAuditLog::new()
            .record_many(
                &mut Tx::from_pg(&mut tx),
                audit_events_totalling(n, MAX_AUDIT_BATCH_BYTES + 1),
            )
            .await;
        assert_eq!(
            result,
            Err(AuditError::BatchTooLarge),
            "{n} events, one byte over"
        );
        sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(count(&pool).await, rows);
}

#[tokio::test]
async fn three_of_the_largest_events_are_refused_by_their_bytes_though_they_are_three() {
    // 3 events, 192 KiB of diff, and 1.2 MB of canonical JSON: a count of events or characters would have let them through.
    let db = db!();
    let pool = db.app_pool(4).await;
    let mut tx = pool.begin().await.unwrap();
    let result = PgAuditLog::new()
        .record_many(
            &mut Tx::from_pg(&mut tx),
            (0..3).map(worst_case_audit_event).collect(),
        )
        .await;
    assert_eq!(result, Err(AuditError::BatchTooLarge));
    tx.rollback().await.unwrap();
    // Two of them fit.
    let two = many(&pool, (0..2).map(worst_case_audit_event).collect()).await;
    assert_eq!(two.len(), 2);
    assert_eq!(stored_bytes(&pool).await, 2 * (MAX_AUDIT_EVENT_BYTES as i64 - 4));
}

#[tokio::test]
async fn a_batch_over_the_byte_budget_is_refused_before_the_lock_and_after_work_of_about_the_budget() {
    let db = db!();
    let pool = db.app_pool(4).await;
    // 190 of the largest events: 76 MB of canonical JSON, if all were encoded. The refusal comes when the sum passes the budget,
    // after the third event, so the work is that of three and not of 190.
    let hostile: Vec<AuditEvent> = (0..190).map(worst_case_audit_event).collect();
    let started = Instant::now();
    for e in &hostile {
        hub_identity::audit::log::prepare(e).unwrap();
    }
    let encode_everything = started.elapsed();

    let holder = hold_chain_lock(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let started = Instant::now();
    let result = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), hostile)
        .await;
    let refused_in = started.elapsed();
    assert_eq!(result, Err(AuditError::BatchTooLarge));
    // The lock is held by someone else (the default lock timeout is 5 s): nobody waited for it. And the encoding stopped at the
    // budget: the refusal took a small part of what encoding all 190 takes.
    assert!(
        refused_in < Duration::from_secs(2),
        "the refusal waited for the lock: {refused_in:?}"
    );
    assert!(
        refused_in * 4 < encode_everything,
        "the refusal took {refused_in:?}, and encoding all 190 events takes {encode_everything:?}: the work was not cut at the budget"
    );
    tx.rollback().await.unwrap();
    holder.rollback().await.unwrap();
    assert_eq!(count(&pool).await, 0);
}

// Rule 4: CPU-heavy work does not run on the async runtime. `prepare_batch` (checks and canonical JSON, before the lock) and
// `link_chain` (SHA-256, inside the lock) of a batch at the byte budget cost tens of milliseconds in a debug build and a few in a
// release build. These tests run on a current-thread runtime, the one thread that runs the test, and compare the CPU time that the
// calls use on it with the CPU time of the same work done in place: done in place, it would be that much of this thread's.
//
// They read the thread's CPU time from `/proc`, so they run on Linux only. They run in any profile: the work is repeated until it
// has cost 40 ms of CPU, so how fast the build is does not decide whether there is anything to measure, and a build so fast that
// 4,096 repetitions are not enough fails the test instead of passing it.

/// The CPU time that this thread has used, in nanoseconds, from `/proc` (the first field of `schedstat`). It does not move while the
/// thread waits, so it is not subject to the scheduling noise of a loaded machine.
#[cfg(target_os = "linux")]
fn thread_cpu_ns() -> u64 {
    let stat = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap();
    stat.split(' ').next().unwrap().parse().unwrap()
}

/// Run `work` until it has used at least 40 ms of this thread's CPU, doubling the number of runs. Returns how many runs that took and
/// the CPU time they used.
#[cfg(target_os = "linux")]
fn repeated_until_measurable(mut work: impl FnMut()) -> (usize, u64) {
    const TARGET_NS: u64 = 40_000_000;
    const MOST: usize = 4_096;
    let mut runs = 1;
    loop {
        let before = thread_cpu_ns();
        for _ in 0..runs {
            work();
        }
        let used = thread_cpu_ns() - before;
        if used >= TARGET_NS {
            return (runs, used);
        }
        assert!(
            runs < MOST,
            "the work is too small to measure: {used} ns of CPU in {runs} runs"
        );
        runs *= 2;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn checking_and_encoding_run_off_the_async_thread() {
    // Three of the largest events pass the budget at the third, which refuses the batch. That path takes no database at all (a
    // transaction of another store is never looked at), so the only CPU of this thread in the call is what it does itself.
    let batch: Vec<AuditEvent> = (0..3).map(worst_case_audit_event).collect();
    let (runs, in_place) = repeated_until_measurable(|| {
        for e in &batch {
            hub_identity::audit::log::prepare(e).unwrap();
        }
    });
    println!(
        "checking and encoding three of the largest events, {runs} times: {in_place} ns of CPU, in place"
    );
    let batches: Vec<Vec<AuditEvent>> = (0..runs).map(|_| batch.clone()).collect();
    let mut state = ports::fakes::FakeTxState::default();
    let before = thread_cpu_ns();
    for batch in batches {
        let result = PgAuditLog::new()
            .record_many(&mut Tx::in_memory(&mut state), batch)
            .await;
        assert_eq!(result, Err(AuditError::BatchTooLarge));
    }
    let on_this_thread = thread_cpu_ns() - before;
    println!("the {runs} refused calls used {on_this_thread} ns of CPU on the async thread");
    assert!(
        on_this_thread * 4 < in_place,
        "{on_this_thread} ns of CPU on the async thread for work that costs {in_place} ns"
    );
}

/// The CPU time that the threads of the runtime's blocking pool, named `name`, have used in all, in nanoseconds.
#[cfg(target_os = "linux")]
fn named_threads_cpu_ns(name: &str) -> (usize, u64) {
    let (mut threads, mut ns) = (0, 0);
    for task in std::fs::read_dir("/proc/self/task").unwrap() {
        let dir = task.unwrap().path();
        let Ok(comm) = std::fs::read_to_string(dir.join("comm")) else {
            continue;
        };
        if comm.trim_end() == name {
            let stat = std::fs::read_to_string(dir.join("schedstat")).unwrap();
            ns += stat.split(' ').next().unwrap().parse::<u64>().unwrap();
            threads += 1;
        }
    }
    (threads, ns)
}

#[cfg(target_os = "linux")]
#[test]
fn hashing_inside_the_lock_runs_off_the_async_thread() {
    // On the call's success path the async thread has its own work, the database's I/O, which is not small next to the hashing in a
    // release build (a megabyte through the protocol is about as much CPU as hashing it). So this asks the other question: where did
    // the hashing happen? The runtime is built here with its blocking pool's threads named, and the CPU of those threads over the
    // calls must include the hashing as well as the checking and encoding that also run there. With the hashing done in place it
    // would include only the checking and encoding, and the half of the hashing that the bound allows for is missing.
    const NAME: &str = "lk-hop-test";
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name(NAME)
        .build()
        .unwrap();
    rt.block_on(async {
        let db = db!();
        let pool = db.app_pool(4).await;
        let batch = audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES);
        let prepared = hub_identity::audit::log::prepare_batch(&batch).unwrap();
        let (runs, hashing) = repeated_until_measurable(|| {
            std::hint::black_box(hub_identity::audit::log::link_chain(genesis_hash(), &prepared));
        });
        let before = thread_cpu_ns();
        for _ in 0..runs {
            std::hint::black_box(hub_identity::audit::log::prepare_batch(&batch).unwrap());
        }
        let checking = thread_cpu_ns() - before;
        println!("{runs} runs in place: hashing {hashing} ns of CPU, checking and encoding {checking} ns");

        let batches: Vec<Vec<AuditEvent>> = (0..runs).map(|_| batch.clone()).collect();
        let (_, pool_before) = named_threads_cpu_ns(NAME);
        for batch in batches {
            assert_eq!(many(&pool, batch).await.len(), 3);
        }
        let (threads, pool_after) = named_threads_cpu_ns(NAME);
        assert!(threads > 0, "no blocking thread of this runtime was found by its name");
        let on_the_pool = pool_after - pool_before;
        println!("the {runs} calls used {on_the_pool} ns of CPU on the blocking pool");
        assert!(
            on_the_pool >= checking + hashing / 2,
            "{on_the_pool} ns of CPU on the blocking pool for calls whose checking and encoding cost {checking} ns and hashing {hashing} ns: the hashing did not run there"
        );
    });
}

// -------------------------------------------------------------------------------------------------------------------
// How long the chain lock is held

/// How long another writer waits for the chain lock behind `batch`. Another transaction holds the lock; the batch is checked and
/// encoded, and queues for it; then the holder lets go and the clock starts. It stops when the batch's transaction has committed:
/// the time from the lock being granted to its release, which is what the next writer waits for.
async fn lock_hold_of(pool: &PgPool, batch: Vec<AuditEvent>) -> Duration {
    /// Nothing here waits for longer than this: a batch that never queues for the lock, or never finishes, is a failure and not a
    /// hang. The default lock timeout of the audit log is 5 s, so a healthy batch is far inside it.
    const DEADLINE: Duration = Duration::from_secs(20);

    let holder = hold_chain_lock(pool).await;
    let batch_pool = pool.clone();
    let mut batch_task = tokio::spawn(async move {
        let mut tx = batch_pool.begin().await.unwrap();
        PgAuditLog::new()
            .record_many(&mut Tx::from_pg(&mut tx), batch)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        Instant::now()
    });
    // The batch is queued for the lock when a request for it is waiting in this database. The poll is raced against the batch's own
    // task: the task is expected to block on the lock, so one that has ended (it panicked, or was refused) will never queue, and the
    // wait must stop at once and say why instead of polling for ever.
    let queued = async {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
                 AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    };
    tokio::select! {
        ended = &mut batch_task => {
            panic!("the batch ended before it queued for the chain lock, which another transaction holds: {ended:?}");
        }
        waited = tokio::time::timeout(DEADLINE, queued) => {
            assert!(waited.is_ok(), "the batch did not queue for the chain lock within {DEADLINE:?}");
        }
    }
    let granted = Instant::now();
    holder.rollback().await.unwrap();
    let committed = tokio::time::timeout(DEADLINE, batch_task)
        .await
        .unwrap_or_else(|_| panic!("the batch did not commit within {DEADLINE:?} of the lock being released"))
        .unwrap();
    committed - granted
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lock_hold_measurement_fails_fast_when_the_batch_cannot_queue() {
    // A batch one byte over the budget is refused before it takes the lock, so it never queues behind the holder. The measurement
    // must stop at once and say so: a regression in the budget check showed up as a test that hung for 18 minutes.
    let db = db!();
    let pool = db.app_pool(4).await;
    let refused = audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES + 1);
    let measurement = tokio::spawn({
        let pool = pool.clone();
        async move { lock_hold_of(&pool, refused).await }
    });
    let ended = tokio::time::timeout(Duration::from_secs(10), measurement)
        .await
        .expect("the measurement hung instead of failing")
        .expect_err("the measurement of a refused batch cannot succeed");
    assert!(ended.is_panic());
    let message = ended.into_panic();
    let message = message
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| message.downcast_ref::<&str>().copied())
        .unwrap_or_default()
        .to_owned();
    assert!(
        message.contains("the batch ended before it queued for the chain lock")
            && message.contains("BatchTooLarge"),
        "{message}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_chain_lock_is_held_for_a_short_time_by_a_batch_at_the_byte_budget() {
    // The worst cases at the budget, all made of control characters (six bytes each in canonical JSON): the fewest, largest
    // events, the most events the count allows, the largest events and a rest, and the usual: 50 typical events.
    let db = db!();
    let pool = db.app_pool(8).await;
    for n in 0..30 {
        single(&pool, event(n)).await;
    }
    let two_largest_and_a_rest = {
        let mut v: Vec<AuditEvent> = (0..2).map(worst_case_audit_event).collect();
        v.extend(audit_events_totalling(
            1,
            MAX_AUDIT_BATCH_BYTES - 2 * (MAX_AUDIT_EVENT_BYTES - 4),
        ));
        v
    };
    let shapes: [(&str, Vec<AuditEvent>); 4] = [
        (
            "3 events at the budget",
            audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES),
        ),
        ("2 of the largest and a rest", two_largest_and_a_rest),
        (
            "200 events at the budget",
            audit_events_totalling(MAX_AUDIT_BATCH, MAX_AUDIT_BATCH_BYTES),
        ),
        ("50 typical events", events(10_000, 50)),
    ];
    let optimised = !cfg!(debug_assertions);
    println!(
        "\nchain lock held behind a batch (build: {}):",
        if optimised { "release" } else { "debug" }
    );
    for (what, batch) in shapes {
        let bytes: usize = batch.iter().map(|e| ports::canonical_size(e).unwrap()).sum();
        assert!(
            bytes <= MAX_AUDIT_BATCH_BYTES,
            "{what}: {bytes} bytes is over the budget"
        );
        let count = batch.len();
        // The fastest and the slowest of five: the numbers are those of a machine that is doing other things too.
        let mut held = Vec::new();
        for _ in 0..5 {
            held.push(lock_hold_of(&pool, batch.clone()).await);
        }
        held.sort();
        println!(
            "  {what:<30} {count:>3} events {bytes:>9} bytes   hold min {:>6.1} ms  median {:>6.1} ms  max {:>6.1} ms",
            held[0].as_secs_f64() * 1000.0,
            held[2].as_secs_f64() * 1000.0,
            held[4].as_secs_f64() * 1000.0
        );
        // Against a lock timeout of 5 s. Debug builds hash and encode ten times slower, so the bounds differ.
        let bound = if optimised {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(1_500)
        };
        assert!(held[4] < bound, "{what}: the lock was held for {:?}", held[4]);
    }
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

#[tokio::test]
async fn rows_that_do_not_land_in_chain_order_fail_the_call_and_the_caller_rolls_back() {
    // The guard of `ids_in_chain_order`, through the public path. The insert draws the `seq` of the rows in chain order; if a
    // sequence ran backwards (an owner changed it), the batch would be stored in an order that the verifier, which reads by `seq`,
    // calls a broken chain. The call must say so and not hand back ids.
    let db = db!();
    let pool = db.app_pool(4).await;
    many(&pool, events(0, 2)).await;
    let mut admin = db.admin().await;
    sqlx::query(
        "ALTER SEQUENCE audit_events_seq_seq INCREMENT BY -1 MINVALUE 1 MAXVALUE 100000 RESTART WITH 5000",
    )
    .execute(&mut admin)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let result = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), events(100, 3))
        .await;
    assert_eq!(
        result,
        Err(AuditError::Unavailable),
        "no ids for rows that are not in chain order"
    );
    // The rows are in the caller's transaction: only a rollback removes them, which is what the error is for.
    let inside: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(inside, 5);
    // And a caller that committed anyway would have written a chain that fails verification: the guard is not a formality.
    tx.commit().await.unwrap();
    assert!(verify_chain(&committed(&pool).await).is_err());
}

#[tokio::test]
async fn a_rollback_after_rows_out_of_chain_order_leaves_the_chain_as_it_was() {
    let db = db!();
    let pool = db.app_pool(4).await;
    many(&pool, events(0, 2)).await;
    let mut admin = db.admin().await;
    sqlx::query(
        "ALTER SEQUENCE audit_events_seq_seq INCREMENT BY -1 MINVALUE 1 MAXVALUE 100000 RESTART WITH 5000",
    )
    .execute(&mut admin)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let result = PgAuditLog::new()
        .record_many(&mut Tx::from_pg(&mut tx), events(100, 3))
        .await;
    assert_eq!(result, Err(AuditError::Unavailable));
    tx.rollback().await.unwrap();
    let chain = committed(&pool).await;
    assert_eq!(chain.len(), 2);
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

/// Hashing a batch where it is (on the async thread) against the hop to the blocking pool that `record_many` makes, inside the
/// chain lock. No database: the question is what the hop costs against what it saves. Each size is measured hot (calls back to
/// back, the blocking thread is awake) and spaced (5 ms between calls, as audited writes arrive in service, the blocking thread
/// asleep). `cargo test --release -p hub-identity --test audit_batch -- --ignored --nocapture report_hashing`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
async fn report_hashing_inline_against_the_hop_to_the_blocking_pool() {
    use hub_identity::audit::log::{link_chain, prepare_batch};

    fn percentile(v: &mut [Duration], p: usize) -> f64 {
        v.sort();
        v[(v.len() * p / 100).min(v.len() - 1)].as_secs_f64() * 1e6
    }
    let shapes: [(&str, Vec<AuditEvent>); 5] = [
        ("1 typical event", events(0, 1)),
        ("10 typical events", events(0, 10)),
        ("50 typical events", events(0, 50)),
        (
            "3 events at the budget",
            audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES),
        ),
        (
            "200 events at the budget",
            audit_events_totalling(MAX_AUDIT_BATCH, MAX_AUDIT_BATCH_BYTES),
        ),
    ];
    println!("\nhash the links of a batch, microseconds (median / p95 of 400 calls)");
    println!(
        "{:<26} {:>10} | {:>14} | {:>14} | {:>14}",
        "batch", "bytes", "inline", "hop, hot", "hop, spaced"
    );
    for (what, batch) in shapes {
        let mut prepared = prepare_batch(&batch).unwrap();
        let bytes: usize = batch.iter().map(|e| ports::canonical_size(e).unwrap()).sum();
        let (mut inline, mut hot, mut spaced) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..400 {
            let started = Instant::now();
            let links = link_chain(genesis_hash(), &prepared);
            inline.push(started.elapsed());
            std::hint::black_box(links);
        }
        for _ in 0..400 {
            let started = Instant::now();
            let (back, links) = tokio::task::spawn_blocking(move || {
                let links = link_chain(genesis_hash(), &prepared);
                (prepared, links)
            })
            .await
            .unwrap();
            hot.push(started.elapsed());
            prepared = back;
            std::hint::black_box(links);
        }
        for _ in 0..400 {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let started = Instant::now();
            let (back, links) = tokio::task::spawn_blocking(move || {
                let links = link_chain(genesis_hash(), &prepared);
                (prepared, links)
            })
            .await
            .unwrap();
            spaced.push(started.elapsed());
            prepared = back;
            std::hint::black_box(links);
        }
        println!(
            "{what:<26} {bytes:>10} | {:>6.0} /{:>6.0} | {:>6.0} /{:>6.0} | {:>6.0} /{:>6.0}",
            percentile(&mut inline, 50),
            percentile(&mut inline, 95),
            percentile(&mut hot, 50),
            percentile(&mut hot, 95),
            percentile(&mut spaced, 50),
            percentile(&mut spaced, 95)
        );
    }
}
