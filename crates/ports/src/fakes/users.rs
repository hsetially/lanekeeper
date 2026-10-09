use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{Role, User, UserId, UserStatus};

use super::util::lock;
use crate::conformance::UsersScenario;
use crate::{AuthError, UserError, Users};

/// Most users the fake holds.
const MAX_USERS: usize = 10_000;

#[derive(Clone, Default)]
pub struct FakeUsers {
    users: Arc<Mutex<HashMap<UserId, User>>>,
}

impl std::fmt::Debug for FakeUsers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeUsers").finish_non_exhaustive()
    }
}

impl FakeUsers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a user. Returns false when the fake is full.
    pub fn insert(&self, user: User) -> bool {
        let mut users = lock(&self.users);
        if users.len() >= MAX_USERS && !users.contains_key(&user.id) {
            return false;
        }
        users.insert(user.id.clone(), user);
        true
    }
}

#[async_trait]
impl Users for FakeUsers {
    async fn get(&self, id: &UserId) -> Result<Option<User>, UserError> {
        Ok(lock(&self.users).get(id).cloned())
    }

    async fn require_active(&self, id: &UserId, min: Role) -> Result<User, AuthError> {
        let user = lock(&self.users).get(id).cloned().ok_or(AuthError::UnknownUser)?;
        match user.status {
            UserStatus::Pending => Err(AuthError::Pending),
            UserStatus::Disabled => Err(AuthError::Disabled),
            UserStatus::Active if user.allows(min) => Ok(user),
            UserStatus::Active => Err(AuthError::InsufficientRole),
        }
    }
}

#[async_trait]
impl UsersScenario for FakeUsers {
    async fn seed(&self, user: User) {
        self.insert(user);
    }
}
