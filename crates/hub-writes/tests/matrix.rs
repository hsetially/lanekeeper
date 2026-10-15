//! Authorization is the first step of every action (S4): the role floors, pending and disabled users, a role the directory
//! no longer grants, and the approval user. Nothing is claimed, sent or recorded for a user who may not act.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use bytes::Bytes;
use domain::{
    ContentHash, DeleteRequest, DraftId, Expected, NotifyRequest, PrRequest, PreviewChange, PreviewRequest,
    RestartRequest, RevertRequest, Role, ServiceRef, ShortText, SwimlaneId, UploadRequest, User, UserStatus,
    WriteCtx, WriteOutcome,
};
use ports::conformance::sample::{active, nfs};
use ports::{WriteError, WriteService};
use support::rig::{Rig, at, ctx, edit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Edit,
    Upload,
    Delete,
    Revert,
    Restart,
    RaisePr,
    ProposeDraft,
    ApplyDraft,
    PreviewEffect,
    Notify,
}

const ALL: [Action; 10] = [
    Action::Edit,
    Action::Upload,
    Action::Delete,
    Action::Revert,
    Action::Restart,
    Action::RaisePr,
    Action::ProposeDraft,
    Action::ApplyDraft,
    Action::PreviewEffect,
    Action::Notify,
];

impl Action {
    /// The lowest role that may do it (the doc of `WriteService`).
    fn floor(self) -> Role {
        match self {
            Action::Restart | Action::Notify => Role::Operator,
            _ => Role::Editor,
        }
    }
}

#[allow(clippy::too_many_lines)]
/// What calling the action as `ctx` comes to: an authorization failure or whatever the service does after it.
async fn call(rig: &Rig, action: Action, ctx: &WriteCtx) -> Result<(), WriteError> {
    let s = SwimlaneId::parse("sit1").unwrap();
    let p = nfs("a.yml");
    let hash = ContentHash::from_bytes([7; 32]);
    let hub = &rig.hub;
    match action {
        Action::Edit => hub
            .edit(ctx, edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
            .await
            .map(drop),
        Action::Upload => hub
            .upload(
                ctx,
                UploadRequest {
                    swimlane: s,
                    path: p,
                    expected: Expected::Absent,
                    content: Bytes::from_static(b"x"),
                    preview: None,
                },
            )
            .await
            .map(drop),
        Action::Delete => hub
            .delete(
                ctx,
                DeleteRequest {
                    swimlane: s,
                    path: p,
                    expected: hash,
                    preview: None,
                },
            )
            .await
            .map(drop),
        Action::Revert => hub
            .revert(
                ctx,
                RevertRequest {
                    swimlane: s,
                    path: p,
                    expected: Expected::Hash { hash },
                    to: hash,
                    change_eol: None,
                },
            )
            .await
            .map(drop),
        Action::Restart => hub
            .restart(
                ctx,
                RestartRequest {
                    swimlane: s,
                    service: ServiceRef::new("sit1", "payments").unwrap(),
                },
            )
            .await
            .map(drop),
        Action::RaisePr => hub
            .raise_pr(
                ctx,
                PrRequest {
                    swimlane: s,
                    paths: vec![p],
                    title: ShortText::parse("t").unwrap(),
                    body: None,
                    target: None,
                },
            )
            .await
            .map(drop),
        Action::ProposeDraft => hub
            .propose_draft(ctx, edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
            .await
            .map(drop),
        Action::ApplyDraft => hub
            .apply_draft(ctx, DraftId::parse("d-1").unwrap())
            .await
            .map(drop),
        Action::PreviewEffect => hub
            .preview_effect(
                ctx,
                PreviewRequest {
                    swimlane: s,
                    path: p,
                    change: PreviewChange::Delete { expected: hash },
                },
            )
            .await
            .map(drop),
        Action::Notify => hub
            .notify(
                ctx,
                NotifyRequest {
                    swimlane: s,
                    paths: vec![p],
                },
            )
            .await
            .map(drop),
    }
}

async fn rig() -> Option<Rig> {
    let rig = Rig::new(support::pg::TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    Some(rig)
}

async fn untouched(rig: &Rig) {
    assert_eq!(rig.agent.commands().len(), 0, "nothing is sent to the agent");
    assert_eq!(rig.count("idempotency_keys").await, 0, "nothing is claimed");
    assert_eq!(rig.count("proposals").await, 0, "no proposal");
    assert_eq!(rig.chain().await.len(), 0, "no audit event");
    assert_eq!(rig.bus.published().await.len(), 0, "no event");
}

#[tokio::test]
async fn matrix_every_action_by_role_and_status() {
    let Some(rig) = rig().await else { return };
    let viewer = rig.user(1, Role::Viewer).await;
    let editor = rig.user(2, Role::Editor).await;
    let operator = rig.user(3, Role::Operator).await;
    let pending_admin = rig
        .user_with_status(4, Some(Role::Admin), UserStatus::Pending)
        .await;
    let disabled_admin = rig
        .user_with_status(5, Some(Role::Admin), UserStatus::Disabled)
        .await;
    let roleless = rig.user_with_status(6, None, UserStatus::Active).await;

    for action in ALL {
        for (who, name) in [
            (&viewer, "viewer"),
            (&pending_admin, "pending"),
            (&disabled_admin, "disabled"),
            (&roleless, "no role"),
        ] {
            assert_eq!(
                call(&rig, action, &ctx(who, "k")).await,
                Err(WriteError::Forbidden),
                "{action:?} as {name}"
            );
        }
        // Below the floor is forbidden; at or above it the request gets past authorization.
        for (who, role) in [(&editor, Role::Editor), (&operator, Role::Operator)] {
            let out = call(&rig, action, &ctx(who, "k")).await;
            if role.at_least(action.floor()) {
                assert_ne!(out, Err(WriteError::Forbidden), "{action:?} as {role:?}");
            } else {
                assert_eq!(out, Err(WriteError::Forbidden), "{action:?} as {role:?}");
            }
        }
    }
}

#[tokio::test]
async fn pending_and_disabled_users_fail_before_any_side_effect() {
    let Some(rig) = rig().await else { return };
    let pending = rig
        .user_with_status(4, Some(Role::Admin), UserStatus::Pending)
        .await;
    let disabled = rig
        .user_with_status(5, Some(Role::Admin), UserStatus::Disabled)
        .await;
    let viewer = rig.user(1, Role::Viewer).await;
    for who in [&pending, &disabled, &viewer] {
        assert_eq!(
            call(&rig, Action::Edit, &ctx(who, "k-1")).await,
            Err(WriteError::Forbidden)
        );
    }
    untouched(&rig).await;
}

#[tokio::test]
async fn the_role_is_the_directorys_and_not_the_one_in_the_request() {
    let Some(rig) = rig().await else { return };
    // The directory knows a viewer; the request carries a snapshot that says Admin (a stale session, or a forged context).
    let real = rig.user(1, Role::Viewer).await;
    let mut claimed: User = real.clone();
    claimed.role = Some(Role::Admin);
    claimed.requires_approval = false;
    assert_eq!(
        call(&rig, Action::Edit, &ctx(&claimed, "k-1")).await,
        Err(WriteError::Forbidden)
    );
    // And the other way: the user was disabled after the session was made.
    let disabled = rig
        .user_with_status(2, Some(Role::Editor), UserStatus::Disabled)
        .await;
    let mut snapshot = disabled.clone();
    snapshot.status = UserStatus::Active;
    assert_eq!(
        call(&rig, Action::Edit, &ctx(&snapshot, "k-2")).await,
        Err(WriteError::Forbidden)
    );
    // A user the directory has never seen is no one.
    let stranger = active(77, Role::Admin);
    assert_eq!(
        call(&rig, Action::Edit, &ctx(&stranger, "k-3")).await,
        Err(WriteError::Forbidden)
    );
    untouched(&rig).await;
}

#[tokio::test]
async fn approval_flag_comes_from_the_directory_too() {
    let Some(rig) = rig().await else { return };
    // The snapshot in the request says "no approval needed"; the directory says the user must be approved.
    let careful = rig.approval_user(1, Role::Editor).await;
    let mut snapshot = careful.clone();
    snapshot.requires_approval = false;
    let s = SwimlaneId::parse("sit1").unwrap();
    let out = rig
        .hub
        .edit(&ctx(&snapshot, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::ProposalCreated { .. }), "{out:?}");
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));
}

#[tokio::test]
async fn edit_as_editor_and_approval_user_are_the_two_outcomes_of_a_normal_edit() {
    let Some(rig) = rig().await else { return };
    let editor = rig.user(1, Role::Editor).await;
    let careful = rig.approval_user(2, Role::Editor).await;
    let s = SwimlaneId::parse("sit1").unwrap();
    let applied = rig
        .hub
        .edit(&ctx(&editor, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert!(matches!(applied, WriteOutcome::Applied { .. }));
    let proposed = rig
        .hub
        .edit(&ctx(&careful, "k-2"), edit(&s, "a.yml", at("x: 2\n"), "x: 3\n"))
        .await
        .unwrap();
    assert!(matches!(proposed, WriteOutcome::ProposalCreated { .. }));
}
