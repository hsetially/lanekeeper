//! The rules for changing a user (S1, S4), as pure functions so that every branch is table-tested.
//!
//! The database code in [`super::UserAdmin`] reads the current state under a lock, asks these functions what the change
//! means, and only then writes. Nothing here touches I/O.

use domain::{Role, UserStatus};

/// What a user is at the moment: the three fields an admin can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    /// `None` is the "none" role: signed in, nothing granted yet.
    pub role: Option<Role>,
    pub status: UserStatus,
    pub requires_approval: bool,
}

/// What an admin asks for. A field left as `None` is not touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Change {
    /// `Some(None)` removes the role, `Some(Some(r))` sets it.
    pub role: Option<Option<Role>>,
    pub status: Option<UserStatus>,
    pub requires_approval: Option<bool>,
}

/// Why a change was refused. Never carries a user id, an email or any other input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("the change touches nothing")]
    EmptyChange,
    #[error("an admin cannot change their own role")]
    SelfRoleChange,
    #[error("an admin cannot change their own status")]
    SelfStatusChange,
    #[error("an admin cannot change their own approval flag")]
    SelfFlagChange,
    #[error("the last active admin cannot be demoted or disabled")]
    LastActiveAdmin,
    /// `pending` is the state of a user nobody has decided about yet. Nothing moves a user back to it.
    #[error("a user cannot be moved to pending")]
    CannotMoveToPending,
    /// An active user must have a role: an active user with none passes no check, which is a mistake, not a state.
    #[error("an active user needs a role")]
    ActiveNeedsRole,
    /// Approving a request of a disabled user would quietly undo the disable.
    #[error("a disabled user cannot be approved")]
    TargetDisabled,
    #[error("only an active user can ask for another role")]
    NotActive,
    #[error("the requested role is the one the user already has")]
    SameRole,
}

/// What a valid change does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub after: Standing,
    pub role_changed: bool,
    pub status_changed: bool,
    pub flag_changed: bool,
}

impl Outcome {
    /// The change asked for what is already true: write nothing, audit nothing.
    pub fn is_noop(&self) -> bool {
        !(self.role_changed || self.status_changed || self.flag_changed)
    }

    /// A role or status change ends every session of the user (S2). The approval flag does not.
    pub fn ends_sessions(&self) -> bool {
        self.role_changed || self.status_changed
    }
}

/// What `change` does to `current`, or why it is refused.
///
/// - `actor_is_target`: the admin is changing themselves. They may not change their own role, status or approval flag (S4).
/// - `other_active_admins`: how many active admins there are besides the target, counted under the lock the caller holds.
///   The last active admin cannot be demoted, disabled or left without the admin role.
/// - Asking for what is already true is not an error and not a change ([`Outcome::is_noop`]).
pub fn plan_update(
    actor_is_target: bool,
    current: Standing,
    change: Change,
    other_active_admins: u64,
) -> Result<Outcome, RuleError> {
    if change.role.is_none() && change.status.is_none() && change.requires_approval.is_none() {
        return Err(RuleError::EmptyChange);
    }
    if change.status == Some(UserStatus::Pending) && current.status != UserStatus::Pending {
        return Err(RuleError::CannotMoveToPending);
    }
    let after = Standing {
        role: change.role.unwrap_or(current.role),
        status: change.status.unwrap_or(current.status),
        requires_approval: change.requires_approval.unwrap_or(current.requires_approval),
    };
    let outcome = Outcome {
        after,
        role_changed: after.role != current.role,
        status_changed: after.status != current.status,
        flag_changed: after.requires_approval != current.requires_approval,
    };
    if actor_is_target {
        if outcome.role_changed {
            return Err(RuleError::SelfRoleChange);
        }
        if outcome.status_changed {
            return Err(RuleError::SelfStatusChange);
        }
        if outcome.flag_changed {
            return Err(RuleError::SelfFlagChange);
        }
    }
    if outcome.ends_sessions() && after.status == UserStatus::Active && after.role.is_none() {
        return Err(RuleError::ActiveNeedsRole);
    }
    let is_active_admin = |s: &Standing| s.status == UserStatus::Active && s.role == Some(Role::Admin);
    if is_active_admin(&current) && !is_active_admin(&after) && other_active_admins == 0 {
        return Err(RuleError::LastActiveAdmin);
    }
    Ok(outcome)
}

/// Approving an access request with `role`: the role is set and the user becomes active. A disabled user is never approved,
/// and nobody approves their own request, even to the role they hold.
pub fn plan_approval(
    actor_is_target: bool,
    current: Standing,
    role: Role,
    other_active_admins: u64,
) -> Result<Outcome, RuleError> {
    if actor_is_target {
        return Err(RuleError::SelfRoleChange);
    }
    if current.status == UserStatus::Disabled {
        return Err(RuleError::TargetDisabled);
    }
    plan_update(
        false,
        current,
        Change {
            role: Some(Some(role)),
            status: Some(UserStatus::Active),
            requires_approval: None,
        },
        other_active_admins,
    )
}

/// A user asking for another role (`POST /me/access-request`).
pub fn check_request(current: Standing, requested: Role) -> Result<(), RuleError> {
    if current.status != UserStatus::Active {
        return Err(RuleError::NotActive);
    }
    if current.role == Some(requested) {
        return Err(RuleError::SameRole);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_STATUS: [UserStatus; 3] = [UserStatus::Pending, UserStatus::Active, UserStatus::Disabled];

    fn standing(role: Option<Role>, status: UserStatus) -> Standing {
        Standing {
            role,
            status,
            requires_approval: false,
        }
    }

    fn set_role(r: Role) -> Change {
        Change {
            role: Some(Some(r)),
            ..Change::default()
        }
    }

    fn set_status(s: UserStatus) -> Change {
        Change {
            status: Some(s),
            ..Change::default()
        }
    }

    fn set_flag(f: bool) -> Change {
        Change {
            requires_approval: Some(f),
            ..Change::default()
        }
    }

    #[test]
    fn an_empty_change_is_refused() {
        let s = standing(Some(Role::Viewer), UserStatus::Active);
        assert_eq!(
            plan_update(false, s, Change::default(), 1),
            Err(RuleError::EmptyChange)
        );
    }

    #[test]
    fn asking_for_what_is_already_true_is_a_noop() {
        let s = standing(Some(Role::Editor), UserStatus::Active);
        for c in [
            set_role(Role::Editor),
            set_status(UserStatus::Active),
            set_flag(false),
        ] {
            let o = plan_update(false, s, c, 1).unwrap();
            assert!(o.is_noop(), "{c:?}");
            assert!(!o.ends_sessions());
            assert_eq!(o.after, s);
        }
    }

    #[test]
    fn role_and_status_changes_end_sessions_and_the_flag_does_not() {
        let s = standing(Some(Role::Editor), UserStatus::Active);
        assert!(
            plan_update(false, s, set_role(Role::Operator), 1)
                .unwrap()
                .ends_sessions()
        );
        assert!(
            plan_update(false, s, set_status(UserStatus::Disabled), 1)
                .unwrap()
                .ends_sessions()
        );
        let flag = plan_update(false, s, set_flag(true), 1).unwrap();
        assert!(!flag.ends_sessions() && !flag.is_noop() && flag.flag_changed);
        assert!(flag.after.requires_approval);
    }

    #[test]
    fn status_transition_table() {
        // (from, to, allowed). Pending is only ever the initial state.
        let table = [
            (UserStatus::Pending, UserStatus::Active, true),
            (UserStatus::Pending, UserStatus::Disabled, true),
            (UserStatus::Pending, UserStatus::Pending, true),
            (UserStatus::Active, UserStatus::Disabled, true),
            (UserStatus::Active, UserStatus::Pending, false),
            (UserStatus::Disabled, UserStatus::Active, true),
            (UserStatus::Disabled, UserStatus::Pending, false),
        ];
        for (from, to, allowed) in table {
            // The user has a role so that "active" is valid, and another admin exists so that the last-admin rule is out of the way.
            let s = standing(Some(Role::Editor), from);
            let got = plan_update(false, s, set_status(to), 1);
            if allowed {
                assert_eq!(got.unwrap().after.status, to, "{from:?} -> {to:?}");
            } else {
                assert_eq!(got, Err(RuleError::CannotMoveToPending), "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn an_active_user_needs_a_role() {
        let pending = standing(None, UserStatus::Pending);
        assert_eq!(
            plan_update(false, pending, set_status(UserStatus::Active), 1),
            Err(RuleError::ActiveNeedsRole)
        );
        let active = standing(Some(Role::Viewer), UserStatus::Active);
        let remove_role = Change {
            role: Some(None),
            ..Change::default()
        };
        assert_eq!(
            plan_update(false, active, remove_role, 1),
            Err(RuleError::ActiveNeedsRole)
        );
        // Granting the role and activating in one change is what approval does.
        let both = Change {
            role: Some(Some(Role::Viewer)),
            status: Some(UserStatus::Active),
            ..Change::default()
        };
        let o = plan_update(false, pending, both, 0).unwrap();
        assert_eq!(o.after, standing(Some(Role::Viewer), UserStatus::Active));
    }

    #[test]
    fn a_legacy_active_user_without_a_role_can_still_have_the_flag_changed() {
        let legacy = standing(None, UserStatus::Active);
        assert!(plan_update(false, legacy, set_flag(true), 1).is_ok());
    }

    #[test]
    fn disable_then_reenable_keeps_the_role() {
        let s = standing(Some(Role::Operator), UserStatus::Active);
        let off = plan_update(false, s, set_status(UserStatus::Disabled), 1)
            .unwrap()
            .after;
        assert_eq!(off.role, Some(Role::Operator));
        let on = plan_update(false, off, set_status(UserStatus::Active), 1)
            .unwrap()
            .after;
        assert_eq!(on, s);
    }

    #[test]
    fn last_active_admin_cannot_be_demoted_or_disabled_or_unroled() {
        let admin = standing(Some(Role::Admin), UserStatus::Active);
        for role in [Role::Viewer, Role::Editor, Role::Operator] {
            assert_eq!(
                plan_update(false, admin, set_role(role), 0),
                Err(RuleError::LastActiveAdmin),
                "{role:?}"
            );
        }
        assert_eq!(
            plan_update(false, admin, set_status(UserStatus::Disabled), 0),
            Err(RuleError::LastActiveAdmin)
        );
        // With another active admin both are fine.
        assert!(plan_update(false, admin, set_role(Role::Viewer), 1).is_ok());
        assert!(plan_update(false, admin, set_status(UserStatus::Disabled), 1).is_ok());
        // Staying an admin is not a removal.
        assert!(plan_update(false, admin, set_flag(true), 0).is_ok());
        assert!(
            plan_update(false, admin, set_role(Role::Admin), 0)
                .unwrap()
                .is_noop()
        );
    }

    #[test]
    fn the_last_admin_rule_only_protects_active_admins() {
        // A disabled or pending admin is not "an active admin", so changing them cannot remove the last one.
        for status in [UserStatus::Pending, UserStatus::Disabled] {
            let s = standing(Some(Role::Admin), status);
            assert!(
                plan_update(false, s, set_role(Role::Viewer), 0).is_ok(),
                "{status:?}"
            );
        }
        // Not an admin at all.
        let viewer = standing(Some(Role::Viewer), UserStatus::Active);
        assert!(plan_update(false, viewer, set_status(UserStatus::Disabled), 0).is_ok());
    }

    #[test]
    fn an_admin_cannot_change_their_own_role_status_or_flag() {
        let me = standing(Some(Role::Admin), UserStatus::Active);
        assert_eq!(
            plan_update(true, me, set_role(Role::Viewer), 5),
            Err(RuleError::SelfRoleChange)
        );
        assert_eq!(
            plan_update(true, me, set_status(UserStatus::Disabled), 5),
            Err(RuleError::SelfStatusChange)
        );
        assert_eq!(
            plan_update(true, me, set_flag(true), 5),
            Err(RuleError::SelfFlagChange)
        );
        // Restating what is already true is not a change.
        assert!(plan_update(true, me, set_role(Role::Admin), 5).unwrap().is_noop());
        // Someone else may do all three.
        assert!(plan_update(false, me, set_role(Role::Viewer), 5).is_ok());
    }

    #[test]
    fn approval_sets_role_and_status_for_pending_and_active_but_never_for_disabled() {
        let pending = standing(None, UserStatus::Pending);
        let o = plan_approval(false, pending, Role::Editor, 1).unwrap();
        assert_eq!(o.after, standing(Some(Role::Editor), UserStatus::Active));
        assert!(o.ends_sessions());

        let active = standing(Some(Role::Viewer), UserStatus::Active);
        assert_eq!(
            plan_approval(false, active, Role::Operator, 1)
                .unwrap()
                .after
                .role,
            Some(Role::Operator)
        );

        let disabled = standing(Some(Role::Viewer), UserStatus::Disabled);
        assert_eq!(
            plan_approval(false, disabled, Role::Viewer, 1),
            Err(RuleError::TargetDisabled)
        );
    }

    #[test]
    fn nobody_approves_their_own_request_and_the_last_admin_cannot_be_approved_down() {
        let admin = standing(Some(Role::Admin), UserStatus::Active);
        assert_eq!(
            plan_approval(true, admin, Role::Admin, 3),
            Err(RuleError::SelfRoleChange),
            "even to the role they hold"
        );
        assert_eq!(
            plan_approval(false, admin, Role::Viewer, 0),
            Err(RuleError::LastActiveAdmin)
        );
    }

    #[test]
    fn a_request_needs_an_active_user_and_a_different_role() {
        let active = standing(Some(Role::Viewer), UserStatus::Active);
        assert_eq!(check_request(active, Role::Editor), Ok(()));
        assert_eq!(check_request(active, Role::Viewer), Err(RuleError::SameRole));
        for status in [UserStatus::Pending, UserStatus::Disabled] {
            let s = standing(Some(Role::Viewer), status);
            assert_eq!(
                check_request(s, Role::Editor),
                Err(RuleError::NotActive),
                "{status:?}"
            );
        }
        let no_role = standing(None, UserStatus::Active);
        assert_eq!(check_request(no_role, Role::Viewer), Ok(()));
    }

    #[test]
    fn every_combination_keeps_the_invariants() {
        // Exhaustive over a small space: whatever is asked, a success never leaves the system without an active
        // admin when one was needed, never leaves pending as a target, and never leaves an active user without a role
        // when the change touched role or status.
        let roles = [
            None,
            Some(Role::Viewer),
            Some(Role::Editor),
            Some(Role::Operator),
            Some(Role::Admin),
        ];
        for cur_role in roles {
            for cur_status in ALL_STATUS {
                for flag in [false, true] {
                    let cur = Standing {
                        role: cur_role,
                        status: cur_status,
                        requires_approval: flag,
                    };
                    for new_role in [
                        None,
                        Some(None),
                        Some(Some(Role::Viewer)),
                        Some(Some(Role::Admin)),
                    ] {
                        for new_status in [
                            None,
                            Some(UserStatus::Active),
                            Some(UserStatus::Disabled),
                            Some(UserStatus::Pending),
                        ] {
                            for new_flag in [None, Some(false), Some(true)] {
                                for (me, others) in [(false, 0), (false, 1), (true, 1)] {
                                    let change = Change {
                                        role: new_role,
                                        status: new_status,
                                        requires_approval: new_flag,
                                    };
                                    let Ok(o) = plan_update(me, cur, change, others) else {
                                        continue;
                                    };
                                    let was_admin =
                                        cur.status == UserStatus::Active && cur.role == Some(Role::Admin);
                                    let is_admin = o.after.status == UserStatus::Active
                                        && o.after.role == Some(Role::Admin);
                                    assert!(!was_admin || is_admin || others > 0, "{cur:?} {change:?}");
                                    assert!(
                                        o.after.status != UserStatus::Pending
                                            || cur.status == UserStatus::Pending
                                    );
                                    assert!(
                                        !o.ends_sessions()
                                            || o.after.status != UserStatus::Active
                                            || o.after.role.is_some(),
                                        "{cur:?} {change:?}"
                                    );
                                    if me {
                                        assert!(o.is_noop(), "{cur:?} {change:?}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
