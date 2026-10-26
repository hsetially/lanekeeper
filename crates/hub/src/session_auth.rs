//! The session check the process starts with (plan 18, T1): it lets nobody in.
//!
//! `Deps.auth` must be something, and the process must never fall back to a fake session store, because a fake in production
//! would let anyone in. Until the identity module is wired (T4) the hub holds this: every request is "no valid session"
//! (401 on a guarded route) and no stream stays open. Serving the app shell and the operations endpoints needs no session, so
//! the first commits of the assembly are a hub that answers `/healthz`, `/readyz` and the web app and nothing that is guarded.
//!
//! This is the deny-by-default value in its simplest form; `release_build.rs` also keeps the platform's `FakeSessionAuth` out of
//! the release graph.

use async_trait::async_trait;
use domain::{User, UserId};
use http::request::Parts;
use hub_platform::SessionAuth;
use ports::AuthError;

/// Authenticates nobody.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenySessions;

#[async_trait]
impl SessionAuth for DenySessions {
    async fn authenticate(&self, _parts: &Parts) -> Result<User, AuthError> {
        Err(AuthError::InvalidToken)
    }

    async fn still_valid(&self, _user: &UserId) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nobody_is_authenticated_whatever_the_request_carries() {
        let (parts, ()) = http::Request::builder()
            .uri("/api/v1/me")
            .header("cookie", "lk_session=anything")
            .header("authorization", "Bearer anything")
            .body(())
            .unwrap_or_default()
            .into_parts();
        assert_eq!(
            DenySessions.authenticate(&parts).await.unwrap_err(),
            AuthError::InvalidToken
        );
    }

    #[tokio::test]
    async fn no_stream_stays_open() {
        let guid = |text: &str| domain::Guid::parse(text).unwrap();
        let user = UserId::new(
            guid("11111111-2222-3333-4444-555555555555"),
            guid("66666666-7777-8888-9999-000000000000"),
        );
        assert!(!DenySessions.still_valid(&user).await);
    }
}
