//! Authorization of the read port (S4, rule 9).
//!
//! Two layers, because two callers reach the read service: REST through the guarded router (every route is registered at Viewer,
//! so a pending, disabled or role-less user never reaches a handler) and MCP (07) through the port, which has no router. Roles are
//! global (D35), so a Viewer reads every swimlane and there is no per-swimlane filter to apply: the check is the user's own record,
//! not a claim by the caller, and it comes first in every method, before any parameter is looked at.

use domain::{Role, User};
use ports::ReadError;

/// Active, and a role of Viewer or above. Everything else is `Forbidden`, whatever the other arguments are.
pub fn require_viewer(user: &User) -> Result<(), ReadError> {
    if user.allows(Role::Viewer) {
        Ok(())
    } else {
        Err(ReadError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ShortText, UserId, UserStatus};

    fn user(role: Option<Role>, status: UserStatus) -> User {
        let guid = |s: &str| domain::Guid::parse(s).unwrap();
        User {
            id: UserId::new(
                guid("00000000-0000-0000-0000-000000000001"),
                guid("00000000-0000-0000-0000-000000000002"),
            ),
            email: ShortText::parse("a@example.test").unwrap(),
            display_name: ShortText::parse("A").unwrap(),
            role,
            status,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        }
    }

    #[test]
    fn only_an_active_user_with_a_role_passes() {
        for role in [Role::Viewer, Role::Editor, Role::Operator, Role::Admin] {
            assert_eq!(require_viewer(&user(Some(role), UserStatus::Active)), Ok(()));
        }
        for (role, status) in [
            (Some(Role::Admin), UserStatus::Pending),
            (Some(Role::Admin), UserStatus::Disabled),
            (None, UserStatus::Active),
            (None, UserStatus::Pending),
        ] {
            assert_eq!(require_viewer(&user(role, status)), Err(ReadError::Forbidden));
        }
    }
}
