//! S10: what the write path logs holds paths, hashes and ids, never file content, a secret, or a value of the request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use bytes::Bytes;
use domain::{
    DeleteRequest, Expected, PreviewChange, PreviewRequest, RestartRequest, RevertRequest, Role, ServiceRef,
    SwimlaneId, UploadRequest,
};
use ports::conformance::sample::nfs;
use ports::{BlobStore, WriteService};
use support::fake_agent::Fault;
use support::log_capture;
use support::rig::{Rig, at, ctx, edit, h};

const MARKER: &str = "content-marker-7f3a91";

#[tokio::test]
async fn write_path_logs_hold_paths_hashes_and_ids_only() {
    let (capture, _guard) = log_capture::install();
    let rig = Rig::new(db!()).await;
    let s: SwimlaneId = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n"), ("b.yml", "k: v\n")]);
    let editor = rig.user(1, Role::Editor).await;
    let careful = rig.approval_user(2, Role::Editor).await;
    let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();

    // An applied write, a conflict, a secret, a parse error, a proposal, a lost reply, a replay: each leaves a log line or none.
    let hub = &rig.hub;
    hub.edit(
        &ctx(&editor, "k-1"),
        edit(&s, "a.yml", at("x: 1\n"), &format!("mark: {MARKER}\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&editor, "k-2"),
        edit(&s, "a.yml", at("x: 1\n"), &format!("mark: {MARKER}-2\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&editor, "k-3"),
        edit(&s, "b.yml", at("k: v\n"), &format!("k: {key}\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&editor, "k-4"),
        edit(&s, "b.yml", at("k: v\n"), &format!("k: [{MARKER}\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&careful, "k-5"),
        edit(&s, "b.yml", at("k: v\n"), &format!("k: {MARKER}-3\n")),
    )
    .await
    .unwrap();
    rig.agent.fault_next_write(Fault::LandThenTimeout);
    let _ = hub
        .edit(
            &ctx(&editor, "k-6"),
            edit(&s, "b.yml", at("k: v\n"), &format!("k: {MARKER}-4\n")),
        )
        .await;
    rig.audit.fail_next();
    let _ = hub
        .edit(
            &ctx(&editor, "k-7"),
            edit(&s, "c.yml", domain::Expected::Absent, &format!("c: {MARKER}-5\n")),
        )
        .await;
    hub.edit(
        &ctx(&editor, "k-1"),
        edit(&s, "a.yml", at("x: 1\n"), &format!("mark: {MARKER}\n")),
    )
    .await
    .unwrap();

    let logged = capture.text();
    assert!(!logged.contains(MARKER), "file content in the log:\n{logged}");
    assert!(!logged.contains(&key), "a secret in the log");
    assert!(!logged.contains("user1@example.com"), "an email in the log");
    assert!(!logged.contains("Bearer"), "credentials in the log");
}

/// A failure inside the write path that is not a database error or a timeout (those log themselves) is not dropped: an operator
/// who sees `Unavailable` finds the stage and the kind of failure in the log, and nothing of the request (S10).
#[tokio::test]
async fn a_failure_inside_the_write_path_is_logged_by_stage_and_kind_only() {
    let (capture, _guard) = log_capture::install();
    let rig = Rig::new(db!()).await;
    let s: SwimlaneId = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    let editor = rig.user(1, Role::Editor).await;
    rig.audit.fail_next();
    let out = rig
        .hub
        .edit(
            &ctx(&editor, "k-1"),
            edit(&s, "a.yml", at("x: 1\n"), &format!("mark: {MARKER}\n")),
        )
        .await;
    assert_eq!(out, Err(ports::WriteError::Unavailable));
    let logged = capture.text();
    assert!(
        logged.contains("write path call failed")
            && logged.contains("audit log")
            && logged.contains("commit"),
        "the stage and the kind of the failure are logged:\n{logged}"
    );
    assert!(!logged.contains(MARKER), "file content in the log:\n{logged}");
}

/// The other actions are held to the same rule: the content of an upload, of a file being deleted, of a version being restored and
/// of a preview, whether the action goes through or is refused, is not in the log.
#[tokio::test]
async fn upload_delete_revert_restart_and_preview_log_no_content() {
    let (capture, _guard) = log_capture::install();
    let rig = Rig::new(db!()).await;
    let s: SwimlaneId = rig.swimlane("sit1", true).await;
    let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
    let secret_file = format!("old: {MARKER}-old\nk: {key}\n");
    rig.seed(
        &s,
        &[
            ("doomed.yml", &secret_file),
            ("a.yml", &format!("x: {MARKER}-a\n")),
        ],
    );
    rig.service(&s, "sit1", "payments", None).await;
    let editor = rig.user(1, Role::Editor).await;
    let operator = rig.user(2, Role::Operator).await;
    let hub = &rig.hub;

    // A preview of an upload with a marker and with a secret; an upload with a good token and one without.
    let upload = |content: String| PreviewRequest {
        swimlane: s.clone(),
        path: nfs("new.yml"),
        change: PreviewChange::Upload {
            expected: Expected::Absent,
            content: Bytes::from(content.into_bytes()),
        },
    };
    let pv = hub
        .preview_effect(&ctx(&editor, "p-1"), upload(format!("n: {MARKER}-up\n")))
        .await
        .unwrap();
    let _ = hub
        .preview_effect(&ctx(&editor, "p-2"), upload(format!("n: {key}\n")))
        .await
        .unwrap();
    hub.upload(
        &ctx(&editor, "u-1"),
        UploadRequest {
            swimlane: s.clone(),
            path: nfs("new.yml"),
            expected: Expected::Absent,
            content: Bytes::from(format!("n: {MARKER}-up\n").into_bytes()),
            preview: Some(pv.preview_id.clone()),
        },
    )
    .await
    .unwrap();
    hub.upload(
        &ctx(&editor, "u-2"),
        UploadRequest {
            swimlane: s.clone(),
            path: nfs("other.yml"),
            expected: Expected::Absent,
            content: Bytes::from(format!("n: {MARKER}-no-token\n").into_bytes()),
            preview: None,
        },
    )
    .await
    .unwrap();

    // A delete of a file that holds a secret, with its token; one with a stale hash.
    let del_pv = hub
        .preview_effect(
            &ctx(&editor, "p-3"),
            PreviewRequest {
                swimlane: s.clone(),
                path: nfs("doomed.yml"),
                change: PreviewChange::Delete {
                    expected: h(&secret_file),
                },
            },
        )
        .await
        .unwrap();
    hub.delete(
        &ctx(&editor, "d-1"),
        DeleteRequest {
            swimlane: s.clone(),
            path: nfs("doomed.yml"),
            expected: h(&secret_file),
            preview: Some(del_pv.preview_id),
        },
    )
    .await
    .unwrap();
    let _ = hub
        .delete(
            &ctx(&editor, "d-2"),
            DeleteRequest {
                swimlane: s.clone(),
                path: nfs("a.yml"),
                expected: h("not what is there\n"),
                preview: None,
            },
        )
        .await;

    // A revert to a version that held a secret (refused) and to one that is clean.
    let leaky = format!("x: {key}\n");
    rig.blobs
        .put(Bytes::from(leaky.clone().into_bytes()))
        .await
        .unwrap();
    rig.blobs
        .put(Bytes::from(format!("x: {MARKER}-clean\n").into_bytes()))
        .await
        .unwrap();
    rig.registry
        .observe(&s, &nfs("a.yml"), Bytes::from(leaky.clone().into_bytes()));
    rig.registry.observe(
        &s,
        &nfs("a.yml"),
        Bytes::from(format!("x: {MARKER}-clean\n").into_bytes()),
    );
    rig.registry.observe(
        &s,
        &nfs("a.yml"),
        Bytes::from(format!("x: {MARKER}-a\n").into_bytes()),
    );
    for (n, to) in [h(&leaky), h(&format!("x: {MARKER}-clean\n"))]
        .into_iter()
        .enumerate()
    {
        hub.revert(
            &ctx(&editor, &format!("r-{n}")),
            RevertRequest {
                swimlane: s.clone(),
                path: nfs("a.yml"),
                expected: at(&format!("x: {MARKER}-a\n")),
                to,
                change_eol: None,
            },
        )
        .await
        .unwrap();
    }
    hub.restart(
        &ctx(&operator, "s-1"),
        RestartRequest {
            swimlane: s.clone(),
            service: ServiceRef::new("sit1", "payments").unwrap(),
        },
    )
    .await
    .unwrap();

    let logged = capture.text();
    assert!(!logged.contains(MARKER), "file content in the log:\n{logged}");
    assert!(!logged.contains(&key), "a secret in the log");
    assert!(
        !logged.contains(pv.preview_id.as_str()),
        "a preview id in the log"
    );
    assert!(!logged.contains("Bearer"), "credentials in the log");
}

/// Approvals, rejections, expiry and drafts are held to the same rule (S10): the content of a proposal, of a draft and of the
/// file it replaces, and the text of a comment or a reason, is not in the log, whatever way the decision goes.
#[tokio::test]
async fn approvals_drafts_and_expiry_log_no_content() {
    use hub_writes::ProposalDesk;

    let (capture, _guard) = log_capture::install();
    let rig = Rig::new(db!()).await;
    let s: SwimlaneId = rig.swimlane("sit1", true).await;
    let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
    rig.seed(
        &s,
        &[
            ("a.yml", &format!("x: {MARKER}-a\nk: {key}\n")),
            ("b.yml", &format!("y: {MARKER}-b\n")),
            ("c.yml", &format!("z: {MARKER}-c\n")),
            ("d.yml", &format!("w: {MARKER}-d\n")),
        ],
    );
    let careful = rig.approval_user(1, Role::Editor).await;
    let admin = rig.user(2, Role::Admin).await;
    let hub = &rig.hub;
    let propose = |n: usize, path: &str, from: &str, to: &str| {
        let r = edit(&s, path, at(from), to);
        let c = ctx(&careful, &format!("p-{n}"));
        async move { hub.edit(&c, r).await.unwrap() }
    };
    let id = |o: domain::WriteOutcome| match o {
        domain::WriteOutcome::ProposalCreated { id } => id,
        other => panic!("{other:?}"),
    };
    let to_apply = id(propose(
        1,
        "a.yml",
        &format!("x: {MARKER}-a\nk: {key}\n"),
        &format!("x: {MARKER}-a2\n"),
    )
    .await);
    let to_reject = id(propose(
        2,
        "b.yml",
        &format!("y: {MARKER}-b\n"),
        &format!("y: {MARKER}-b2\n"),
    )
    .await);
    let to_stale = id(propose(
        3,
        "c.yml",
        &format!("z: {MARKER}-c\n"),
        &format!("z: {MARKER}-c2\n"),
    )
    .await);
    id(propose(
        4,
        "d.yml",
        &format!("w: {MARKER}-d\n"),
        &format!("w: {MARKER}-d2\n"),
    )
    .await);

    hub.approve_proposal(
        &ctx(&admin, "a-1"),
        to_apply,
        Some(domain::ShortText::parse(&format!("comment {MARKER}-comment")).unwrap()),
    )
    .await
    .unwrap();
    hub.reject_proposal(
        &ctx(&admin, "a-2"),
        to_reject,
        domain::ShortText::parse(&format!("reason {MARKER}-reason")).unwrap(),
    )
    .await
    .unwrap();
    rig.agent
        .put(&s, &nfs("c.yml"), format!("z: {MARKER}-other\n").as_bytes());
    hub.approve_proposal(&ctx(&admin, "a-3"), to_stale, None)
        .await
        .unwrap();
    // The queue, with the diffs an approver reads.
    let queue = hub
        .list_proposals(
            &admin,
            hub_writes::ProposalQuery {
                status: None,
                swimlane: None,
                page: domain::Page::new(10, None).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(queue.items.len(), 4);
    rig.advance_secs(73 * 3600);
    assert_eq!(hub.expirer().run_once(&|| true).await.proposals, 1);

    // Drafts: one made over a file that holds a secret, applied; one refused for holding a secret.
    let editor = rig.user(3, Role::Editor).await;
    let _ = hub
        .propose_draft(
            &ctx(&editor, "d-1"),
            edit(
                &s,
                "b.yml",
                at(&format!("y: {MARKER}-b\n")),
                &format!("y: {MARKER}-draft\n"),
            ),
        )
        .await;
    let _ = hub
        .propose_draft(
            &ctx(&editor, "d-2"),
            edit(
                &s,
                "b.yml",
                at(&format!("y: {MARKER}-b\n")),
                &format!("y: {key}\n"),
            ),
        )
        .await;

    let logged = capture.text();
    assert!(
        !logged.contains(MARKER),
        "content or a comment in the log:\n{logged}"
    );
    assert!(!logged.contains(&key), "a secret in the log");
    assert!(!logged.contains("Bearer"), "credentials in the log");
}
