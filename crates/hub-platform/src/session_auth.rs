//! How a request becomes an active user (S4), until the identity crate (03a) plugs its own implementation in (plan A5).
//!
//! The platform needs three things from identity: the user behind a request, whether a long-lived stream's user is still
//! allowed to listen, and a place to count a user's requests. It does not know about cookies, CSRF or Entra: 03a's
//! `SessionAuth` reads the session cookie from the request parts and answers with a [`User`].
//!
//! `hub`'s binary refuses to start without a real implementation. [`FakeSessionAuth`] exists for tests only and is behind
//! the `fakes` feature, so a release build of the hub cannot contain it by accident.

use async_trait::async_trait;
use domain::{User, UserId};
use http::request::Parts;
use ports::AuthError;

/// The user a guarded route's handler is running for. Inserted into the request extensions after the guard passed.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub User);

#[async_trait]
pub trait SessionAuth: Send + Sync + 'static {
    /// The user behind the request, with their current role and status. A request without a valid session is
    /// [`AuthError::InvalidToken`] (401). Pending, disabled and unknown users are errors too (403 or 401): the guard never
    /// sees a user it must trust blindly.
    async fn authenticate(&self, parts: &Parts) -> Result<User, AuthError>;

    /// Whether this user may still hold a stream open: the session has not ended and the role is at least Viewer.
    /// Streams call this on their heartbeat tick, so a revoked user stops receiving within one tick.
    async fn still_valid(&self, user: &UserId) -> bool;
}

/// Per-user request accounting. `hub` implements it with a bounded bucket table; the guard calls it once the user is known.
pub trait UserThrottle: Send + Sync + 'static {
    /// True when this user may make another request now.
    fn allow(&self, user: &UserId) -> bool;
}

#[cfg(any(test, feature = "fakes"))]
pub use fake::{FakeSessionAuth, TEST_USER_HEADER, user_id};

#[cfg(any(test, feature = "fakes"))]
// A test double: a malformed fixture is a bug in the test, so it may panic.
#[allow(clippy::expect_used)]
mod fake {
    use std::collections::HashMap;
    use std::sync::{Mutex, PoisonError};

    use async_trait::async_trait;
    use domain::{Role, ShortText, User, UserId, UserStatus};
    use http::request::Parts;
    use ports::AuthError;

    use super::SessionAuth;

    /// The header a test sets to say who it is. A real implementation reads a session cookie.
    pub const TEST_USER_HEADER: &str = "x-test-user";

    /// A session store for tests: the `x-test-user` header holds a user's key, and the table maps keys to users.
    #[derive(Debug, Default)]
    pub struct FakeSessionAuth {
        users: Mutex<HashMap<String, User>>,
    }

    impl FakeSessionAuth {
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }

        /// Register `key` as a user with this role and status.
        #[must_use]
        pub fn with_user(self, key: &str, role: Option<Role>, status: UserStatus) -> Self {
            self.set(key, role, status);
            self
        }

        /// Add or change a user, for example to revoke one while a stream is open.
        pub fn set(&self, key: &str, role: Option<Role>, status: UserStatus) {
            let user = User {
                id: user_id(key),
                email: short(&format!("{key}@example.test")),
                display_name: short(key),
                role,
                status,
                requires_approval: false,
                github_login: None,
                last_seen_at: None,
            };
            self.users
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key.to_owned(), user);
        }

        /// Remove a user, as if the session ended.
        pub fn remove(&self, key: &str) {
            self.users
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(key);
        }
    }

    fn short(s: &str) -> ShortText {
        ShortText::parse(s).expect("test fixture text is short and has no control characters")
    }

    /// A stable user id for a key: the key hashed into two GUID-shaped halves.
    ///
    /// # Panics
    ///
    /// Never in practice: the hex of 16 bytes is always a valid GUID.
    #[must_use]
    pub fn user_id(key: &str) -> UserId {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(key.as_bytes());
        let guid = |bytes: &[u8]| -> domain::Guid {
            let h = hex::encode(bytes);
            let text = format!(
                "{}-{}-{}-{}-{}",
                &h[0..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..32]
            );
            domain::Guid::parse(&text).expect("hex of 16 bytes is a GUID")
        };
        UserId::new(guid(&digest[0..16]), guid(&digest[16..32]))
    }

    #[async_trait]
    impl SessionAuth for FakeSessionAuth {
        async fn authenticate(&self, parts: &Parts) -> Result<User, AuthError> {
            let key = parts
                .headers
                .get(TEST_USER_HEADER)
                .and_then(|v| v.to_str().ok())
                .ok_or(AuthError::InvalidToken)?;
            let users = self.users.lock().unwrap_or_else(PoisonError::into_inner);
            let user = users.get(key).ok_or(AuthError::UnknownUser)?;
            match user.status {
                UserStatus::Active => Ok(user.clone()),
                UserStatus::Pending => Err(AuthError::Pending),
                UserStatus::Disabled => Err(AuthError::Disabled),
            }
        }

        async fn still_valid(&self, id: &UserId) -> bool {
            self.users
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
                .any(|u| &u.id == id && u.allows(Role::Viewer))
        }
    }
}
