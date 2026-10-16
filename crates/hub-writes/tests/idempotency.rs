//! `Idempotency-Key` (D68): the same key and request replay the stored response, another request with the key is refused, a
//! request in flight is waited for once, rows live 24 hours, and what is not an outcome is not stored.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use domain::{Expected, Role, SwimlaneId, WriteOutcome};
use hub_writes::WriteConfig;
use ports::{WriteError, WriteService};
use support::rig::{Rig, applied, at, ctx, edit, h};

async fn rig_with_a() -> Option<Rig> {
    let rig = Rig::new(support::pg::TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n"), ("b.yml", "k: v\n")]);
    Some(rig)
}

fn sit1() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

#[tokio::test]
async fn same_key_same_request_replays_stored_response_and_writes_once() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = sit1();
    let req = edit(&s, "a.yml", at("x: 1\n"), "x: 2\n");
    let first = rig.hub.edit(&ctx(&editor, "k-1"), req.clone()).await.unwrap();
    assert_eq!(applied(&first).0, Some(h("x: 2\n")));
    let again = rig.hub.edit(&ctx(&editor, "k-1"), req.clone()).await.unwrap();
    assert_eq!(again, first, "the stored response, audit id included");
    // Another replica serves the replay too: the response is in the database, not in memory.
    let other = rig.replica().edit(&ctx(&editor, "k-1"), req).await.unwrap();
    assert_eq!(other, first);
    assert_eq!(rig.agent.write_commands(), 1);
    assert_eq!(rig.agent.writes_applied(), 1);
    assert_eq!(rig.chain().await.len(), 1);
    assert_eq!(rig.bus.published().await.len(), 1);
}

#[tokio::test]
async fn a_replayed_conflict_is_the_first_conflict() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = sit1();
    let req = edit(&s, "a.yml", at("old: 0\n"), "x: 2\n");
    let first = rig.hub.edit(&ctx(&editor, "k-1"), req.clone()).await.unwrap();
    assert!(matches!(first, WriteOutcome::Conflict { .. }));
    // The file changes; the same key and request still answer what they answered the first time.
    rig.agent
        .put(&s, &domain::NfsPath::parse("a.yml").unwrap(), b"old: 0\n");
    let again = rig.hub.edit(&ctx(&editor, "k-1"), req).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(rig.agent.write_commands(), 1, "the replay does not ask the agent");
}

#[tokio::test]
async fn same_key_different_request_is_422_and_writes_nothing() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = sit1();
    rig.hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    // Another file, another content, another expected hash, another swimlane: each is another request.
    for other in [
        edit(&s, "b.yml", at("k: v\n"), "k: w\n"),
        edit(&s, "a.yml", at("x: 1\n"), "x: 3\n"),
        edit(&s, "a.yml", at("x: 2\n"), "x: 2\n"),
        edit(&s, "a.yml", Expected::Absent, "x: 2\n"),
        edit(
            &SwimlaneId::parse("sit2").unwrap(),
            "a.yml",
            at("x: 1\n"),
            "x: 2\n",
        ),
    ] {
        let out = rig.hub.edit(&ctx(&editor, "k-1"), other).await;
        assert_eq!(out, Err(WriteError::IdempotencyConflict));
    }
    assert_eq!(rig.agent.write_commands(), 1);
    assert_eq!(rig.nfs_of(&s, "b.yml").as_deref(), Some(&b"k: v\n"[..]));
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn keys_belong_to_the_user_who_sent_them() {
    let Some(rig) = rig_with_a().await else { return };
    let one = rig.user(1, Role::Editor).await;
    let two = rig.user(2, Role::Editor).await;
    let s = sit1();
    let first = rig
        .hub
        .edit(&ctx(&one, "shared"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert!(matches!(first, WriteOutcome::Applied { .. }));
    // Another user's request with the same key text is a request of its own (here a conflict, because the file moved on).
    let theirs = rig
        .hub
        .edit(&ctx(&two, "shared"), edit(&s, "a.yml", at("x: 1\n"), "x: 9\n"))
        .await
        .unwrap();
    assert!(matches!(theirs, WriteOutcome::Conflict { .. }), "{theirs:?}");
}

#[tokio::test]
async fn concurrent_same_key_executes_once() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = sit1();
    let gate = rig.agent.hold_writes();
    let req = edit(&s, "a.yml", at("x: 1\n"), "x: 2\n");
    let (ca, cb) = (ctx(&editor, "k-1"), ctx(&editor, "k-1"));
    let a = rig.hub.edit(&ca, req.clone());
    let b = rig.hub.edit(&cb, req);
    let release = async {
        rig.agent.entered.notified().await;
        gate.add_permits(1);
    };
    let (ra, rb, ()) = tokio::join!(a, b, release);
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    assert!(matches!(ra, WriteOutcome::Applied { .. }), "{ra:?}");
    assert_eq!(ra, rb, "both callers get the one outcome");
    assert_eq!(rig.agent.write_commands(), 1);
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn a_request_in_flight_that_does_not_finish_answers_unavailable_after_the_wait() {
    let config = WriteConfig {
        idempotency_wait: Duration::from_millis(200),
        ..WriteConfig::default()
    };
    let rig = Rig::with_config(db!(), config).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    let editor = rig.user(1, Role::Editor).await;
    let gate = rig.agent.hold_writes();
    let req = edit(&s, "a.yml", at("x: 1\n"), "x: 2\n");
    let (c1, c2) = (ctx(&editor, "k-1"), ctx(&editor, "k-1"));
    let first = rig.hub.edit(&c1, req.clone());
    let second = async {
        rig.agent.entered.notified().await;
        let out = rig.hub.edit(&c2, req.clone()).await;
        gate.add_permits(1);
        out
    };
    let (first, second) = tokio::join!(first, second);
    assert_eq!(
        second,
        Err(WriteError::Unavailable),
        "the caller is told to retry"
    );
    assert!(matches!(first.unwrap(), WriteOutcome::Applied { .. }));
    // Once the first request has finished, the retry gets its answer.
    let replay = rig.hub.edit(&ctx(&editor, "k-1"), req).await.unwrap();
    assert!(matches!(replay, WriteOutcome::Applied { .. }));
    assert_eq!(rig.agent.write_commands(), 1);
}

#[tokio::test]
async fn idempotency_rows_expire_after_24_hours() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = sit1();
    let first = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    // Just inside the day the key still replays, and another request with it is still refused.
    rig.advance_secs(24 * 3600 - 1);
    let same = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(same, first);
    assert_eq!(
        rig.hub
            .edit(&ctx(&editor, "k-1"), edit(&s, "b.yml", at("k: v\n"), "k: w\n"))
            .await,
        Err(WriteError::IdempotencyConflict)
    );
    // A day after it was made the row is gone: the key is free for another request.
    rig.advance_secs(2);
    let later = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "b.yml", at("k: v\n"), "k: w\n"))
        .await
        .unwrap();
    assert!(matches!(later, WriteOutcome::Applied { .. }), "{later:?}");

    // The sweep removes the finished, expired rows and never an unfinished one.
    rig.advance_secs(24 * 3600 + 1);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert!(report.purged >= 1, "{report:?}");
    assert_eq!(rig.count("idempotency_keys").await, 0);
}

#[tokio::test]
async fn transient_outcomes_are_not_stored() {
    let rig = Rig::new(db!()).await;
    let s = rig.swimlane("sit1", false).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    let editor = rig.user(1, Role::Editor).await;
    let locked = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(locked, WriteOutcome::Locked { until: None });
    assert_eq!(rig.count("idempotency_keys").await, 0, "a lock leaves no row");
    // Another request may then use the same key, because the first one never happened.
    sqlx::query("UPDATE swimlanes SET editing_unlocked = true")
        .execute(&rig.pool)
        .await
        .unwrap();
    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 5\n"))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }));
}

#[tokio::test]
async fn the_row_holds_a_hash_of_the_request_and_never_its_content() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = sit1();
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&s, "a.yml", at("x: 1\n"), "unique-body-marker: 4242\n"),
        )
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }));
    let (request_hash, body): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT request_hash, response_body FROM idempotency_keys WHERE key = 'k-1'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(request_hash.len(), 32);
    let text = String::from_utf8_lossy(&body);
    assert!(
        !text.contains("unique-body-marker"),
        "the stored response holds no content"
    );
    let expires: i64 =
        sqlx::query_scalar("SELECT (extract(epoch from expires_at) * 1000)::bigint FROM idempotency_keys")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(expires, support::rig::START + 24 * 3600 * 1000);
}
