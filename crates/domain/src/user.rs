//! The application user (`docs/domain-model.md`, "Users and access").

use serde::{Deserialize, Serialize};

use crate::{Role, ShortText, Timestamp, UserId, UserStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub email: ShortText,
    pub display_name: ShortText,
    /// `None` is the "none" role: signed in but not granted anything yet.
    pub role: Option<Role>,
    pub status: UserStatus,
    pub requires_approval: bool,
    pub github_login: Option<ShortText>,
    pub last_seen_at: Option<Timestamp>,
}

impl User {
    /// True for an active user whose role is at least `min`. Pending and disabled users never pass (S4).
    pub fn allows(&self, min: Role) -> bool {
        self.status == UserStatus::Active && self.role.is_some_and(|r| r.at_least(min))
    }
}
