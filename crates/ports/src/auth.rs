//! Identity ports (implemented by 03a): token verification and the user directory.

use async_trait::async_trait;
use domain::{Role, User, UserId};

use crate::{GoogleIdentity, VerifiedUser};

/// Why a token or a user was refused. A `Display` message never contains the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("token is not valid")]
    InvalidToken,
    #[error("token has expired")]
    Expired,
    #[error("token audience is wrong")]
    WrongAudience,
    /// The user has never signed in or was removed.
    #[error("user is unknown")]
    UnknownUser,
    /// Signed in but not yet granted access (D36).
    #[error("user is pending approval")]
    Pending,
    #[error("user is disabled")]
    Disabled,
    #[error("user role is not sufficient")]
    InsufficientRole,
    #[error("identity provider is unavailable")]
    Unavailable,
}

/// The longest token a verifier parses. Longer input is [`AuthError::InvalidToken`] without parsing.
pub const MAX_TOKEN_BYTES: usize = 16 * 1024;

#[async_trait]
pub trait TokenVerifier: Send + Sync + 'static {
    /// MCP: audience = the hub API, scope contains `mcp.access`, tenant matches.
    async fn entra_access_token(&self, jwt: &str) -> Result<VerifiedUser, AuthError>;

    /// Agent join: Google issuer, the given audience. The caller matches the returned email to the swimlane.
    async fn google_id_token(&self, jwt: &str, aud: &str) -> Result<GoogleIdentity, AuthError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UserError {
    #[error("user directory is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait Users: Send + Sync + 'static {
    async fn get(&self, id: &UserId) -> Result<Option<User>, UserError>;

    /// The user, if active and at least `min`. Pending and disabled users never pass, whatever their role
    /// (S4). A user with no role passes nothing.
    async fn require_active(&self, id: &UserId, min: Role) -> Result<User, AuthError>;
}
