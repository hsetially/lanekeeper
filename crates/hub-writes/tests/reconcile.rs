//! The crash window of a write (plan A1): the file landed on NFS and the transaction that records it did not commit. The
//! claim row keeps what is needed to finish it, and a leased job (or the retry with the same key) completes the write when
//! the file on NFS is the one that was sent, and clears the claim when it is not.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use domain::{Role, SwimlaneId, WriteOutcome};
use ports::{WriteService, verify_chain};
use support::fake_agent::Fault;
use support::pg_tool_writes::registered;
use support::rig::{Rig, applied, at, ctx, edit, h};

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
