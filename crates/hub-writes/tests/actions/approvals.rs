//! T4: approvals (D37). Only an admin decides, never the author; the file is looked at again and a change makes the proposal
//! stale; a leased job expires the ones nobody decided; every transition publishes `ProposalChanged`; the audit event of an
//! approved change names both the author and the approver. All of it over a real Postgres, with the production grants.
#![allow(clippy::too_many_lines)]

use bytes::Bytes;
use domain::{
    DomainEvent, Page, ProposalId, ProposalStatus, RejectReason, RestartRequest, Role, ServiceRef, ShortText,
    SwimlaneId, User, UserStatus, WriteOutcome,
};
use hub_writes::{NO_HASH, ProposalDesk, ProposalQuery, RejectResult};
use ports::{
    AuditAction, AuditActor, AuditVia, BlobStore, Leases as _, WriteError, WriteService, verify_chain,
};

use crate::support::pg::sqlstate;
use crate::support::rig::{Rig, applied, at, ctx, edit, h};

const AWS_KEY_PARTS: [&str; 2] = ["AKIA", "IOSFODNN7EXAMPLE"];

fn aws_key() -> String {
    AWS_KEY_PARTS.concat()
}

fn text(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

fn rejected(reason: RejectReason) -> WriteOutcome {
    WriteOutcome::Rejected {
        reason,
        issues: Vec::new(),
    }
}

/// A swimlane with a few files, an admin, an editor who needs approval, and one who does not.
struct Lane {
    rig: Rig,
    s: SwimlaneId,
    admin: User,
    careful: User,
}

async fn lane() -> Option<Lane> {
    lane_with(hub_writes::WriteConfig::default()).await
}

async fn lane_with(config: hub_writes::WriteConfig) -> Option<Lane> {
    let rig = Rig::with_config(crate::support::pg::TestDb::ready().await?, config).await;
    let s = rig.swimlane("sit1", true).await;
    rig.tenant(&s, "sit1").await;
    rig.seed(
        &s,
        &[
            ("a.yml", "x: 1\n"),
            ("b.yml", "y: 1\n"),
            ("c.yml", "z: 1\n"),
            ("old.txt", "keep\n"),
        ],
    );
    rig.service(&s, "sit1", "payments", Some("payments")).await;
    let admin = rig.user(1, Role::Admin).await;
    let careful = rig.approval_user(2, Role::Editor).await;
    Some(Lane {
        rig,
        s,
        admin,
        careful,
    })
}

impl Lane {
    /// The careful editor proposes `content` for `path`, which is `from` now.
    async fn propose(&self, key: &str, path: &str, from: &str, content: &str) -> ProposalId {
        let out = self
            .rig
            .hub
            .edit(&ctx(&self.careful, key), edit(&self.s, path, at(from), content))
            .await
            .unwrap();
        match out {
            WriteOutcome::ProposalCreated { id } => id,
            other => panic!("expected a proposal, got {other:?}"),
        }
    }

    async fn approve(&self, who: &User, key: &str, id: ProposalId) -> Result<WriteOutcome, WriteError> {
        self.rig.hub.approve_proposal(&ctx(who, key), id, None).await
    }

    async fn status(&self, id: ProposalId) -> String {
        sqlx::query_scalar("SELECT status FROM proposals WHERE id = $1")
            .bind(id.get())
            .fetch_one(&self.rig.pool)
            .await
            .unwrap()
    }

    /// The statuses `ProposalChanged` was published with for this proposal, in order.
    async fn transitions(&self, id: ProposalId) -> Vec<ProposalStatus> {
        self.rig
            .bus
            .published()
            .await
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::ProposalChanged { id: got, status } if got == id => Some(status),
                _ => None,
            })
            .collect()
    }
}

// ------------------------------------------------------------------------------------------------------ who may decide

#[tokio::test]
async fn only_admin_approves() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    let editor = l.rig.user(3, Role::Editor).await;
    let operator = l.rig.user(4, Role::Operator).await;
    let viewer = l.rig.user(5, Role::Viewer).await;
    let pending = l
        .rig
        .user_with_status(6, Some(Role::Admin), UserStatus::Pending)
        .await;
    let disabled = l
        .rig
        .user_with_status(7, Some(Role::Admin), UserStatus::Disabled)
        .await;
    let roleless = l.rig.user_with_status(8, None, UserStatus::Active).await;
    // A request whose snapshot says Admin for somebody the directory knows as an editor.
    let mut forged = editor.clone();
    forged.role = Some(Role::Admin);

    for (who, name) in [
        (&editor, "editor"),
        (&operator, "operator"),
        (&viewer, "viewer"),
        (&pending, "pending admin"),
        (&disabled, "disabled admin"),
        (&roleless, "no role"),
        (&forged, "forged snapshot"),
    ] {
        assert_eq!(
            l.approve(who, "k", id).await,
            Err(WriteError::Forbidden),
            "approve as {name}"
        );
        assert_eq!(
            l.rig.hub.reject_proposal(&ctx(who, "k"), id, text("no")).await,
            Err(WriteError::Forbidden),
            "reject as {name}"
        );
    }
    // Nothing was claimed, sent, decided or recorded.
    assert_eq!(l.status(id).await, "pending");
    assert_eq!(l.rig.count("approvals").await, 0);
    assert_eq!(l.rig.agent.write_commands(), 0);
    assert_eq!(
        l.rig.chain().await.len(),
        1,
        "only the audit event of the proposal"
    );
    assert_eq!(
        l.rig.count("idempotency_keys").await,
        1,
        "only the claim of the proposal"
    );

    // An admin who did not write it can.
    let out = l.approve(&l.admin, "k-ok", id).await.unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    assert_eq!(l.status(id).await, "applied");
}

#[tokio::test]
async fn author_cannot_approve_own_proposal() {
    let Some(l) = lane().await else { return };
    // An admin who needs approval anyway (the flag is the user's, whatever the role).
    let admin_author = l.rig.approval_user(9, Role::Admin).await;
    let id = match l
        .rig
        .hub
        .edit(
            &ctx(&admin_author, "p-1"),
            edit(&l.s, "a.yml", at("x: 1\n"), "x: 2\n"),
        )
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        l.approve(&admin_author, "k-1", id).await,
        Err(WriteError::Forbidden)
    );
    assert_eq!(
        l.rig
            .hub
            .reject_proposal(&ctx(&admin_author, "k-2"), id, text("mine"))
            .await,
        Err(WriteError::Forbidden),
        "nor reject it"
    );
    assert_eq!(l.status(id).await, "pending");
    assert_eq!(l.rig.count("approvals").await, 0);
    assert_eq!(l.rig.agent.write_commands(), 0);
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));

    // Another admin can.
    let out = l.approve(&l.admin, "k-3", id).await.unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }));
}

#[tokio::test]
async fn the_database_refuses_self_approval_and_changes_to_a_decision() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    // The insert trigger: a decision by the author is refused even when the application code is bypassed.
    let own = sqlx::query(
        "INSERT INTO approvals (proposal_id, decision, approver_tid, approver_oid) \
         SELECT id, 'approved', author_tid, author_oid FROM proposals WHERE id = $1",
    )
    .bind(id.get())
    .execute(&l.rig.pool)
    .await
    .unwrap_err();
    assert_eq!(sqlstate(&own).as_deref(), Some("23514"));

    // The author of a proposal cannot be changed (migration 0004), so a proposal cannot be handed to somebody else to approve.
    let swap = sqlx::query("UPDATE proposals SET author_oid = $2 WHERE id = $1")
        .bind(id.get())
        .bind(l.admin.id.oid().as_str())
        .execute(&l.rig.pool)
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&swap).as_deref(), Some("23514"));

    // A decision is written once: the application role has no UPDATE or DELETE on it.
    l.approve(&l.admin, "k-1", id).await.unwrap();
    for sql in [
        "UPDATE approvals SET decision = 'rejected'",
        "UPDATE approvals SET approver_oid = approver_oid",
        "DELETE FROM approvals",
    ] {
        let e = sqlx::query(sql).execute(&l.rig.pool).await.unwrap_err();
        assert_eq!(sqlstate(&e).as_deref(), Some("42501"), "{sql}");
    }
}

// ----------------------------------------------------------------------------------------------------------- applying

#[tokio::test]
async fn approval_event_names_author_and_approver() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    assert_eq!(l.status(id).await, "pending");
    assert_eq!(
        l.rig.nfs_of(&l.s, "a.yml").as_deref(),
        Some(&b"x: 1\n"[..]),
        "a proposal writes nothing"
    );

    let out = l
        .rig
        .hub
        .approve_proposal(&ctx(&l.admin, "k-1"), id, Some(text("looks right")))
        .await
        .unwrap();
    let (hash, audit_id) = applied(&out);
    assert_eq!(hash, Some(h("x: 2\n")));
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(l.rig.agent.write_commands(), 1);

    // One audit event for the approval, the change itself, with the approver as actor and the proposer as author.
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 2, "the proposal's event and the approval's");
    assert!(verify_chain(&chain).is_ok());
    let e = &chain[1].event;
    assert_eq!(chain[1].id, audit_id);
    assert_eq!(e.action, AuditAction::FileEdited);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: l.admin.id.clone()
        }
    );
    assert_eq!(e.author.as_ref(), Some(&l.careful.id));
    assert_eq!(e.approval, Some(id));
    assert_eq!(e.hash_before, Some(h("x: 1\n")));
    assert_eq!(e.hash_after, Some(h("x: 2\n")));
    assert_eq!(e.via, AuditVia::Ui);
    assert!(
        e.diff
            .as_deref()
            .is_some_and(|d| d.contains("-x: 1") && d.contains("+x: 2"))
    );
    // The proposal's own event names its author as the actor.
    assert_eq!(chain[0].event.action, AuditAction::ProposalCreated);
    assert_eq!(
        chain[0].event.actor,
        AuditActor::User {
            user: l.careful.id.clone()
        }
    );

    // The decision is written once, with the hash the re-check found.
    let (decision, approver, reason, rechecked): (String, String, Option<String>, Option<Vec<u8>>) =
        sqlx::query_as(
            "SELECT decision, approver_oid, reason, rechecked_hash FROM approvals WHERE proposal_id = $1",
        )
        .bind(id.get())
        .fetch_one(&l.rig.pool)
        .await
        .unwrap();
    assert_eq!(decision, "approved");
    assert_eq!(approver, l.admin.id.oid().as_str());
    assert_eq!(reason.as_deref(), Some("looks right"));
    assert_eq!(rechecked, Some(h("x: 1\n").as_bytes().to_vec()));
    assert_eq!(l.status(id).await, "applied");
    let applied_at: Option<i64> = sqlx::query_scalar(
        "SELECT (extract(epoch from applied_at) * 1000)::bigint FROM proposals WHERE id = $1",
    )
    .bind(id.get())
    .fetch_one(&l.rig.pool)
    .await
    .unwrap();
    assert_eq!(applied_at, Some(crate::support::rig::START));

    // The tool write is registered, and the screens are told: the file, then the proposal.
    assert_eq!(
        crate::support::pg_tool_writes::registered(&l.rig.pool).await,
        vec![("a.yml".to_owned(), h("x: 2\n").to_string())]
    );
    assert_eq!(
        l.transitions(id).await,
        vec![ProposalStatus::Pending, ProposalStatus::Applied]
    );
}

#[tokio::test]
async fn approving_twice_with_one_key_writes_once_and_a_second_admin_finds_it_decided() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    let first = l.approve(&l.admin, "k-1", id).await.unwrap();
    let replay = l.approve(&l.admin, "k-1", id).await.unwrap();
    assert_eq!(first, replay, "the same key, the same answer");
    assert_eq!(l.rig.agent.write_commands(), 1);
    assert_eq!(l.rig.chain().await.len(), 2);

    // Another key is another request: the proposal is not pending any more, and nothing happens.
    let second = l.rig.user(10, Role::Admin).await;
    assert_eq!(
        l.approve(&second, "k-2", id).await.unwrap(),
        rejected(RejectReason::NotAllowed)
    );
    assert_eq!(l.rig.agent.write_commands(), 1);
    assert_eq!(l.rig.chain().await.len(), 2);

    // The same key for another proposal is refused.
    let other = l.propose("p-2", "b.yml", "y: 1\n", "y: 2\n").await;
    assert_eq!(
        l.approve(&l.admin, "k-1", other).await,
        Err(WriteError::IdempotencyConflict)
    );
    // And a proposal that does not exist is not found.
    assert_eq!(
        l.approve(&l.admin, "k-3", ProposalId::from(9_999)).await,
        Err(WriteError::NotFound)
    );
}

#[tokio::test]
async fn two_admins_at_once_apply_it_once() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    let second = l.rig.user(10, Role::Admin).await;
    let (a, b) = tokio::join!(l.approve(&l.admin, "k-1", id), l.approve(&second, "k-2", id));
    let outcomes = [a.unwrap(), b.unwrap()];
    let wins = outcomes
        .iter()
        .filter(|o| matches!(o, WriteOutcome::Applied { .. }))
        .count();
    assert_eq!(wins, 1, "{outcomes:?}");
    // The other one met the lease or a decided proposal; neither is a conflict, and neither marked it stale.
    for o in &outcomes {
        assert!(
            matches!(
                o,
                WriteOutcome::Applied { .. } | WriteOutcome::Locked { .. } | WriteOutcome::Rejected { .. }
            ),
            "{o:?}"
        );
    }
    assert_eq!(l.status(id).await, "applied");
    assert_eq!(l.rig.agent.write_commands(), 1);
    assert_eq!(l.rig.count("approvals").await, 1);
    assert_eq!(l.rig.chain().await.len(), 2);
}

#[tokio::test]
async fn two_admins_at_once_restart_the_service_once() {
    let Some(l) = lane().await else { return };
    let careful_operator = l.rig.approval_user(11, Role::Operator).await;
    let WriteOutcome::ProposalCreated { id } = l
        .rig
        .hub
        .restart(
            &ctx(&careful_operator, "r-1"),
            RestartRequest {
                swimlane: l.s.clone(),
                service: ServiceRef::new("sit1", "payments").unwrap(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("not a proposal")
    };
    let second = l.rig.user(10, Role::Admin).await;
    let (a, b) = tokio::join!(l.approve(&l.admin, "k-1", id), l.approve(&second, "k-2", id));
    let wins = [a.unwrap(), b.unwrap()]
        .iter()
        .filter(|o| matches!(o, WriteOutcome::Applied { .. }))
        .count();
    assert_eq!(wins, 1);
    assert_eq!(
        l.rig.agent.restarts_applied(),
        1,
        "a restart is not repeated by the second admin"
    );
    assert_eq!(l.rig.chain().await.len(), 2);
}

#[tokio::test]
async fn a_decision_is_announced_to_the_channel_without_content() {
    let Some(l) = lane().await else { return };
    let applied_id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    let rejected_id = l.propose("p-2", "b.yml", "y: 1\n", "y: 2\n").await;
    let before = l.rig.notifier.sent().len();
    l.approve(&l.admin, "k-1", applied_id).await.unwrap();
    l.rig
        .hub
        .reject_proposal(&ctx(&l.admin, "k-2"), rejected_id, text("no"))
        .await
        .unwrap();
    let sent = l.rig.notifier.sent();
    assert_eq!(sent.len(), before + 2);
    for n in &sent[before..] {
        assert_eq!(n.kind, ports::NotificationKind::ProposalDecided);
        let shown = format!("{n:?}");
        assert!(!shown.contains("x: 2") && !shown.contains("y: 2"), "{shown}");
    }
}

#[tokio::test]
async fn approval_rechecks_the_hash_and_marks_stale() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    // Somebody else changes the file after the proposal was made.
    let editor = l.rig.user(3, Role::Editor).await;
    l.rig
        .hub
        .edit(&ctx(&editor, "e-1"), edit(&l.s, "a.yml", at("x: 1\n"), "x: 9\n"))
        .await
        .unwrap();
    let writes_before = l.rig.agent.write_commands();
    let chain_before = l.rig.chain().await.len();

    let out = l.approve(&l.admin, "k-1", id).await.unwrap();
    assert_eq!(
        out,
        WriteOutcome::Conflict {
            current: Some(h("x: 9\n"))
        }
    );
    // Nothing was written: the file is as the other editor left it and no write command went out.
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 9\n"[..]));
    assert_eq!(l.rig.agent.write_commands(), writes_before);
    assert_eq!(l.status(id).await, "stale");

    // One audit event, naming the proposer and the approver, and the hash that was found.
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), chain_before + 1);
    assert!(verify_chain(&chain).is_ok());
    let e = &chain.last().unwrap().event;
    assert_eq!(e.action, AuditAction::ProposalStale);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: l.admin.id.clone()
        }
    );
    assert_eq!(e.author.as_ref(), Some(&l.careful.id));
    assert_eq!(e.approval, Some(id));
    assert_eq!(e.hash_before, Some(h("x: 1\n")));
    assert_eq!(e.hash_after, Some(h("x: 9\n")));
    let rechecked: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT rechecked_hash FROM approvals WHERE proposal_id = $1")
            .bind(id.get())
            .fetch_one(&l.rig.pool)
            .await
            .unwrap();
    assert_eq!(rechecked, Some(h("x: 9\n").as_bytes().to_vec()));
    assert_eq!(
        l.transitions(id).await,
        vec![ProposalStatus::Pending, ProposalStatus::Stale]
    );

    // The answer is the first request's for the same key; a stale proposal cannot be approved again.
    assert_eq!(l.approve(&l.admin, "k-1", id).await.unwrap(), out);
    assert_eq!(
        l.approve(&l.admin, "k-2", id).await.unwrap(),
        rejected(RejectReason::NotAllowed)
    );
    assert_eq!(l.rig.chain().await.len(), chain_before + 1);
}

#[tokio::test]
async fn approval_on_changed_file_marks_stale_and_writes_nothing() {
    let Some(l) = lane().await else { return };
    // The proposal creates a file; somebody created it first.
    let create = match l
        .rig
        .hub
        .edit(
            &ctx(&l.careful, "p-1"),
            edit(&l.s, "new.yml", domain::Expected::Absent, "n: 1\n"),
        )
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    l.rig
        .agent
        .put(&l.s, &ports::conformance::sample::nfs("new.yml"), b"n: 0\n");
    assert_eq!(
        l.approve(&l.admin, "k-1", create).await.unwrap(),
        WriteOutcome::Conflict {
            current: Some(h("n: 0\n"))
        }
    );
    assert_eq!(l.status(create).await, "stale");

    // The proposal edits a file; it is gone.
    let edit_gone = l.propose("p-2", "b.yml", "y: 1\n", "y: 2\n").await;
    l.rig
        .agent
        .remove(&l.s, &ports::conformance::sample::nfs("b.yml"));
    assert_eq!(
        l.approve(&l.admin, "k-2", edit_gone).await.unwrap(),
        WriteOutcome::Conflict { current: None }
    );
    assert_eq!(l.status(edit_gone).await, "stale");
    assert_eq!(l.rig.agent.write_commands(), 0, "a stale proposal writes nothing");
    assert_eq!(l.rig.agent.deletes_applied(), 0);
}

#[tokio::test]
async fn a_failed_audit_leaves_the_proposal_pending_and_the_retry_completes_it() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    l.rig.audit.fail_next();
    assert_eq!(l.approve(&l.admin, "k-1", id).await, Err(WriteError::Unavailable));
    // The file landed; nothing of the transaction survived: no decision, still pending, no event for it.
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(l.status(id).await, "pending");
    assert_eq!(l.rig.count("approvals").await, 0);
    assert_eq!(l.rig.chain().await.len(), 1);
    assert_eq!(l.transitions(id).await, vec![ProposalStatus::Pending]);

    // The retry with the same key records it, once, and does not write again.
    let retry = l.approve(&l.admin, "k-1", id).await.unwrap();
    assert_eq!(applied(&retry).0, Some(h("x: 2\n")));
    assert_eq!(l.rig.agent.write_commands(), 1);
    assert_eq!(l.status(id).await, "applied");
    assert_eq!(l.rig.count("approvals").await, 1);
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[1].event.author.as_ref(), Some(&l.careful.id));
    assert_eq!(chain[1].event.approval, Some(id));
}

#[tokio::test]
async fn an_unreachable_agent_changes_nothing_and_a_locked_swimlane_is_423() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    l.rig.agent.set_down(true);
    let out = l.approve(&l.admin, "k-1", id).await.unwrap();
    assert_eq!(out, rejected(RejectReason::AgentUnavailable));
    assert_eq!(
        l.status(id).await,
        "pending",
        "a transient failure is not a decision"
    );
    assert_eq!(l.rig.count("approvals").await, 0);
    l.rig.agent.set_down(false);

    sqlx::query("UPDATE swimlanes SET editing_unlocked = false WHERE id = 'sit1'")
        .execute(&l.rig.pool)
        .await
        .unwrap();
    let locked = l.approve(&l.admin, "k-1", id).await.unwrap();
    assert!(matches!(locked, WriteOutcome::Locked { .. }), "{locked:?}");
    assert_eq!(l.status(id).await, "pending");
    sqlx::query("UPDATE swimlanes SET editing_unlocked = true WHERE id = 'sit1'")
        .execute(&l.rig.pool)
        .await
        .unwrap();
    // With the lock open and the agent back, the same request goes through.
    assert!(matches!(
        l.approve(&l.admin, "k-1", id).await.unwrap(),
        WriteOutcome::Applied { .. }
    ));
}

#[tokio::test]
async fn content_is_checked_again_at_approval() {
    let Some(l) = lane().await else { return };
    // A proposal row that holds a secret (as if the rules had learned a new shape since it was made).
    let content = format!("k: {}\n", aws_key());
    let blob = l.rig.blobs.put(Bytes::from(content)).await.unwrap();
    sqlx::query(
        "INSERT INTO proposals (author_tid, author_oid, action, swimlane_id, paths, base_hashes, new_hash, via, expires_at) \
         SELECT $1, $2, 'edit', 'sit1', ARRAY['a.yml'], jsonb_build_object('a.yml', $3::text), $4, 'ui', now() + interval '1 day'",
    )
    .bind(l.careful.id.tid().as_str())
    .bind(l.careful.id.oid().as_str())
    .bind(h("x: 1\n").to_string())
    .bind(blob.as_bytes().as_slice())
    .execute(&l.rig.pool)
    .await
    .unwrap();
    let id: i64 = sqlx::query_scalar("SELECT max(id) FROM proposals")
        .fetch_one(&l.rig.pool)
        .await
        .unwrap();
    // The expiry of the row above is real time; make it the rig's.
    sqlx::query(
        "UPDATE proposals SET expires_at = to_timestamp($2 / 1000.0) + interval '1 day' WHERE id = $1",
    )
    .bind(id)
    .bind(crate::support::rig::START)
    .execute(&l.rig.pool)
    .await
    .unwrap();
    let out = l.approve(&l.admin, "k-1", ProposalId::from(id)).await.unwrap();
    match out {
        WriteOutcome::Rejected { reason, issues } => {
            assert_eq!(reason, RejectReason::SecretDetected);
            assert_eq!(issues.len(), 1);
            assert!(!format!("{issues:?}").contains(&aws_key()));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(l.rig.agent.write_commands(), 0);
}

// ---------------------------------------------------------------------------------------------- deletes and restarts

#[tokio::test]
async fn approving_a_delete_removes_the_file_and_keeps_its_last_version() {
    let Some(l) = lane().await else { return };
    let pv = l
        .rig
        .hub
        .preview_effect(
            &ctx(&l.careful, "pv-1"),
            domain::PreviewRequest {
                swimlane: l.s.clone(),
                path: ports::conformance::sample::nfs("old.txt"),
                change: domain::PreviewChange::Delete {
                    expected: h("keep\n"),
                },
            },
        )
        .await
        .unwrap();
    let id = match l
        .rig
        .hub
        .delete(
            &ctx(&l.careful, "d-1"),
            domain::DeleteRequest {
                swimlane: l.s.clone(),
                path: ports::conformance::sample::nfs("old.txt"),
                expected: h("keep\n"),
                preview: Some(pv.preview_id),
            },
        )
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    assert!(
        l.rig.nfs_of(&l.s, "old.txt").is_some(),
        "a proposal deletes nothing"
    );

    let out = l.approve(&l.admin, "k-1", id).await.unwrap();
    assert!(matches!(out, WriteOutcome::Applied { hash: None, .. }), "{out:?}");
    assert!(l.rig.nfs_of(&l.s, "old.txt").is_none());
    assert!(
        l.rig.blobs.get(&h("keep\n")).await.unwrap().is_some(),
        "the deleted version is kept"
    );
    let chain = l.rig.chain().await;
    let e = &chain.last().unwrap().event;
    assert_eq!(e.action, AuditAction::FileDeleted);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: l.admin.id.clone()
        }
    );
    assert_eq!(e.author.as_ref(), Some(&l.careful.id));
    assert_eq!(e.approval, Some(id));
    assert_eq!(chain.len(), 2);
    assert_eq!(l.status(id).await, "applied");
    assert_eq!(
        l.transitions(id).await,
        vec![ProposalStatus::Pending, ProposalStatus::Applied]
    );
}

/// The deleted version is kept so that a revert can bring it back, and a revert refuses what is above the size limit: the
/// approval of a delete is refused for a file above it too (the limit is the approving hub's, and may be lower than it was when
/// the proposal was made), and deletes nothing.
#[tokio::test]
async fn approving_the_delete_of_a_file_above_the_size_limit_deletes_nothing() {
    let Some(l) = lane().await else { return };
    let pv = l
        .rig
        .hub
        .preview_effect(
            &ctx(&l.careful, "pv-1"),
            domain::PreviewRequest {
                swimlane: l.s.clone(),
                path: ports::conformance::sample::nfs("old.txt"),
                change: domain::PreviewChange::Delete {
                    expected: h("keep\n"),
                },
            },
        )
        .await
        .unwrap();
    let id = match l
        .rig
        .hub
        .delete(
            &ctx(&l.careful, "d-1"),
            domain::DeleteRequest {
                swimlane: l.s.clone(),
                path: ports::conformance::sample::nfs("old.txt"),
                expected: h("keep\n"),
                preview: Some(pv.preview_id),
            },
        )
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    let strict = l.rig.hub_with(
        hub_writes::WriteConfig {
            max_content_bytes: 4,
            ..hub_writes::WriteConfig::default()
        },
        None,
    );
    let out = strict
        .approve_proposal(&ctx(&l.admin, "k-1"), id, None)
        .await
        .unwrap();
    assert_eq!(out, rejected(RejectReason::TooLarge));
    assert!(l.rig.nfs_of(&l.s, "old.txt").is_some());
    assert_eq!(l.rig.agent.deletes_applied(), 0);
    assert!(l.rig.blobs.get(&h("keep\n")).await.unwrap().is_none());
}

#[tokio::test]
async fn approving_a_restart_restarts_the_service() {
    let Some(l) = lane().await else { return };
    let careful_operator = l.rig.approval_user(11, Role::Operator).await;
    let service = ServiceRef::new("sit1", "payments").unwrap();
    let id = match l
        .rig
        .hub
        .restart(
            &ctx(&careful_operator, "r-1"),
            RestartRequest {
                swimlane: l.s.clone(),
                service: service.clone(),
            },
        )
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(l.rig.agent.restarts_applied(), 0);
    let out = l.approve(&l.admin, "k-1", id).await.unwrap();
    assert!(matches!(out, WriteOutcome::Applied { hash: None, .. }), "{out:?}");
    assert_eq!(l.rig.agent.restarts_applied(), 1);
    let chain = l.rig.chain().await;
    let e = &chain.last().unwrap().event;
    assert_eq!(e.action, AuditAction::ServiceRestarted);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: l.admin.id.clone()
        }
    );
    assert_eq!(e.author.as_ref(), Some(&careful_operator.id));
    assert_eq!(e.approval, Some(id));
    assert_eq!(l.status(id).await, "applied");
    assert_eq!(
        l.transitions(id).await,
        vec![ProposalStatus::Pending, ProposalStatus::Applied]
    );
}

// ---------------------------------------------------------------------------------------------------------- rejecting

#[tokio::test]
async fn rejecting_records_the_reason_and_one_audit_event_and_writes_nothing() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    let out = l
        .rig
        .hub
        .reject_proposal(&ctx(&l.admin, "k-1"), id, text("not now"))
        .await
        .unwrap();
    let RejectResult::Rejected(view) = out else {
        panic!("{out:?}")
    };
    assert_eq!(view.id, id.get());
    assert_eq!(view.status, ProposalStatus::Rejected);
    assert_eq!(view.decided_by.as_ref(), Some(&l.admin.id));
    assert_eq!(
        view.decision_comment.as_ref().map(ShortText::as_str),
        Some("not now")
    );
    assert_eq!(view.author, l.careful.id);
    assert_eq!(l.status(id).await, "rejected");
    assert_eq!(l.rig.agent.write_commands(), 0);
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));

    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 2);
    assert!(verify_chain(&chain).is_ok());
    let e = &chain[1].event;
    assert_eq!(e.action, AuditAction::ProposalRejected);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: l.admin.id.clone()
        }
    );
    assert_eq!(e.author.as_ref(), Some(&l.careful.id));
    assert_eq!(e.approval, Some(id));
    assert_eq!(
        l.transitions(id).await,
        vec![ProposalStatus::Pending, ProposalStatus::Rejected]
    );

    // The same key replays the answer; another key finds it decided; a rejected proposal cannot be approved.
    assert!(matches!(
        l.rig
            .hub
            .reject_proposal(&ctx(&l.admin, "k-1"), id, text("not now"))
            .await
            .unwrap(),
        RejectResult::Rejected(_)
    ));
    assert_eq!(
        l.rig
            .hub
            .reject_proposal(&ctx(&l.admin, "k-2"), id, text("again"))
            .await
            .unwrap(),
        RejectResult::Refused(rejected(RejectReason::NotAllowed))
    );
    assert_eq!(
        l.approve(&l.admin, "k-3", id).await.unwrap(),
        rejected(RejectReason::NotAllowed)
    );
    assert_eq!(l.rig.chain().await.len(), 2);
}

// ------------------------------------------------------------------------------------------------------------ expiring

#[tokio::test]
async fn expired_proposals_are_marked_by_a_leased_job() {
    let Some(l) = lane().await else { return };
    let old = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    l.rig.advance_secs(60 * 60 * 48);
    let young = l.propose("p-2", "b.yml", "y: 1\n", "y: 2\n").await;
    let job = l.rig.hub.expirer();

    // Nothing has run out yet.
    l.rig.advance_secs(60 * 60 * 23);
    let report = job.run_once(&|| true).await;
    assert_eq!((report.proposals, report.failed), (0, 0));

    // Past 72 hours from the first one. A job that has lost its lease does nothing.
    l.rig.advance_secs(60 * 60 * 2);
    let lost = job.run_once(&|| false).await;
    assert_eq!(lost.proposals, 0);
    assert_eq!(l.status(old).await, "pending");
    let report = job.run_once(&|| true).await;
    assert_eq!(report.proposals, 1);
    assert_eq!(l.status(old).await, "expired");
    assert_eq!(l.status(young).await, "pending");

    // One audit event, by the system, naming the proposer; and the screens are told.
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 3, "two proposals and one expiry");
    assert!(verify_chain(&chain).is_ok());
    let e = &chain.last().unwrap().event;
    assert_eq!(e.action, AuditAction::ProposalExpired);
    assert!(matches!(e.actor, AuditActor::System { .. }));
    assert_eq!(e.via, AuditVia::System);
    assert_eq!(e.author.as_ref(), Some(&l.careful.id));
    assert_eq!(e.approval, Some(old));
    assert_eq!(
        l.transitions(old).await,
        vec![ProposalStatus::Pending, ProposalStatus::Expired]
    );

    // Running again marks nothing twice, and an expired proposal cannot be approved or rejected.
    assert_eq!(job.run_once(&|| true).await.proposals, 0);
    assert_eq!(l.rig.chain().await.len(), 3);
    assert_eq!(
        l.approve(&l.admin, "k-1", old).await.unwrap(),
        rejected(RejectReason::NotAllowed)
    );
    assert_eq!(l.rig.agent.write_commands(), 0);
}

#[tokio::test]
async fn a_proposal_past_its_time_cannot_be_approved_before_the_job_runs() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    l.rig.advance_secs(60 * 60 * 73);
    assert_eq!(
        l.approve(&l.admin, "k-1", id).await.unwrap(),
        rejected(RejectReason::NotAllowed)
    );
    assert_eq!(l.rig.agent.write_commands(), 0);
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
}

/// The expiry job takes the lease that deciders take (review of 06, NB3): a proposal whose approval is at the agent is skipped, so
/// the approval is the only thing that moves it. The job finds it decided on its next run and has nothing to do.
#[tokio::test]
async fn the_expiry_job_skips_a_proposal_whose_approval_is_at_the_agent() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    // An hour before it runs out: the approval is not refused for its age.
    l.rig.advance_secs(60 * 60 * 71);
    let gate = l.rig.agent.hold_writes();
    let approval = l.approve(&l.admin, "k-1", id);
    let meanwhile = async {
        // The command is at the agent. The time runs out and the expiry job looks at the proposal.
        l.rig.agent.entered.notified().await;
        l.rig.advance_secs(60 * 60 * 2);
        let report = l.rig.hub.expirer().run_once(&|| true).await;
        gate.add_permits(1);
        report
    };
    let (outcome, report) = tokio::join!(approval, meanwhile);
    assert_eq!(
        report.proposals, 0,
        "the job marked a proposal that was being approved: {report:?}"
    );
    assert_eq!(report.busy, 1, "{report:?}");
    assert!(matches!(outcome.unwrap(), WriteOutcome::Applied { .. }));

    // The approval decided it, and the screens were told so.
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(l.status(id).await, "applied");
    assert_eq!(l.rig.count("approvals").await, 1);
    assert_eq!(
        l.transitions(id).await,
        vec![ProposalStatus::Pending, ProposalStatus::Applied]
    );
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 2, "the proposal and the write; no expiry");
    assert!(verify_chain(&chain).is_ok());

    // Nothing is left for the job.
    assert_eq!(l.rig.hub.expirer().run_once(&|| true).await.proposals, 0);
}

/// What remains of the race (a residual risk of the threat model, 06 T8): the lease on a proposal lasts 60 seconds, so a command
/// that outlasts it leaves the proposal open to the job. This puts the proposal in that state by hand while the command is at the
/// agent. The file has changed by then, so the write is still audited (naming the approver, the author and the proposal) and the
/// proposal is left as it is: `expired`, with no approval row.
#[tokio::test]
async fn a_proposal_that_expires_while_its_write_is_on_the_way_is_still_audited_and_left_expired() {
    let Some(l) = lane().await else { return };
    let id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    l.rig.advance_secs(60 * 60 * 71);
    let gate = l.rig.agent.hold_writes();
    let approval = l.approve(&l.admin, "k-1", id);
    let meanwhile = async {
        // The command is at the agent and its lease has run out: whatever expires the proposal now is not stopped by it.
        l.rig.agent.entered.notified().await;
        sqlx::query("UPDATE proposals SET status = 'expired' WHERE id = $1")
            .bind(id.get())
            .execute(&l.rig.pool)
            .await
            .unwrap();
        gate.add_permits(1);
    };
    let (outcome, ()) = tokio::join!(approval, meanwhile);
    assert!(
        matches!(outcome.unwrap(), WriteOutcome::Applied { .. }),
        "the admin is told the change was applied"
    );

    // The file changed, and the proposal is as it was left.
    assert_eq!(l.rig.nfs_of(&l.s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(l.status(id).await, "expired");
    assert_eq!(l.rig.count("approvals").await, 0, "no decision was written");
    assert_eq!(
        l.transitions(id).await,
        vec![ProposalStatus::Pending],
        "and the screens were never told it was applied"
    );

    // One audit event for the write, after the proposal, naming the approver, the author and the proposal.
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 2, "the proposal and the write");
    assert!(verify_chain(&chain).is_ok());
    let e = &chain.last().unwrap().event;
    assert_eq!(e.action, AuditAction::FileEdited);
    assert!(matches!(&e.actor, AuditActor::User { user } if *user == l.admin.id));
    assert_eq!(e.author.as_ref(), Some(&l.careful.id));
    assert_eq!(e.approval, Some(id));
}

#[tokio::test]
async fn the_expiry_job_works_in_batches() {
    let Some(l) = lane_with(hub_writes::WriteConfig {
        expire_batch: 2,
        ..hub_writes::WriteConfig::default()
    })
    .await
    else {
        return;
    };
    let ids = [
        l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await,
        l.propose("p-2", "b.yml", "y: 1\n", "y: 2\n").await,
        l.propose("p-3", "c.yml", "z: 1\n", "z: 2\n").await,
    ];
    l.rig.advance_secs(60 * 60 * 73);
    let report = l.rig.hub.expirer().run_once(&|| true).await;
    assert_eq!(report.proposals, 3, "a full batch is followed by another");
    for id in ids {
        assert_eq!(l.status(id).await, "expired");
    }
}

/// A proposal that the job cannot move (here its lease is held; a row that fails every time is the same to the scan) is passed
/// over for the rest of the run, so it does not hold up the ones behind it even when a batch is one proposal. It is looked at
/// again by the next run.
#[tokio::test]
async fn a_proposal_the_job_cannot_move_does_not_hold_up_the_ones_behind_it() {
    let Some(l) = lane_with(hub_writes::WriteConfig {
        expire_batch: 1,
        ..hub_writes::WriteConfig::default()
    })
    .await
    else {
        return;
    };
    let first = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    l.rig.advance_secs(1);
    let second = l.propose("p-2", "b.yml", "y: 1\n", "y: 2\n").await;
    l.rig.advance_secs(1);
    let third = l.propose("p-3", "c.yml", "z: 1\n", "z: 2\n").await;
    l.rig.advance_secs(60 * 60 * 73);

    let held = l
        .rig
        .leases
        .try_acquire(
            &format!("proposal:{}", first.get()),
            std::time::Duration::from_secs(600),
        )
        .await
        .unwrap()
        .expect("the lease is free");
    let report = l.rig.hub.expirer().run_once(&|| true).await;
    assert_eq!((report.proposals, report.busy), (2, 1), "{report:?}");
    assert_eq!(l.status(first).await, "pending");
    assert_eq!(l.status(second).await, "expired");
    assert_eq!(l.status(third).await, "expired");

    held.release().await.unwrap();
    let report = l.rig.hub.expirer().run_once(&|| true).await;
    assert_eq!(report.proposals, 1, "{report:?}");
    assert_eq!(l.status(first).await, "expired");
}

// -------------------------------------------------------------------------------------------------------------- events

#[tokio::test]
async fn proposal_changed_event_follows_every_transition() {
    let Some(l) = lane().await else { return };
    // The one that nobody decides is the oldest.
    let expired_id = l.propose("p-4", "old.txt", "keep\n", "kept\n").await;
    l.rig.advance_secs(60 * 60 * 71);
    let applied_id = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    let rejected_id = l.propose("p-2", "b.yml", "y: 1\n", "y: 2\n").await;
    let stale_id = l.propose("p-3", "c.yml", "z: 1\n", "z: 2\n").await;

    l.approve(&l.admin, "k-1", applied_id).await.unwrap();
    l.rig
        .hub
        .reject_proposal(&ctx(&l.admin, "k-2"), rejected_id, text("no"))
        .await
        .unwrap();
    l.rig
        .agent
        .put(&l.s, &ports::conformance::sample::nfs("c.yml"), b"z: 5\n");
    l.approve(&l.admin, "k-3", stale_id).await.unwrap();
    l.rig.advance_secs(60 * 60 * 2);
    // The other three were decided and are not pending; only the oldest has run out.
    assert_eq!(l.rig.hub.expirer().run_once(&|| true).await.proposals, 1);

    for (id, last) in [
        (applied_id, ProposalStatus::Applied),
        (rejected_id, ProposalStatus::Rejected),
        (stale_id, ProposalStatus::Stale),
        (expired_id, ProposalStatus::Expired),
    ] {
        assert_eq!(
            l.transitions(id).await,
            vec![ProposalStatus::Pending, last],
            "proposal {id}"
        );
    }
}

// ----------------------------------------------------------------------------------------------------------------- list

#[tokio::test]
async fn the_list_shows_hunks_newest_first_masks_secrets_and_pages_by_keyset() {
    let Some(l) = lane().await else { return };
    // A file whose old content holds a secret, and its base version in the blob store (the registry keeps every version).
    let leaky = format!("k: {}\nok: 1\n", aws_key());
    l.rig.agent.put(
        &l.s,
        &ports::conformance::sample::nfs("leaky.yml"),
        leaky.as_bytes(),
    );
    l.rig.catch_up_registry(&l.s);
    l.rig.blobs.put(Bytes::from(leaky.clone())).await.unwrap();
    l.rig.blobs.put(Bytes::from_static(b"x: 1\n")).await.unwrap();

    let first = l.propose("p-1", "a.yml", "x: 1\n", "x: 2\n").await;
    let second = match l
        .rig
        .hub
        .edit(
            &ctx(&l.careful, "p-2"),
            edit(&l.s, "leaky.yml", at(&leaky), "k: gone\nok: 1\n"),
        )
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    let third = l.propose("p-3", "b.yml", "y: 1\n", "y: 2\n").await;

    let viewer = l.rig.user(5, Role::Viewer).await;
    let q = |page: Page, status| ProposalQuery {
        status,
        swimlane: None,
        page,
    };
    let all = l
        .rig
        .hub
        .list_proposals(&viewer, q(Page::new(100, None).unwrap(), None))
        .await
        .unwrap();
    assert_eq!(
        all.items.iter().map(|v| v.id).collect::<Vec<_>>(),
        vec![third.get(), second.get(), first.get()],
        "newest first"
    );
    assert!(all.next.is_none());
    let v1 = all.items.iter().find(|v| v.id == first.get()).unwrap();
    assert_eq!(v1.status, ProposalStatus::Pending);
    assert_eq!(v1.new_hash, h("x: 2\n"));
    assert_eq!(
        v1.hunks.len(),
        1,
        "the base version is in the blob store, so the diff is shown"
    );
    assert_eq!(v1.setting_changes.len(), 1);
    // The secret in the old file is masked everywhere it could leave.
    let json = serde_json::to_string(&all.items).unwrap();
    assert!(!json.contains(&aws_key()), "{json}");
    assert!(json.contains("[REDACTED: "), "{json}");
    // A view fits the schema of the API: the fields it names and no more.
    let value: serde_json::Value = serde_json::to_value(v1).unwrap();
    let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "author",
            "created_at",
            "decided_by",
            "decision_comment",
            "expected",
            "expires_at",
            "hunks",
            "id",
            "new_hash",
            "path",
            "setting_changes",
            "severity",
            "status",
            "swimlane"
        ]
    );

    // Keyset pages.
    let page1 = l
        .rig
        .hub
        .list_proposals(&viewer, q(Page::new(2, None).unwrap(), None))
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    let next = page1.next.clone().expect("there is another page");
    let page2 = l
        .rig
        .hub
        .list_proposals(&viewer, q(Page::new(2, Some(next)).unwrap(), None))
        .await
        .unwrap();
    assert_eq!(
        page2.items.iter().map(|v| v.id).collect::<Vec<_>>(),
        vec![first.get()]
    );
    assert!(page2.next.is_none());

    // Filters.
    l.approve(&l.admin, "k-1", first).await.unwrap();
    let pending = l
        .rig
        .hub
        .list_proposals(
            &viewer,
            q(Page::new(100, None).unwrap(), Some(ProposalStatus::Pending)),
        )
        .await
        .unwrap();
    assert_eq!(pending.items.len(), 2);
    let done = l
        .rig
        .hub
        .list_proposals(
            &viewer,
            q(Page::new(100, None).unwrap(), Some(ProposalStatus::Applied)),
        )
        .await
        .unwrap();
    assert_eq!(done.items.len(), 1);
    assert_eq!(done.items[0].decided_by.as_ref(), Some(&l.admin.id));

    // A viewer may look; nobody below Viewer may.
    let pending_user = l
        .rig
        .user_with_status(6, Some(Role::Viewer), UserStatus::Pending)
        .await;
    assert_eq!(
        l.rig
            .hub
            .list_proposals(&pending_user, q(Page::new(10, None).unwrap(), None))
            .await,
        Err(WriteError::Forbidden)
    );
}

#[tokio::test]
async fn the_list_shows_a_delete_and_a_restart_in_the_shape_of_the_schema() {
    let Some(l) = lane().await else { return };
    let careful_operator = l.rig.approval_user(11, Role::Operator).await;
    let restart_id = match l
        .rig
        .hub
        .restart(
            &ctx(&careful_operator, "r-1"),
            RestartRequest {
                swimlane: l.s.clone(),
                service: ServiceRef::new("sit1", "payments").unwrap(),
            },
        )
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    let viewer = l.rig.user(5, Role::Viewer).await;
    let page = l
        .rig
        .hub
        .list_proposals(
            &viewer,
            ProposalQuery {
                status: None,
                swimlane: None,
                page: Page::new(10, None).unwrap(),
            },
        )
        .await
        .unwrap();
    let v = page.items.iter().find(|v| v.id == restart_id.get()).unwrap();
    assert_eq!(v.path.as_str(), "sit1/payments");
    assert_eq!(
        v.new_hash, NO_HASH,
        "no content, and the schema has no way to say so"
    );
    assert!(v.hunks.is_empty() && v.setting_changes.is_empty());
}

/// An approval holds a lease on the proposal while its write runs: the lease follows the agent's timeout, so that it does not
/// run out under a command that is still at the agent.
#[tokio::test]
async fn the_approval_lease_outlives_the_agent_timeout() {
    let config = hub_writes::WriteConfig {
        agent_timeout: std::time::Duration::from_secs(90),
        ..hub_writes::WriteConfig::default()
    };
    let Some(l) = lane_with(config).await else { return };
    let id = l.propose("k-p", "a.yml", "x: 1\n", "x: 2\n").await;
    let out = l.approve(&l.admin, "k-a", id).await.unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    let asked = l.rig.leases.asked();
    let name = format!("proposal:{}", id.get());
    let held: Vec<_> = asked.iter().filter(|(n, _)| *n == name).collect();
    assert_eq!(held.len(), 1, "{asked:?}");
    assert!(
        held[0].1 > std::time::Duration::from_secs(91),
        "the lease ({:?}) ends before the write can",
        held[0].1
    );
}
