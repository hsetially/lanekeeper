use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    ChangeEol, ChangePreview, DeleteRequest, EditRequest, Expected, NfsPath, NotifyRequest, PrRequest,
    PrTarget, PreviewChange, PreviewId, PreviewRequest, RejectReason, RestartRequest, RevertRequest, Role,
    ServiceRef, ShortText, SwimlaneId, TenantId, UploadRequest, User, UserStatus, WriteCtx, WriteOutcome,
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
    /// Move the service's clock forward by `secs`, so that a preview can run out without the suite sleeping.
    async fn advance_clock(&self, secs: u64);
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

/// A value the secret scan flags in every implementation: an AWS access key id (`AKIA` or `ASIA`, then 16 upper case
/// letters or digits; the example key of the AWS documentation). Built from two parts so that no scanner reads this
/// file as a leak.
fn aws_key() -> String {
    ["AKIA", "IOSFODNN7EXAMPLE"].concat()
}

/// A temporary-credentials key id: the scan's AWS rule flags the `ASIA` prefix as well.
fn session_aws_key() -> String {
    ["ASIA", "IOSFODNN7EXAMPLE"].concat()
}

/// A second key of the same shape.
fn other_aws_key() -> String {
    ["AKIA", "IOSFODNN7EXAMPL2"].concat()
}

/// Everything a preview says, as text: what a leak anywhere in it would show up in.
fn shown(p: &ChangePreview) -> String {
    serde_json::to_string(p).unwrap_or_default()
}

/// What the preview of an upload says.
async fn upload_preview<W: WriteScenario + ?Sized>(
    w: &W,
    ctx: &WriteCtx,
    s: &SwimlaneId,
    path: &str,
    expected: Expected,
    content: &[u8],
) -> ChangePreview {
    let r = PreviewRequest {
        swimlane: s.clone(),
        path: nfs(path),
        change: PreviewChange::Upload {
            expected,
            content: Bytes::copy_from_slice(content),
        },
    };
    w.preview_effect(ctx, r).await.unwrap()
}

/// What the preview of a delete says.
async fn delete_preview<W: WriteScenario + ?Sized>(
    w: &W,
    ctx: &WriteCtx,
    s: &SwimlaneId,
    path: &str,
    expected: domain::ContentHash,
) -> ChangePreview {
    let r = PreviewRequest {
        swimlane: s.clone(),
        path: nfs(path),
        change: PreviewChange::Delete { expected },
    };
    w.preview_effect(ctx, r).await.unwrap()
}

/// The change entry for a setting path.
fn change_of<'a>(p: &'a ChangePreview, path: &str) -> &'a domain::PreviewedChange {
    p.changes
        .iter()
        .find(|c| c.change.path.as_str() == path)
        .unwrap_or_else(|| panic!("the preview lists a change for `{path}`: {p:?}"))
}

fn is_marker(v: Option<&domain::SettingValue>) -> bool {
    v.is_some_and(|v| v.text.starts_with("[REDACTED: ") && v.text.ends_with(']'))
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
/// - A preview is bound to the user, swimlane, path, expected state and content (an upload) or hash (a delete) it was
///   issued for, runs out after [`ChangePreview::TTL_SECS`], and writes nothing.
/// - A preview never shows a value the secret scan flags: the value is masked and the entry says `flagged` (S14b, D79).
/// - A new file takes the line endings of its folder (the content as it is, when the folder has none); a file with
///   mixed endings takes the dominant ones, and a file with none takes LF (the engine's `dominant_eol`); a lone CR is a
///   line ending.
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
            ("bound/one.yml", "k: v\n"),
            ("bound/two.yml", "k: v\n"),
            ("secrets/keys.yml", &format!("aws_key: {}\nplain: 1\n", aws_key())),
            ("secrets/lone.yml", &format!("token: {}\rplain: 1\r", aws_key())),
            ("no-endings-here/flat.yml", "a: 1"),
            ("eol/same.yml", "k: 1\r\nj: 2\r\n"),
            ("eol/mixed-crlf.yml", "a: 1\r\nb: 2\r\nc: 3\n"),
            ("eol/mixed-lf.yml", "a: 1\nb: 2\r\nc: 3\n"),
            ("eol/lone.yml", "a: 1\nb: 2\n"),
            ("eol/single.yml", "a: 1"),
            ("crlf-dir/one.yml", "a: 1\r\nb: 2\r\n"),
            ("crlf-dir/two.yml", "c: 3\r\n"),
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
    let theirs = upload_preview(
        w,
        &write_ctx(&editor2, "pv-a"),
        &s,
        "img/logo.bmp",
        Expected::Absent,
        &logo,
    )
    .await
    .preview_id;
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
    let for_other = upload_preview(
        w,
        &write_ctx(&editor, "pv-b"),
        &s,
        "img/logo.bmp",
        Expected::Absent,
        &other_bytes,
    )
    .await
    .preview_id;
    let swapped = w
        .upload(&write_ctx(&editor, "k-5d"), upload_with(Some(for_other), &logo))
        .await
        .unwrap();
    assert_eq!(
        rejection(&swapped),
        RejectReason::NotAllowed,
        "bound to the content"
    );
    let for_path = upload_preview(
        w,
        &write_ctx(&editor, "pv-c"),
        &s,
        "img/other.bmp",
        Expected::Absent,
        &logo,
    )
    .await
    .preview_id;
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
    let pv = upload_preview(
        w,
        &write_ctx(&editor, "pv-d"),
        &s,
        "img/logo.bmp",
        Expected::Absent,
        &logo,
    )
    .await
    .preview_id;
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
    let stale_preview = delete_preview(w, &write_ctx(&editor, "pv-f"), &s, "b.yml", hash(b"k: v\n"))
        .await
        .preview_id;
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
    let fresh = delete_preview(w, &write_ctx(&editor, "pv-g"), &s, "b.yml", hash(b"k: w\n"))
        .await
        .preview_id;
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

    previews_are_bound_and_expire(w, &s).await;
    previews_write_nothing_and_mask_secrets(w, &s).await;
    line_endings_follow_the_file_the_folder_and_the_majority(w, &s).await;
}

/// A token is good for the user, swimlane, path, expected state and content (a delete: the hash) it was issued for, and
/// only until it runs out. Failed attempts neither write nor use the token up.
async fn previews_are_bound_and_expire<W: WriteScenario + ?Sized>(w: &W, s: &SwimlaneId) {
    let editor = active(1, Role::Editor);
    let other_user = active(8, Role::Editor);
    let s2 = swimlane("sit2");
    w.seed(
        &s2,
        &files(&[("bound/one.yml", "k: v\n"), ("bound/two.yml", "k: v\n")]),
    )
    .await;
    let content = Bytes::from_static(b"n: 1\n");
    let upload =
        |user: &User, key: &str, sw: &SwimlaneId, path: &str, expected: Expected, token: &PreviewId| {
            let ctx = write_ctx(user, key);
            let req = UploadRequest {
                swimlane: sw.clone(),
                path: nfs(path),
                expected,
                content: content.clone(),
                preview: Some(token.clone()),
            };
            async move { w.upload(&ctx, req).await.unwrap() }
        };

    // An upload: the swimlane and the expected state are bound too (the user, path and content are, above).
    let token = upload_preview(
        w,
        &write_ctx(&editor, "pb-1"),
        s,
        "bound/new.yml",
        Expected::Absent,
        &content,
    )
    .await
    .preview_id;
    let elsewhere = upload(&editor, "pb-2", &s2, "bound/new.yml", Expected::Absent, &token).await;
    assert_eq!(
        rejection(&elsewhere),
        RejectReason::NotAllowed,
        "bound to the swimlane"
    );
    let other_state = Expected::Hash { hash: hash(b"old") };
    let stale = upload(&editor, "pb-3", s, "bound/new.yml", other_state, &token).await;
    assert_eq!(
        rejection(&stale),
        RejectReason::NotAllowed,
        "bound to the expected state, so not a conflict either"
    );
    assert_eq!(w.nfs_content(s, &nfs("bound/new.yml")).await, None);
    assert_eq!(w.nfs_content(&s2, &nfs("bound/new.yml")).await, None);
    let done = upload(&editor, "pb-4", s, "bound/new.yml", Expected::Absent, &token).await;
    assert!(
        done.is_success(),
        "the refusals did not use the token up: {done:?}"
    );

    // A delete: bound to the user, swimlane, path and hash.
    let h = hash(b"k: v\n");
    let delete = |user: &User, key: &str, sw: &SwimlaneId, path: &str, expected, token: &PreviewId| {
        let ctx = write_ctx(user, key);
        let req = DeleteRequest {
            swimlane: sw.clone(),
            path: nfs(path),
            expected,
            preview: Some(token.clone()),
        };
        async move { w.delete(&ctx, req).await.unwrap() }
    };
    let token = delete_preview(w, &write_ctx(&editor, "pb-5"), s, "bound/one.yml", h)
        .await
        .preview_id;
    let attempts = [
        (
            delete(&other_user, "pb-6", s, "bound/one.yml", h, &token).await,
            "the user",
        ),
        (
            delete(&editor, "pb-7", &s2, "bound/one.yml", h, &token).await,
            "the swimlane",
        ),
        (
            delete(&editor, "pb-8", s, "bound/two.yml", h, &token).await,
            "the path",
        ),
        (
            delete(&editor, "pb-9", s, "bound/one.yml", hash(b"k: w\n"), &token).await,
            "the hash, so not a conflict either",
        ),
    ];
    for (outcome, bound_to) in attempts {
        assert_eq!(
            rejection(&outcome),
            RejectReason::NotAllowed,
            "a delete preview is bound to {bound_to}"
        );
    }
    for (sw, path) in [
        (s, "bound/one.yml"),
        (s, "bound/two.yml"),
        (&s2, "bound/one.yml"),
        (&s2, "bound/two.yml"),
    ] {
        assert!(
            w.nfs_content(sw, &nfs(path)).await.is_some(),
            "{path}: refused deletes delete nothing"
        );
    }
    assert!(
        delete(&editor, "pb-10", s, "bound/one.yml", h, &token)
            .await
            .is_success(),
        "the refusals did not use the token up"
    );

    // A preview runs out: still good one second before `TTL_SECS`, gone at `TTL_SECS`.
    let ttl = ChangePreview::TTL_SECS;
    let late = Bytes::from_static(b"late: 1\n");
    let upload_late = |key: &str, path: &str, token: &PreviewId| {
        let ctx = write_ctx(&editor, key);
        let req = UploadRequest {
            swimlane: s.clone(),
            path: nfs(path),
            expected: Expected::Absent,
            content: late.clone(),
            preview: Some(token.clone()),
        };
        async move { w.upload(&ctx, req).await.unwrap() }
    };
    let issued = upload_preview(
        w,
        &write_ctx(&editor, "pt-1"),
        s,
        "bound/ttl-1.yml",
        Expected::Absent,
        &late,
    )
    .await;
    w.advance_clock(ttl - 1).await;
    let in_time = upload_late("pt-2", "bound/ttl-1.yml", &issued.preview_id).await;
    assert!(in_time.is_success(), "one second before it runs out: {in_time:?}");
    let issued = upload_preview(
        w,
        &write_ctx(&editor, "pt-3"),
        s,
        "bound/ttl-2.yml",
        Expected::Absent,
        &late,
    )
    .await;
    w.advance_clock(ttl).await;
    let too_late = upload_late("pt-4", "bound/ttl-2.yml", &issued.preview_id).await;
    assert_eq!(rejection(&too_late), RejectReason::NotAllowed, "after TTL_SECS");
    assert_eq!(
        w.nfs_content(s, &nfs("bound/ttl-2.yml")).await,
        None,
        "an expired preview writes nothing"
    );
    let again = upload_preview(
        w,
        &write_ctx(&editor, "pt-5"),
        s,
        "bound/ttl-2.yml",
        Expected::Absent,
        &late,
    )
    .await;
    assert!(
        upload_late("pt-6", "bound/ttl-2.yml", &again.preview_id)
            .await
            .is_success(),
        "a new preview works"
    );
    let gone = Bytes::from_static(b"k: v\n");
    let issued = delete_preview(w, &write_ctx(&editor, "pt-7"), s, "bound/two.yml", hash(&gone))
        .await
        .preview_id;
    w.advance_clock(ttl).await;
    let expired_delete = w
        .delete(
            &write_ctx(&editor, "pt-8"),
            DeleteRequest {
                swimlane: s.clone(),
                path: nfs("bound/two.yml"),
                expected: hash(&gone),
                preview: Some(issued),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rejection(&expired_delete),
        RejectReason::NotAllowed,
        "a delete preview runs out too"
    );
    assert!(w.nfs_content(s, &nfs("bound/two.yml")).await.is_some());
}

/// A preview reads and tells; it does not write, and it does not show a value the secret scan flags.
async fn previews_write_nothing_and_mask_secrets<W: WriteScenario + ?Sized>(w: &W, s: &SwimlaneId) {
    let editor = active(1, Role::Editor);
    let secrets = nfs("secrets/keys.yml");
    let same = nfs("eol/same.yml");
    let key = aws_key();
    let seeded = format!("aws_key: {key}\nplain: 1\n");
    let h_secrets = hash(seeded.as_bytes());
    let before = (
        w.nfs_content(s, &secrets).await,
        w.nfs_content(s, &same).await,
        w.nfs_content(s, &nfs("secrets/fresh.yml")).await,
    );

    // The limit is the upload's: 2 MiB, no more.
    let at_limit = vec![b'a'; 2 * 1024 * 1024];
    let big = PreviewRequest {
        swimlane: s.clone(),
        path: nfs("big/big.yml"),
        change: PreviewChange::Upload {
            expected: Expected::Absent,
            content: Bytes::from(at_limit.clone()),
        },
    };
    assert!(
        w.preview_effect(&write_ctx(&editor, "pm-0"), big).await.is_ok(),
        "2 MiB is allowed"
    );
    let too_big = PreviewRequest {
        swimlane: s.clone(),
        path: nfs("big/big.yml"),
        change: PreviewChange::Upload {
            expected: Expected::Absent,
            content: Bytes::from([at_limit, vec![b'a']].concat()),
        },
    };
    assert_eq!(
        w.preview_effect(&write_ctx(&editor, "pm-0"), too_big)
            .await
            .map(|p| p.preview_id),
        Err(WriteError::BadRequest),
        "one byte over 2 MiB is not"
    );

    // A removed secret: the left side is the marker.
    let removed = upload_preview(
        w,
        &write_ctx(&editor, "pm-1"),
        s,
        "secrets/keys.yml",
        Expected::Hash { hash: h_secrets },
        b"plain: 2\n",
    )
    .await;
    assert!(
        !shown(&removed).contains(&key),
        "a removed secret is not shown: {removed:?}"
    );
    let entry = change_of(&removed, "aws_key");
    assert!(
        entry.flagged && is_marker(entry.change.left.as_ref()),
        "{entry:?}"
    );
    let plain = change_of(&removed, "plain");
    assert!(!plain.flagged, "a value nobody flagged is shown and not flagged");
    assert_eq!(plain.change.left.as_ref().map(|v| v.text.as_str()), Some("1"));
    assert_eq!(plain.change.right.as_ref().map(|v| v.text.as_str()), Some("2"));

    // A changed secret: both sides are markers, and neither key is shown.
    let other = other_aws_key();
    let swapped_text = format!("aws_key: {other}\nplain: 1\n");
    let swapped = upload_preview(
        w,
        &write_ctx(&editor, "pm-2"),
        s,
        "secrets/keys.yml",
        Expected::Hash { hash: h_secrets },
        swapped_text.as_bytes(),
    )
    .await;
    let text = shown(&swapped);
    assert!(
        !text.contains(&key) && !text.contains(&other),
        "a changed secret is not shown: {swapped:?}"
    );
    let entry = change_of(&swapped, "aws_key");
    assert!(
        entry.flagged && is_marker(entry.change.left.as_ref()) && is_marker(entry.change.right.as_ref()),
        "{entry:?}"
    );

    // A new secret, in a new file.
    let added_text = format!("token: {key}\n");
    let added = upload_preview(
        w,
        &write_ctx(&editor, "pm-3"),
        s,
        "secrets/fresh.yml",
        Expected::Absent,
        added_text.as_bytes(),
    )
    .await;
    assert!(
        !shown(&added).contains(&key),
        "an added secret is not shown: {added:?}"
    );
    let entry = change_of(&added, "token");
    assert!(
        entry.flagged && is_marker(entry.change.right.as_ref()),
        "{entry:?}"
    );

    // A secret on a line of a file that uses lone CRs (old Mac style) is masked too: a lone CR ends a line for the
    // scan, so the setting after it is a setting of its own and is not read as part of the key. The file that is there
    // is read as it is on NFS, and so is new content for a folder without line endings.
    let lone_old = format!("token: {key}\rplain: 1\r");
    let lone = upload_preview(
        w,
        &write_ctx(&editor, "pm-3a"),
        s,
        "secrets/lone.yml",
        Expected::Hash {
            hash: hash(lone_old.as_bytes()),
        },
        b"plain: 2\n",
    )
    .await;
    assert!(
        !shown(&lone).contains(&key),
        "a secret before a lone CR is not shown (left side): {lone:?}"
    );
    let entry = change_of(&lone, "token");
    assert!(
        entry.flagged && is_marker(entry.change.left.as_ref()),
        "{entry:?}"
    );
    let entry = change_of(&lone, "plain");
    assert!(
        !entry.flagged
            && entry.change.left.as_ref().map(|v| v.text.as_str()) == Some("1")
            && entry.change.right.as_ref().map(|v| v.text.as_str()) == Some("2"),
        "the setting after the lone CR is a setting of its own: {entry:?}"
    );
    let lone_new = format!("token: {key}\rplain: 1\r");
    let lone = upload_preview(
        w,
        &write_ctx(&editor, "pm-3c"),
        s,
        "no-endings-here/new.yml",
        Expected::Absent,
        lone_new.as_bytes(),
    )
    .await;
    assert!(
        !shown(&lone).contains(&key),
        "a secret before a lone CR is not shown (right side): {lone:?}"
    );
    let entry = change_of(&lone, "token");
    assert!(
        entry.flagged && is_marker(entry.change.right.as_ref()),
        "{entry:?}"
    );

    // The scan's AWS rule knows `ASIA` ids (temporary credentials) as well as `AKIA` ones.
    let session = session_aws_key();
    let session_text = format!("session_key: {session}\n");
    let temporary = upload_preview(
        w,
        &write_ctx(&editor, "pm-3b"),
        s,
        "secrets/session.yml",
        Expected::Absent,
        session_text.as_bytes(),
    )
    .await;
    assert!(
        !shown(&temporary).contains(&session),
        "an ASIA key is not shown: {temporary:?}"
    );
    let entry = change_of(&temporary, "session_key");
    assert!(
        entry.flagged && is_marker(entry.change.right.as_ref()),
        "{entry:?}"
    );

    // A delete lists what goes away, and never a secret.
    let deleted = delete_preview(w, &write_ctx(&editor, "pm-4"), s, "secrets/keys.yml", h_secrets).await;
    assert!(
        !shown(&deleted).contains(&key),
        "a deleted secret is not shown: {deleted:?}"
    );

    // `effective_changed` is about what the services get: the same text with the file's own line endings is no change,
    // another value is one, and deleting a file that is not there changes nothing.
    let unchanged = upload_preview(
        w,
        &write_ctx(&editor, "pm-5"),
        s,
        "eol/same.yml",
        Expected::Hash {
            hash: hash(b"k: 1\r\nj: 2\r\n"),
        },
        b"k: 1\nj: 2\n",
    )
    .await;
    assert!(
        !unchanged.effective_changed,
        "LF text for a CRLF file that is the same text: {unchanged:?}"
    );
    let changed = upload_preview(
        w,
        &write_ctx(&editor, "pm-6"),
        s,
        "eol/same.yml",
        Expected::Hash {
            hash: hash(b"k: 1\r\nj: 2\r\n"),
        },
        b"k: 1\nj: 9\n",
    )
    .await;
    assert!(changed.effective_changed, "{changed:?}");
    let nothing_to_delete = delete_preview(
        w,
        &write_ctx(&editor, "pm-7"),
        s,
        "no-such-file.yml",
        hash(b"k: v\n"),
    )
    .await;
    assert!(
        !nothing_to_delete.effective_changed,
        "an absent file: {nothing_to_delete:?}"
    );

    // After all of that, nothing was written.
    assert_eq!(
        before,
        (
            w.nfs_content(s, &secrets).await,
            w.nfs_content(s, &same).await,
            w.nfs_content(s, &nfs("secrets/fresh.yml")).await,
        ),
        "previews write nothing"
    );
    assert!(w.nfs_content(s, &nfs("big/big.yml")).await.is_none());
    assert!(w.nfs_content(s, &nfs("no-such-file.yml")).await.is_none());
}

/// Rule 11, D17: a file is written back in the style it has. For a file with mixed endings that is the style most of its
/// lines have; for a new file it is the style of the folder; a lone CR is a line ending like any other.
async fn line_endings_follow_the_file_the_folder_and_the_majority<W: WriteScenario + ?Sized>(
    w: &W,
    s: &SwimlaneId,
) {
    let editor = active(1, Role::Editor);
    let edit_to =
        |key: &str, path: &str, expected: Expected, content: &str, change_eol: Option<ChangeEol>| {
            let ctx = write_ctx(&editor, key);
            let req = EditRequest {
                change_eol,
                ..edit(s, path, expected, content)
            };
            async move { applied_hash(&w.edit(&ctx, req).await.unwrap()) }
        };
    let on = |h: &str| Expected::Hash {
        hash: hash(h.as_bytes()),
    };

    // Mixed: two CRLF and one LF is a CRLF file; two LF and one CRLF is an LF file.
    assert_eq!(
        edit_to(
            "le-1",
            "eol/mixed-crlf.yml",
            on("a: 1\r\nb: 2\r\nc: 3\n"),
            "a: 1\nb: 9\nc: 3\n",
            None
        )
        .await,
        Some(hash(b"a: 1\r\nb: 9\r\nc: 3\r\n")),
        "mostly CRLF: the file is written with CRLF"
    );
    assert_eq!(
        edit_to(
            "le-2",
            "eol/mixed-lf.yml",
            on("a: 1\nb: 2\r\nc: 3\n"),
            "a: 1\r\nb: 9\r\nc: 3\r\n",
            None
        )
        .await,
        Some(hash(b"a: 1\nb: 9\nc: 3\n")),
        "mostly LF: the file is written with LF"
    );

    // A lone CR is a line ending in a conversion, so it becomes one.
    assert_eq!(
        edit_to(
            "le-3",
            "eol/lone.yml",
            on("a: 1\nb: 2\n"),
            "a: 1\rb: 2\rc: 3\r",
            Some(ChangeEol::Lf)
        )
        .await,
        Some(hash(b"a: 1\nb: 2\nc: 3\n")),
        "lone CR to LF"
    );
    assert_eq!(
        edit_to(
            "le-4",
            "eol/lone.yml",
            on("a: 1\nb: 2\nc: 3\n"),
            "a: 1\rb: 2\r\nc: 3\n",
            Some(ChangeEol::Crlf)
        )
        .await,
        Some(hash(b"a: 1\r\nb: 2\r\nc: 3\r\n")),
        "lone CR to CRLF, and a CRLF pair is not doubled"
    );

    // A file with no line ending has no style of its own, and the engine's rule for that is LF (`dominant_eol`): the
    // content is written as LF. (A new file in a folder with no line endings is different: its content is kept, below.)
    assert_eq!(
        edit_to("le-3a", "eol/single.yml", on("a: 1"), "a: 1\r\nb: 2\r\n", None).await,
        Some(hash(b"a: 1\nb: 2\n")),
        "no line ending in the file: LF, like the engine's dominant_eol"
    );

    // A new file takes the style of its folder.
    assert_eq!(
        edit_to("le-5", "crlf-dir/new.yml", Expected::Absent, "n: 1\nm: 2\n", None).await,
        Some(hash(b"n: 1\r\nm: 2\r\n")),
        "a folder of CRLF files: a new file is CRLF"
    );
    assert_eq!(
        edit_to(
            "le-6",
            "crlf-dir/new2.yml",
            Expected::Absent,
            "n: 1\nm: 2\n",
            Some(ChangeEol::Lf)
        )
        .await,
        Some(hash(b"n: 1\nm: 2\n")),
        "an explicit style wins over the folder's"
    );
    assert_eq!(
        edit_to(
            "le-7",
            "empty-dir/new.yml",
            Expected::Absent,
            "n: 1\r\nm: 2\r\n",
            None
        )
        .await,
        Some(hash(b"n: 1\r\nm: 2\r\n")),
        "no files in the folder: the content as it is"
    );
}
