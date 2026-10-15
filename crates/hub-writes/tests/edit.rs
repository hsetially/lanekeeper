//! The write pipeline for `edit` (T1): authorization, the idempotency claim, the editing lock, validation, approval and
//! the execution with its single transaction (audit, tool-write registration, outbox, the claim's response). Real Postgres
//! through `LK_TEST_PG_ADMIN_URL` or a container; every test makes its own database.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use domain::{
    DomainEvent, Expected, IssueKind, NfsPath, ObservationSource, ProposalStatus, RejectReason, Role,
    WriteOutcome,
};
use ports::conformance::sample::{hash, nfs};
use ports::{AuditAction, AuditActor, AuditVia, WriteError, WriteService, verify_chain};
use support::fake_agent::Fault;
use support::pg_tool_writes::registered;
use support::rig::{Rig, applied, at, ctx, edit, h};

/// A value the secret scan flags in every implementation (the AWS documentation's example key id), built in two parts so that
/// no scanner reads this file as a leak.
fn aws_key() -> String {
    ["AKIA", "IOSFODNN7EXAMPLE"].concat()
}

async fn rig_with_a() -> Option<Rig> {
    let rig = Rig::new(support::pg::TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n"), ("b.yml", "k: v\n")]);
    Some(rig)
}

#[tokio::test]
async fn edit_writes_nfs_then_commits_audit_registration_event_and_response_together() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();

    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    let (new_hash, audit_id) = applied(&out);
    assert_eq!(new_hash, Some(h("x: 2\n")));
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));

    // Exactly one audit event, chained, with the facts of D38.
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    assert!(verify_chain(&chain).is_ok());
    assert_eq!(chain[0].id, audit_id);
    let e = &chain[0].event;
    assert_eq!(e.action, AuditAction::FileEdited);
    assert_eq!(e.via, AuditVia::Ui);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: editor.id.clone()
        }
    );
    assert_eq!(e.swimlane.as_ref(), Some(&s));
    assert_eq!(e.path.as_ref(), Some(&nfs("a.yml")));
    assert_eq!(e.hash_before, Some(h("x: 1\n")));
    assert_eq!(e.hash_after, Some(h("x: 2\n")));
    assert_eq!(
        e.request_id.as_ref().map(domain::RequestId::as_str),
        Some("req-k-1")
    );
    let diff = e.diff.as_deref().unwrap();
    assert!(diff.contains("-x: 1") && diff.contains("+x: 2"), "{diff}");

    // The registry is told which hash to expect, so the observation is attributed with certainty (D73).
    assert_eq!(
        registered(&rig.pool).await,
        vec![("a.yml".to_owned(), h("x: 2\n").to_string())]
    );
    // One event in the outbox, written in the same transaction.
    assert_eq!(
        rig.bus.published().await,
        vec![DomainEvent::FileObserved {
            swimlane: s.clone(),
            path: nfs("a.yml"),
            hash: h("x: 2\n"),
            source: ObservationSource::ToolWrite,
        }]
    );
    // The new version is kept (D47, D78) and referenced by the audit event.
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blob_refs WHERE hash = $1 AND ref_kind = 'audit_event'")
            .bind(h("x: 2\n").as_bytes().as_slice())
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(kept, 1);
    // The claim carries the response.
    let (status, body): (Option<i32>, Option<Vec<u8>>) =
        sqlx::query_as("SELECT response_status, response_body FROM idempotency_keys WHERE key = 'k-1'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert!(status.is_some());
    assert_eq!(
        serde_json::from_slice::<WriteOutcome>(&body.unwrap()).unwrap(),
        out
    );
}

#[tokio::test]
async fn edit_conflict_is_409_with_current_hash_and_writes_nothing() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();

    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("old: 0\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Conflict {
            current: Some(h("x: 1\n"))
        }
    );
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(
        rig.chain().await.len(),
        0,
        "a conflict changes nothing, so it audits nothing"
    );
    assert_eq!(rig.bus.published().await.len(), 0);
    assert!(registered(&rig.pool).await.is_empty());

    // Create needs `Absent`: over an existing file it is a conflict, and over a missing one it works.
    let over = rig
        .hub
        .edit(
            &ctx(&editor, "k-2"),
            edit(&s, "a.yml", Expected::Absent, "x: 3\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        over,
        WriteOutcome::Conflict {
            current: Some(h("x: 1\n"))
        }
    );
    let created = rig
        .hub
        .edit(
            &ctx(&editor, "k-3"),
            edit(&s, "new.yml", Expected::Absent, "n: 1\n"),
        )
        .await
        .unwrap();
    assert_eq!(applied(&created).0, Some(h("n: 1\n")));
    let again = rig
        .hub
        .edit(
            &ctx(&editor, "k-4"),
            edit(&s, "new.yml", Expected::Absent, "n: 2\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        again,
        WriteOutcome::Conflict {
            current: Some(h("n: 1\n"))
        }
    );
    // A file that was there when the caller saw it and is gone now is a conflict with no current hash.
    let gone = rig
        .hub
        .edit(
            &ctx(&editor, "k-5"),
            edit(&s, "missing.yml", at("q: 1\n"), "q: 2\n"),
        )
        .await
        .unwrap();
    assert_eq!(gone, WriteOutcome::Conflict { current: None });
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn unconfirmed_baseline_returns_locked_423_and_writes_nothing() {
    let rig = Rig::new(db!()).await;
    let s = rig.swimlane("sit1", false).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    let editor = rig.user(1, Role::Editor).await;

    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(out, WriteOutcome::Locked { until: None });
    assert_eq!(rig.agent.write_commands(), 0, "the agent is never asked");
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(rig.chain().await.len(), 0);

    // The lock is not stored: once an admin confirms the baseline, the same key works.
    sqlx::query("UPDATE swimlanes SET editing_unlocked = true WHERE id = 'sit1'")
        .execute(&rig.pool)
        .await
        .unwrap();
    let again = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert!(matches!(again, WriteOutcome::Applied { .. }), "{again:?}");
}

#[tokio::test]
async fn unknown_swimlane_is_rejected_and_not_stored() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let nowhere = domain::SwimlaneId::parse("nowhere").unwrap();
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&nowhere, "a.yml", Expected::Absent, "x: 1\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Rejected {
            reason: RejectReason::UnknownSwimlane,
            issues: Vec::new()
        }
    );
    assert_eq!(rig.agent.commands().len(), 0);
}

#[tokio::test]
async fn write_above_the_limit_is_rejected_too_large_and_the_limit_itself_passes() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    let limit = 2 * 1024 * 1024;

    let huge = "x".repeat(limit + 1);
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&s, "huge.txt", Expected::Absent, &huge),
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Rejected {
            reason: RejectReason::TooLarge,
            issues: Vec::new()
        }
    );
    assert_eq!(rig.agent.write_commands(), 0);

    let exact = "ab\n".repeat(limit / 3) + &"a".repeat(limit % 3);
    assert_eq!(exact.len(), limit);
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-2"),
            edit(&s, "limit.txt", Expected::Absent, &exact),
        )
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
}

#[tokio::test]
async fn denied_path_is_refused_and_nothing_is_sent() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    for (n, path) in ["certs/server.pem", "store.jks", "docs/private-notes.txt"]
        .into_iter()
        .enumerate()
    {
        let out = rig
            .hub
            .edit(
                &ctx(&editor, &format!("k-{n}")),
                edit(&s, path, Expected::Absent, "x\n"),
            )
            .await
            .unwrap();
        assert_eq!(
            out,
            WriteOutcome::Rejected {
                reason: RejectReason::Denied,
                issues: Vec::new()
            },
            "{path}"
        );
    }
    assert_eq!(rig.agent.write_commands(), 0);
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn parse_error_blocks_with_the_place_and_never_the_text() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&s, "a.yml", at("x: 1\n"), "ok: 1\nnot-closed: [unique-marker-1\n"),
        )
        .await
        .unwrap();
    let WriteOutcome::Rejected { reason, issues } = &out else {
        panic!("expected a rejection: {out:?}");
    };
    assert_eq!(*reason, RejectReason::ParseError);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, IssueKind::ParseError);
    assert!(issues[0].line.is_some());
    assert!(!format!("{out:?}{}", serde_json::to_string(&out).unwrap()).contains("unique-marker-1"));
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn duplicate_keys_are_a_warning_and_the_write_goes_through() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&s, "a.yml", at("x: 1\n"), "a: 1\nb: 2\na: 3\n"),
        )
        .await
        .unwrap();
    let WriteOutcome::Applied { warnings, .. } = &out else {
        panic!("expected Applied: {out:?}");
    };
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].kind, IssueKind::DuplicateKey);
    assert_eq!(warnings[0].line.map(std::num::NonZeroU32::get), Some(3));
    assert_eq!(
        rig.nfs_of(&s, "a.yml").as_deref(),
        Some(&b"a: 1\nb: 2\na: 3\n"[..])
    );
    // The audit event still records one write.
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn secret_in_content_blocks_and_reports_lines_only() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    let key = aws_key();
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&s, "a.yml", at("x: 1\n"), &format!("ok: 1\naws_key: {key}\n")),
        )
        .await
        .unwrap();
    let WriteOutcome::Rejected { reason, issues } = &out else {
        panic!("expected a rejection: {out:?}");
    };
    assert_eq!(*reason, RejectReason::SecretDetected);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, IssueKind::SecretMatch);
    assert_eq!(issues[0].line.map(std::num::NonZeroU32::get), Some(2));
    assert_eq!(
        issues[0].rule.as_ref().map(domain::ShortText::as_str),
        Some("aws_access_key")
    );
    assert!(!format!("{out:?}{}", serde_json::to_string(&out).unwrap()).contains(&key));
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn a_secret_already_in_the_old_file_is_redacted_in_the_audit_diff() {
    let rig = Rig::new(db!()).await;
    let s = rig.swimlane("sit1", true).await;
    let key = aws_key();
    let old = format!("aws_key: {key}\nplain: 1\n");
    rig.seed(&s, &[("keys.yml", &old)]);
    let editor = rig.user(1, Role::Editor).await;
    // The new content has no secret (it is removed), so the save is allowed; the diff's removed line holds the old key.
    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "keys.yml", at(&old), "plain: 1\n"))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    let chain = rig.chain().await;
    let diff = chain[0].event.diff.clone().unwrap();
    assert!(
        !diff.contains(&key),
        "the audit diff must not hold the old secret: {diff}"
    );
    assert!(diff.contains("[REDACTED: "), "{diff}");
    let stored: String = sqlx::query_scalar("SELECT event_json FROM audit_events LIMIT 1")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert!(!stored.contains(&key));
}

#[tokio::test]
async fn binary_content_skips_the_text_checks_and_is_written_untouched() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    let logo: &[u8] = &[0, 1, 2, 0xff, b'A', b'K', b'I', b'A', 0];
    let mut req = edit(&s, "img/logo.bmp", Expected::Absent, "");
    req.content = bytes::Bytes::copy_from_slice(logo);
    let out = rig.hub.edit(&ctx(&editor, "k-1"), req).await.unwrap();
    assert_eq!(applied(&out).0, Some(hash(logo)));
    assert_eq!(rig.nfs_of(&s, "img/logo.bmp").as_deref(), Some(logo));
}

#[tokio::test]
async fn second_edit_right_after_the_first_succeeds_though_the_registry_lags() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    let first = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(applied(&first).0, Some(h("x: 2\n")));
    // The registry has not seen the first write yet (it learns from the agent's next scan), so it still holds `x: 1`. The
    // second edit carries the hash the first one returned and must not be a conflict.
    let second = rig
        .hub
        .edit(&ctx(&editor, "k-2"), edit(&s, "a.yml", at("x: 2\n"), "x: 3\n"))
        .await
        .unwrap();
    assert_eq!(applied(&second).0, Some(h("x: 3\n")));
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 2);
    assert!(verify_chain(&chain).is_ok());
    let diff = chain[1].event.diff.clone().unwrap();
    assert!(diff.contains("-x: 2") && diff.contains("+x: 3"), "{diff}");
}

#[tokio::test]
async fn approval_user_gets_a_pending_proposal_and_nothing_is_written() {
    let Some(rig) = rig_with_a().await else { return };
    let careful = rig.approval_user(6, Role::Editor).await;
    let admin = rig.user(2, Role::Admin).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    let _ = admin;

    let out = rig
        .hub
        .edit(&ctx(&careful, "p-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    let WriteOutcome::ProposalCreated { id } = out else {
        panic!("expected a proposal: {out:?}");
    };
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(rig.agent.write_commands(), 0);

    let row: (String, String, Vec<String>, String, i64, serde_json::Value) = sqlx::query_as(
        "SELECT action, status, paths, via, (extract(epoch from expires_at) * 1000)::bigint, base_hashes FROM proposals WHERE id = $1",
    )
    .bind(id.get())
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        (row.0.as_str(), row.1.as_str(), row.3.as_str()),
        ("edit", "pending", "ui")
    );
    assert_eq!(row.2, vec!["a.yml".to_owned()]);
    assert_eq!(
        row.4,
        support::rig::START + 72 * 3600 * 1000,
        "a proposal lives 72 hours (D37)"
    );
    assert_eq!(row.5["a.yml"], h("x: 1\n").to_string());
    let new_hash: Vec<u8> = sqlx::query_scalar("SELECT new_hash FROM proposals WHERE id = $1")
        .bind(id.get())
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(new_hash, h("x: 2\n").as_bytes().to_vec());
    let refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blob_refs WHERE ref_kind = 'proposal' AND ref_id = $1")
            .bind(id.get().to_string())
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(
        refs, 1,
        "the proposed content is kept while the proposal lives (D78)"
    );

    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].event.action, AuditAction::ProposalCreated);
    assert_eq!(chain[0].event.hash_after, Some(h("x: 2\n")));
    assert!(chain[0].event.diff.as_deref().unwrap().contains("+x: 2"));
    assert_eq!(
        rig.bus.published().await,
        vec![DomainEvent::ProposalChanged {
            id,
            status: ProposalStatus::Pending
        }]
    );
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1, "admins are told");
    assert_eq!(sent[0].kind, ports::NotificationKind::ProposalPending);
    assert!(
        registered(&rig.pool).await.is_empty(),
        "nothing was written, so nothing is registered"
    );

    // The proposal is still validated: a secret blocks it before it is created.
    let blocked = rig
        .hub
        .edit(
            &ctx(&careful, "p-2"),
            edit(&s, "a.yml", at("x: 1\n"), &format!("k: {}\n", aws_key())),
        )
        .await
        .unwrap();
    assert!(matches!(
        blocked,
        WriteOutcome::Rejected {
            reason: RejectReason::SecretDetected,
            ..
        }
    ));
    // And a locked swimlane gives no proposals either.
    sqlx::query("UPDATE swimlanes SET editing_unlocked = false")
        .execute(&rig.pool)
        .await
        .unwrap();
    let locked = rig
        .hub
        .edit(&ctx(&careful, "p-3"), edit(&s, "a.yml", at("x: 1\n"), "x: 9\n"))
        .await
        .unwrap();
    assert_eq!(locked, WriteOutcome::Locked { until: None });
    assert_eq!(rig.count("proposals").await, 1);
}

#[tokio::test]
async fn audit_failure_rolls_back_the_registration_the_event_and_the_response() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    rig.audit.fail_next();
    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await;
    assert_eq!(out, Err(WriteError::Unavailable));
    // The file did land on NFS (the agent had answered), but nothing of the transaction survived.
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(rig.chain().await.len(), 0);
    assert_eq!(rig.bus.published().await.len(), 0);
    assert!(registered(&rig.pool).await.is_empty());
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM idempotency_keys WHERE key = 'k-1' AND response_status IS NULL",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        unfinished, 1,
        "the claim stays, with what is needed to finish the write"
    );

    // The retry with the same key finishes the write: one audit event, the agent is not asked to write again.
    let retry = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(applied(&retry).0, Some(h("x: 2\n")));
    assert_eq!(rig.agent.write_commands(), 1);
    assert_eq!(rig.chain().await.len(), 1);
    assert_eq!(registered(&rig.pool).await.len(), 1);
}

#[tokio::test]
async fn a_lost_reply_is_resolved_by_the_retry_without_a_second_write() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    rig.agent.fault_next_write(Fault::LandThenTimeout);
    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await;
    assert_eq!(
        out,
        Err(WriteError::Unavailable),
        "the hub cannot know whether it landed"
    );
    assert_eq!(rig.chain().await.len(), 0);

    let retry = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(applied(&retry).0, Some(h("x: 2\n")));
    assert_eq!(
        rig.agent.write_commands(),
        1,
        "the file was found and not written again"
    );
    assert_eq!(rig.agent.writes_applied(), 1);
    assert_eq!(rig.chain().await.len(), 1);

    // A closed stream is the same.
    rig.agent.fault_next_write(Fault::LandThenClosed);
    let out = rig
        .hub
        .edit(&ctx(&editor, "k-2"), edit(&s, "b.yml", at("k: v\n"), "k: w\n"))
        .await;
    assert_eq!(out, Err(WriteError::Unavailable));
}

#[tokio::test]
async fn an_agent_that_never_received_the_command_is_rejected_and_the_retry_runs_again() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    rig.agent.fault_next_write(Fault::NotConnected);
    let out = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Rejected {
            reason: RejectReason::AgentUnavailable,
            issues: Vec::new()
        }
    );
    // Transient: not stored, so the same key tries again and works.
    let again = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert!(matches!(again, WriteOutcome::Applied { .. }), "{again:?}");

    // An I/O error on the agent wrote nothing: also transient. A refusal there is a denial.
    rig.agent.fault_next_write(Fault::Io);
    let io = rig
        .hub
        .edit(&ctx(&editor, "k-2"), edit(&s, "b.yml", at("k: v\n"), "k: w\n"))
        .await
        .unwrap();
    assert_eq!(
        io,
        WriteOutcome::Rejected {
            reason: RejectReason::AgentUnavailable,
            issues: Vec::new()
        }
    );
    rig.agent.fault_next_write(Fault::Denied);
    let denied = rig
        .hub
        .edit(&ctx(&editor, "k-3"), edit(&s, "b.yml", at("k: v\n"), "k: w\n"))
        .await
        .unwrap();
    assert_eq!(
        denied,
        WriteOutcome::Rejected {
            reason: RejectReason::Denied,
            issues: Vec::new()
        }
    );
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn every_applied_write_has_exactly_one_chained_audit_event() {
    let Some(rig) = rig_with_a().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let s = domain::SwimlaneId::parse("sit1").unwrap();
    // Three writes, and four attempts that change nothing.
    for (n, (path, from, to)) in [
        ("a.yml", "x: 1\n", "x: 2\n"),
        ("b.yml", "k: v\n", "k: w\n"),
        ("a.yml", "x: 2\n", "x: 3\n"),
    ]
    .into_iter()
    .enumerate()
    {
        let out = rig
            .hub
            .edit(&ctx(&editor, &format!("w-{n}")), edit(&s, path, at(from), to))
            .await
            .unwrap();
        assert!(matches!(out, WriteOutcome::Applied { .. }));
    }
    let stale = rig
        .hub
        .edit(&ctx(&editor, "c-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 9\n"))
        .await
        .unwrap();
    assert!(matches!(stale, WriteOutcome::Conflict { .. }));
    let bad = rig
        .hub
        .edit(&ctx(&editor, "c-2"), edit(&s, "a.yml", at("x: 3\n"), "a: [\n"))
        .await
        .unwrap();
    assert!(matches!(bad, WriteOutcome::Rejected { .. }));
    let nowhere = domain::SwimlaneId::parse("nowhere").unwrap();
    let missing = rig
        .hub
        .edit(
            &ctx(&editor, "c-3"),
            edit(&nowhere, "a.yml", Expected::Absent, "x\n"),
        )
        .await
        .unwrap();
    assert!(matches!(missing, WriteOutcome::Rejected { .. }));
    let replay = rig
        .hub
        .edit(&ctx(&editor, "w-0"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert!(matches!(replay, WriteOutcome::Applied { .. }));

    let chain = rig.chain().await;
    assert_eq!(chain.len(), 3, "one event per applied write, none for the rest");
    assert!(verify_chain(&chain).is_ok());
    assert!(chain.iter().all(|e| e.event.action == AuditAction::FileEdited));
    assert_eq!(rig.bus.published().await.len(), 3);
    let _ = NfsPath::root();
}
