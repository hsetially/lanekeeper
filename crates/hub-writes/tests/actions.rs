//! T3: upload, delete, revert, restart and the effect preview, through the write pipeline over a real Postgres. Every action has
//! its 409, its 423 and its misuse; every applied one has exactly one chained audit event; none of them writes blind.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

#[path = "actions/approvals.rs"]
mod approvals;
#[path = "actions/drafts.rs"]
mod drafts;
#[path = "actions/github_client.rs"]
mod github_client;
#[path = "actions/notify.rs"]
mod notify;
#[path = "actions/pr_jobs.rs"]
mod pr_jobs;
#[path = "actions/pull_requests.rs"]
mod pull_requests;
#[path = "actions/reveal.rs"]
mod reveal;
#[path = "actions/teams.rs"]
mod teams;
#[path = "actions/vault.rs"]
mod vault;

use std::time::Duration;

use bytes::Bytes;
use domain::{
    ChangeEol, ChangePreview, ContentHash, DeleteRequest, DomainEvent, Expected, PreviewChange, PreviewId,
    PreviewRequest, PreviewScope, PreviewWarning, ProposalStatus, RejectReason, RestartRequest,
    RevertRequest, Role, ServiceRef, SwimlaneId, UploadRequest, User, WriteOutcome,
};
use ports::conformance::sample::nfs;
use ports::{AuditAction, BlobStore, WriteError, WriteService, content_hash, verify_chain};
use support::fake_agent::Fault;
use support::pg_tool_writes::registered;
use support::rig::{Rig, applied, at, ctx, h};

/// A value the secret scan flags (the AWS documentation's example key id), built in two parts so that no scanner reads this file
/// as a leak.
fn aws_key() -> String {
    ["AKIA", "IOSFODNN7EXAMPLE"].concat()
}

async fn rig() -> Option<(Rig, SwimlaneId)> {
    let rig = Rig::new(support::pg::TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.tenant(&s, "sit1").await;
    rig.seed(
        &s,
        &[
            ("a.yml", "x: 1\n"),
            ("tx-api/entry.yml", "a: 1\nb: 2\n"),
            ("tx-api/solo.yml", "s: 1\n"),
            ("crlf/c.yml", "k: 1\r\nm: 2\r\n"),
        ],
    );
    Some((rig, s))
}

fn upload_preview(s: &SwimlaneId, path: &str, expected: Expected, content: &[u8]) -> PreviewRequest {
    PreviewRequest {
        swimlane: s.clone(),
        path: nfs(path),
        change: PreviewChange::Upload {
            expected,
            content: Bytes::copy_from_slice(content),
        },
    }
}

fn delete_preview(s: &SwimlaneId, path: &str, expected: ContentHash) -> PreviewRequest {
    PreviewRequest {
        swimlane: s.clone(),
        path: nfs(path),
        change: PreviewChange::Delete { expected },
    }
}

fn upload_req(
    s: &SwimlaneId,
    path: &str,
    expected: Expected,
    content: &[u8],
    preview: Option<&ChangePreview>,
) -> UploadRequest {
    UploadRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        content: Bytes::copy_from_slice(content),
        preview: preview.map(|p| p.preview_id.clone()),
    }
}

fn delete_req(
    s: &SwimlaneId,
    path: &str,
    expected: ContentHash,
    preview: Option<&ChangePreview>,
) -> DeleteRequest {
    DeleteRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        preview: preview.map(|p| p.preview_id.clone()),
    }
}

async fn preview(rig: &Rig, who: &User, key: &str, r: PreviewRequest) -> ChangePreview {
    rig.hub.preview_effect(&ctx(who, key), r).await.unwrap()
}

fn rejected(reason: RejectReason) -> WriteOutcome {
    WriteOutcome::Rejected {
        reason,
        issues: Vec::new(),
    }
}

// ------------------------------------------------------------------------------------------------------------- upload

#[tokio::test]
async fn upload_with_a_valid_preview_writes_audits_registers_and_publishes_once() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let content = b"n: 1\n";
    let pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "new.yml", Expected::Absent, content),
    )
    .await;

    let out = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "new.yml", Expected::Absent, content, Some(&pv)),
        )
        .await
        .unwrap();
    let (new_hash, audit_id) = applied(&out);
    assert_eq!(new_hash, Some(h("n: 1\n")));
    assert_eq!(rig.nfs_of(&s, "new.yml").as_deref(), Some(&content[..]));

    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1, "exactly one audit event");
    assert!(verify_chain(&chain).is_ok());
    assert_eq!(chain[0].id, audit_id);
    assert_eq!(chain[0].event.action, AuditAction::FileUploaded);
    assert_eq!(chain[0].event.hash_before, None);
    assert_eq!(chain[0].event.hash_after, Some(h("n: 1\n")));
    assert_eq!(
        registered(&rig.pool).await,
        vec![("new.yml".to_owned(), h("n: 1\n").to_string())]
    );
    assert_eq!(rig.bus.published().await.len(), 1);
    assert!(matches!(
        rig.bus.published().await[0],
        DomainEvent::FileObserved { .. }
    ));
}

#[tokio::test]
async fn upload_and_delete_without_preview_are_refused() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;

    let up = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "new.yml", Expected::Absent, b"n: 1\n", None),
        )
        .await
        .unwrap();
    assert_eq!(up, rejected(RejectReason::NotAllowed));
    let del = rig
        .hub
        .delete(&ctx(&editor, "k-2"), delete_req(&s, "a.yml", h("x: 1\n"), None))
        .await
        .unwrap();
    assert_eq!(del, rejected(RejectReason::NotAllowed));

    assert!(rig.agent.commands().is_empty(), "nothing reached the agent");
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert!(rig.nfs_of(&s, "new.yml").is_none());
    assert_eq!(rig.chain().await.len(), 0);
    assert_eq!(
        rig.count("idempotency_keys").await,
        0,
        "a refusal is not an outcome: the keys are free, so a retry with a token is the same request"
    );
}

#[tokio::test]
async fn preview_bound_to_user_path_and_hash() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let other = rig.user(2, Role::Editor).await;
    let content = b"n: 1\n";
    let pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "new.yml", Expected::Absent, content),
    )
    .await;

    // Every part the token is bound to, changed one at a time.
    let attempts: Vec<(&str, &User, UploadRequest)> = vec![
        (
            "another user",
            &other,
            upload_req(&s, "new.yml", Expected::Absent, content, Some(&pv)),
        ),
        (
            "another path",
            &editor,
            upload_req(&s, "other.yml", Expected::Absent, content, Some(&pv)),
        ),
        (
            "other content",
            &editor,
            upload_req(&s, "new.yml", Expected::Absent, b"n: 2\n", Some(&pv)),
        ),
        (
            "another expected state",
            &editor,
            upload_req(&s, "new.yml", at("x: 1\n"), content, Some(&pv)),
        ),
        (
            "another swimlane",
            &editor,
            UploadRequest {
                swimlane: SwimlaneId::parse("sit2").unwrap(),
                ..upload_req(&s, "new.yml", Expected::Absent, content, Some(&pv))
            },
        ),
    ];
    for (n, (what, who, req)) in attempts.into_iter().enumerate() {
        let out = rig.hub.upload(&ctx(who, &format!("bad-{n}")), req).await.unwrap();
        // The other swimlane does not exist, so it is refused for that reason before the token is looked at; every other
        // attempt is refused for the token.
        let expected = if what == "another swimlane" {
            rejected(RejectReason::UnknownSwimlane)
        } else {
            rejected(RejectReason::NotAllowed)
        };
        assert_eq!(out, expected, "{what}");
    }
    assert!(rig.nfs_of(&s, "new.yml").is_none());
    assert!(rig.nfs_of(&s, "other.yml").is_none());
    assert_eq!(rig.chain().await.len(), 0);

    // A delete token is not an upload token, and the other way round.
    let del_pv = preview(&rig, &editor, "p-2", delete_preview(&s, "a.yml", h("x: 1\n"))).await;
    let wrong_kind = rig
        .hub
        .upload(
            &ctx(&editor, "bad-kind"),
            upload_req(&s, "a.yml", at("x: 1\n"), b"x: 1\n", Some(&del_pv)),
        )
        .await
        .unwrap();
    assert_eq!(wrong_kind, rejected(RejectReason::NotAllowed));
    let wrong_hash = rig
        .hub
        .delete(
            &ctx(&editor, "bad-hash"),
            delete_req(&s, "a.yml", h("something else\n"), Some(&del_pv)),
        )
        .await
        .unwrap();
    assert_eq!(wrong_hash, rejected(RejectReason::NotAllowed));
    let wrong_path = rig
        .hub
        .delete(
            &ctx(&editor, "bad-path"),
            delete_req(&s, "tx-api/solo.yml", h("x: 1\n"), Some(&del_pv)),
        )
        .await
        .unwrap();
    assert_eq!(wrong_path, rejected(RejectReason::NotAllowed));

    // Refused attempts did not use the token up: the right user with the right request still gets through.
    let ok = rig
        .hub
        .upload(
            &ctx(&editor, "good"),
            upload_req(&s, "new.yml", Expected::Absent, content, Some(&pv)),
        )
        .await
        .unwrap();
    assert!(matches!(ok, WriteOutcome::Applied { .. }), "{ok:?}");
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn preview_older_than_10_minutes_is_refused() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "new.yml", Expected::Absent, b"n: 1\n"),
    )
    .await;
    assert_eq!(
        pv.expires_at.unix_millis(),
        support::rig::START + 600_000,
        "the token says when it runs out"
    );

    rig.advance_secs(600);
    let req = || upload_req(&s, "new.yml", Expected::Absent, b"n: 1\n", Some(&pv));
    let still = rig.hub.upload(&ctx(&editor, "k-1"), req()).await.unwrap();
    assert!(
        matches!(still, WriteOutcome::Applied { .. }),
        "at exactly ten minutes: {still:?}"
    );

    let pv2 = preview(&rig, &editor, "p-2", delete_preview(&s, "new.yml", h("n: 1\n"))).await;
    rig.advance_secs(601);
    let late = rig
        .hub
        .delete(
            &ctx(&editor, "k-2"),
            delete_req(&s, "new.yml", h("n: 1\n"), Some(&pv2)),
        )
        .await
        .unwrap();
    assert_eq!(late, rejected(RejectReason::NotAllowed));
    assert!(rig.nfs_of(&s, "new.yml").is_some(), "nothing was deleted");
}

#[tokio::test]
async fn tampered_preview_is_refused() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "new.yml", Expected::Absent, b"n: 1\n"),
    )
    .await;
    let id = pv.preview_id.as_str().to_owned();

    let mut forged: Vec<String> = Vec::new();
    // One character changed anywhere in the token, to another character of the alphabet.
    for at in [0, 5, 12, 20, 30, 40, 54] {
        let mut chars: Vec<char> = id.chars().collect();
        chars[at] = if chars[at] == 'A' { 'B' } else { 'A' };
        forged.push(chars.into_iter().collect());
    }
    forged.push(id[..id.len() - 1].to_owned());
    forged.push(format!("{id}A"));
    forged.push("not-a-token".to_owned());
    for (n, text) in forged.into_iter().enumerate() {
        let req = UploadRequest {
            preview: Some(PreviewId::parse(&text).unwrap()),
            ..upload_req(&s, "new.yml", Expected::Absent, b"n: 1\n", None)
        };
        let out = rig
            .hub
            .upload(&ctx(&editor, &format!("bad-{n}")), req)
            .await
            .unwrap();
        assert_eq!(out, rejected(RejectReason::NotAllowed), "{text}");
    }
    assert!(rig.nfs_of(&s, "new.yml").is_none());
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn a_missing_preview_key_refuses_to_judge_and_issues_nothing() {
    let Some(db) = support::pg::TestDb::ready().await else {
        return;
    };
    let rig = Rig::with_secrets(db, ports::fakes::FakeSecretSource::new()).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    let editor = rig.user(1, Role::Editor).await;
    assert_eq!(
        rig.hub
            .preview_effect(
                &ctx(&editor, "p"),
                upload_preview(&s, "n.yml", Expected::Absent, b"n: 1\n")
            )
            .await
            .err(),
        Some(WriteError::Unavailable)
    );
    // An upload with any token is not served (the request cannot be judged), and nothing is written or kept.
    let id = PreviewId::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    let out = rig
        .hub
        .upload(
            &ctx(&editor, "k"),
            UploadRequest {
                preview: Some(id),
                ..upload_req(&s, "n.yml", Expected::Absent, b"n: 1\n", None)
            },
        )
        .await;
    assert_eq!(out, Err(WriteError::Unavailable));
    assert_eq!(rig.count("idempotency_keys").await, 0);
    assert!(rig.agent.commands().is_empty());
}

#[tokio::test]
async fn upload_conflict_is_409() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    // The caller saw another version of the file.
    let pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "a.yml", at("old: 0\n"), b"x: 9\n"),
    )
    .await;
    let out = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "a.yml", at("old: 0\n"), b"x: 9\n", Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Conflict {
            current: Some(h("x: 1\n"))
        }
    );
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(rig.chain().await.len(), 0, "a conflict audits nothing");
    assert!(registered(&rig.pool).await.is_empty());

    // Creating over a file that exists is the same conflict.
    let pv = preview(
        &rig,
        &editor,
        "p-2",
        upload_preview(&s, "a.yml", Expected::Absent, b"x: 9\n"),
    )
    .await;
    let over = rig
        .hub
        .upload(
            &ctx(&editor, "k-2"),
            upload_req(&s, "a.yml", Expected::Absent, b"x: 9\n", Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(
        over,
        WriteOutcome::Conflict {
            current: Some(h("x: 1\n"))
        }
    );
}

#[tokio::test]
async fn every_file_action_is_locked_until_baseline_confirmed() {
    let Some(db) = support::pg::TestDb::ready().await else {
        return;
    };
    let rig = Rig::new(db).await;
    let s = rig.swimlane("sit1", false).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    let editor = rig.user(1, Role::Editor).await;

    // The preview changes nothing, so it is served on a locked swimlane; the actions are not.
    let up_pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "n.yml", Expected::Absent, b"n: 1\n"),
    )
    .await;
    let del_pv = preview(&rig, &editor, "p-2", delete_preview(&s, "a.yml", h("x: 1\n"))).await;

    let locked = WriteOutcome::Locked { until: None };
    assert_eq!(
        rig.hub
            .upload(
                &ctx(&editor, "k-1"),
                upload_req(&s, "n.yml", Expected::Absent, b"n: 1\n", Some(&up_pv))
            )
            .await
            .unwrap(),
        locked
    );
    assert_eq!(
        rig.hub
            .delete(
                &ctx(&editor, "k-2"),
                delete_req(&s, "a.yml", h("x: 1\n"), Some(&del_pv))
            )
            .await
            .unwrap(),
        locked
    );
    assert_eq!(
        rig.hub
            .revert(
                &ctx(&editor, "k-3"),
                RevertRequest {
                    swimlane: s.clone(),
                    path: nfs("a.yml"),
                    expected: at("x: 1\n"),
                    to: h("x: 0\n"),
                    change_eol: None,
                }
            )
            .await
            .unwrap(),
        locked
    );
    assert!(rig.agent.commands().is_empty(), "the agent is never asked");
    assert_eq!(rig.chain().await.len(), 0);
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));

    // A restart is not a file action: the lock does not apply to it.
    let operator = rig.user(2, Role::Operator).await;
    rig.service(&s, "sit1", "payments", None).await;
    let restart = rig
        .hub
        .restart(
            &ctx(&operator, "k-4"),
            RestartRequest {
                swimlane: s.clone(),
                service: ServiceRef::new("sit1", "payments").unwrap(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(restart, WriteOutcome::Applied { .. }), "{restart:?}");
}

#[tokio::test]
async fn upload_checks_size_deny_globs_and_secrets_before_anything_is_written() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;

    // Above the limit: no preview, and a refusal from the action (the token proves nothing about size).
    let big = vec![b'a'; 2 * 1024 * 1024 + 1];
    assert_eq!(
        rig.hub
            .preview_effect(
                &ctx(&editor, "p-1"),
                upload_preview(&s, "big.txt", Expected::Absent, &big)
            )
            .await
            .err(),
        Some(WriteError::BadRequest)
    );
    let pv = preview(
        &rig,
        &editor,
        "p-2",
        upload_preview(&s, "ok.txt", Expected::Absent, b"ok\n"),
    )
    .await;
    let too_big = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "ok.txt", Expected::Absent, &big, Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(too_big, rejected(RejectReason::TooLarge));
    // Exactly at the limit passes the size check.
    let limit = vec![b'a'; 2 * 1024 * 1024];
    let pv = preview(
        &rig,
        &editor,
        "p-3",
        upload_preview(&s, "limit.txt", Expected::Absent, &limit),
    )
    .await;
    let at_limit = rig
        .hub
        .upload(
            &ctx(&editor, "k-2"),
            upload_req(&s, "limit.txt", Expected::Absent, &limit, Some(&pv)),
        )
        .await
        .unwrap();
    assert!(matches!(at_limit, WriteOutcome::Applied { .. }), "{at_limit:?}");

    // A denied path gets no preview and no upload (D79).
    assert_eq!(
        rig.hub
            .preview_effect(
                &ctx(&editor, "p-4"),
                upload_preview(&s, "certs/server.pem", Expected::Absent, b"x\n")
            )
            .await
            .err(),
        Some(WriteError::BadRequest)
    );
    let denied = rig
        .hub
        .upload(
            &ctx(&editor, "k-3"),
            upload_req(&s, "certs/server.pem", Expected::Absent, b"x\n", Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(denied, rejected(RejectReason::Denied));

    // A secret in the content blocks the save and names lines and rules, never the text.
    let secret = format!("a: 1\nkey: {}\n", aws_key());
    let pv = preview(
        &rig,
        &editor,
        "p-5",
        upload_preview(&s, "s.yml", Expected::Absent, secret.as_bytes()),
    )
    .await;
    let out = rig
        .hub
        .upload(
            &ctx(&editor, "k-4"),
            upload_req(&s, "s.yml", Expected::Absent, secret.as_bytes(), Some(&pv)),
        )
        .await
        .unwrap();
    let WriteOutcome::Rejected { reason, issues } = &out else {
        panic!("{out:?}")
    };
    assert_eq!(*reason, RejectReason::SecretDetected);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].line.map(std::num::NonZeroU32::get), Some(2));
    assert!(!format!("{out:?}").contains(&aws_key()));
    assert!(rig.nfs_of(&s, "s.yml").is_none());
}

#[tokio::test]
async fn upload_of_binary_content_is_written_untouched_and_over_a_crlf_file_keeps_crlf() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let png: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x00, 0xFF, 0x0A];
    let pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "img/logo.png", Expected::Absent, &png),
    )
    .await;
    let out = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "img/logo.png", Expected::Absent, &png, Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(applied(&out).0, Some(content_hash(&png)));
    assert_eq!(
        rig.nfs_of(&s, "img/logo.png").as_deref(),
        Some(&png[..]),
        "byte for byte"
    );

    // A text upload over a CRLF file is written in the file's style (D17); the diff of the audit event holds the changed line only.
    let new_lf = b"k: 1\nm: 3\n";
    let pv = preview(
        &rig,
        &editor,
        "p-2",
        upload_preview(&s, "crlf/c.yml", at("k: 1\r\nm: 2\r\n"), new_lf),
    )
    .await;
    let out = rig
        .hub
        .upload(
            &ctx(&editor, "k-2"),
            upload_req(&s, "crlf/c.yml", at("k: 1\r\nm: 2\r\n"), new_lf, Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(applied(&out).0, Some(h("k: 1\r\nm: 3\r\n")));
    assert_eq!(
        rig.nfs_of(&s, "crlf/c.yml").as_deref(),
        Some(&b"k: 1\r\nm: 3\r\n"[..])
    );
    let chain = rig.chain().await;
    let diff = chain[1].event.diff.as_deref().unwrap();
    assert!(diff.contains("-m: 2") && diff.contains("+m: 3"), "{diff}");
    assert!(
        !diff.contains("-k: 1") && !diff.contains("+k: 1"),
        "an unchanged line is context, not a change: {diff}"
    );
}

#[tokio::test]
async fn upload_replay_with_a_fresh_token_writes_once() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let first_pv = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "new.yml", Expected::Absent, b"n: 1\n"),
    )
    .await;
    let first = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "new.yml", Expected::Absent, b"n: 1\n", Some(&first_pv)),
        )
        .await
        .unwrap();
    // The reply was lost; the screen retries with the same key, a token it fetched again, and after the first token ran out.
    rig.advance_secs(900);
    let second_pv = preview(
        &rig,
        &editor,
        "p-2",
        upload_preview(&s, "new.yml", Expected::Absent, b"n: 1\n"),
    )
    .await;
    assert_ne!(first_pv.preview_id, second_pv.preview_id);
    let second = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "new.yml", Expected::Absent, b"n: 1\n", Some(&second_pv)),
        )
        .await
        .unwrap();
    assert_eq!(first, second);
    // The same key for another upload is refused.
    let reused = rig
        .hub
        .upload(
            &ctx(&editor, "k-1"),
            upload_req(&s, "new.yml", Expected::Absent, b"n: 2\n", Some(&second_pv)),
        )
        .await;
    assert_eq!(reused, Err(WriteError::IdempotencyConflict));
    assert_eq!(rig.agent.writes_applied(), 1);
    assert_eq!(rig.chain().await.len(), 1);
}

// ------------------------------------------------------------------------------------------------------------- delete

#[tokio::test]
async fn delete_keeps_content_in_blob_store() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let doomed = "doomed: yes\nsecond: line\n";
    rig.agent.put(&s, &nfs("gone.yml"), doomed.as_bytes());
    rig.catch_up_registry(&s);
    let pv = preview(&rig, &editor, "p-1", delete_preview(&s, "gone.yml", h(doomed))).await;

    let out = rig
        .hub
        .delete(
            &ctx(&editor, "k-1"),
            delete_req(&s, "gone.yml", h(doomed), Some(&pv)),
        )
        .await
        .unwrap();
    let (hash, audit_id) = applied(&out);
    assert_eq!(hash, None, "a delete leaves no hash");
    assert!(rig.nfs_of(&s, "gone.yml").is_none());
    assert_eq!(rig.agent.deletes_applied(), 1);

    // The last version is still in the blob store, and the audit event keeps it for as long as the event lives (D78).
    assert_eq!(
        rig.blobs.get(&h(doomed)).await.unwrap().as_deref(),
        Some(doomed.as_bytes())
    );
    let refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM blob_refs WHERE hash = $1 AND ref_kind = 'audit_event' AND ref_id = $2",
    )
    .bind(h(doomed).as_bytes().as_slice())
    .bind(audit_id.get().to_string())
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(refs, 1);

    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    let e = &chain[0].event;
    assert_eq!(e.action, AuditAction::FileDeleted);
    assert_eq!(e.hash_before, Some(h(doomed)));
    assert_eq!(e.hash_after, None);
    let diff = e.diff.as_deref().unwrap();
    assert!(
        diff.contains("-doomed: yes") && diff.contains("-second: line"),
        "{diff}"
    );
    assert_eq!(
        rig.bus.published().await,
        vec![DomainEvent::DriftChanged {
            swimlane: s.clone(),
            paths: vec![nfs("gone.yml")]
        }]
    );
    assert!(
        registered(&rig.pool).await.is_empty(),
        "a delete registers no hash"
    );
}

/// The agent answers `IO` when the unlink happened and the directory could not be synced: the file is gone. The delete is kept as
/// unfinished, and the retry finds the file gone and records it, with the last version kept.
#[tokio::test]
async fn an_io_error_after_the_unlink_is_found_and_recorded_not_lost() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let doomed = "doomed: yes\n";
    rig.agent.put(&s, &nfs("gone.yml"), doomed.as_bytes());
    rig.catch_up_registry(&s);
    let pv = preview(&rig, &editor, "p-1", delete_preview(&s, "gone.yml", h(doomed))).await;
    rig.agent.fault_next_write(Fault::LandThenIo);
    let first = rig
        .hub
        .delete(
            &ctx(&editor, "k-1"),
            delete_req(&s, "gone.yml", h(doomed), Some(&pv)),
        )
        .await;
    assert_eq!(first, Err(WriteError::Unavailable));
    assert!(rig.nfs_of(&s, "gone.yml").is_none());
    assert_eq!(rig.chain().await.len(), 0);

    let retry = rig
        .hub
        .delete(
            &ctx(&editor, "k-1"),
            delete_req(&s, "gone.yml", h(doomed), Some(&pv)),
        )
        .await
        .unwrap();
    assert!(matches!(retry, WriteOutcome::Applied { .. }), "{retry:?}");
    assert_eq!(rig.chain().await.len(), 1, "the delete that landed is audited");
    assert_eq!(rig.agent.deletes_applied(), 1);
}

#[tokio::test]
async fn delete_conflict_is_409() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;

    // Another version is there now: a conflict with its hash.
    let pv = preview(&rig, &editor, "p-1", delete_preview(&s, "a.yml", h("old: 0\n"))).await;
    let out = rig
        .hub
        .delete(
            &ctx(&editor, "k-1"),
            delete_req(&s, "a.yml", h("old: 0\n"), Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Conflict {
            current: Some(h("x: 1\n"))
        }
    );
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));

    // No file at all: a conflict with no current hash.
    let pv = preview(
        &rig,
        &editor,
        "p-2",
        delete_preview(&s, "missing.yml", h("q: 1\n")),
    )
    .await;
    let out = rig
        .hub
        .delete(
            &ctx(&editor, "k-2"),
            delete_req(&s, "missing.yml", h("q: 1\n"), Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(out, WriteOutcome::Conflict { current: None });
    assert_eq!(rig.agent.deletes_applied(), 0);
    assert_eq!(rig.chain().await.len(), 0);

    // The conflict is the answer to this request: the same key gets it again, and the file is still there.
    let pv = preview(&rig, &editor, "p-3", delete_preview(&s, "a.yml", h("old: 0\n"))).await;
    let again = rig
        .hub
        .delete(
            &ctx(&editor, "k-1"),
            delete_req(&s, "a.yml", h("old: 0\n"), Some(&pv)),
        )
        .await
        .unwrap();
    assert_eq!(
        again,
        WriteOutcome::Conflict {
            current: Some(h("x: 1\n"))
        }
    );
}

#[tokio::test]
async fn a_lost_delete_reply_is_resolved_by_the_retry_without_a_second_delete() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let pv = preview(&rig, &editor, "p-1", delete_preview(&s, "a.yml", h("x: 1\n"))).await;
    rig.agent.fault_next_write(Fault::LandThenTimeout);
    let out = rig
        .hub
        .delete(
            &ctx(&editor, "k-1"),
            delete_req(&s, "a.yml", h("x: 1\n"), Some(&pv)),
        )
        .await;
    assert_eq!(
        out,
        Err(WriteError::Unavailable),
        "the hub cannot know whether it landed"
    );
    assert!(rig.nfs_of(&s, "a.yml").is_none(), "it landed");
    assert_eq!(rig.chain().await.len(), 0);

    let retry = rig
        .hub
        .delete(
            &ctx(&editor, "k-1"),
            delete_req(&s, "a.yml", h("x: 1\n"), Some(&pv)),
        )
        .await
        .unwrap();
    assert!(
        matches!(retry, WriteOutcome::Applied { hash: None, .. }),
        "{retry:?}"
    );
    assert_eq!(rig.agent.deletes_applied(), 1, "found gone, not deleted again");
    assert_eq!(rig.chain().await.len(), 1);
    assert_eq!(rig.chain().await[0].event.action, AuditAction::FileDeleted);
}

// ------------------------------------------------------------------------------------------------------------- revert

/// `a.yml` has two versions: "x: 1" (kept in the blob store) and "x: 2" (on NFS now).
async fn with_history(rig: &Rig, s: &SwimlaneId) {
    rig.blobs.put(Bytes::from_static(b"x: 1\n")).await.unwrap();
    rig.agent.put(s, &nfs("a.yml"), b"x: 2\n");
    assert!(
        rig.registry
            .observe(s, &nfs("a.yml"), Bytes::from_static(b"x: 2\n"))
    );
}

fn revert_req(s: &SwimlaneId, path: &str, expected: Expected, to: ContentHash) -> RevertRequest {
    RevertRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        to,
        change_eol: None,
    }
}

#[tokio::test]
async fn revert_writes_a_stored_version_with_the_current_hash_precondition() {
    let Some((rig, s)) = rig().await else { return };
    with_history(&rig, &s).await;
    let editor = rig.user(1, Role::Editor).await;

    let out = rig
        .hub
        .revert(
            &ctx(&editor, "k-1"),
            revert_req(&s, "a.yml", at("x: 2\n"), h("x: 1\n")),
        )
        .await
        .unwrap();
    assert_eq!(applied(&out).0, Some(h("x: 1\n")));
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));

    // The agent was told which hash to find: the one the caller saw.
    let write = rig
        .agent
        .commands()
        .into_iter()
        .find_map(|c| match c {
            domain::HubCommand::WriteFile { expected, .. } => Some(expected),
            _ => None,
        })
        .unwrap();
    assert_eq!(write, at("x: 2\n"));

    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].event.action, AuditAction::FileReverted);
    assert_eq!(chain[0].event.hash_before, Some(h("x: 2\n")));
    assert_eq!(chain[0].event.hash_after, Some(h("x: 1\n")));
    let diff = chain[0].event.diff.as_deref().unwrap();
    assert!(diff.contains("-x: 2") && diff.contains("+x: 1"), "{diff}");
}

#[tokio::test]
async fn revert_conflict_is_409() {
    let Some((rig, s)) = rig().await else { return };
    with_history(&rig, &s).await;
    let editor = rig.user(1, Role::Editor).await;
    // The caller believes the file is at a third version.
    let out = rig
        .hub
        .revert(
            &ctx(&editor, "k-1"),
            revert_req(&s, "a.yml", at("x: 3\n"), h("x: 1\n")),
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Conflict {
            current: Some(h("x: 2\n"))
        }
    );
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn revert_cannot_copy_another_files_content() {
    let Some((rig, s)) = rig().await else { return };
    with_history(&rig, &s).await;
    let editor = rig.user(1, Role::Editor).await;
    // A blob that the store has, that is not a version of this file: another file's content, or a denied file's.
    let elsewhere = rig
        .blobs
        .put(Bytes::from_static(b"private: stuff\n"))
        .await
        .unwrap();
    let out = rig
        .hub
        .revert(
            &ctx(&editor, "k-1"),
            revert_req(&s, "a.yml", at("x: 2\n"), elsewhere),
        )
        .await
        .unwrap();
    assert_eq!(out, rejected(RejectReason::InvalidContent));
    // And a hash nobody ever stored.
    let nowhere = rig
        .hub
        .revert(
            &ctx(&editor, "k-2"),
            revert_req(&s, "a.yml", at("x: 2\n"), h("never existed\n")),
        )
        .await
        .unwrap();
    assert_eq!(nowhere, rejected(RejectReason::InvalidContent));
    // A file the registry does not know has no versions.
    let no_file = rig
        .hub
        .revert(
            &ctx(&editor, "k-3"),
            revert_req(&s, "nope.yml", Expected::Absent, elsewhere),
        )
        .await
        .unwrap();
    assert_eq!(no_file, rejected(RejectReason::InvalidContent));
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn revert_writes_the_version_as_it_was_and_converts_only_when_asked() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    // A version that mixes line endings, and a current one that is CRLF.
    let stored = b"a: 1\r\nb: 2\nc: 3\r\n";
    rig.blobs.put(Bytes::from_static(stored)).await.unwrap();
    rig.agent.put(&s, &nfs("m.yml"), b"a: 9\r\n");
    rig.registry
        .observe(&s, &nfs("m.yml"), Bytes::from_static(stored));
    rig.registry
        .observe(&s, &nfs("m.yml"), Bytes::from_static(b"a: 9\r\n"));

    let out = rig
        .hub
        .revert(
            &ctx(&editor, "k-1"),
            revert_req(&s, "m.yml", at("a: 9\r\n"), content_hash(stored)),
        )
        .await
        .unwrap();
    assert_eq!(applied(&out).0, Some(content_hash(stored)));
    assert_eq!(
        rig.nfs_of(&s, "m.yml").as_deref(),
        Some(&stored[..]),
        "byte for byte"
    );

    // With `change_eol` the style changes, and only then.
    rig.agent.put(&s, &nfs("m.yml"), b"a: 9\r\n");
    let out = rig
        .hub
        .revert(
            &ctx(&editor, "k-2"),
            RevertRequest {
                change_eol: Some(ChangeEol::Lf),
                ..revert_req(&s, "m.yml", at("a: 9\r\n"), content_hash(stored))
            },
        )
        .await
        .unwrap();
    assert_eq!(applied(&out).0, Some(h("a: 1\nb: 2\nc: 3\n")));
    assert_eq!(
        rig.nfs_of(&s, "m.yml").as_deref(),
        Some(&b"a: 1\nb: 2\nc: 3\n"[..])
    );
}

#[tokio::test]
async fn revert_does_not_restore_a_version_that_held_a_secret() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let leaky = format!("key: {}\n", aws_key());
    rig.blobs.put(Bytes::from(leaky.clone())).await.unwrap();
    rig.registry
        .observe(&s, &nfs("a.yml"), Bytes::from(leaky.clone()));
    rig.registry
        .observe(&s, &nfs("a.yml"), Bytes::from_static(b"x: 1\n"));
    let out = rig
        .hub
        .revert(
            &ctx(&editor, "k-1"),
            revert_req(&s, "a.yml", at("x: 1\n"), h(&leaky)),
        )
        .await
        .unwrap();
    let WriteOutcome::Rejected { reason, issues } = out else {
        panic!("{out:?}")
    };
    assert_eq!(reason, RejectReason::SecretDetected);
    assert_eq!(issues.len(), 1);
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
}

// ------------------------------------------------------------------------------------------------------------- restart

fn restart_req(s: &SwimlaneId, ns: &str, name: &str) -> RestartRequest {
    RestartRequest {
        swimlane: s.clone(),
        service: ServiceRef::new(ns, name).unwrap(),
    }
}

#[tokio::test]
async fn restart_sends_the_command_audits_once_and_hints_the_screens() {
    let Some((rig, s)) = rig().await else { return };
    rig.service(&s, "sit1", "payments", None).await;
    let operator = rig.user(1, Role::Operator).await;

    let out = rig
        .hub
        .restart(&ctx(&operator, "k-1"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { hash: None, .. }), "{out:?}");
    assert_eq!(rig.agent.restarts_applied(), 1);
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].event.action, AuditAction::ServiceRestarted);
    assert_eq!(
        chain[0].event.diff.as_deref(),
        Some("restart deployment sit1/payments")
    );
    assert_eq!(
        rig.bus.published().await,
        vec![DomainEvent::PickupChanged {
            swimlane: s.clone(),
            services: vec![ServiceRef::new("sit1", "payments").unwrap()]
        }]
    );
    // The same key again replays and restarts nothing.
    let again = rig
        .hub
        .restart(&ctx(&operator, "k-1"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert_eq!(again, out);
    assert_eq!(rig.agent.restarts_applied(), 1);
}

#[tokio::test]
async fn restart_of_a_service_the_swimlane_does_not_have_is_refused_before_anything_is_sent() {
    let Some((rig, s)) = rig().await else { return };
    rig.service(&s, "sit1", "payments", None).await;
    let operator = rig.user(1, Role::Operator).await;
    for (n, (ns, name)) in [("sit1", "ledger"), ("other", "payments")]
        .into_iter()
        .enumerate()
    {
        let out = rig
            .hub
            .restart(&ctx(&operator, &format!("k-{n}")), restart_req(&s, ns, name))
            .await
            .unwrap();
        assert_eq!(out, rejected(RejectReason::NotAllowed), "{ns}/{name}");
    }
    let unknown = rig
        .hub
        .restart(
            &ctx(&operator, "k-x"),
            restart_req(&SwimlaneId::parse("nope").unwrap(), "sit1", "payments"),
        )
        .await
        .unwrap();
    assert_eq!(unknown, rejected(RejectReason::UnknownSwimlane));
    assert!(rig.agent.commands().is_empty());
    assert_eq!(rig.chain().await.len(), 0);
}

#[tokio::test]
async fn restart_allows_one_per_service_at_a_time() {
    let Some((rig, s)) = rig().await else { return };
    rig.service(&s, "sit1", "payments", None).await;
    rig.service(&s, "sit1", "ledger", None).await;
    let operator = rig.user(1, Role::Operator).await;
    let second_operator = rig.user(2, Role::Operator).await;
    // The agent takes a while to answer, so the first restart is still running when the second arrives.
    rig.agent.set_delay(Duration::from_millis(400));

    let replica = rig.replica();
    let (c1, c2, c3) = (
        ctx(&operator, "k-1"),
        ctx(&second_operator, "k-2"),
        ctx(&operator, "k-3"),
    );
    let (a, b, other) = tokio::join!(
        rig.hub.restart(&c1, restart_req(&s, "sit1", "payments")),
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            replica.restart(&c2, restart_req(&s, "sit1", "payments")).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            rig.hub.restart(&c3, restart_req(&s, "sit1", "ledger")).await
        },
    );
    assert!(matches!(a.unwrap(), WriteOutcome::Applied { .. }));
    assert_eq!(
        b.unwrap(),
        WriteOutcome::Locked { until: None },
        "the same service, on another replica"
    );
    assert!(
        matches!(other.unwrap(), WriteOutcome::Applied { .. }),
        "another service is not held up"
    );
    assert_eq!(rig.agent.restarts_applied(), 2);

    // The lease is released with the restart: the service can be restarted again at once. The refused key is free too.
    rig.agent.set_delay(Duration::ZERO);
    let later = rig
        .hub
        .restart(&ctx(&second_operator, "k-2"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert!(matches!(later, WriteOutcome::Applied { .. }), "{later:?}");
    assert_eq!(rig.agent.restarts_applied(), 3);
    assert_eq!(rig.chain().await.len(), 3);
}

/// A lease that runs out while the command it guards is still running lets another replica send a second restart. The lease
/// follows the agent's timeout, however long that is set.
#[tokio::test]
async fn the_restart_lease_outlives_the_agent_timeout() {
    let Some(db) = support::pg::TestDb::ready().await else {
        return;
    };
    let config = hub_writes::WriteConfig {
        agent_timeout: Duration::from_secs(90),
        ..hub_writes::WriteConfig::default()
    };
    let rig = Rig::with_config(db, config).await;
    let s = rig.swimlane("sit1", true).await;
    rig.service(&s, "sit1", "payments", None).await;
    let operator = rig.user(1, Role::Operator).await;
    let out = rig
        .hub
        .restart(&ctx(&operator, "k-1"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    let asked = rig.leases.asked();
    let restart: Vec<_> = asked.iter().filter(|(n, _)| n.starts_with("restart:")).collect();
    assert_eq!(restart.len(), 1, "{asked:?}");
    assert!(
        restart[0].1 > Duration::from_secs(91),
        "the lease ({:?}) ends before the command can (90 s and the hub's extra second)",
        restart[0].1
    );
}

#[tokio::test]
async fn restart_answers_of_the_agent_become_outcomes() {
    let Some((rig, s)) = rig().await else { return };
    rig.service(&s, "sit1", "payments", None).await;
    let operator = rig.user(1, Role::Operator).await;

    rig.agent.fault_next_write(Fault::Denied);
    let denied = rig
        .hub
        .restart(&ctx(&operator, "k-1"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert_eq!(denied, rejected(RejectReason::Denied));

    rig.agent.fault_next_write(Fault::Io);
    let io = rig
        .hub
        .restart(&ctx(&operator, "k-2"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert_eq!(io, rejected(RejectReason::AgentUnavailable));

    rig.agent.fault_next_write(Fault::NotConnected);
    let down = rig
        .hub
        .restart(&ctx(&operator, "k-3"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert_eq!(down, rejected(RejectReason::AgentUnavailable));

    // The answer is lost: it is not known whether the rollout started. Nothing is recorded, and the key is free for a retry.
    rig.agent.fault_next_write(Fault::LandThenTimeout);
    let lost = rig
        .hub
        .restart(&ctx(&operator, "k-4"), restart_req(&s, "sit1", "payments"))
        .await;
    assert_eq!(lost, Err(WriteError::Unavailable));
    assert_eq!(
        rig.count("idempotency_keys").await,
        1,
        "only the stored denial is kept"
    );
    assert_eq!(rig.chain().await.len(), 0);
    // The lease was released on every path: the service is not stuck.
    let ok = rig
        .hub
        .restart(&ctx(&operator, "k-5"), restart_req(&s, "sit1", "payments"))
        .await
        .unwrap();
    assert!(matches!(ok, WriteOutcome::Applied { .. }), "{ok:?}");
}

#[tokio::test]
async fn restart_all_offers_every_service_in_the_confirmation() {
    let Some((rig, s)) = rig().await else { return };
    for (ns, name, app) in [
        ("sit1", "payments", Some("tx-api")),
        ("sit1", "ledger", Some("tx-ledger")),
        ("sit1", "gateway", None),
    ] {
        rig.service(&s, ns, name, app).await;
    }
    sqlx::query("UPDATE services SET removed_at = now() WHERE name = 'gateway'")
        .execute(&rig.pool)
        .await
        .unwrap();
    rig.service(&s, "sit1", "tx-api", Some("tx-api")).await;
    let operator = rig.user(1, Role::Operator).await;
    let editor = rig.user(2, Role::Editor).await;

    // A file in the root folder is read by every service: the offer lists all of them (the removed one is gone).
    let all = rig
        .hub
        .restart_offer(&ctx(&operator, "o-1"), &s, &nfs("application.yml"))
        .await
        .unwrap();
    let names: Vec<String> = all
        .iter()
        .map(|r| format!("{}/{}", r.namespace(), r.name()))
        .collect();
    assert_eq!(names, ["sit1/ledger", "sit1/payments", "sit1/tx-api"]);

    // A file in an application's folder is read by that application's Deployments.
    let one = rig
        .hub
        .restart_offer(&ctx(&operator, "o-2"), &s, &nfs("tx-api/entry.yml"))
        .await
        .unwrap();
    assert_eq!(one, vec![ServiceRef::new("sit1", "tx-api").unwrap()]);

    // The offer is for an Operator, who is the only one who can act on it.
    assert_eq!(
        rig.hub
            .restart_offer(&ctx(&editor, "o-3"), &s, &nfs("application.yml"))
            .await,
        Err(WriteError::Forbidden)
    );
    // And each restart is its own request: one that fails does not hide the others.
    for (n, svc) in all.iter().enumerate() {
        let out = rig
            .hub
            .restart(
                &ctx(&operator, &format!("all-{n}")),
                restart_req(&s, svc.namespace(), svc.name()),
            )
            .await
            .unwrap();
        assert!(matches!(out, WriteOutcome::Applied { .. }));
    }
    assert_eq!(rig.chain().await.len(), 3, "one audit event for each restart");
}

// ------------------------------------------------------------------------------------------------------ approval users

#[tokio::test]
async fn approval_user_gets_a_proposal_for_every_t3_action_and_nothing_is_done() {
    let Some((rig, s)) = rig().await else { return };
    with_history(&rig, &s).await;
    rig.service(&s, "sit1", "payments", None).await;
    let careful = rig.approval_user(1, Role::Operator).await;

    let up_pv = preview(
        &rig,
        &careful,
        "p-1",
        upload_preview(&s, "new.yml", Expected::Absent, b"n: 1\n"),
    )
    .await;
    let del_pv = preview(
        &rig,
        &careful,
        "p-2",
        delete_preview(&s, "tx-api/solo.yml", h("s: 1\n")),
    )
    .await;
    let outcomes = [
        rig.hub
            .upload(
                &ctx(&careful, "k-1"),
                upload_req(&s, "new.yml", Expected::Absent, b"n: 1\n", Some(&up_pv)),
            )
            .await
            .unwrap(),
        rig.hub
            .delete(
                &ctx(&careful, "k-2"),
                delete_req(&s, "tx-api/solo.yml", h("s: 1\n"), Some(&del_pv)),
            )
            .await
            .unwrap(),
        rig.hub
            .revert(
                &ctx(&careful, "k-3"),
                revert_req(&s, "a.yml", at("x: 2\n"), h("x: 1\n")),
            )
            .await
            .unwrap(),
        rig.hub
            .restart(&ctx(&careful, "k-4"), restart_req(&s, "sit1", "payments"))
            .await
            .unwrap(),
    ];
    for (o, action) in outcomes.iter().zip(["upload", "delete", "revert", "restart"]) {
        assert!(
            matches!(o, WriteOutcome::ProposalCreated { .. }),
            "{action}: {o:?}"
        );
    }
    // Nothing reached NFS or the cluster; each proposal is pending, with its audit event.
    assert!(rig.nfs_of(&s, "new.yml").is_none());
    assert!(rig.nfs_of(&s, "tx-api/solo.yml").is_some());
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    assert_eq!(rig.agent.write_commands(), 0);
    assert_eq!(rig.agent.deletes_applied(), 0);
    assert_eq!(rig.agent.restarts_applied(), 0);
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM proposals ORDER BY id")
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(actions, ["upload", "delete", "revert", "restart"]);
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 4);
    assert!(
        chain
            .iter()
            .all(|e| e.event.action == AuditAction::ProposalCreated)
    );
    assert_eq!(
        rig.bus
            .published()
            .await
            .iter()
            .filter(|e| matches!(
                e,
                DomainEvent::ProposalChanged {
                    status: ProposalStatus::Pending,
                    ..
                }
            ))
            .count(),
        4
    );
    // The revert proposal names the version it restores; the restart names the service.
    let params: Vec<serde_json::Value> = sqlx::query_scalar("SELECT params FROM proposals ORDER BY id")
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(params[2], serde_json::json!({ "to": h("x: 1\n").to_string() }));
    assert_eq!(
        params[3],
        serde_json::json!({ "namespace": "sit1", "name": "payments" })
    );
}

// ------------------------------------------------------------------------------------------------------------ the preview

fn warnings(p: &ChangePreview) -> &[PreviewWarning] {
    &p.warnings
}

#[tokio::test]
async fn preview_of_a_new_tenant_file_forks_from_base_and_says_who_reads_it() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let p = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "tx-api/entry-sit1.yml", Expected::Absent, b"a: 2\nb: 2\n"),
    )
    .await;
    assert!(warnings(&p).contains(&PreviewWarning::ForksFromBase), "{p:?}");
    assert!(
        matches!(&p.scope, PreviewScope::Application { app } if app.as_str() == "tx-api"),
        "{p:?}"
    );
    assert!(p.effective_changed);
    assert_eq!(p.changes_total, 1);
    assert_eq!(p.changes[0].change.path.as_str(), "a");
    assert_eq!(p.changes[0].change.left.as_ref().unwrap().text, "1");
    assert_eq!(p.changes[0].change.right.as_ref().unwrap().text, "2");
    assert!(!p.changes[0].flagged);
    // The token it carries is the one the upload takes.
    assert_eq!(p.expires_at.unix_millis(), support::rig::START + 600_000);
}

#[tokio::test]
async fn a_suffix_that_is_not_a_tenant_of_the_swimlane_is_part_of_the_name() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    // `sit9` is not one of the swimlane's tenants, so `-sit9` is part of the file's name: a base file, which forks from nothing.
    let p = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "tx-api/entry-sit9.yml", Expected::Absent, b"a: 2\n"),
    )
    .await;
    assert!(!warnings(&p).contains(&PreviewWarning::ForksFromBase), "{p:?}");
}

#[tokio::test]
async fn preview_of_a_delete_says_what_the_services_fall_back_to() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    rig.agent.put(&s, &nfs("tx-api/entry-sit1.yml"), b"a: 5\nb: 2\n");
    rig.catch_up_registry(&s);

    let tenant = preview(
        &rig,
        &editor,
        "p-1",
        delete_preview(&s, "tx-api/entry-sit1.yml", h("a: 5\nb: 2\n")),
    )
    .await;
    assert!(
        warnings(&tenant).contains(&PreviewWarning::FallsBackToBase),
        "{tenant:?}"
    );
    assert!(tenant.effective_changed);

    let only = preview(
        &rig,
        &editor,
        "p-2",
        delete_preview(&s, "tx-api/solo.yml", h("s: 1\n")),
    )
    .await;
    assert!(warnings(&only).contains(&PreviewWarning::NoFileLeft), "{only:?}");
    assert!(only.effective_changed);

    // A file that is not there: nothing changes.
    let none = preview(
        &rig,
        &editor,
        "p-3",
        delete_preview(&s, "tx-api/missing.yml", h("zzz\n")),
    )
    .await;
    assert!(!none.effective_changed, "{none:?}");
    assert_eq!(none.changes_total, 0);
}

#[tokio::test]
async fn preview_of_a_property_source_and_a_new_folder_say_what_has_to_restart() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    rig.agent
        .put(&s, &nfs("application.yml"), b"server:\n  port: 80\n");
    rig.catch_up_registry(&s);

    let p = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(
            &s,
            "application.yml",
            at("server:\n  port: 80\n"),
            b"server:\n  port: 81\n",
        ),
    )
    .await;
    assert_eq!(p.scope, PreviewScope::EveryService);
    assert!(
        warnings(&p).contains(&PreviewWarning::NeedsNotifyOrRestart),
        "{p:?}"
    );
    assert_eq!(p.changes[0].change.path.as_str(), "server.port");

    let new_folder = preview(
        &rig,
        &editor,
        "p-2",
        upload_preview(&s, "brand-new/x.yml", Expected::Absent, b"q: 1\n"),
    )
    .await;
    assert!(
        warnings(&new_folder).contains(&PreviewWarning::NewFolderNeedsConfigServerRestart),
        "{new_folder:?}"
    );
    let existing = preview(
        &rig,
        &editor,
        "p-3",
        upload_preview(&s, "tx-api/new.yml", Expected::Absent, b"q: 1\n"),
    )
    .await;
    assert!(!warnings(&existing).contains(&PreviewWarning::NewFolderNeedsConfigServerRestart));
    let root = preview(
        &rig,
        &editor,
        "p-4",
        upload_preview(&s, "root-new.yml", Expected::Absent, b"q: 1\n"),
    )
    .await;
    assert!(!warnings(&root).contains(&PreviewWarning::NewFolderNeedsConfigServerRestart));
}

#[tokio::test]
async fn preview_masks_a_flagged_value_on_either_side_and_never_shows_it() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let key = aws_key();
    let leaky_old = format!("a: 1\nkey: {key}\n");
    rig.agent.put(&s, &nfs("tx-api/leaky.yml"), leaky_old.as_bytes());
    rig.catch_up_registry(&s);

    // The new value is the secret.
    let adds = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(
            &s,
            "tx-api/entry.yml",
            at("a: 1\nb: 2\n"),
            format!("a: 1\nb: {key}\n").as_bytes(),
        ),
    )
    .await;
    let c = &adds.changes[0];
    assert!(c.flagged, "{adds:?}");
    assert!(
        c.change.right.as_ref().unwrap().text.starts_with("[REDACTED: "),
        "{:?}",
        c.change.right
    );
    assert_eq!(
        c.change.left.as_ref().unwrap().text,
        "2",
        "the side that holds no secret is shown"
    );

    // The old value is the secret.
    let removes = preview(
        &rig,
        &editor,
        "p-2",
        upload_preview(&s, "tx-api/leaky.yml", at(&leaky_old), b"a: 1\nkey: plain\n"),
    )
    .await;
    let c = &removes.changes[0];
    assert!(c.flagged);
    assert!(c.change.left.as_ref().unwrap().text.starts_with("[REDACTED: "));
    assert_eq!(c.change.right.as_ref().unwrap().text, "plain");

    // A delete shows the value it removes only as a marker.
    let del = preview(
        &rig,
        &editor,
        "p-3",
        delete_preview(&s, "tx-api/leaky.yml", h(&leaky_old)),
    )
    .await;
    assert!(del.changes.iter().any(|c| c.flagged));

    for p in [&adds, &removes, &del] {
        let json = serde_json::to_string(p).unwrap();
        assert!(!json.contains(&key), "{json}");
        assert!(!format!("{p:?}").contains(&key));
    }
}

#[tokio::test]
async fn preview_judges_the_bytes_that_would_be_written() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    // The file is CRLF and the upload is LF: written as CRLF, it is the same file, so the services see no change.
    let p = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "crlf/c.yml", at("k: 1\r\nm: 2\r\n"), b"k: 1\nm: 2\n"),
    )
    .await;
    assert!(!p.effective_changed, "{p:?}");
    assert_eq!(p.changes_total, 0);
}

#[tokio::test]
async fn preview_effect_is_for_editors_and_asks_about_swimlanes_that_exist() {
    let Some((rig, s)) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let viewer = rig.user(2, Role::Viewer).await;
    assert_eq!(
        rig.hub
            .preview_effect(
                &ctx(&viewer, "p"),
                upload_preview(&s, "n.yml", Expected::Absent, b"n: 1\n")
            )
            .await
            .err(),
        Some(WriteError::Forbidden)
    );
    assert_eq!(
        rig.hub
            .preview_effect(
                &ctx(&editor, "p"),
                upload_preview(
                    &SwimlaneId::parse("nope").unwrap(),
                    "n.yml",
                    Expected::Absent,
                    b"n: 1\n"
                )
            )
            .await
            .err(),
        Some(WriteError::NotFound)
    );
    // It writes, audits and records nothing, and is not idempotency-keyed.
    let _ = preview(
        &rig,
        &editor,
        "p-1",
        upload_preview(&s, "n.yml", Expected::Absent, b"n: 1\n"),
    )
    .await;
    assert_eq!(rig.count("idempotency_keys").await, 0);
    assert_eq!(rig.chain().await.len(), 0);
    assert!(rig.bus.published().await.is_empty());
    assert_eq!(rig.agent.write_commands(), 0);
}
