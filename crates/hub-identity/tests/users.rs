//! T2 (S1, S4, S9, S10, P12): users, roles and access requests, against a real Postgres as `lanekeeper_app`.
//!
//! The audit chain here is the real `PgAuditLog` (T5).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;

use domain::{DomainEvent, Role, ShortText, Timestamp, UserStatus};
use hub_identity::users::rules::{Change, RuleError};
use hub_identity::users::{AccessError, AccessRequestStatus, RequestQuery, UserQuery};
use ports::conformance::UsersScenario;
use ports::{NotificationKind, Users};
use support::users_rig::{FailingAudit, FailingNotifier, START_MS, UsersRig, actor, oid, sign_in, text, uid};

// ---------------------------------------------------------------------------------------------------------------
// Identity: tid plus oid, never the email (S1)

#[tokio::test]
async fn user_is_keyed_by_tid_and_oid_not_email() {
    let rig = UsersRig::new(db!()).await;
    // Same email, different object ids: two users.
    let mut a = sign_in(1);
    let mut b = sign_in(2);
    a.email = Some(text("shared@example.com"));
    b.email = Some(text("shared@example.com"));
    rig.users.first_sign_in(&a, None).await.unwrap();
    rig.users.first_sign_in(&b, None).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    // Same object id in another tenant is another user, even with the same email.
    let other_tid = domain::UserId::new(
        domain::Guid::parse("99999999-9999-4999-8999-999999999999").unwrap(),
        oid(1),
    );
    let mut c = sign_in(1);
    c.user = other_tid.clone();
    let signed = rig.users.first_sign_in(&c, None).await.unwrap();
    assert!(signed.created);
    assert_ne!(signed.user.id, uid(1));
}

#[tokio::test]
async fn changed_email_does_not_change_identity() {
    let rig = UsersRig::new(db!()).await;
    let first = rig.users.first_sign_in(&sign_in(1), None).await.unwrap();
    assert!(first.created);
    let mut again = sign_in(1);
    again.email = Some(text("renamed@example.com"));
    again.display_name = Some(text("Renamed"));
    let second = rig.users.first_sign_in(&again, None).await.unwrap();
    assert!(!second.created);
    assert_eq!(second.user.id, first.user.id);
    assert_eq!(second.user.email.as_str(), "renamed@example.com");
    assert_eq!(second.user.display_name.as_str(), "Renamed");
    assert_eq!(
        rig.request_rows().await.len(),
        1,
        "no second request for a renamed user"
    );

    // A token without an email claim keeps the stored one.
    let mut no_email = sign_in(1);
    no_email.email = None;
    let third = rig.users.first_sign_in(&no_email, None).await.unwrap();
    assert_eq!(third.user.email.as_str(), "renamed@example.com");
}

#[tokio::test]
async fn a_user_without_an_email_claim_still_gets_a_row() {
    let rig = UsersRig::new(db!()).await;
    let mut s = sign_in(7);
    s.email = None;
    s.display_name = None;
    let signed = rig.users.first_sign_in(&s, None).await.unwrap();
    assert!(!signed.user.email.as_str().is_empty());
    assert!(!signed.user.display_name.as_str().is_empty());
}

// ---------------------------------------------------------------------------------------------------------------
// First sign-in (S1)

#[tokio::test]
async fn first_sign_in_creates_pending_user_request_event_and_notification() {
    let rig = UsersRig::new(db!()).await;
    let signed = rig
        .users
        .first_sign_in(&sign_in(1), Some(&domain::RequestId::parse("req-first").unwrap()))
        .await
        .unwrap();
    assert!(signed.created);
    let u = &signed.user;
    assert_eq!(u.status, UserStatus::Pending);
    assert_eq!(u.role, None);
    assert!(!u.requires_approval);
    assert_eq!(u.last_seen_at, Some(Timestamp::from_unix_millis(START_MS)));

    // The access request.
    let rows = rig.request_rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2, "pending");

    // The event, through the outbox, in the same transaction.
    assert_eq!(
        rig.bus.published().await,
        vec![DomainEvent::AccessRequestCreated { user: uid(1) }]
    );

    // One audit event.
    assert_eq!(rig.audit_actions().await, vec!["access_requested"]);

    // One notification, after the commit.
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, NotificationKind::AccessRequested);
    assert_eq!(
        sent[0].link.as_ref().map(ShortText::as_str),
        Some("/admin/access-requests")
    );
}

#[tokio::test]
async fn second_sign_in_creates_no_second_request() {
    let rig = UsersRig::new(db!()).await;
    rig.users.first_sign_in(&sign_in(1), None).await.unwrap();
    rig.clock.advance_secs(3600);
    let again = rig.users.first_sign_in(&sign_in(1), None).await.unwrap();
    assert!(!again.created);
    assert_eq!(rig.request_rows().await.len(), 1);
    assert_eq!(rig.bus.published().await.len(), 1);
    assert_eq!(rig.audit_count().await, 1);
    assert_eq!(rig.notifier.sent().len(), 1);
    // But the sign-in is remembered.
    assert_eq!(
        again.user.last_seen_at,
        Some(Timestamp::from_unix_millis(START_MS + 3_600_000))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_sign_ins_create_one_user_one_request_one_audit_event() {
    let rig = UsersRig::new(db!()).await;
    let users = Arc::new(rig.users.clone());
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let users = Arc::clone(&users);
        tasks.push(tokio::spawn(async move {
            users.first_sign_in(&sign_in(1), None).await.unwrap()
        }));
    }
    let mut created = 0;
    for t in tasks {
        if t.await.unwrap().created {
            created += 1;
        }
    }
    assert_eq!(created, 1);
    assert_eq!(rig.request_rows().await.len(), 1);
    assert_eq!(rig.audit_count().await, 1);
    assert_eq!(rig.outbox_count().await, 1);
    assert_eq!(rig.notifier.sent().len(), 1);
}

#[tokio::test]
async fn bootstrap_admin_is_active_admin_at_first_sign_in_only() {
    let rig = UsersRig::with_bootstrap(db!(), &[oid(1)]).await;
    let signed = rig.users.first_sign_in(&sign_in(1), None).await.unwrap();
    assert!(signed.created);
    assert_eq!(signed.user.role, Some(Role::Admin));
    assert_eq!(signed.user.status, UserStatus::Active);
    // No access request, no event, no notification: nobody needs to approve this user.
    assert!(rig.request_rows().await.is_empty());
    assert!(rig.bus.published().await.is_empty());
    assert!(rig.notifier.sent().is_empty());
    // The creation is audited, once, by the system.
    assert_eq!(rig.audit_actions().await, vec!["role_changed"]);
    assert!(rig.last_audit_diff().await.contains("bootstrap"));
    // Another user is not promoted by the list.
    let other = rig.users.first_sign_in(&sign_in(2), None).await.unwrap();
    assert_eq!(other.user.status, UserStatus::Pending);
}

#[tokio::test]
async fn bootstrap_list_does_not_promote_existing_users() {
    let db = db!();
    let rig = UsersRig::new(db).await;
    let signed = rig.users.first_sign_in(&sign_in(1), None).await.unwrap();
    assert_eq!(signed.user.status, UserStatus::Pending);
    // The operator adds the object id to the configuration later and the hub restarts.
    let configured = hub_identity::users::PgUsers::new(rig.svc.clone(), BTreeSet::from([oid(1)]));
    let again = configured.first_sign_in(&sign_in(1), None).await.unwrap();
    assert!(!again.created);
    assert_eq!(again.user.status, UserStatus::Pending);
    assert_eq!(again.user.role, None);
}

#[tokio::test]
async fn bootstrap_admin_who_is_disabled_is_not_reactivated_by_the_list() {
    let rig = UsersRig::with_bootstrap(db!(), &[oid(1)]).await;
    let other_admin = rig.seed_admin(2).await;
    rig.users.first_sign_in(&sign_in(1), None).await.unwrap();
    rig.admin
        .update_user(
            &actor(2),
            &uid(1),
            Change {
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    let again = rig.users.first_sign_in(&sign_in(1), None).await.unwrap();
    assert_eq!(again.user.status, UserStatus::Disabled);
    assert_eq!(rig.stored(&other_admin).await.status, UserStatus::Active);
}

#[tokio::test]
async fn notifier_failure_does_not_fail_sign_in() {
    let rig = UsersRig::new(db!()).await;
    let (users, _admin) = rig.variant(
        Arc::new(hub_identity::audit::PgAuditLog::new()),
        Arc::new(FailingNotifier),
    );
    let signed = users.first_sign_in(&sign_in(1), None).await.unwrap();
    assert!(signed.created);
    // Everything else committed.
    assert_eq!(rig.request_rows().await.len(), 1);
    assert_eq!(rig.audit_count().await, 1);
    assert_eq!(rig.outbox_count().await, 1);
}

#[tokio::test]
async fn a_failed_audit_write_rolls_back_the_whole_first_sign_in() {
    let rig = UsersRig::new(db!()).await;
    let (users, _admin) = rig.variant(Arc::new(FailingAudit), Arc::new(FailingNotifier));
    let err = users.first_sign_in(&sign_in(1), None).await.unwrap_err();
    assert!(matches!(err, AccessError::Audit(_)), "{err:?}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "the user row is part of the transaction");
    assert!(rig.request_rows().await.is_empty());
    assert_eq!(
        rig.outbox_count().await,
        0,
        "nothing is fanned out for a rolled-back change"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The Users port (S4)

/// The `Users` port is `'static`, so the scenario owns clones of what it needs.
struct Scenario {
    users: hub_identity::users::PgUsers,
    pool: sqlx::PgPool,
}

impl Scenario {
    fn of(rig: &UsersRig) -> Self {
        Self {
            users: rig.users.clone(),
            pool: rig.pool.clone(),
        }
    }
}

#[async_trait::async_trait]
impl Users for Scenario {
    async fn get(&self, id: &domain::UserId) -> Result<Option<domain::User>, ports::UserError> {
        self.users.get(id).await
    }
    async fn require_active(&self, id: &domain::UserId, min: Role) -> Result<domain::User, ports::AuthError> {
        self.users.require_active(id, min).await
    }
}

#[async_trait::async_trait]
impl UsersScenario for Scenario {
    async fn seed(&self, user: domain::User) {
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status, requires_approval) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (tid, oid) DO UPDATE SET role = EXCLUDED.role, status = EXCLUDED.status",
        )
        .bind(user.id.tid().as_str())
        .bind(user.id.oid().as_str())
        .bind(user.email.as_str())
        .bind(user.display_name.as_str())
        .bind(user.role.map(Role::as_str))
        .bind(user.status.as_str())
        .bind(user.requires_approval)
        .execute(&self.pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn pending_and_disabled_users_fail_require_active_for_every_role() {
    let rig = UsersRig::new(db!()).await;
    ports::conformance::users(&Scenario::of(&rig)).await;
    // And with the real first sign-in path: a pending user fails for every role.
    rig.users.first_sign_in(&sign_in(40), None).await.unwrap();
    for role in Role::ALL {
        assert_eq!(
            rig.users.require_active(&uid(40), role).await.unwrap_err(),
            ports::AuthError::Pending
        );
    }
}

#[tokio::test]
async fn role_none_passes_no_minimum() {
    let rig = UsersRig::new(db!()).await;
    rig.seed(1, None, UserStatus::Active).await;
    for role in Role::ALL {
        assert_eq!(
            rig.users.require_active(&uid(1), role).await.unwrap_err(),
            ports::AuthError::InsufficientRole,
            "{role:?}"
        );
    }
    assert_eq!(
        rig.users
            .require_active(&uid(404), Role::Viewer)
            .await
            .unwrap_err(),
        ports::AuthError::UnknownUser
    );
    assert_eq!(rig.users.get(&uid(404)).await.unwrap(), None);
}

#[tokio::test]
async fn touch_last_seen_writes_the_clock_time() {
    let rig = UsersRig::new(db!()).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    assert_eq!(rig.stored(&uid(1)).await.last_seen_at, None);
    rig.clock.advance_secs(90);
    rig.users.touch_last_seen(&uid(1)).await.unwrap();
    assert_eq!(
        rig.stored(&uid(1)).await.last_seen_at,
        Some(Timestamp::from_unix_millis(START_MS + 90_000))
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Admin operations (S1, S4, S9)

async fn pending_user_with_request(rig: &UsersRig, n: u32) -> i64 {
    rig.users.first_sign_in(&sign_in(n), None).await.unwrap();
    rig.request_rows()
        .await
        .into_iter()
        .find(|r| r.1 == oid(n).as_str())
        .unwrap()
        .0
}

#[tokio::test]
async fn approve_sets_role_status_and_closes_request() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    let request = pending_user_with_request(&rig, 1).await;
    rig.seed_session(&uid(1), 1).await;
    let before = rig.audit_count().await;

    let user = rig.admin.approve(&actor(9), request, Role::Editor).await.unwrap();
    assert_eq!(user.id, uid(1));
    assert_eq!(user.role, Some(Role::Editor));
    assert_eq!(user.status, UserStatus::Active);
    assert_eq!(rig.stored(&uid(1)).await, user);

    let rows = rig.request_rows().await;
    assert_eq!(rows[0].2, "approved");
    let (decided_by, decided_at): (Option<String>, bool) =
        sqlx::query_as("SELECT decided_by_oid, decided_at IS NOT NULL FROM access_requests WHERE id = $1")
            .bind(request)
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(decided_by.as_deref(), Some(oid(9).as_str()));
    assert!(decided_at);

    assert_eq!(rig.audit_count().await, before + 1);
    assert_eq!(
        rig.audit_actions().await.last().map(String::as_str),
        Some("access_approved")
    );
    assert_eq!(
        rig.session_count(&uid(1)).await,
        0,
        "approval ends the sessions the pending user held"
    );
}

#[tokio::test]
async fn reject_keeps_user_without_role() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    let request = pending_user_with_request(&rig, 1).await;
    let before = rig.audit_count().await;

    let rejected = rig
        .admin
        .reject(&actor(9), request, &text("not on the team"))
        .await
        .unwrap();
    assert_eq!(rejected.status, AccessRequestStatus::Rejected);
    assert_eq!(rejected.user, uid(1));
    assert_eq!(rejected.decided_by, Some(uid(9)));
    assert!(rejected.decided_at.is_some());

    let u = rig.stored(&uid(1)).await;
    assert_eq!(u.status, UserStatus::Pending);
    assert_eq!(u.role, None);
    assert_eq!(rig.audit_count().await, before + 1);
    assert_eq!(
        rig.audit_actions().await.last().map(String::as_str),
        Some("access_denied")
    );
    assert!(
        rig.last_audit_diff().await.contains("not on the team"),
        "the reason is in the audit event"
    );
}

#[tokio::test]
async fn approve_twice_is_409() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    let request = pending_user_with_request(&rig, 1).await;
    rig.admin.approve(&actor(9), request, Role::Viewer).await.unwrap();
    let before = rig.audit_count().await;
    assert_eq!(
        rig.admin
            .approve(&actor(9), request, Role::Admin)
            .await
            .unwrap_err(),
        AccessError::AlreadyDecided
    );
    assert_eq!(
        rig.admin
            .reject(&actor(9), request, &text("late"))
            .await
            .unwrap_err(),
        AccessError::AlreadyDecided
    );
    assert_eq!(
        rig.stored(&uid(1)).await.role,
        Some(Role::Viewer),
        "the second approval changed nothing"
    );
    assert_eq!(rig.audit_count().await, before, "and wrote nothing");
}

#[tokio::test]
async fn approve_unknown_request_is_404() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    assert_eq!(
        rig.admin
            .approve(&actor(9), 424_242, Role::Viewer)
            .await
            .unwrap_err(),
        AccessError::NotFound
    );
    assert_eq!(
        rig.admin
            .reject(&actor(9), 424_242, &text("x"))
            .await
            .unwrap_err(),
        AccessError::NotFound
    );
    assert_eq!(
        rig.admin
            .update_user(
                &actor(9),
                &uid(404),
                Change {
                    requires_approval: Some(true),
                    ..Change::default()
                }
            )
            .await
            .unwrap_err(),
        AccessError::NotFound
    );
}

#[tokio::test]
async fn nobody_approves_their_own_request_and_a_disabled_user_is_not_approved() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    // An admin who asks for another role cannot approve it themselves.
    let own = rig
        .admin
        .create_request(&actor(9), Role::Operator, &text("testing"))
        .await
        .unwrap();
    assert_eq!(
        rig.admin
            .approve(&actor(9), own.id, Role::Operator)
            .await
            .unwrap_err(),
        AccessError::Rule(RuleError::SelfRoleChange)
    );
    // A user disabled while a request was open stays disabled.
    let request = pending_user_with_request(&rig, 1).await;
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rig.admin
            .approve(&actor(9), request, Role::Viewer)
            .await
            .unwrap_err(),
        AccessError::Rule(RuleError::TargetDisabled)
    );
    assert_eq!(rig.stored(&uid(1)).await.status, UserStatus::Disabled);
}

#[tokio::test]
async fn requires_approval_flag_toggle_is_audited() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Editor), UserStatus::Active).await;
    rig.seed_session(&uid(1), 1).await;
    let before = rig.audit_count().await;
    let user = rig
        .admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                requires_approval: Some(true),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert!(user.requires_approval);
    assert_eq!(rig.audit_count().await, before + 1);
    assert_eq!(
        rig.audit_actions().await.last().map(String::as_str),
        Some("role_changed")
    );
    assert!(
        rig.last_audit_diff()
            .await
            .contains("requires_approval: false -> true")
    );
    assert_eq!(
        rig.session_count(&uid(1)).await,
        1,
        "a flag change does not end sessions"
    );
}

#[tokio::test]
async fn a_change_that_changes_nothing_writes_nothing() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Editor), UserStatus::Active).await;
    rig.seed_session(&uid(1), 1).await;
    let user = rig
        .admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Editor)),
                status: Some(UserStatus::Active),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(user.role, Some(Role::Editor));
    assert_eq!(rig.audit_count().await, 0);
    assert_eq!(rig.session_count(&uid(1)).await, 1);
}

#[tokio::test]
async fn disable_then_reenable_keeps_role() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Operator), UserStatus::Active).await;
    let off = rig
        .admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (off.status, off.role),
        (UserStatus::Disabled, Some(Role::Operator))
    );
    assert_eq!(
        rig.users.require_active(&uid(1), Role::Viewer).await.unwrap_err(),
        ports::AuthError::Disabled
    );
    let on = rig
        .admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                status: Some(UserStatus::Active),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!((on.status, on.role), (UserStatus::Active, Some(Role::Operator)));
    assert_eq!(
        rig.audit_actions().await,
        vec!["user_status_changed", "user_status_changed"]
    );
}

#[tokio::test]
async fn last_active_admin_cannot_be_demoted() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(1).await;
    rig.seed_admin(9).await;
    // Admin 9 demotes admin 1: fine, there is another one.
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Viewer)),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    // Now 9 is the last. A third party cannot demote them.
    rig.seed(5, Some(Role::Admin), UserStatus::Pending).await; // pending admins do not count
    let before = rig.audit_count().await;
    assert_eq!(
        rig.admin
            .update_user(
                &actor(5),
                &uid(9),
                Change {
                    role: Some(Some(Role::Editor)),
                    ..Change::default()
                }
            )
            .await
            .unwrap_err(),
        AccessError::Rule(RuleError::LastActiveAdmin)
    );
    assert_eq!(rig.stored(&uid(9)).await.role, Some(Role::Admin));
    assert_eq!(rig.audit_count().await, before, "a refused change writes nothing");
}

#[tokio::test]
async fn last_active_admin_cannot_be_disabled() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(8, Some(Role::Admin), UserStatus::Disabled).await; // a disabled admin does not count
    assert_eq!(
        rig.admin
            .update_user(
                &actor(8),
                &uid(9),
                Change {
                    status: Some(UserStatus::Disabled),
                    ..Change::default()
                }
            )
            .await
            .unwrap_err(),
        AccessError::Rule(RuleError::LastActiveAdmin)
    );
    assert_eq!(rig.stored(&uid(9)).await.status, UserStatus::Active);
}

#[tokio::test]
async fn admin_cannot_change_own_role() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(1).await;
    rig.seed_admin(2).await;
    let err = rig
        .admin
        .update_user(
            &actor(1),
            &uid(1),
            Change {
                role: Some(Some(Role::Viewer)),
                ..Change::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, AccessError::Rule(RuleError::SelfRoleChange));
    assert_eq!(rig.stored(&uid(1)).await.role, Some(Role::Admin));
}

#[tokio::test]
async fn admin_cannot_change_own_status() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(1).await;
    rig.seed_admin(2).await;
    let err = rig
        .admin
        .update_user(
            &actor(1),
            &uid(1),
            Change {
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, AccessError::Rule(RuleError::SelfStatusChange));
    let err = rig
        .admin
        .update_user(
            &actor(1),
            &uid(1),
            Change {
                requires_approval: Some(true),
                ..Change::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, AccessError::Rule(RuleError::SelfFlagChange));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_demotion_of_two_admins_leaves_one() {
    let rig = UsersRig::new(db!()).await;
    let admin = Arc::new(rig.admin.clone());
    for round in 0..12_u32 {
        let (a, b) = (100 + round * 2, 101 + round * 2);
        rig.seed_admin(a).await;
        rig.seed_admin(b).await;
        // The other admins of earlier rounds would hide the race: make these two the only active admins.
        sqlx::query("UPDATE users SET status = 'disabled' WHERE role = 'admin' AND oid NOT IN ($1, $2)")
            .bind(oid(a).as_str())
            .bind(oid(b).as_str())
            .execute(&rig.pool)
            .await
            .unwrap();
        let demote = |by: u32, target: u32| {
            let admin = Arc::clone(&admin);
            tokio::spawn(async move {
                admin
                    .update_user(
                        &actor(by),
                        &uid(target),
                        Change {
                            role: Some(Some(Role::Viewer)),
                            ..Change::default()
                        },
                    )
                    .await
            })
        };
        let (x, y) = (demote(a, b), demote(b, a));
        let (x, y) = (x.await.unwrap(), y.await.unwrap());
        let ok = [&x, &y].iter().filter(|r| r.is_ok()).count();
        assert_eq!(ok, 1, "round {round}: exactly one demotion wins: {x:?} {y:?}");
        let active_admins: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users WHERE role = 'admin' AND status = 'active' AND oid IN ($1, $2)",
        )
        .bind(oid(a).as_str())
        .bind(oid(b).as_str())
        .fetch_one(&rig.pool)
        .await
        .unwrap();
        assert_eq!(active_admins, 1, "round {round}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Sessions end with a role or status change (S2), in the same transaction

#[tokio::test]
async fn role_change_deletes_all_sessions_in_same_transaction() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    rig.seed(2, Some(Role::Viewer), UserStatus::Active).await;
    for tag in 0..3 {
        rig.seed_session(&uid(1), tag).await;
    }
    rig.seed_session(&uid(2), 50).await;
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Editor)),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(rig.session_count(&uid(1)).await, 0);
    assert_eq!(
        rig.session_count(&uid(2)).await,
        1,
        "another user's sessions stay"
    );
    assert_eq!(
        rig.bus.published().await,
        vec![DomainEvent::UserSessionsRevoked { user: uid(1) }],
        "the revocation is published in the same transaction, once, for that user only"
    );
}

#[tokio::test]
async fn status_change_deletes_all_sessions() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    rig.seed_session(&uid(1), 1).await;
    rig.seed_session(&uid(1), 2).await;
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(rig.session_count(&uid(1)).await, 0);
}

#[tokio::test]
async fn rolled_back_change_keeps_sessions() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    rig.seed_session(&uid(1), 1).await;
    let (_users, admin) = rig.variant(Arc::new(FailingAudit), Arc::new(FailingNotifier));
    let err = admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Editor)),
                ..Change::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AccessError::Audit(_)));
    assert_eq!(rig.session_count(&uid(1)).await, 1);
    assert_eq!(rig.stored(&uid(1)).await.role, Some(Role::Viewer));
    assert_eq!(
        rig.outbox_count().await,
        0,
        "the revocation event rolled back with the change: nothing is fanned out for it"
    );
}

#[tokio::test]
async fn every_admin_operation_writes_exactly_one_audit_event_in_its_transaction() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed_admin(8).await;
    let r1 = pending_user_with_request(&rig, 1).await;
    let r2 = pending_user_with_request(&rig, 2).await;
    rig.seed(3, Some(Role::Viewer), UserStatus::Active).await;
    let mut expected = rig.audit_count().await;

    // Each success adds exactly one event.
    rig.admin.approve(&actor(9), r1, Role::Editor).await.unwrap();
    expected += 1;
    assert_eq!(rig.audit_count().await, expected);
    rig.admin.reject(&actor(9), r2, &text("no")).await.unwrap();
    expected += 1;
    assert_eq!(rig.audit_count().await, expected);
    rig.admin
        .update_user(
            &actor(9),
            &uid(3),
            Change {
                role: Some(Some(Role::Operator)),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    expected += 1;
    assert_eq!(rig.audit_count().await, expected);
    // A role and a status change together are still one operation and one event; the status wins the action name.
    rig.admin
        .update_user(
            &actor(9),
            &uid(3),
            Change {
                role: Some(Some(Role::Viewer)),
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    expected += 1;
    assert_eq!(rig.audit_count().await, expected);
    assert_eq!(
        rig.audit_actions().await.last().map(String::as_str),
        Some("user_status_changed")
    );
    let diff = rig.last_audit_diff().await;
    assert!(
        diff.contains("role: operator -> viewer") && diff.contains("status: active -> disabled"),
        "{diff}"
    );
    rig.clock.advance_secs(11 * 60); // the request made at sign-in counts for the cooldown too
    rig.admin
        .create_request(&actor(1), Role::Admin, &text("please"))
        .await
        .unwrap();
    expected += 1;
    assert_eq!(rig.audit_count().await, expected);

    // A refused operation adds none, and a failing audit write undoes the change.
    assert!(rig.admin.approve(&actor(9), r1, Role::Admin).await.is_err());
    assert_eq!(rig.audit_count().await, expected);
    let (_u, failing) = rig.variant(Arc::new(FailingAudit), Arc::new(FailingNotifier));
    let r3 = rig.request_rows().await.last().unwrap().0;
    assert!(failing.approve(&actor(9), r3, Role::Admin).await.is_err());
    assert_eq!(rig.audit_count().await, expected);
    assert_eq!(
        rig.request_rows().await.last().unwrap().2,
        "pending",
        "the request is still open: the decision rolled back with the audit write"
    );
    assert_eq!(rig.stored(&uid(1)).await.role, Some(Role::Editor));
}

#[tokio::test]
async fn the_audit_event_names_the_actor_and_the_request() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Editor)),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    let (kind, who, req, via): (String, Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT actor_kind, actor_ref, request_id, via FROM audit_events ORDER BY seq DESC LIMIT 1",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(kind, "user");
    assert_eq!(who, Some(uid(9).to_string()));
    assert_eq!(req.as_deref(), Some("req-9"));
    assert_eq!(via, "ui");
    assert!(
        rig.last_audit_diff().await.contains(&uid(1).to_string()),
        "the target is named in the diff"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Access requests from an active user (S4, B21)

#[tokio::test]
async fn an_active_user_can_ask_for_a_different_role() {
    let rig = UsersRig::new(db!()).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    let req = rig
        .admin
        .create_request(&actor(1), Role::Editor, &text("I edit configs"))
        .await
        .unwrap();
    assert_eq!(req.status, AccessRequestStatus::Pending);
    assert_eq!(req.user, uid(1));
    assert_eq!(req.requested_role, Some(Role::Editor));
    assert_eq!(req.reason.as_ref().map(ShortText::as_str), Some("I edit configs"));
    assert_eq!(
        rig.bus.published().await,
        vec![DomainEvent::AccessRequestCreated { user: uid(1) }]
    );
    assert_eq!(rig.notifier.sent().len(), 1);
    assert_eq!(rig.audit_actions().await, vec!["access_requested"]);

    // Same role, a pending user and an unknown user are refused with a reason each.
    assert_eq!(
        rig.admin
            .create_request(&actor(1), Role::Operator, &text("x"))
            .await
            .unwrap_err(),
        AccessError::RequestPending,
        "one open request at a time"
    );
    rig.seed(2, Some(Role::Viewer), UserStatus::Active).await;
    assert_eq!(
        rig.admin
            .create_request(&actor(2), Role::Viewer, &text("x"))
            .await
            .unwrap_err(),
        AccessError::Rule(RuleError::SameRole)
    );
    rig.seed(3, None, UserStatus::Pending).await;
    assert_eq!(
        rig.admin
            .create_request(&actor(3), Role::Viewer, &text("x"))
            .await
            .unwrap_err(),
        AccessError::Rule(RuleError::NotActive)
    );
    assert_eq!(
        rig.admin
            .create_request(&actor(404), Role::Viewer, &text("x"))
            .await
            .unwrap_err(),
        AccessError::NotFound
    );
}

#[tokio::test]
async fn new_request_cooldown_blocks_spam() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    let first = rig
        .admin
        .create_request(&actor(1), Role::Editor, &text("one"))
        .await
        .unwrap();
    rig.admin.reject(&actor(9), first.id, &text("no")).await.unwrap();
    // Decided, so nothing is pending, but ten minutes have not passed.
    rig.clock.advance_secs(9 * 60 + 59);
    assert_eq!(
        rig.admin
            .create_request(&actor(1), Role::Editor, &text("two"))
            .await
            .unwrap_err(),
        AccessError::Cooldown
    );
    assert_eq!(rig.request_rows().await.len(), 1);
    rig.clock.advance_secs(2);
    rig.admin
        .create_request(&actor(1), Role::Editor, &text("two"))
        .await
        .unwrap();
    assert_eq!(rig.request_rows().await.len(), 2);
    assert_eq!(
        rig.notifier.sent().len(),
        2,
        "one notification per request created"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Lists: keyset, bounded, indexed (P12)

async fn seed_many_users(rig: &UsersRig, count: u32) {
    sqlx::query(
        "INSERT INTO users (tid, oid, email, display_name, role, status) \
         SELECT $1, '00000000-0000-4000-8000-' || lpad(to_hex(g), 12, '0'), 'u' || g || '@example.com', 'User ' || g, \
                CASE WHEN g % 3 = 0 THEN 'viewer' END, CASE WHEN g % 3 = 0 THEN 'active' ELSE 'pending' END \
         FROM generate_series(1, $2::int) g",
    )
    .bind(support::users_rig::TID)
    .bind(i32::try_from(count).unwrap())
    .execute(&rig.pool)
    .await
    .unwrap();
}

async fn analyze(rig: &UsersRig, tables: &str) {
    let mut admin = rig.db.admin().await;
    sqlx::query(&format!("ANALYZE {tables}"))
        .execute(&mut admin)
        .await
        .unwrap();
}

/// Every `(Node Type, Relation Name or Index Name)` of an `EXPLAIN (FORMAT JSON)` plan.
fn plan_nodes(explain: &serde_json::Value) -> Vec<(String, String)> {
    fn walk(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
        let ty = node["Node Type"].as_str().unwrap_or_default().to_owned();
        let what = node["Index Name"]
            .as_str()
            .or_else(|| node["Relation Name"].as_str())
            .unwrap_or_default()
            .to_owned();
        out.push((ty, what));
        if let Some(children) = node["Plans"].as_array() {
            for c in children {
                walk(c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&explain[0]["Plan"], &mut out);
    out
}

#[tokio::test]
async fn explain_user_list_is_keyset_index_scan() {
    let rig = UsersRig::new(db!()).await;
    seed_many_users(&rig, 5_000).await;
    analyze(&rig, "users").await;
    // A page from the middle of the table: the cursor must be an index condition, not a filter over a scan from the start.
    let first = rig
        .admin
        .list_users(&UserQuery {
            limit: Some(100),
            ..UserQuery::default()
        })
        .await
        .unwrap();
    let query = UserQuery {
        cursor: first.next.clone(),
        limit: Some(100),
        ..UserQuery::default()
    };
    let plan = hub_identity::users::explain::list_users(&rig.pool, &query)
        .await
        .unwrap();
    let nodes = plan_nodes(&plan);
    assert!(
        nodes.iter().any(|(t, i)| t == "Index Scan" && i == "users_pkey"),
        "expected an index scan on users_pkey: {nodes:?}"
    );
    assert!(!nodes.iter().any(|(t, _)| t == "Seq Scan"), "{nodes:?}");
    assert!(
        plan.to_string().contains("Index Cond"),
        "the cursor is an index condition: {plan}"
    );
}

#[tokio::test]
async fn explain_access_request_list_uses_status_index() {
    let rig = UsersRig::new(db!()).await;
    seed_many_users(&rig, 5_000).await;
    // 10,000 requests, one open one per twelfth user, spread evenly over the ids.
    sqlx::query(
        "INSERT INTO access_requests (user_tid, user_oid, status) \
         SELECT $1, '00000000-0000-4000-8000-' || lpad(to_hex(u), 12, '0'), \
                CASE WHEN g % 2 = 1 AND u % 12 = 0 THEN 'pending' ELSE 'approved' END \
         FROM (SELECT g, CASE WHEN g % 2 = 1 THEN (g + 1) / 2 ELSE 1 + (g * 7) % 5000 END AS u \
               FROM generate_series(1, 10000) g) x",
    )
    .bind(support::users_rig::TID)
    .execute(&rig.pool)
    .await
    .unwrap();
    analyze(&rig, "users, access_requests").await;
    let query = RequestQuery {
        status: Some(AccessRequestStatus::Pending),
        limit: Some(50),
        ..RequestQuery::default()
    };
    let plan = hub_identity::users::explain::list_requests(&rig.pool, &query)
        .await
        .unwrap();
    let nodes = plan_nodes(&plan);
    assert!(
        nodes.iter().any(|(_, i)| i == "access_requests_status_idx"),
        "expected the (status, id) index: {nodes:?}"
    );
    assert!(
        !nodes
            .iter()
            .any(|(t, r)| t == "Seq Scan" && r == "access_requests"),
        "{nodes:?}"
    );
    let page = rig.admin.list_requests(&query).await.unwrap();
    assert_eq!(page.items.len(), 50);
    assert!(
        page.items
            .iter()
            .all(|r| r.status == AccessRequestStatus::Pending)
    );
}

#[tokio::test]
async fn lists_have_no_offset_and_cap_limit_at_500() {
    // No OFFSET anywhere in the module.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/users");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap().to_ascii_uppercase();
        assert!(
            !text.contains("OFFSET"),
            "{} paginates with OFFSET",
            path.display()
        );
    }
    let rig = UsersRig::new(db!()).await;
    seed_many_users(&rig, 701).await;
    let huge = rig
        .admin
        .list_users(&UserQuery {
            limit: Some(100_000),
            ..UserQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(huge.items.len(), 500);
    assert!(huge.next.is_some());
    let none = rig
        .admin
        .list_users(&UserQuery {
            limit: Some(0),
            ..UserQuery::default()
        })
        .await
        .unwrap();
    assert!(
        none.items.len() <= 500 && !none.items.is_empty(),
        "a zero limit falls back to a page, not to everything"
    );

    // Walking the pages yields every user once, in order.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = rig
            .admin
            .list_users(&UserQuery {
                cursor: cursor.clone(),
                limit: Some(200),
                ..UserQuery::default()
            })
            .await
            .unwrap();
        assert!(page.items.len() <= 200);
        seen.extend(page.items.iter().map(|u| u.id.to_string()));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen.len(), 701);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, seen, "ordered and without repeats");
}

#[tokio::test]
async fn list_filters_and_cursor_checks() {
    let rig = UsersRig::new(db!()).await;
    seed_many_users(&rig, 30).await;
    let active = rig
        .admin
        .list_users(&UserQuery {
            status: Some(UserStatus::Active),
            ..UserQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(active.items.len(), 10);
    assert!(active.items.iter().all(|u| u.status == UserStatus::Active));
    // The search is a substring of the email or the name, case-insensitive, and `%` and `_` are plain characters.
    let by_name = rig
        .admin
        .list_users(&UserQuery {
            q: Some(text("USER 17")),
            ..UserQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(by_name.items.len(), 1);
    for wildcard in ["%", "_", "\\"] {
        let page = rig
            .admin
            .list_users(&UserQuery {
                q: Some(text(wildcard)),
                ..UserQuery::default()
            })
            .await
            .unwrap();
        assert!(page.items.is_empty(), "{wildcard} matched");
    }
    for bad in ["", "not base64!", "AAAA", &"A".repeat(600)] {
        assert_eq!(
            rig.admin
                .list_users(&UserQuery {
                    cursor: Some(bad.to_owned()),
                    ..UserQuery::default()
                })
                .await
                .unwrap_err(),
            AccessError::BadCursor,
            "{bad:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Onboarding: does an Editor still need to store a GitHub token? (S8 stays with prompt 06)

use hub_identity::users::onboarding::{
    GithubTokenPresence, PRESENCE_SQL, PgGithubTokenPresence, needs_github_token,
};

async fn store_credential(rig: &UsersRig, n: u32) {
    sqlx::query(
        "INSERT INTO github_credentials (user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext) \
         VALUES ($1, $2, 'octo', '\\x01', 'v1', '\\x000000000000000000000000', '\\x02')",
    )
    .bind(support::users_rig::TID)
    .bind(oid(n).as_str())
    .execute(&rig.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn needs_github_token_true_for_editor_without_credential() {
    let rig = UsersRig::new(db!()).await;
    let presence = PgGithubTokenPresence::new(rig.pool.clone());
    for (n, role) in [(1, Role::Editor), (2, Role::Operator), (3, Role::Admin)] {
        rig.seed(n, Some(role), UserStatus::Active).await;
        let user = rig.stored(&uid(n)).await;
        assert!(needs_github_token(&user, &presence).await.unwrap(), "{role:?}");
    }
}

#[tokio::test]
async fn needs_github_token_false_for_viewer_and_for_stored_credential() {
    let rig = UsersRig::new(db!()).await;
    let presence = PgGithubTokenPresence::new(rig.pool.clone());
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    assert!(
        !needs_github_token(&rig.stored(&uid(1)).await, &presence)
            .await
            .unwrap()
    );
    rig.seed(2, Some(Role::Editor), UserStatus::Active).await;
    store_credential(&rig, 2).await;
    assert!(presence.has_token(&uid(2)).await.unwrap());
    assert!(
        !needs_github_token(&rig.stored(&uid(2)).await, &presence)
            .await
            .unwrap()
    );
    // A pending or disabled editor is not asked: they cannot reach the page at all.
    rig.seed(3, Some(Role::Editor), UserStatus::Pending).await;
    assert!(
        !needs_github_token(&rig.stored(&uid(3)).await, &presence)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn github_presence_query_never_selects_ciphertext() {
    // The statement itself.
    let sql = PRESENCE_SQL.to_ascii_lowercase();
    for column in [
        "ciphertext",
        "wrapped_dek",
        "nonce",
        "kms_key_version",
        "select *",
        ".*",
    ] {
        assert!(!sql.contains(column), "the presence query mentions {column}");
    }
    // And the database: a role that may read only the id and owner columns can still run it, so it touches nothing else.
    let rig = UsersRig::new(db!()).await;
    rig.seed(1, Some(Role::Editor), UserStatus::Active).await;
    store_credential(&rig, 1).await;
    let mut admin = rig.db.admin().await;
    let role = format!("lk_presence_{}", std::process::id());
    for stmt in [
        format!("CREATE ROLE {role} LOGIN PASSWORD 'probe' NOSUPERUSER"),
        format!("GRANT CONNECT ON DATABASE {} TO {role}", rig.db.database),
        format!("GRANT USAGE ON SCHEMA public TO {role}"),
        format!("GRANT SELECT (id, user_tid, user_oid) ON github_credentials TO {role}"),
    ] {
        // Roles are cluster-wide: another test thread may have made it already.
        let _ = sqlx::query(&stmt).execute(&mut admin).await;
    }
    let probe_pool = rig.db.pool_as(&role, "probe", 2).await;
    let probe = PgGithubTokenPresence::new(probe_pool);
    assert!(probe.has_token(&uid(1)).await.unwrap());
    assert!(!probe.has_token(&uid(2)).await.unwrap());
}

// ---------------------------------------------------------------------------------------------------------------
// S10: no names, emails or reasons in the logs

#[tokio::test]
async fn user_operations_log_no_names_emails_or_reasons() {
    let (capture, _guard) = support::log_capture::install();
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    let request = pending_user_with_request(&rig, 1).await;
    // Successes and failures of every kind.
    let _ = rig
        .admin
        .reject(&actor(9), request, &text("SECRET-REASON-TEXT"))
        .await;
    let _ = rig
        .admin
        .reject(&actor(9), request, &text("SECRET-REASON-TEXT"))
        .await;
    let _ = rig.admin.approve(&actor(9), 999_999, Role::Viewer).await;
    let (users, admin) = rig.variant(Arc::new(FailingAudit), Arc::new(FailingNotifier));
    let _ = users.first_sign_in(&sign_in(5), None).await;
    let _ = admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Editor)),
                ..Change::default()
            },
        )
        .await;
    let (users, _) = rig.variant(
        Arc::new(hub_identity::audit::PgAuditLog::new()),
        Arc::new(FailingNotifier),
    );
    let _ = users.first_sign_in(&sign_in(6), None).await;

    let log = capture.text();
    assert!(!log.is_empty(), "the failures are logged, so the capture works");
    for secret in [
        "SECRET-REASON-TEXT",
        "user1@example.com",
        "user5@example.com",
        "User 5",
        "User 6",
    ] {
        assert!(!log.contains(secret), "{secret} reached the log");
    }
}

#[tokio::test]
async fn error_display_never_contains_input() {
    // Every variant, by Display and Debug: fixed text only.
    let errors = [
        AccessError::NotFound,
        AccessError::AlreadyDecided,
        AccessError::RequestPending,
        AccessError::Cooldown,
        AccessError::BadCursor,
        AccessError::Unavailable,
        AccessError::Rule(RuleError::LastActiveAdmin),
        AccessError::Audit(ports::AuditError::TooLarge),
        AccessError::Bus(ports::BusError::Unavailable),
    ];
    for e in errors {
        let shown = format!("{e} {e:?}");
        assert!(!shown.contains("00000000-"), "{shown}");
        assert!(!shown.contains('@'), "{shown}");
    }
}
