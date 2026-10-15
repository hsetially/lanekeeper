//! `ports::ToolWrites::register` and `register_delete` over a real Postgres: what the port's suites cannot see.
//!
//! - **The savepoint** (the port's MUST): a registration statement that fails (forced here by a trigger that raises, and by a lock that is
//!   held until the registration's wait runs out) leaves the caller's transaction usable. The caller's audit insert and its commit still
//!   succeed in the same transaction, and the registration is absent.
//! - **A bounded wait** that does not leak: the caller's `lock_timeout` and `statement_timeout` are what they were after every outcome.
//! - **The stamp** is the registry's injected clock, and the TTL runs from it: an expired registration does not count towards the bound.
//! - **S10**: no path, hash or user is in a log line.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::{Duration, Instant};

use domain::{Guid, UserId};
use hub_registry::metrics::ToolWriteResult;
use hub_registry::{ManualClock, RegistryConfig};
use ports::fakes::FakeTxState;
use ports::{
    AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, ToolWriteError, ToolWrites, Tx, content_hash,
};
use sqlx::{PgPool, Row};
use support::log_capture::LogCapture;
use support::pg_audit::audit_rows;
use support::rig::{Rig, path};

fn user(n: u32) -> UserId {
    UserId::new(
        Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
        Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
    )
}

fn request(s: &str) -> domain::RequestId {
    domain::RequestId::parse(s).unwrap()
}

/// What the caller (prompt 06) records in the transaction that carries the write: its own audit event.
fn own_audit_event(rig: &Rig, p: &str) -> AuditEvent {
    let mut event = AuditEvent::new(
        hub_registry::Clock::now(&*rig.clock),
        AuditActor::User { user: user(1) },
        AuditAction::FileEdited,
        AuditVia::Ui,
    );
    event.swimlane = Some(rig.swimlane.clone());
    event.path = Some(path(p));
    event.hash_after = Some(content_hash(b"edited"));
    event
}

async fn registrations(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM tool_write_registrations")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A trigger that makes the insert of `svc/boom.yml` fail: a statement of the registration that fails, whatever the reason (a deadlock,
/// a serialization failure, a lock timeout), as far as the caller's transaction is concerned.
async fn make_boom_fail(rig: &Rig) {
    let mut admin = rig.db.admin().await;
    sqlx::raw_sql(
        "CREATE FUNCTION lk_force_registration_failure() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.path = 'svc/boom.yml' THEN RAISE EXCEPTION 'forced' USING ERRCODE = 'XX000'; END IF; RETURN NEW; END $$; \
         CREATE TRIGGER lk_force BEFORE INSERT ON tool_write_registrations FOR EACH ROW EXECUTE FUNCTION lk_force_registration_failure();",
    )
    .execute(&mut admin)
    .await
    .unwrap();
}

async fn setting(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, name: &str) -> String {
    sqlx::query(&format!("SHOW {name}"))
        .fetch_one(&mut **tx)
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn a_failed_registration_leaves_the_callers_transaction_usable_for_its_audit_event() {
    let Some(rig) = Rig::new().await else { return };
    make_boom_fail(&rig).await;

    let mut tx = rig.pool.begin().await.unwrap();
    let result = rig
        .tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/boom.yml"),
            &content_hash(b"x: 2\n"),
            &user(1),
            &request("req-boom"),
        )
        .await;
    assert_eq!(
        result,
        Err(ToolWriteError::Unavailable),
        "a failed statement is `Unavailable`"
    );

    // The transaction is not aborted: the caller records the operation's audit event, and commits.
    rig.audit
        .record(&mut Tx::from_pg(&mut tx), own_audit_event(&rig, "svc/boom.yml"))
        .await
        .expect("the audit insert works: the registration's failure was rolled back to its savepoint");
    tx.commit().await.expect("and the commit");

    assert_eq!(registrations(&rig.pool).await, 0, "the registration is absent");
    let audit = audit_rows(&rig.pool).await;
    assert_eq!(audit.len(), 1, "the caller's one audit event is there");
    assert_eq!(audit[0].action, "file_edited");
    assert_eq!(rig.metrics.tool_write_count(ToolWriteResult::Unavailable), 1);
}

#[tokio::test]
async fn the_work_the_caller_did_before_a_failed_registration_commits_with_it() {
    let Some(rig) = Rig::new().await else { return };
    make_boom_fail(&rig).await;
    let mut tx = rig.pool.begin().await.unwrap();
    // The caller's own change comes first, and a good registration, then the failing one, then the caller's last statement.
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('p2', 'before')")
        .execute(&mut *tx)
        .await
        .unwrap();
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/good.yml"),
            &content_hash(b"good"),
            &user(1),
            &request("req-good"),
        )
        .await
        .unwrap();
    assert_eq!(
        rig.tool_writes
            .register(
                &mut Tx::from_pg(&mut tx),
                &rig.swimlane,
                &path("svc/boom.yml"),
                &content_hash(b"boom"),
                &user(1),
                &request("req-boom"),
            )
            .await,
        Err(ToolWriteError::Unavailable)
    );
    sqlx::query("INSERT INTO projects (id, display_name) VALUES ('p3', 'after')")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        rig.count("SELECT count(*) FROM projects WHERE id IN ('p2', 'p3')")
            .await,
        2,
        "the caller's statements before and after the failed registration are in the commit"
    );
    assert_eq!(
        registrations(&rig.pool).await,
        1,
        "the good registration before the failed one is kept: only the failed one was rolled back"
    );
}

#[tokio::test]
async fn the_callers_timeouts_are_what_they_were_after_every_outcome() {
    let Some(rig) = Rig::new().await else { return };
    make_boom_fail(&rig).await;
    let mut tx = rig.pool.begin().await.unwrap();
    // A caller with timeouts of its own: the registry's wait must not replace them.
    sqlx::query("SELECT set_config('lock_timeout', '7s', true), set_config('statement_timeout', '9s', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let before = (
        setting(&mut tx, "lock_timeout").await,
        setting(&mut tx, "statement_timeout").await,
    );
    assert_eq!(before, ("7s".to_owned(), "9s".to_owned()));

    let w = content_hash(b"w");
    // A new registration, a repeat, a delete, and a failed one: four outcomes.
    for (what, p) in [("new", "svc/a.yml"), ("repeat", "svc/a.yml")] {
        rig.tool_writes
            .register(
                &mut Tx::from_pg(&mut tx),
                &rig.swimlane,
                &path(p),
                &w,
                &user(1),
                &request("req-a"),
            )
            .await
            .unwrap();
        assert_eq!(
            (
                setting(&mut tx, "lock_timeout").await,
                setting(&mut tx, "statement_timeout").await
            ),
            before,
            "after a {what} registration"
        );
    }
    rig.tool_writes
        .register_delete(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &w,
            &user(1),
            &request("req-a"),
        )
        .await
        .unwrap();
    assert_eq!(
        (
            setting(&mut tx, "lock_timeout").await,
            setting(&mut tx, "statement_timeout").await
        ),
        before,
        "after a delete registration"
    );
    assert_eq!(
        rig.tool_writes
            .register(
                &mut Tx::from_pg(&mut tx),
                &rig.swimlane,
                &path("svc/boom.yml"),
                &w,
                &user(1),
                &request("req-b"),
            )
            .await,
        Err(ToolWriteError::Unavailable)
    );
    assert_eq!(
        (
            setting(&mut tx, "lock_timeout").await,
            setting(&mut tx, "statement_timeout").await
        ),
        before,
        "after a registration that failed: the rollback to the savepoint reverted what was set inside it"
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_caller_without_timeouts_gets_none_from_the_registry_either() {
    let Some(rig) = Rig::new().await else { return };
    let mut tx = rig.pool.begin().await.unwrap();
    let before = (
        setting(&mut tx, "lock_timeout").await,
        setting(&mut tx, "statement_timeout").await,
    );
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &content_hash(b"w"),
            &user(1),
            &request("req-a"),
        )
        .await
        .unwrap();
    // The registration's wait (2 s by default) must not outlive it: it would leak into the audit insert that follows.
    assert_eq!(
        (
            setting(&mut tx, "lock_timeout").await,
            setting(&mut tx, "statement_timeout").await
        ),
        before
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_registration_waits_for_a_lock_no_longer_than_its_timeout_and_the_transaction_goes_on() {
    let config = RegistryConfig {
        tool_write_timeout: Duration::from_millis(300),
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    // Someone holds the table: nothing can be read or written until they let go.
    let mut admin = rig.db.admin().await;
    sqlx::raw_sql("BEGIN; LOCK TABLE tool_write_registrations IN ACCESS EXCLUSIVE MODE")
        .execute(&mut admin)
        .await
        .unwrap();

    let mut tx = rig.pool.begin().await.unwrap();
    let started = Instant::now();
    let result = rig
        .tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &content_hash(b"w"),
            &user(1),
            &request("req-a"),
        )
        .await;
    let waited = started.elapsed();
    assert_eq!(
        result,
        Err(ToolWriteError::Unavailable),
        "a lock timeout is `Unavailable`"
    );
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_millis(1_500),
        "it waited for its own timeout (300 ms) and not for the lock, nor for the client-side grace: {waited:?}"
    );
    // The caller's transaction is usable: its audit insert and its commit succeed while the lock is still held.
    rig.audit
        .record(&mut Tx::from_pg(&mut tx), own_audit_event(&rig, "svc/a.yml"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(audit_rows(&rig.pool).await.len(), 1);

    sqlx::raw_sql("ROLLBACK").execute(&mut admin).await.unwrap();
    assert_eq!(registrations(&rig.pool).await, 0, "the registration is absent");
    // And once the lock is gone the same registration is taken.
    let mut tx = rig.pool.begin().await.unwrap();
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &content_hash(b"w"),
            &user(1),
            &request("req-a"),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(registrations(&rig.pool).await, 1);
}

#[tokio::test]
async fn a_transaction_that_sqlx_did_not_open_is_served_all_the_same() {
    // The savepoint is raw (`SAVEPOINT`, `RELEASE`, `ROLLBACK TO`), so a caller that began with a plain BEGIN is no problem: sqlx's nested
    // `begin()` would have issued a second BEGIN there, and its rollback would have ended the caller's transaction.
    let Some(rig) = Rig::new().await else { return };
    make_boom_fail(&rig).await;
    let mut conn = rig.pool.acquire().await.unwrap();
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut conn),
            &rig.swimlane,
            &path("svc/a.yml"),
            &content_hash(b"w"),
            &user(1),
            &request("req-a"),
        )
        .await
        .unwrap();
    assert_eq!(
        rig.tool_writes
            .register(
                &mut Tx::from_pg(&mut conn),
                &rig.swimlane,
                &path("svc/boom.yml"),
                &content_hash(b"w"),
                &user(1),
                &request("req-b"),
            )
            .await,
        Err(ToolWriteError::Unavailable)
    );
    rig.audit
        .record(&mut Tx::from_pg(&mut conn), own_audit_event(&rig, "svc/a.yml"))
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert_eq!(registrations(&rig.pool).await, 1);
    assert_eq!(audit_rows(&rig.pool).await.len(), 1);
}

#[tokio::test]
async fn with_no_transaction_open_a_registration_is_unavailable_and_registers_nothing() {
    let Some(rig) = Rig::new().await else { return };
    let mut conn = rig.pool.acquire().await.unwrap();
    // No BEGIN: a savepoint cannot be made, and a registration made on its own would be a write the caller could not roll back.
    let result = rig
        .tool_writes
        .register(
            &mut Tx::from_pg(&mut conn),
            &rig.swimlane,
            &path("svc/a.yml"),
            &content_hash(b"w"),
            &user(1),
            &request("req-a"),
        )
        .await;
    assert_eq!(result, Err(ToolWriteError::Unavailable));
    drop(conn);
    assert_eq!(registrations(&rig.pool).await, 0);
}

#[tokio::test]
async fn a_transaction_that_is_already_aborted_is_unavailable_and_nothing_else_changes() {
    let Some(rig) = Rig::new().await else { return };
    let mut tx = rig.pool.begin().await.unwrap();
    // The caller's own statement fails: the transaction is aborted (25P02 for everything that follows).
    assert!(sqlx::query("SELECT 1/0").execute(&mut *tx).await.is_err());
    let result = rig
        .tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &content_hash(b"w"),
            &user(1),
            &request("req-a"),
        )
        .await;
    assert_eq!(result, Err(ToolWriteError::Unavailable));
    tx.rollback().await.unwrap();
    assert_eq!(registrations(&rig.pool).await, 0);
}

#[tokio::test]
async fn a_transaction_of_another_store_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    let mut state = FakeTxState::new();
    let mut tx = Tx::in_memory(&mut state);
    let w = content_hash(b"w");
    assert_eq!(
        rig.tool_writes
            .register(
                &mut tx,
                &rig.swimlane,
                &path("svc/a.yml"),
                &w,
                &user(1),
                &request("req-a")
            )
            .await,
        Err(ToolWriteError::WrongTx)
    );
    assert_eq!(
        rig.tool_writes
            .register_delete(
                &mut tx,
                &rig.swimlane,
                &path("svc/a.yml"),
                &w,
                &user(1),
                &request("req-a")
            )
            .await,
        Err(ToolWriteError::WrongTx),
        "register_delete refuses it too: the port's default would have said Ok"
    );
    assert_eq!(registrations(&rig.pool).await, 0);
    assert_eq!(rig.metrics.tool_write_count(ToolWriteResult::WrongTx), 2);
}

#[tokio::test]
async fn the_stamp_is_the_registry_s_clock_and_never_the_databases() {
    let Some(rig) = Rig::new().await else { return };
    let mut tx = rig.pool.begin().await.unwrap();
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &content_hash(b"w"),
            &user(1),
            &request("req-a"),
        )
        .await
        .unwrap();
    rig.clock.advance_ms(1_234);
    rig.tool_writes
        .register_delete(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/b.yml"),
            &content_hash(b"d"),
            &user(1),
            &request("req-b"),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stamps: Vec<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM registered_at) * 1000)::bigint FROM tool_write_registrations ORDER BY id LIMIT 10",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        stamps,
        vec![ManualClock::T0_MS, ManualClock::T0_MS + 1_234],
        "the injected clock, to the millisecond, for both kinds (the test clock is far from the database's own `now()`)"
    );
}

#[tokio::test]
async fn an_expired_registration_does_not_count_towards_the_bound() {
    let config = RegistryConfig {
        tool_write_max_pending: 3,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    let register = |n: u32| {
        let rig = &rig;
        async move {
            let mut tx = rig.pool.begin().await.unwrap();
            let r = rig
                .tool_writes
                .register(
                    &mut Tx::from_pg(&mut tx),
                    &rig.swimlane,
                    &path(&format!("svc/f{n}.yml")),
                    &content_hash(format!("w{n}").as_bytes()),
                    &user(1),
                    &request(&format!("req-{n}")),
                )
                .await;
            tx.commit().await.unwrap();
            r
        }
    };
    for n in 0..3 {
        register(n).await.unwrap();
        // One minute apart: the first expires first.
        rig.clock.advance_secs(60);
    }
    assert_eq!(
        register(3).await,
        Err(ToolWriteError::Full),
        "three are younger than the TTL"
    );
    // The oldest was stamped 3 minutes ago: move to 15 minutes after its stamp, and not a millisecond before.
    rig.clock.set_ms(ManualClock::T0_MS + 15 * 60 * 1000 - 1);
    assert_eq!(
        register(3).await,
        Err(ToolWriteError::Full),
        "the oldest is 1 ms short of the TTL: it counts"
    );
    rig.clock.advance_ms(1);
    register(3).await.expect("the oldest has expired: there is room");
    assert_eq!(
        rig.count("SELECT count(*) FROM tool_write_registrations").await,
        4,
        "the expired row is still in the table (the sweep deletes it), and it takes no place"
    );
}

#[tokio::test]
async fn a_hash_registration_and_a_delete_registration_share_the_one_queue() {
    let config = RegistryConfig {
        tool_write_max_pending: 2,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    let mut tx = rig.pool.begin().await.unwrap();
    let w = content_hash(b"w");
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &w,
            &user(1),
            &request("r-1"),
        )
        .await
        .unwrap();
    rig.tool_writes
        .register_delete(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/b.yml"),
            &w,
            &user(1),
            &request("r-2"),
        )
        .await
        .unwrap();
    assert_eq!(
        rig.tool_writes
            .register(
                &mut Tx::from_pg(&mut tx),
                &rig.swimlane,
                &path("svc/c.yml"),
                &w,
                &user(1),
                &request("r-3")
            )
            .await,
        Err(ToolWriteError::Full)
    );
    assert_eq!(
        rig.tool_writes
            .register_delete(
                &mut Tx::from_pg(&mut tx),
                &rig.swimlane,
                &path("svc/d.yml"),
                &w,
                &user(1),
                &request("r-4")
            )
            .await,
        Err(ToolWriteError::Full)
    );
    // `Full` is not a failed transaction: the caller records its audit event.
    rig.audit
        .record(&mut Tx::from_pg(&mut tx), own_audit_event(&rig, "svc/c.yml"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rig.metrics.tool_write_count(ToolWriteResult::Full), 2);
}

#[tokio::test]
async fn which_registration_is_the_oldest_is_decided_by_the_order_of_insertion_and_not_by_the_stamp() {
    // Two replicas with clocks that disagree: the second registration is stamped earlier than the first.
    let Some(rig) = Rig::new().await else { return };
    let w = content_hash(b"w");
    let mut tx = rig.pool.begin().await.unwrap();
    rig.clock.advance_secs(100);
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &w,
            &user(1),
            &request("req-fast"),
        )
        .await
        .unwrap();
    rig.clock.set_ms(ManualClock::T0_MS);
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path("svc/a.yml"),
            &w,
            &user(2),
            &request("req-slow"),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT request_id, (EXTRACT(EPOCH FROM registered_at) * 1000)::bigint FROM tool_write_registrations ORDER BY id LIMIT 10",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        rows[0].0, "req-fast",
        "inserted first: the lowest id is the oldest registration"
    );
    assert!(rows[0].1 > rows[1].1, "although its stamp is the later one");
}

#[tokio::test]
async fn no_path_hash_or_user_is_in_a_log_line() {
    let Some(rig) = Rig::new().await else { return };
    make_boom_fail(&rig).await;
    let config = RegistryConfig {
        tool_write_max_pending: 1,
        ..RegistryConfig::default()
    };
    let small = Rig::with("sit1", config).await;
    let (capture, _guard) = LogCapture::install();
    let secret_path = "svc/boom.yml";
    let secret_hash = content_hash(b"a very secret value");
    let who = user(4242);
    let mut tx = rig.pool.begin().await.unwrap();
    // A failed statement.
    let _ = rig
        .tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path(secret_path),
            &secret_hash,
            &who,
            &request("req-secret-1"),
        )
        .await;
    // A transaction of another store.
    let mut state = FakeTxState::new();
    let _ = rig
        .tool_writes
        .register(
            &mut Tx::in_memory(&mut state),
            &rig.swimlane,
            &path(secret_path),
            &secret_hash,
            &who,
            &request("req-secret-2"),
        )
        .await;
    tx.rollback().await.unwrap();
    // A full queue.
    if let Some(small) = &small {
        let mut tx = small.pool.begin().await.unwrap();
        small
            .tool_writes
            .register(
                &mut Tx::from_pg(&mut tx),
                &small.swimlane,
                &path("svc/one.yml"),
                &secret_hash,
                &who,
                &request("req-1"),
            )
            .await
            .unwrap();
        let _ = small
            .tool_writes
            .register(
                &mut Tx::from_pg(&mut tx),
                &small.swimlane,
                &path(secret_path),
                &secret_hash,
                &who,
                &request("req-secret-3"),
            )
            .await;
        tx.rollback().await.unwrap();
    }
    let text = capture.text();
    assert!(
        text.contains("tool_write_register"),
        "the failed statement is logged as an operation: {text}"
    );
    assert!(text.contains("queue_full"), "{text}");
    for secret in [
        secret_path.to_owned(),
        "svc/".to_owned(),
        hex::encode(secret_hash.as_bytes()),
        who.oid().to_string(),
        who.to_string(),
        "req-secret".to_owned(),
        "forced".to_owned(),
    ] {
        assert!(
            !text.contains(&secret),
            "S10: `{secret}` is in a log line: {text}"
        );
    }
}
