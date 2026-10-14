use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    ChangeEol, DeleteRequest, EditRequest, Expected, NfsPath, NotifyRequest, PrRequest, PrTarget,
    PreviewChange, PreviewId, PreviewRequest, RejectReason, RestartRequest, RevertRequest, Role, ServiceRef,
    ShortText, SwimlaneId, TenantId, UploadRequest, UserStatus, WriteCtx, WriteOutcome,
};

use super::sample::{active, files, hash, nfs, swimlane, user, write_ctx};
use crate::{WriteError, WriteService};

/// What the write conformance suite needs from the test harness.
#[async_trait]
pub trait WriteScenario: WriteService {
    /// Make `s` exist with exactly these files on NFS.
    async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]);
    /// The file's current content on NFS.
    async fn nfs_content(&self, s: &SwimlaneId, p: &NfsPath) -> Option<Bytes>;
}

fn edit(s: &SwimlaneId, path: &str, expected: Expected, content: &str) -> EditRequest {
    EditRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        content: Bytes::copy_from_slice(content.as_bytes()),
        change_eol: None,
    }
}

/// The reason of a rejection, whatever detail it carries.
fn rejection(o: &WriteOutcome) -> RejectReason {
    match o {
        WriteOutcome::Rejected { reason, .. } => *reason,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

/// A preview for an upload, issued to `ctx`'s user.
async fn preview_upload<W: WriteScenario + ?Sized>(
    w: &W,
    ctx: &WriteCtx,
    s: &SwimlaneId,
    path: &str,
    expected: Expected,
    content: &Bytes,
) -> PreviewId {
    let r = PreviewRequest {
        swimlane: s.clone(),
        path: nfs(path),
        change: PreviewChange::Upload {
            expected,
            content: content.clone(),
        },
    };
    w.preview_effect(ctx, r).await.unwrap().preview_id
}

async fn preview_delete<W: WriteScenario + ?Sized>(
    w: &W,
    ctx: &WriteCtx,
    s: &SwimlaneId,
    path: &str,
    expected: domain::ContentHash,
) -> PreviewId {
    let r = PreviewRequest {
        swimlane: s.clone(),
        path: nfs(path),
        change: PreviewChange::Delete { expected },
    };
    w.preview_effect(ctx, r).await.unwrap().preview_id
}

fn nowhere_swimlane() -> SwimlaneId {
    swimlane("nowhere")
}

fn applied_hash(o: &WriteOutcome) -> Option<domain::ContentHash> {
    match o {
        WriteOutcome::Applied { hash, .. } => *hash,
        other => panic!("expected Applied, got {other:?}"),
    }
}

/// - Role floors: Editor for file writes, drafts and PRs, Operator for restarts; pending, disabled and
///   role-less users are `Forbidden` (S4). A user who `requires_approval` gets a proposal and nothing is
///   written (D37).
/// - No blind writes: a stale hash is `Conflict { current }` and writes nothing; creating needs `Absent`.
/// - Retrying with the same idempotency key returns the first outcome and writes nothing again; the same
///   key for another request is `IdempotencyConflict` (D66).
/// - Unknown swimlanes and oversized content are rejected without writing.
/// - A write keeps the file's line endings unless `change_eol` says otherwise; a revert writes the stored version as
///   it was unless `change_eol` says otherwise.
/// - An upload or a delete needs a preview issued to the same user for the same change, or it is `NotAllowed` and
///   writes nothing; previewing needs Editor and changes nothing.
/// - Notify needs Operator, takes 1-100 paths, and becomes a proposal for a user who `requires_approval`.
/// - A draft has no side effects until applied, only its author can apply it, and applying re-checks the hash.
/// - Revert restores a known earlier version only; a PR names 1-100 paths.
pub async fn write_service<W: WriteScenario + ?Sized>(w: &W) {
    let s = swimlane("sit1");
    let editor = active(1, Role::Editor);
    let h0 = hash(b"x: 1\n");
    w.seed(
        &s,
        &files(&[
            ("a.yml", "x: 1\n"),
            ("b.yml", "k: v\n"),
            ("crlf.yml", "k: 1\r\nj: 2\r\n"),
        ]),
    )
    .await;
    let a = nfs("a.yml");

    // Edit, conflict, idempotency.
    let first = edit(&s, "a.yml", Expected::Hash { hash: h0 }, "x: 2\n");
    let out = w.edit(&write_ctx(&editor, "k-1"), first.clone()).await.unwrap();
    let h1 = applied_hash(&out).expect("hash of the new content");
    assert_eq!(h1, hash(b"x: 2\n"));
    assert_eq!(w.nfs_content(&s, &a).await.as_deref(), Some(&b"x: 2\n"[..]));

    let stale = w
        .edit(
            &write_ctx(&editor, "k-2"),
            edit(&s, "a.yml", Expected::Hash { hash: h0 }, "x: 3\n"),
        )
        .await
        .unwrap();
    assert_eq!(stale, WriteOutcome::Conflict { current: Some(h1) }, "stale hash");
    assert_eq!(
        w.nfs_content(&s, &a).await.as_deref(),
        Some(&b"x: 2\n"[..]),
        "a conflict writes nothing"
    );

    let retry = w.edit(&write_ctx(&editor, "k-1"), first.clone()).await.unwrap();
    assert_eq!(retry, out, "same key and request: the first outcome again");
    assert_eq!(w.nfs_content(&s, &a).await.as_deref(), Some(&b"x: 2\n"[..]));
    let reused = w
        .edit(
            &write_ctx(&editor, "k-1"),
            edit(&s, "b.yml", Expected::Absent, "z: 0\n"),
        )
        .await;
    assert_eq!(
        reused,
        Err(WriteError::IdempotencyConflict),
        "key reused for another request"
    );

    // Create needs Absent.
    let created = w
        .edit(
            &write_ctx(&editor, "k-3"),
            edit(&s, "new.yml", Expected::Absent, "n: 1\n"),
        )
        .await
        .unwrap();
    assert!(applied_hash(&created).is_some());
    let again = w
        .edit(
            &write_ctx(&editor, "k-4"),
            edit(&s, "new.yml", Expected::Absent, "n: 2\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        again,
        WriteOutcome::Conflict {
            current: Some(hash(b"n: 1\n"))
        },
        "create over an existing file"
    );

    // Line endings: kept unless the caller asks for another style.
    let crlf = nfs("crlf.yml");
    let hc0 = hash(b"k: 1\r\nj: 2\r\n");
    let kept = w
        .edit(
            &write_ctx(&editor, "e-1"),
            edit(&s, "crlf.yml", Expected::Hash { hash: hc0 }, "k: 1\nj: 3\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        applied_hash(&kept),
        Some(hash(b"k: 1\r\nj: 3\r\n")),
        "LF input keeps CRLF"
    );
    assert_eq!(
        w.nfs_content(&s, &crlf).await.as_deref(),
        Some(&b"k: 1\r\nj: 3\r\n"[..])
    );
    let to_lf = EditRequest {
        change_eol: Some(ChangeEol::Lf),
        ..edit(
            &s,
            "crlf.yml",
            Expected::Hash {
                hash: hash(b"k: 1\r\nj: 3\r\n"),
            },
            "k: 1\r\nj: 3\r\n",
        )
    };
    assert_eq!(
        applied_hash(&w.edit(&write_ctx(&editor, "e-2"), to_lf).await.unwrap()),
        Some(hash(b"k: 1\nj: 3\n")),
        "an explicit change to LF"
    );
    let kept_lf = w
        .edit(
            &write_ctx(&editor, "e-3"),
            edit(
                &s,
                "crlf.yml",
                Expected::Hash {
                    hash: hash(b"k: 1\nj: 3\n"),
                },
                "k: 1\r\nj: 4\r\n",
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        applied_hash(&kept_lf),
        Some(hash(b"k: 1\nj: 4\n")),
        "the file is LF now, and CRLF input does not change that"
    );
    let to_crlf = EditRequest {
        change_eol: Some(ChangeEol::Crlf),
        ..edit(
            &s,
            "crlf.yml",
            Expected::Hash {
                hash: hash(b"k: 1\nj: 4\n"),
            },
            "k: 1\nj: 4\n",
        )
    };
    assert_eq!(
        applied_hash(&w.edit(&write_ctx(&editor, "e-4"), to_crlf).await.unwrap()),
        Some(hash(b"k: 1\r\nj: 4\r\n")),
        "an explicit change to CRLF"
    );
    // A revert writes the stored version as it was, or in the style asked for.
    let revert_crlf = |change_eol, key: &str, expected: &[u8]| {
        let ctx = write_ctx(&editor, key);
        let req = RevertRequest {
            swimlane: s.clone(),
            path: crlf.clone(),
            expected: Expected::Hash { hash: hash(expected) },
            to: hc0,
            change_eol,
        };
        async move { w.revert(&ctx, req).await.unwrap() }
    };
    assert_eq!(
        applied_hash(&revert_crlf(Some(ChangeEol::Lf), "e-5", b"k: 1\r\nj: 4\r\n").await),
        Some(hash(b"k: 1\nj: 2\n")),
        "revert with change_eol"
    );
    assert_eq!(
        applied_hash(&revert_crlf(None, "e-6", b"k: 1\nj: 2\n").await),
        Some(hc0),
        "revert without change_eol writes the stored version as it was"
    );

    // Upload shares the rules, and needs a preview issued to this user for this change (D46).
    let logo = Bytes::from_static(&[0, 1, 2, 0xff]);
    let upload_with = |preview: Option<PreviewId>, content: &Bytes| UploadRequest {
        swimlane: s.clone(),
        path: nfs("img/logo.bmp"),
        expected: Expected::Absent,
        content: content.clone(),
        preview,
    };
    let no_preview = w
        .upload(&write_ctx(&editor, "k-5a"), upload_with(None, &logo))
        .await
        .unwrap();
    assert_eq!(rejection(&no_preview), RejectReason::NotAllowed, "no preview");
    let made_up = PreviewId::parse("made-up-token").unwrap();
    let guessed = w
        .upload(&write_ctx(&editor, "k-5b"), upload_with(Some(made_up), &logo))
        .await
        .unwrap();
    assert_eq!(
        rejection(&guessed),
        RejectReason::NotAllowed,
        "a token nobody issued"
    );
    let editor2 = active(8, Role::Editor);
    let theirs = preview_upload(
        w,
        &write_ctx(&editor2, "pv-a"),
        &s,
        "img/logo.bmp",
        Expected::Absent,
        &logo,
    )
    .await;
    let borrowed = w
        .upload(&write_ctx(&editor, "k-5c"), upload_with(Some(theirs), &logo))
        .await
        .unwrap();
    assert_eq!(
        rejection(&borrowed),
        RejectReason::NotAllowed,
        "bound to the user"
    );
    let other_bytes = Bytes::from_static(&[9, 9, 9, 0]);
    let for_other = preview_upload(
        w,
        &write_ctx(&editor, "pv-b"),
        &s,
        "img/logo.bmp",
        Expected::Absent,
        &other_bytes,
    )
    .await;
    let swapped = w
        .upload(&write_ctx(&editor, "k-5d"), upload_with(Some(for_other), &logo))
        .await
        .unwrap();
    assert_eq!(
        rejection(&swapped),
        RejectReason::NotAllowed,
        "bound to the content"
    );
    let for_path = preview_upload(
        w,
        &write_ctx(&editor, "pv-c"),
        &s,
        "img/other.bmp",
        Expected::Absent,
        &logo,
    )
    .await;
    let moved = w
        .upload(&write_ctx(&editor, "k-5e"), upload_with(Some(for_path), &logo))
        .await
        .unwrap();
    assert_eq!(rejection(&moved), RejectReason::NotAllowed, "bound to the path");
    assert_eq!(
        w.nfs_content(&s, &nfs("img/logo.bmp")).await,
        None,
        "refused uploads write nothing"
    );
    let pv = preview_upload(
        w,
        &write_ctx(&editor, "pv-d"),
        &s,
        "img/logo.bmp",
        Expected::Absent,
        &logo,
    )
    .await;
    assert!(
        w.upload(&write_ctx(&editor, "k-5"), upload_with(Some(pv), &logo))
            .await
            .unwrap()
            .is_success()
    );
    assert_eq!(
        w.nfs_content(&s, &nfs("img/logo.bmp")).await.as_deref(),
        Some(&[0u8, 1, 2, 0xff][..])
    );

    // Previewing needs Editor, a swimlane that exists, and changes nothing.
    let viewer_ctx = write_ctx(&active(2, Role::Viewer), "pv-v");
    assert_eq!(
        w.preview_effect(
            &viewer_ctx,
            PreviewRequest {
                swimlane: s.clone(),
                path: nfs("img/logo.bmp"),
                change: PreviewChange::Delete { expected: hash(b"x") },
            }
        )
        .await
        .map(|p| p.preview_id),
        Err(WriteError::Forbidden)
    );
    assert_eq!(
        w.preview_effect(
            &write_ctx(&editor, "pv-e"),
            PreviewRequest {
                swimlane: swimlane("nowhere"),
                path: nfs("a.yml"),
                change: PreviewChange::Delete { expected: hash(b"x") },
            }
        )
        .await
        .map(|p| p.preview_id),
        Err(WriteError::NotFound)
    );

    // Delete needs a preview and the current hash. The file moves on after the preview: a conflict.
    let stale_preview = preview_delete(w, &write_ctx(&editor, "pv-f"), &s, "b.yml", hash(b"k: v\n")).await;
    let delete_with = |preview: Option<PreviewId>, expected| DeleteRequest {
        swimlane: s.clone(),
        path: nfs("b.yml"),
        expected,
        preview,
    };
    assert_eq!(
        rejection(
            &w.delete(&write_ctx(&editor, "k-6a"), delete_with(None, hash(b"k: v\n")))
                .await
                .unwrap()
        ),
        RejectReason::NotAllowed,
        "a delete needs a preview"
    );
    assert!(w.nfs_content(&s, &nfs("b.yml")).await.is_some());
    assert!(
        w.edit(
            &write_ctx(&editor, "k-6b"),
            edit(
                &s,
                "b.yml",
                Expected::Hash {
                    hash: hash(b"k: v\n")
                },
                "k: w\n"
            )
        )
        .await
        .unwrap()
        .is_success()
    );
    let wrong = w
        .delete(
            &write_ctx(&editor, "k-6"),
            delete_with(Some(stale_preview), hash(b"k: v\n")),
        )
        .await
        .unwrap();
    assert_eq!(
        wrong,
        WriteOutcome::Conflict {
            current: Some(hash(b"k: w\n"))
        }
    );
    assert!(w.nfs_content(&s, &nfs("b.yml")).await.is_some());
    let fresh = preview_delete(w, &write_ctx(&editor, "pv-g"), &s, "b.yml", hash(b"k: w\n")).await;
    let del = w
        .delete(
            &write_ctx(&editor, "k-7"),
            delete_with(Some(fresh), hash(b"k: w\n")),
        )
        .await
        .unwrap();
    assert!(del.is_success());
    assert_eq!(w.nfs_content(&s, &nfs("b.yml")).await, None);

    // Revert to a known earlier version, but not to an unknown one.
    let revert = |to, key: &str| {
        let ctx = write_ctx(&editor, key);
        let req = RevertRequest {
            swimlane: s.clone(),
            path: a.clone(),
            expected: Expected::Hash { hash: h1 },
            to,
            change_eol: None,
        };
        async move { w.revert(&ctx, req).await }
    };
    assert_eq!(
        revert(hash(b"never existed"), "k-8").await.unwrap(),
        WriteOutcome::Rejected {
            reason: RejectReason::InvalidContent,
            issues: Vec::new()
        }
    );
    assert_eq!(applied_hash(&revert(h0, "k-9").await.unwrap()), Some(h0));
    assert_eq!(w.nfs_content(&s, &a).await.as_deref(), Some(&b"x: 1\n"[..]));

    // Rejections that write nothing.
    let nowhere = nowhere_swimlane();
    assert_eq!(
        w.edit(
            &write_ctx(&editor, "k-10"),
            edit(&nowhere, "a.yml", Expected::Absent, "x")
        )
        .await
        .unwrap(),
        WriteOutcome::Rejected {
            reason: RejectReason::UnknownSwimlane,
            issues: Vec::new()
        }
    );
    let huge = "x".repeat(2 * 1024 * 1024 + 1);
    assert_eq!(
        w.edit(
            &write_ctx(&editor, "k-11"),
            edit(&s, "huge.yml", Expected::Absent, &huge)
        )
        .await
        .unwrap(),
        WriteOutcome::Rejected {
            reason: RejectReason::TooLarge,
            issues: Vec::new()
        }
    );
    assert_eq!(w.nfs_content(&s, &nfs("huge.yml")).await, None);

    // Role floors.
    let viewer = active(2, Role::Viewer);
    let operator = active(3, Role::Operator);
    let attempt = edit(&s, "a.yml", Expected::Hash { hash: h0 }, "hacked\n");
    assert_eq!(
        w.edit(&write_ctx(&viewer, "v-1"), attempt.clone()).await,
        Err(WriteError::Forbidden)
    );
    for status in [UserStatus::Pending, UserStatus::Disabled] {
        let inactive = user(4, Some(Role::Admin), status);
        assert_eq!(
            w.edit(&write_ctx(&inactive, "i-1"), attempt.clone()).await,
            Err(WriteError::Forbidden),
            "{status:?} users never write"
        );
    }
    assert_eq!(
        w.edit(
            &write_ctx(&user(5, None, UserStatus::Active), "n-1"),
            attempt.clone()
        )
        .await,
        Err(WriteError::Forbidden)
    );
    assert_eq!(w.nfs_content(&s, &a).await.as_deref(), Some(&b"x: 1\n"[..]));
    let restart = RestartRequest {
        swimlane: s.clone(),
        service: ServiceRef::new("sit1", "payments").unwrap(),
    };
    assert_eq!(
        w.restart(&write_ctx(&editor, "r-1"), restart.clone()).await,
        Err(WriteError::Forbidden)
    );
    assert!(
        w.restart(&write_ctx(&operator, "r-2"), restart)
            .await
            .unwrap()
            .is_success()
    );

    // Notify the config-server: Operator, 1-100 paths, a proposal for a user who requires approval.
    let notify = |paths: Vec<NfsPath>, swimlane: &SwimlaneId| NotifyRequest {
        swimlane: swimlane.clone(),
        paths,
    };
    assert_eq!(
        w.notify(&write_ctx(&editor, "n-0"), notify(vec![a.clone()], &s))
            .await,
        Err(WriteError::Forbidden),
        "an Editor does not notify (Q37)"
    );
    let notified = w
        .notify(&write_ctx(&operator, "n-1"), notify(vec![a.clone()], &s))
        .await
        .unwrap();
    assert!(matches!(notified, WriteOutcome::Applied { .. }), "{notified:?}");
    assert_eq!(
        w.notify(&write_ctx(&operator, "n-1"), notify(vec![a.clone()], &s))
            .await
            .unwrap(),
        notified,
        "same key and request: the first outcome again"
    );
    assert_eq!(
        w.notify(&write_ctx(&operator, "n-1"), notify(vec![nfs("b.yml")], &s))
            .await,
        Err(WriteError::IdempotencyConflict)
    );
    assert_eq!(
        w.notify(&write_ctx(&operator, "n-2"), notify(vec![], &s)).await,
        Err(WriteError::BadRequest)
    );
    let too_many_notify: Vec<NfsPath> = (0..=NotifyRequest::MAX_PATHS)
        .map(|i| nfs(&format!("f{i}.yml")))
        .collect();
    assert_eq!(
        w.notify(&write_ctx(&operator, "n-3"), notify(too_many_notify, &s))
            .await,
        Err(WriteError::BadRequest)
    );
    assert_eq!(
        rejection(
            &w.notify(
                &write_ctx(&operator, "n-4"),
                notify(vec![a.clone()], &nowhere_swimlane())
            )
            .await
            .unwrap()
        ),
        RejectReason::UnknownSwimlane
    );
    let mut careful_operator = active(9, Role::Operator);
    careful_operator.requires_approval = true;
    assert!(matches!(
        w.notify(&write_ctx(&careful_operator, "n-5"), notify(vec![a.clone()], &s))
            .await
            .unwrap(),
        WriteOutcome::ProposalCreated { .. }
    ));

    // Approval: a proposal, no write.
    let mut careful = active(6, Role::Editor);
    careful.requires_approval = true;
    let proposal = w
        .edit(&write_ctx(&careful, "p-1"), attempt.clone())
        .await
        .unwrap();
    assert!(matches!(proposal, WriteOutcome::ProposalCreated { .. }));
    assert_eq!(
        w.nfs_content(&s, &a).await.as_deref(),
        Some(&b"x: 1\n"[..]),
        "nothing written"
    );

    // Pull requests.
    let pr = |paths: Vec<NfsPath>| PrRequest {
        swimlane: s.clone(),
        paths,
        title: ShortText::parse("Sync NFS changes").unwrap(),
        body: None,
        target: None,
    };
    assert!(
        w.raise_pr(&write_ctx(&editor, "pr-1"), pr(vec![a.clone()]))
            .await
            .unwrap()
            .is_success()
    );
    let targeted = |target| PrRequest {
        target: Some(target),
        ..pr(vec![a.clone()])
    };
    assert!(
        w.raise_pr(
            &write_ctx(&editor, "pr-4"),
            targeted(PrTarget::Tenant {
                branch: TenantId::parse("sit1").unwrap()
            })
        )
        .await
        .unwrap()
        .is_success(),
        "an explicit tenant target"
    );
    assert!(
        w.raise_pr(&write_ctx(&editor, "pr-5"), targeted(PrTarget::Base))
            .await
            .unwrap()
            .is_success(),
        "an explicit base target"
    );
    assert_eq!(
        w.raise_pr(&write_ctx(&editor, "pr-4"), targeted(PrTarget::Base))
            .await,
        Err(WriteError::IdempotencyConflict),
        "the target is part of what the key stands for"
    );
    let too_many: Vec<NfsPath> = (0..=PrRequest::MAX_PATHS)
        .map(|i| nfs(&format!("f{i}.yml")))
        .collect();
    assert_eq!(
        w.raise_pr(&write_ctx(&editor, "pr-2"), pr(too_many)).await,
        Err(WriteError::BadRequest)
    );
    assert_eq!(
        w.raise_pr(&write_ctx(&editor, "pr-3"), pr(vec![])).await,
        Err(WriteError::BadRequest)
    );

    // Drafts.
    let draft = w
        .propose_draft(
            &write_ctx(&editor, "d-1"),
            edit(&s, "a.yml", Expected::Hash { hash: h0 }, "x: draft\n"),
        )
        .await
        .unwrap();
    assert_eq!(draft.new_hash, hash(b"x: draft\n"));
    assert_eq!(draft.author, editor.id);
    assert_eq!(
        w.nfs_content(&s, &a).await.as_deref(),
        Some(&b"x: 1\n"[..]),
        "a draft has no side effects"
    );
    let other_editor = active(7, Role::Editor);
    assert_eq!(
        w.apply_draft(&write_ctx(&other_editor, "d-2"), draft.id.clone())
            .await,
        Err(WriteError::NotFound),
        "only the author applies a draft"
    );
    let applied = w
        .apply_draft(&write_ctx(&editor, "d-3"), draft.id.clone())
        .await
        .unwrap();
    assert_eq!(applied_hash(&applied), Some(hash(b"x: draft\n")));
    assert_eq!(w.nfs_content(&s, &a).await.as_deref(), Some(&b"x: draft\n"[..]));
    let stale_apply = w
        .apply_draft(&write_ctx(&editor, "d-4"), draft.id.clone())
        .await
        .unwrap();
    assert!(
        matches!(stale_apply, WriteOutcome::Conflict { .. }),
        "the file moved on since the draft"
    );
    let unknown = domain::DraftId::parse("draft-unknown").unwrap();
    assert_eq!(
        w.apply_draft(&write_ctx(&editor, "d-5"), unknown).await,
        Err(WriteError::NotFound)
    );
    assert_eq!(
        w.propose_draft(&write_ctx(&viewer, "d-6"), attempt)
            .await
            .unwrap_err(),
        WriteError::Forbidden
    );
}
