//! T4: drafts. A draft is a change prepared and checked with no effect; applying it is an edit by the same user through the whole
//! pipeline. A draft belongs to its author, expires, and never holds a secret from the file it was made over.
#![allow(clippy::too_many_lines)]

use bytes::Bytes;
use domain::{
    DomainEvent, DraftId, Expected, PickupState, ProposalStatus, RejectReason, Role, ServiceRef, Severity,
    SwimlaneId, WriteOutcome,
};
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

// ---------------------------------------------------------------------------- severity and effects (contract/07-mcp)

fn service(namespace: &str, name: &str) -> ServiceRef {
    ServiceRef::new(namespace, name).unwrap()
}

/// A swimlane with two applications, `tx-api` and `other-app`, one Deployment each, and files of every kind the pickup states tell
/// apart: a root property source, an application's property source, a resource file, `channels.yml`.
async fn lane_with_services() -> Option<(Rig, SwimlaneId)> {
    let rig = Rig::new(TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.tenant(&s, "sit1").await;
    rig.service(&s, "csp", "tx-api", Some("tx-api")).await;
    rig.service(&s, "csp", "other", Some("other-app")).await;
    rig.seed(
        &s,
        &[
            ("application.yml", "r: 1\n"),
            ("channels.yml", "channels: [web]\n"),
            ("tx-api/tx-api.yml", "p: 1\n"),
            ("tx-api/entry.yml", "a: 1\nb: 2\n"),
            ("security/roles.yml", "role: user\n"),
        ],
    );
    Some((rig, s))
}

async fn propose(
    rig: &Rig,
    s: &SwimlaneId,
    editor: &domain::User,
    key: &str,
    path: &str,
    old: &str,
    new: &str,
) -> domain::Draft {
    rig.hub
        .propose_draft(&ctx(editor, key), edit(s, path, at(old), new))
        .await
        .unwrap()
}

/// D85: the draft says when the change would take effect for each service that reads the file. The facts are the engine's
/// `pickup_state`; the services are the ones that read the path (D82): the root's files are every service's, an application's
/// folder is that application's Deployments'.
#[tokio::test]
async fn a_draft_says_when_the_change_would_take_effect_for_each_service_that_reads_the_file() {
    let Some((rig, s)) = lane_with_services().await else {
        return;
    };
    let editor = rig.user(1, Role::Editor).await;
    let (api, other) = (service("csp", "tx-api"), service("csp", "other"));

    // A property source of one application: only its Deployments, and they need a notification or a restart.
    let d = propose(&rig, &s, &editor, "p-1", "tx-api/tx-api.yml", "p: 1\n", "p: 2\n").await;
    assert_eq!(d.effects, vec![(api.clone(), PickupState::NeedsNotifyOrRestart)]);

    // A resource file of the application: live once the client's cache expires.
    let d = propose(
        &rig,
        &s,
        &editor,
        "p-2",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: 9\nb: 2\n",
    )
    .await;
    assert_eq!(d.effects, vec![(api.clone(), PickupState::LiveWithinTtl)]);

    // The root's property source is read by every service, in a fixed order.
    let d = propose(&rig, &s, &editor, "p-3", "application.yml", "r: 1\n", "r: 2\n").await;
    assert_eq!(
        d.effects,
        vec![
            (other.clone(), PickupState::NeedsNotifyOrRestart),
            (api.clone(), PickupState::NeedsNotifyOrRestart)
        ]
    );

    // A new folder, and `channels.yml`: only a restart of the config-server picks them up (D83).
    let d = rig
        .hub
        .propose_draft(
            &ctx(&editor, "p-4"),
            edit(&s, "fresh/new.yml", Expected::Absent, "n: 1\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        d.effects,
        vec![
            (other.clone(), PickupState::NeedsConfigServerRestart),
            (api.clone(), PickupState::NeedsConfigServerRestart)
        ]
    );
    let d = propose(
        &rig,
        &s,
        &editor,
        "p-5",
        "channels.yml",
        "channels: [web]\n",
        "channels: [web, atm]\n",
    )
    .await;
    assert_eq!(
        d.effects,
        vec![
            (other, PickupState::NeedsConfigServerRestart),
            (api, PickupState::NeedsConfigServerRestart)
        ]
    );
}

#[tokio::test]
async fn a_draft_with_no_deployment_to_read_the_file_has_no_effects_and_a_removed_deployment_does_not_count()
{
    let Some((rig, s)) = lane_with_services().await else {
        return;
    };
    let editor = rig.user(1, Role::Editor).await;
    sqlx::query("UPDATE services SET removed_at = now() WHERE name = 'tx-api'")
        .execute(&rig.pool)
        .await
        .unwrap();
    let d = propose(
        &rig,
        &s,
        &editor,
        "p-1",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: 9\nb: 2\n",
    )
    .await;
    // The removed Deployment is not listed. With it gone nobody declares `tx-api`, so its folder is no application's: a search
    // location every remaining service reads (D83).
    assert_eq!(
        d.effects,
        vec![(service("csp", "other"), PickupState::LiveWithinTtl)]
    );

    sqlx::query("DELETE FROM services")
        .execute(&rig.pool)
        .await
        .unwrap();
    let d = propose(&rig, &s, &editor, "p-2", "application.yml", "r: 1\n", "r: 2\n").await;
    assert!(d.effects.is_empty());
    assert_eq!(rig.chain().await.len(), 0, "working out effects records nothing");
}

/// D76: the severity is the engine's, from the default rules and the admin's.
#[tokio::test]
async fn a_draft_carries_the_severity_the_rules_give_its_change() {
    let Some((rig, s)) = lane_with_services().await else {
        return;
    };
    let editor = rig.user(1, Role::Editor).await;
    // Under `security/**`: critical.
    let d = propose(
        &rig,
        &s,
        &editor,
        "s-1",
        "security/roles.yml",
        "role: user\n",
        "role: admin\n",
    )
    .await;
    assert_eq!(d.severity, Some(Severity::Critical));
    // A root file changing affects every service: high.
    let d = propose(&rig, &s, &editor, "s-2", "application.yml", "r: 1\n", "r: 2\n").await;
    assert_eq!(d.severity, Some(Severity::High));
    // Keys removed: high. A value changing type: high.
    let d = propose(
        &rig,
        &s,
        &editor,
        "s-3",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: 1\n",
    )
    .await;
    assert_eq!(d.severity, Some(Severity::High));
    let d = propose(
        &rig,
        &s,
        &editor,
        "s-4",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: x\nb: 2\n",
    )
    .await;
    assert_eq!(d.severity, Some(Severity::High));
    // A value changing in a file below a folder matches no rule: no severity, not a made-up "low".
    let d = propose(
        &rig,
        &s,
        &editor,
        "s-5",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: 9\nb: 2\n",
    )
    .await;
    assert_eq!(d.severity, None);
}

#[tokio::test]
async fn an_admins_severity_rule_applies_to_a_draft_and_a_rule_that_cannot_apply_does_not_lower_the_answer() {
    let Some((rig, s)) = lane_with_services().await else {
        return;
    };
    let editor = rig.user(1, Role::Editor).await;
    let rule = |id: &str, scope: &str, matcher: serde_json::Value, severity: &str, enabled: bool| {
        let (pool, id, scope, severity) = (
            rig.pool.clone(),
            id.to_owned(),
            scope.to_owned(),
            severity.to_owned(),
        );
        async move {
            sqlx::query(
                "INSERT INTO severity_rules (id, scope, matcher, severity, enabled) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(scope)
            .bind(matcher)
            .bind(severity)
            .bind(enabled)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    // Disabled, and about findings: neither applies to a change.
    rule(
        "admin.off",
        "change",
        serde_json::json!({ "path_glob": "tx-api/**" }),
        "critical",
        false,
    )
    .await;
    rule(
        "admin.finding",
        "finding",
        serde_json::json!({ "path_glob": "tx-api/**" }),
        "critical",
        true,
    )
    .await;
    let d = propose(
        &rig,
        &s,
        &editor,
        "r-1",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: 9\nb: 2\n",
    )
    .await;
    assert_eq!(d.severity, None);

    // An enabled rule for changes under the folder.
    rule(
        "admin.api",
        "change",
        serde_json::json!({ "path_glob": "tx-api/**" }),
        "medium",
        true,
    )
    .await;
    let d = propose(
        &rig,
        &s,
        &editor,
        "r-2",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: 9\nb: 2\n",
    )
    .await;
    assert_eq!(d.severity, Some(Severity::Medium));
    // Never lower than a default rule says: the highest wins.
    let d = propose(
        &rig,
        &s,
        &editor,
        "r-3",
        "tx-api/entry.yml",
        "a: 1\nb: 2\n",
        "a: 1\n",
    )
    .await;
    assert_eq!(d.severity, Some(Severity::High));

    // A rule the hub cannot read is no reason to claim a lower severity: the engine has no answer.
    rule(
        "admin.broken",
        "change",
        serde_json::json!({ "bogus": 1 }),
        "low",
        true,
    )
    .await;
    let d = propose(
        &rig,
        &s,
        &editor,
        "r-4",
        "security/roles.yml",
        "role: user\n",
        "role: admin\n",
    )
    .await;
    assert_eq!(d.severity, None);
}

/// `WriteService::draft`: the author reads the draft as it was proposed, with its severity and effects, and nobody else does.
#[tokio::test]
async fn the_author_reads_their_draft_as_proposed_and_nobody_else_does() {
    let Some((rig, s)) = lane_with_services().await else {
        return;
    };
    let editor = rig.user(1, Role::Editor).await;
    let other = rig.user(2, Role::Editor).await;
    let viewer = rig.user(3, Role::Viewer).await;
    let pending = rig
        .user_with_status(4, Some(Role::Admin), domain::UserStatus::Pending)
        .await;
    let proposed = propose(&rig, &s, &editor, "w-1", "application.yml", "r: 1\n", "r: 2\n").await;
    assert_eq!(proposed.severity, Some(Severity::High));
    assert_eq!(proposed.effects.len(), 2);

    let read = rig
        .hub
        .draft(&ctx(&editor, "w-2"), proposed.id.clone())
        .await
        .unwrap();
    assert_eq!(
        read, proposed,
        "the draft that was proposed, with its severity and effects"
    );
    assert_eq!(
        rig.hub
            .draft(&ctx(&editor, "w-3"), proposed.id.clone())
            .await
            .unwrap(),
        proposed,
        "reading twice gives the same draft"
    );

    assert_eq!(
        rig.hub.draft(&ctx(&other, "w-4"), proposed.id.clone()).await,
        Err(WriteError::NotFound)
    );
    assert_eq!(
        rig.hub
            .draft(&ctx(&editor, "w-5"), DraftId::parse("d-nothing").unwrap())
            .await,
        Err(WriteError::NotFound)
    );
    for who in [&viewer, &pending] {
        assert_eq!(
            rig.hub.draft(&ctx(who, "w-6"), proposed.id.clone()).await,
            Err(WriteError::Forbidden)
        );
    }

    // Reading records nothing and claims no key: the same key can be used again.
    assert_eq!(rig.chain().await.len(), 0);
    assert_eq!(rig.count("idempotency_keys").await, 0);
    assert_eq!(rig.agent.write_commands(), 0);
    assert_eq!(draft_status(&rig, &proposed.id).await, "draft");
}

/// An applied draft says so; an expired one is still readable by its author: expiry ends what can be applied, not what can be read.
#[tokio::test]
async fn an_applied_or_expired_draft_is_still_readable_by_its_author() {
    let Some((rig, s)) = lane_with_services().await else {
        return;
    };
    let editor = rig.user(1, Role::Editor).await;
    let other = rig.user(2, Role::Editor).await;
    let first = propose(&rig, &s, &editor, "x-1", "tx-api/tx-api.yml", "p: 1\n", "p: 2\n").await;
    rig.hub
        .apply_draft(&ctx(&editor, "x-2"), first.id.clone())
        .await
        .unwrap();
    let after = rig
        .hub
        .draft(&ctx(&editor, "x-3"), first.id.clone())
        .await
        .unwrap();
    assert_eq!(after.status, ProposalStatus::Applied);
    assert_eq!(
        (after.id, after.path, after.new_hash),
        (first.id, first.path, first.new_hash)
    );

    let second = propose(&rig, &s, &editor, "x-4", "tx-api/tx-api.yml", "p: 2\n", "p: 3\n").await;
    rig.advance_secs(2 * 24 * 3600);
    let read = rig
        .hub
        .draft(&ctx(&editor, "x-5"), second.id.clone())
        .await
        .unwrap();
    assert_eq!(read.id, second.id);
    assert_eq!(read.expires_at, second.expires_at);
    assert_eq!(
        rig.hub.draft(&ctx(&other, "x-6"), second.id.clone()).await,
        Err(WriteError::NotFound),
        "an expired draft is still only the author's"
    );
    assert_eq!(
        rig.nfs_of(&s, "tx-api/tx-api.yml").as_deref(),
        Some(&b"p: 2\n"[..]),
        "reading an expired draft writes nothing"
    );
}
