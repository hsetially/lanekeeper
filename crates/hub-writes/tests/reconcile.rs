//! The crash window of a write (plan A1): the file landed on NFS and the transaction that records it did not commit. The
//! claim row keeps what is needed to finish it, and a leased job (or the retry with the same key) completes the write when
//! the file on NFS is the one that was sent, and clears the claim when it is not.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;

use domain::{Role, SwimlaneId, WriteOutcome};
use hub_writes::WriteConfig;
use ports::{WriteService, verify_chain};
use support::fake_agent::Fault;
use support::pg_tool_writes::registered;
use support::rig::{Rig, applied, at, ctx, edit, h};
use support::withholding_registry::{Stall, WithholdingRegistry};
use tokio::sync::{Notify, Semaphore};

fn sit1() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

async fn rig_with_a() -> Option<Rig> {
    let rig = Rig::new(support::pg::TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    Some(rig)
}

async fn lose_a_reply(rig: &Rig, key: &str) -> domain::User {
    let editor = rig.user(1, Role::Editor).await;
    rig.agent.fault_next_write(Fault::LandThenTimeout);
    let out = rig
        .hub
        .edit(&ctx(&editor, key), edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n"))
        .await;
    assert!(out.is_err());
    editor
}

#[tokio::test]
async fn reconcile_completes_a_write_that_landed() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = lose_a_reply(&rig, "k-1").await;
    assert_eq!(rig.chain().await.len(), 0);

    // Too young: the first request may still be running.
    let early = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!((early.completed, early.cleared), (0, 0));
    assert_eq!(rig.chain().await.len(), 0);

    rig.advance_secs(61);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!((report.completed, report.cleared), (1, 0), "{report:?}");
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    assert!(verify_chain(&chain).is_ok());
    assert_eq!(chain[0].event.hash_after, Some(h("x: 2\n")));
    assert_eq!(registered(&rig.pool).await.len(), 1);
    assert_eq!(rig.bus.published().await.len(), 1);

    // The caller's retry now replays the audited outcome, and a second pass finds nothing to do.
    let retry = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n"),
        )
        .await
        .unwrap();
    assert_eq!(applied(&retry).0, Some(h("x: 2\n")));
    assert_eq!(applied(&retry).1, chain[0].id);
    let again = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!((again.completed, again.cleared), (0, 0));
    assert_eq!(rig.chain().await.len(), 1);
    assert_eq!(rig.agent.write_commands(), 1);
}

#[tokio::test]
async fn two_reconcilers_complete_a_write_once() {
    let Some(rig) = rig_with_a().await else { return };
    lose_a_reply(&rig, "k-1").await;
    rig.advance_secs(61);
    let other = rig.replica();
    let (first, second) = (rig.hub.reconciler(), other.reconciler());
    let (a, b) = tokio::join!(first.run_once(&|| true), second.run_once(&|| true));
    assert_eq!(a.completed + b.completed, 1, "{a:?} {b:?}");
    assert_eq!(rig.chain().await.len(), 1);
    assert_eq!(registered(&rig.pool).await.len(), 1);
}

#[tokio::test]
async fn reconcile_clears_the_claim_when_the_file_is_not_what_was_sent() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = lose_a_reply(&rig, "k-1").await;
    // Somebody else changed the file after the lost reply: what is there is not what this request sent.
    rig.agent
        .put(&sit1(), &domain::NfsPath::parse("a.yml").unwrap(), b"x: other\n");
    rig.advance_secs(61);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!((report.completed, report.cleared), (0, 1), "{report:?}");
    assert_eq!(rig.chain().await.len(), 0, "no write of ours is audited");
    assert_eq!(rig.count("idempotency_keys").await, 0);
    // The key is free again, and the request is evaluated afresh: the file moved on, so it is a conflict.
    let retry = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n"),
        )
        .await
        .unwrap();
    assert!(matches!(retry, WriteOutcome::Conflict { .. }), "{retry:?}");
}

#[tokio::test]
async fn reconcile_clears_a_claim_without_intent_and_leaves_young_ones() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    // A claim from a request that died before it sent anything: no intent.
    let old = support::rig::START - 120_000;
    sqlx::query(
        "INSERT INTO idempotency_keys (user_tid, user_oid, key, request_hash, created_at, expires_at) \
         VALUES ($1, $2, 'dead', $3, timestamptz 'epoch' + ($4::bigint::text || ' milliseconds')::interval, \
                 timestamptz 'epoch' + ($5::bigint::text || ' milliseconds')::interval)",
    )
    .bind(editor.id.tid().as_str())
    .bind(editor.id.oid().as_str())
    .bind(vec![0_u8; 32])
    .bind(old)
    .bind(old + 86_400_000)
    .execute(&rig.pool)
    .await
    .unwrap();
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!(report.cleared, 1, "{report:?}");
    assert_eq!(rig.count("idempotency_keys").await, 0);
}

#[tokio::test]
async fn reconcile_leaves_the_claim_when_the_agent_cannot_be_asked() {
    let Some(rig) = rig_with_a().await else { return };
    lose_a_reply(&rig, "k-1").await;
    rig.advance_secs(61);
    rig.agent.set_down(true);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!(
        (report.completed, report.cleared, report.unreachable),
        (0, 0, 1),
        "{report:?}"
    );
    assert_eq!(rig.count("idempotency_keys").await, 1, "kept for the next run");
    rig.agent.set_down(false);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!(report.completed, 1);
}

#[tokio::test]
async fn reconcile_stops_when_the_lease_is_lost() {
    let Some(rig) = rig_with_a().await else { return };
    lose_a_reply(&rig, "k-1").await;
    rig.advance_secs(61);
    let report = rig.hub.reconciler().run_once(&|| false).await;
    assert_eq!((report.completed, report.cleared, report.purged), (0, 0, 0));
    assert_eq!(rig.chain().await.len(), 0, "no write without the lease");
}

/// A request whose steps before the agent outlast `stale_claim_after` loses its claim to the reconciler (which finds a claim with
/// no intent and clears it). The request must notice when it tries to keep its intent, and must not send the command: a write
/// that lands then has nothing to be audited under.
#[tokio::test]
async fn a_request_whose_claim_was_cleared_before_the_send_does_not_send() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let stall = Stall {
        entered: Arc::new(Notify::new()),
        gate: Arc::new(Semaphore::new(0)),
    };
    let slow = Arc::new(rig.hub_with(
        WriteConfig::default(),
        Some(Arc::new(
            WithholdingRegistry::new(rig.registry.clone(), Vec::new()).stalling(stall.clone()),
        )),
    ));
    let request = {
        let (slow, editor) = (slow.clone(), editor.clone());
        tokio::spawn(async move {
            slow.edit(
                &ctx(&editor, "k-1"),
                edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n"),
            )
            .await
        })
    };

    // The request has its claim and is reading the file; a minute passes, and another replica's reconciler clears the claim.
    stall.entered.notified().await;
    rig.advance_secs(61);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!((report.completed, report.cleared), (0, 1), "{report:?}");
    stall.gate.add_permits(1);

    let out = request.await.unwrap();
    assert!(out.is_err(), "the request went on without its claim: {out:?}");
    assert_eq!(
        rig.agent.write_commands(),
        0,
        "a write was sent with no claim to record it"
    );
    assert_eq!(rig.agent.writes_applied(), 0);
    assert_eq!(rig.nfs_of(&sit1(), "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(rig.chain().await.len(), 0);
    assert_eq!(rig.count("idempotency_keys").await, 0);

    // The retry with the same key is a new request and succeeds.
    let retry = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n"),
        )
        .await
        .unwrap();
    assert_eq!(applied(&retry).0, Some(h("x: 2\n")));
    assert_eq!(rig.chain().await.len(), 1);
}

/// The reconciler cleared a stalled request's claim and the caller's retry made a new row under the same key. When the stalled
/// request wakes, its claim is not the row that is there: it must not keep an intent in the retry's row (and so send a second
/// command), and must not finish or release it either. The row belongs to the request that made it.
#[tokio::test]
async fn a_request_that_lost_its_claim_cannot_touch_the_row_of_the_retry_that_replaced_it() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let stall = Stall {
        entered: Arc::new(Notify::new()),
        gate: Arc::new(Semaphore::new(0)),
    };
    let slow = Arc::new(rig.hub_with(
        WriteConfig::default(),
        Some(Arc::new(
            WithholdingRegistry::new(rig.registry.clone(), Vec::new()).stalling(stall.clone()),
        )),
    ));
    let first = {
        let (slow, editor) = (slow.clone(), editor.clone());
        tokio::spawn(async move {
            slow.edit(
                &ctx(&editor, "k-1"),
                edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n"),
            )
            .await
        })
    };
    stall.entered.notified().await;
    rig.advance_secs(61);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!((report.completed, report.cleared), (0, 1), "{report:?}");

    // The retry claims the key anew, keeps its intent and sends its command, which the agent holds.
    let gate = rig.agent.hold_writes();
    let retry = {
        let (hub, editor) = (rig.hub.clone(), editor.clone());
        tokio::spawn(async move {
            hub.edit(
                &ctx(&editor, "k-1"),
                edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n"),
            )
            .await
        })
    };
    rig.agent.entered.notified().await;
    assert_eq!(rig.agent.write_commands(), 1);

    // The stalled request wakes. Its claim is gone: it ends without sending anything, and the retry's row is as it was.
    stall.gate.add_permits(1);
    let woke = tokio::time::timeout(std::time::Duration::from_secs(5), first)
        .await
        .expect("the stalled request kept going on a row that is not its own")
        .unwrap();
    assert!(woke.is_err(), "{woke:?}");
    assert_eq!(
        rig.agent.write_commands(),
        1,
        "the stalled request sent a second command"
    );
    assert_eq!(
        rig.count("idempotency_keys").await,
        1,
        "the retry's row is still there"
    );

    gate.add_permits(1);
    let done = retry.await.unwrap().unwrap();
    assert_eq!(applied(&done).0, Some(h("x: 2\n")));
    assert_eq!(rig.agent.writes_applied(), 1);
    assert_eq!(rig.chain().await.len(), 1);
}

/// What a request that lost its claim after it had sent its command does when the agent answers: it ends its own row, which is
/// gone, and not the retry's. `how` is how the agent answers the first command: a conflict (the request stores that answer) or an
/// I/O error (the request releases its claim).
async fn the_late_answer_of_a_request_that_lost_its_claim(how: &str) {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let gate = rig.agent.hold_writes();
    let req = || edit(&sit1(), "a.yml", at("x: 1\n"), "x: 2\n");
    if how == "io" {
        rig.agent.fault_next_write(Fault::Io);
    }
    let first = {
        let (hub, editor) = (rig.hub.clone(), editor.clone());
        tokio::spawn(async move { hub.edit(&ctx(&editor, "k-1"), req()).await })
    };
    rig.agent.entered.notified().await;

    // A minute passes with the command at the agent; the file is as it was, so the reconciler clears the claim. The retry makes
    // a new row and sends its own command, which the agent holds behind the first.
    rig.advance_secs(61);
    let report = rig.hub.reconciler().run_once(&|| true).await;
    assert_eq!((report.completed, report.cleared), (0, 1), "{report:?}");
    let retry = {
        let (hub, editor) = (rig.hub.clone(), editor.clone());
        tokio::spawn(async move { hub.edit(&ctx(&editor, "k-1"), req()).await })
    };
    // (`entered` keeps a permit from the first command, so it cannot say that the second has arrived.)
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while rig.agent.write_commands() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the retry's command never reached the agent");

    // The first command is answered: with a conflict (somebody changed the file), or with an I/O error.
    if how == "conflict" {
        rig.agent
            .put(&sit1(), &domain::NfsPath::parse("a.yml").unwrap(), b"y: 1\n");
    }
    gate.add_permits(1);
    let answered = first.await.unwrap();
    match how {
        "conflict" => assert!(
            matches!(answered, Ok(WriteOutcome::Conflict { .. })),
            "{answered:?}"
        ),
        _ => assert!(
            matches!(answered, Ok(WriteOutcome::Rejected { .. })),
            "{answered:?}"
        ),
    }
    assert_eq!(
        rig.count("idempotency_keys").await,
        1,
        "the retry's row survives the other request's end"
    );
    let finished: i64 =
        sqlx::query_scalar("SELECT count(*) FROM idempotency_keys WHERE response_status IS NOT NULL")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(
        finished, 0,
        "the retry's row was finished by a request that is not its own"
    );

    // The file is as the retry expects it, and its command lands: recorded under its own claim.
    rig.agent
        .put(&sit1(), &domain::NfsPath::parse("a.yml").unwrap(), b"x: 1\n");
    gate.add_permits(1);
    let done = retry.await.unwrap().unwrap();
    assert_eq!(applied(&done).0, Some(h("x: 2\n")), "{done:?}");
    assert_eq!(rig.chain().await.len(), 1, "the write that landed is audited");
}

#[tokio::test]
async fn a_conflict_for_a_request_that_lost_its_claim_is_not_stored_on_the_retrys_row() {
    the_late_answer_of_a_request_that_lost_its_claim("conflict").await;
}

#[tokio::test]
async fn a_request_that_lost_its_claim_does_not_release_the_retrys_row() {
    the_late_answer_of_a_request_that_lost_its_claim("io").await;
}
