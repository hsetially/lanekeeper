//! T4: drafts. A draft is a change prepared and checked with no effect; applying it is an edit by the same user through the whole
//! pipeline. A draft belongs to its author, expires, and never holds a secret from the file it was made over.
#![allow(clippy::too_many_lines)]

use bytes::Bytes;
use domain::{DomainEvent, DraftId, Expected, ProposalStatus, RejectReason, Role, SwimlaneId, WriteOutcome};
use hub_writes::ProposalDesk;
use ports::conformance::sample::nfs;
use ports::{AuditAction, AuditActor, BlobStore, WriteError, WriteService, verify_chain};

use crate::support::pg::TestDb;
use crate::support::rig::{Rig, applied, at, ctx, edit, h};

fn aws_key() -> String {
    ["AKIA", "IOSFODNN7EXAMPLE"].concat()
}

fn rejected(reason: RejectReason) -> WriteOutcome {
    WriteOutcome::Rejected {
        reason,
        issues: Vec::new(),
    }
}

async fn lane() -> Option<(Rig, SwimlaneId)> {
    let rig = Rig::new(TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.tenant(&s, "sit1").await;
    rig.seed(&s, &[("a.yml", "x: 1\nok: 1\n"), ("b.yml", "y: 1\n")]);
    Some((rig, s))
}

async fn draft_status(rig: &Rig, id: &DraftId) -> String {
    sqlx::query_scalar("SELECT status FROM drafts WHERE id = $1")
        .bind(id.as_str())
        .fetch_one(&rig.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_draft_has_no_effect_and_no_audit_event() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "d-1"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    assert_eq!(d.status, ProposalStatus::Draft);
    assert_eq!(d.author, editor.id);
    assert_eq!(d.new_hash, h("x: 2\nok: 1\n"));
    assert_eq!(d.hunks.len(), 1);
    assert_eq!(d.setting_changes.len(), 1);
    assert_eq!(
        d.setting_changes[0].left.as_ref().map(|v| v.text.as_str()),
        Some("1")
    );
    assert_eq!(
        d.setting_changes[0].right.as_ref().map(|v| v.text.as_str()),
        Some("2")
    );
    assert_eq!(
        d.expires_at.unix_millis(),
        crate::support::rig::START + 24 * 3600 * 1000
    );

    // Nothing was written, claimed or recorded; the new version is kept, so applying it later needs no upload.
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\nok: 1\n"[..]));
    assert_eq!(rig.agent.write_commands(), 0);
    assert_eq!(rig.chain().await.len(), 0);
    assert_eq!(rig.count("idempotency_keys").await, 0);
    assert_eq!(rig.bus.published().await.len(), 0);
    assert_eq!(rig.count("drafts").await, 1);
    assert_eq!(
        rig.blobs.get(&d.new_hash).await.unwrap().as_deref(),
        Some(&b"x: 2\nok: 1\n"[..])
    );
}

/// A residual risk of the threat model (review of 06, NB5): `POST /drafts` requires an `Idempotency-Key` and does not use it, so
/// a retry with the key makes a second draft. A draft changes nothing, so the harm is the open-draft limit's to bound.
#[tokio::test]
async fn a_retried_draft_with_the_same_key_makes_a_second_draft_within_the_open_limit() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let request = edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n");
    let first = rig
        .hub
        .propose_draft(&ctx(&editor, "d-1"), request.clone())
        .await
        .unwrap();
    let again = rig
        .hub
        .propose_draft(&ctx(&editor, "d-1"), request)
        .await
        .unwrap();
    assert_ne!(first.id, again.id, "the key is not honoured yet");
    assert_eq!(rig.count("drafts").await, 2);
    assert_eq!(rig.count("idempotency_keys").await, 0, "and no claim is made");
}

#[tokio::test]
async fn apply_draft_writes_audits_draft_applied_and_marks_it() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "d-1"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    let out = rig
        .hub
        .apply_draft(&ctx(&editor, "a-1"), d.id.clone())
        .await
        .unwrap();
    let (hash, audit_id) = applied(&out);
    assert_eq!(hash, Some(h("x: 2\nok: 1\n")));
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\nok: 1\n"[..]));

    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1, "exactly one audit event");
    assert!(verify_chain(&chain).is_ok());
    assert_eq!(chain[0].id, audit_id);
    assert_eq!(chain[0].event.action, AuditAction::DraftApplied);
    assert_eq!(
        chain[0].event.actor,
        AuditActor::User {
            user: editor.id.clone()
        }
    );
    assert_eq!(draft_status(&rig, &d.id).await, "applied");

    // The same key replays the answer and writes nothing more; another key finds the draft used up.
    assert_eq!(
        rig.hub
            .apply_draft(&ctx(&editor, "a-1"), d.id.clone())
            .await
            .unwrap(),
        out
    );
    assert_eq!(
        rig.hub
            .apply_draft(&ctx(&editor, "a-2"), d.id.clone())
            .await
            .unwrap(),
        rejected(RejectReason::NotAllowed)
    );
    assert_eq!(rig.agent.write_commands(), 1);
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn a_draft_is_its_authors_alone() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let other = rig.user(2, Role::Editor).await;
    let admin = rig.user(3, Role::Admin).await;
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "d-1"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    for (who, key) in [(&other, "a-1"), (&admin, "a-2")] {
        assert_eq!(
            rig.hub.apply_draft(&ctx(who, key), d.id.clone()).await,
            Err(WriteError::NotFound)
        );
    }
    assert_eq!(
        rig.hub
            .apply_draft(&ctx(&editor, "a-3"), DraftId::parse("d-nothing").unwrap())
            .await,
        Err(WriteError::NotFound)
    );
    assert_eq!(rig.agent.write_commands(), 0);
    assert_eq!(draft_status(&rig, &d.id).await, "draft");
    assert_eq!(
        rig.count("idempotency_keys").await,
        0,
        "a not-found is not an outcome: the keys are free"
    );
}

#[tokio::test]
async fn applying_a_draft_after_the_file_changed_is_a_conflict_and_the_draft_goes_stale() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "d-1"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    rig.agent.put(&s, &nfs("a.yml"), b"x: 7\nok: 1\n");
    let out = rig
        .hub
        .apply_draft(&ctx(&editor, "a-1"), d.id.clone())
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Conflict {
            current: Some(h("x: 7\nok: 1\n"))
        }
    );
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 7\nok: 1\n"[..]));
    assert_eq!(draft_status(&rig, &d.id).await, "stale");
    assert_eq!(rig.chain().await.len(), 0, "a conflict records nothing");
}

#[tokio::test]
async fn a_careful_users_draft_becomes_a_proposal_and_its_approval_applies_the_draft() {
    let Some((rig, s)) = lane().await else { return };
    let careful = rig.approval_user(1, Role::Editor).await;
    let admin = rig.user(2, Role::Admin).await;
    let d = rig
        .hub
        .propose_draft(
            &ctx(&careful, "d-1"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    let out = rig
        .hub
        .apply_draft(&ctx(&careful, "a-1"), d.id.clone())
        .await
        .unwrap();
    let WriteOutcome::ProposalCreated { id } = out else {
        panic!("{out:?}")
    };
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\nok: 1\n"[..]));
    assert_eq!(
        draft_status(&rig, &d.id).await,
        "pending",
        "it waits with its proposal"
    );

    let done = rig
        .hub
        .approve_proposal(&ctx(&admin, "k-1"), id, None)
        .await
        .unwrap();
    assert!(matches!(done, WriteOutcome::Applied { .. }), "{done:?}");
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\nok: 1\n"[..]));
    assert_eq!(draft_status(&rig, &d.id).await, "applied");
    let transitions: Vec<_> = rig
        .bus
        .published()
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::ProposalChanged { status, .. } => Some(status),
            _ => None,
        })
        .collect();
    assert_eq!(
        transitions,
        vec![ProposalStatus::Pending, ProposalStatus::Applied]
    );
}

#[tokio::test]
async fn a_rejected_proposal_rejects_its_draft() {
    let Some((rig, s)) = lane().await else { return };
    let careful = rig.approval_user(1, Role::Editor).await;
    let admin = rig.user(2, Role::Admin).await;
    let d = rig
        .hub
        .propose_draft(
            &ctx(&careful, "d-1"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    let WriteOutcome::ProposalCreated { id } = rig
        .hub
        .apply_draft(&ctx(&careful, "a-1"), d.id.clone())
        .await
        .unwrap()
    else {
        panic!("not a proposal")
    };
    rig.hub
        .reject_proposal(&ctx(&admin, "k-1"), id, domain::ShortText::parse("no").unwrap())
        .await
        .unwrap();
    assert_eq!(draft_status(&rig, &d.id).await, "rejected");
}

#[tokio::test]
async fn apply_draft_scans_again_and_the_write_path_takes_the_draft_through_the_whole_pipeline() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    // A draft row that holds a secret, as if the rules had learned a new shape since it was made.
    let blob = rig
        .blobs
        .put(Bytes::from(format!("k: {}\n", aws_key())))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO drafts (id, author_tid, author_oid, swimlane_id, path, expected, new_hash, expires_at) \
         VALUES ('d-secret', $1, $2, 'sit1', 'b.yml', $3, $4, to_timestamp($5 / 1000.0) + interval '1 day')",
    )
    .bind(editor.id.tid().as_str())
    .bind(editor.id.oid().as_str())
    .bind(serde_json::to_value(at("y: 1\n")).unwrap())
    .bind(blob.as_bytes().as_slice())
    .bind(crate::support::rig::START)
    .execute(&rig.pool)
    .await
    .unwrap();
    let out = rig
        .hub
        .apply_draft(&ctx(&editor, "a-1"), DraftId::parse("d-secret").unwrap())
        .await
        .unwrap();
    match out {
        WriteOutcome::Rejected { reason, issues } => {
            assert_eq!(reason, RejectReason::SecretDetected);
            assert_eq!(issues.len(), 1);
            assert!(!format!("{issues:?}").contains(&aws_key()));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rig.nfs_of(&s, "b.yml").as_deref(), Some(&b"y: 1\n"[..]));
    assert_eq!(rig.agent.write_commands(), 0);

    // And the lock: a swimlane whose baseline is not confirmed cannot be written, drafts included.
    sqlx::query("UPDATE swimlanes SET editing_unlocked = false")
        .execute(&rig.pool)
        .await
        .unwrap();
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "d-2"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    let locked = rig.hub.apply_draft(&ctx(&editor, "a-2"), d.id).await.unwrap();
    assert!(matches!(locked, WriteOutcome::Locked { .. }), "{locked:?}");
}

#[tokio::test]
async fn a_secret_in_the_old_file_is_masked_in_the_draft() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let leaky = format!("k: {}\nok: 1\n", aws_key());
    rig.agent.put(&s, &nfs("leaky.yml"), leaky.as_bytes());
    rig.catch_up_registry(&s);
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "d-1"),
            edit(&s, "leaky.yml", at(&leaky), "k: gone\nok: 1\n"),
        )
        .await
        .unwrap();
    let shown = serde_json::to_string(&d).unwrap();
    assert!(!shown.contains(&aws_key()), "{shown}");
    assert!(shown.contains("[REDACTED: "), "{shown}");
    // And what is stored is masked too.
    let stored: String =
        sqlx::query_scalar("SELECT (hunks::text || setting_changes::text) FROM drafts WHERE id = $1")
            .bind(d.id.as_str())
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert!(!stored.contains(&aws_key()), "{stored}");
}

#[tokio::test]
async fn content_that_an_edit_would_refuse_is_not_drafted() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let bad = [
        ("a.yml", "x: [unclosed\n".to_owned()),
        ("b.yml", format!("y: {}\n", aws_key())),
        ("certs/server.pem", "x\n".to_owned()),
    ];
    for (n, (path, content)) in bad.into_iter().enumerate() {
        let expected = if path == "certs/server.pem" {
            Expected::Absent
        } else {
            at(if path == "a.yml" {
                "x: 1\nok: 1\n"
            } else {
                "y: 1\n"
            })
        };
        let out = rig
            .hub
            .propose_draft(
                &ctx(&editor, &format!("d-{n}")),
                edit(&s, path, expected, &content),
            )
            .await;
        assert_eq!(out, Err(WriteError::BadRequest), "{path}");
    }
    let unknown = SwimlaneId::parse("nowhere").unwrap();
    assert_eq!(
        rig.hub
            .propose_draft(
                &ctx(&editor, "d-9"),
                edit(&unknown, "a.yml", Expected::Absent, "x: 1\n")
            )
            .await,
        Err(WriteError::NotFound)
    );
    assert_eq!(rig.count("drafts").await, 0);
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn drafts_expire_and_an_expired_draft_cannot_be_applied() {
    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "d-1"),
            edit(&s, "a.yml", at("x: 1\nok: 1\n"), "x: 2\nok: 1\n"),
        )
        .await
        .unwrap();
    rig.advance_secs(25 * 3600);
    // Past its time it cannot be applied, whether or not the job has run.
    assert_eq!(
        rig.hub
            .apply_draft(&ctx(&editor, "a-1"), d.id.clone())
            .await
            .unwrap(),
        rejected(RejectReason::NotAllowed)
    );
    let report = rig.hub.expirer().run_once(&|| true).await;
    assert_eq!(report.drafts, 1);
    assert_eq!(draft_status(&rig, &d.id).await, "expired");
    assert_eq!(rig.hub.expirer().run_once(&|| true).await.drafts, 0);
    assert_eq!(rig.agent.write_commands(), 0);
    assert_eq!(
        rig.chain().await.len(),
        0,
        "a draft is a record: its expiry is not an audited change"
    );
}

/// The value-based secret scan (S13) runs on every door that puts content into a file: an edit, an upload, a revert and the
/// application of a draft. None of them writes the secret, and none of their refusals repeats it.
#[tokio::test]
async fn secret_scan_runs_on_edit_upload_revert_and_apply_draft() {
    use domain::{PreviewChange, PreviewRequest, RevertRequest, UploadRequest};

    let Some((rig, s)) = lane().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let key = aws_key();
    let leaky = format!("k: {key}\n");
    let refused = |out: WriteOutcome| match out {
        WriteOutcome::Rejected { reason, issues } => {
            assert_eq!(reason, RejectReason::SecretDetected);
            assert!(!format!("{issues:?}").contains(&aws_key()));
        }
        other => panic!("{other:?}"),
    };

    // An edit.
    refused(
        rig.hub
            .edit(&ctx(&editor, "e-1"), edit(&s, "b.yml", at("y: 1\n"), &leaky))
            .await
            .unwrap(),
    );
    // An upload with a good token.
    let pv = rig
        .hub
        .preview_effect(
            &ctx(&editor, "p-1"),
            PreviewRequest {
                swimlane: s.clone(),
                path: nfs("b.yml"),
                change: PreviewChange::Upload {
                    expected: at("y: 1\n"),
                    content: Bytes::from(leaky.clone().into_bytes()),
                },
            },
        )
        .await
        .unwrap();
    refused(
        rig.hub
            .upload(
                &ctx(&editor, "u-1"),
                UploadRequest {
                    swimlane: s.clone(),
                    path: nfs("b.yml"),
                    expected: at("y: 1\n"),
                    content: Bytes::from(leaky.clone().into_bytes()),
                    preview: Some(pv.preview_id),
                },
            )
            .await
            .unwrap(),
    );
    // A revert to a version that holds one.
    rig.blobs
        .put(Bytes::from(leaky.clone().into_bytes()))
        .await
        .unwrap();
    rig.registry
        .observe(&s, &nfs("b.yml"), Bytes::from(leaky.clone().into_bytes()));
    rig.registry
        .observe(&s, &nfs("b.yml"), Bytes::from_static(b"y: 1\n"));
    refused(
        rig.hub
            .revert(
                &ctx(&editor, "r-1"),
                RevertRequest {
                    swimlane: s.clone(),
                    path: nfs("b.yml"),
                    expected: at("y: 1\n"),
                    to: h(&leaky),
                    change_eol: None,
                },
            )
            .await
            .unwrap(),
    );
    // A draft that holds one is not made; one that is stored holding one is not applied.
    assert_eq!(
        rig.hub
            .propose_draft(&ctx(&editor, "d-1"), edit(&s, "b.yml", at("y: 1\n"), &leaky))
            .await,
        Err(WriteError::BadRequest)
    );
    let blob = rig.blobs.put(Bytes::from(leaky.into_bytes())).await.unwrap();
    sqlx::query(
        "INSERT INTO drafts (id, author_tid, author_oid, swimlane_id, path, expected, new_hash, expires_at) \
         VALUES ('d-secret', $1, $2, 'sit1', 'b.yml', $3, $4, to_timestamp($5 / 1000.0) + interval '1 day')",
    )
    .bind(editor.id.tid().as_str())
    .bind(editor.id.oid().as_str())
    .bind(serde_json::to_value(at("y: 1\n")).unwrap())
    .bind(blob.as_bytes().as_slice())
    .bind(crate::support::rig::START)
    .execute(&rig.pool)
    .await
    .unwrap();
    refused(
        rig.hub
            .apply_draft(&ctx(&editor, "a-1"), DraftId::parse("d-secret").unwrap())
            .await
            .unwrap(),
    );

    assert_eq!(rig.nfs_of(&s, "b.yml").as_deref(), Some(&b"y: 1\n"[..]));
    assert_eq!(rig.agent.write_commands(), 0);
    assert_eq!(rig.chain().await.len(), 0);
}
