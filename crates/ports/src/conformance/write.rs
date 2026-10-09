use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    DeleteRequest, EditRequest, Expected, NfsPath, PrRequest, RejectReason, RestartRequest, RevertRequest,
    Role, ServiceRef, ShortText, SwimlaneId, UploadRequest, UserStatus, WriteOutcome,
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
    }
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
/// - A draft has no side effects until applied, only its author can apply it, and applying re-checks the hash.
/// - Revert restores a known earlier version only; a PR names 1-100 paths.
pub async fn write_service<W: WriteScenario + ?Sized>(w: &W) {
    let s = swimlane("sit1");
    let editor = active(1, Role::Editor);
    let h0 = hash(b"x: 1\n");
    w.seed(&s, &files(&[("a.yml", "x: 1\n"), ("b.yml", "k: v\n")]))
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

    // Upload shares the rules.
    let upload = UploadRequest {
        swimlane: s.clone(),
        path: nfs("img/logo.bmp"),
        expected: Expected::Absent,
        content: Bytes::from_static(&[0, 1, 2, 0xff]),
    };
    assert!(
        w.upload(&write_ctx(&editor, "k-5"), upload)
            .await
            .unwrap()
            .is_success()
    );
    assert_eq!(
        w.nfs_content(&s, &nfs("img/logo.bmp")).await.as_deref(),
        Some(&[0u8, 1, 2, 0xff][..])
    );

    // Delete needs the current hash.
    let wrong = w
        .delete(
            &write_ctx(&editor, "k-6"),
            DeleteRequest {
                swimlane: s.clone(),
                path: nfs("b.yml"),
                expected: h0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        wrong,
        WriteOutcome::Conflict {
            current: Some(hash(b"k: v\n"))
        }
    );
    assert!(w.nfs_content(&s, &nfs("b.yml")).await.is_some());
    let del = w
        .delete(
            &write_ctx(&editor, "k-7"),
            DeleteRequest {
                swimlane: s.clone(),
                path: nfs("b.yml"),
                expected: hash(b"k: v\n"),
            },
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
        };
        async move { w.revert(&ctx, req).await }
    };
    assert_eq!(
        revert(hash(b"never existed"), "k-8").await.unwrap(),
        WriteOutcome::Rejected {
            reason: RejectReason::InvalidContent
        }
    );
    assert_eq!(applied_hash(&revert(h0, "k-9").await.unwrap()), Some(h0));
    assert_eq!(w.nfs_content(&s, &a).await.as_deref(), Some(&b"x: 1\n"[..]));

    // Rejections that write nothing.
    let nowhere = swimlane("nowhere");
    assert_eq!(
        w.edit(
            &write_ctx(&editor, "k-10"),
            edit(&nowhere, "a.yml", Expected::Absent, "x")
        )
        .await
        .unwrap(),
        WriteOutcome::Rejected {
            reason: RejectReason::UnknownSwimlane
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
            reason: RejectReason::TooLarge
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
    };
    assert!(
        w.raise_pr(&write_ctx(&editor, "pr-1"), pr(vec![a.clone()]))
            .await
            .unwrap()
            .is_success()
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
