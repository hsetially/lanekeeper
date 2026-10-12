//! Bearer-token verification (T4; S1): Entra access tokens for MCP, Google ID tokens for agent join.
//!
//! - [`HubTokenVerifier`] implements [`ports::TokenVerifier`]. It verifies a token and nothing else: whether the user behind
//!   an Entra token is active is decided by the user directory.
//! - [`McpAuthenticator`] composes the two (decision B11): a valid token, then [`ports::Users::require_active`], so that a
//!   pending or disabled user is refused over MCP exactly as they are on the web (D36). It never creates a user.
//!
//! Both verifiers sit on [`crate::oidc::jwt`]: RS256 only, the key chosen by `kid` from the issuer's cached key set (never
//! from a URL in the token), then issuer, audience, tenant and times. What this module adds is what is specific to the token's
//! use, in plain functions ([`entra::identity_from_claims`], [`google::identity_from_claims`]) that the fuzz target drives.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{Role, User};
use ports::{AuthError, GoogleIdentity, TokenVerifier, Users, VerifiedUser};

use crate::error::{IdentityError, TokenError};

pub mod entra;
pub mod google;

pub use entra::{EntraAccessTokens, MCP_SCOPE};
pub use google::{GOOGLE_CERTS_URL, GOOGLE_ISSUER, GoogleIdTokens};

/// What the caller is told about a failed verification. The cause is logged by variant; no token text reaches the log or the
/// answer (S10, S21).
pub(crate) fn auth_error(e: IdentityError) -> AuthError {
    match e {
        IdentityError::Token(TokenError::Expired) => AuthError::Expired,
        IdentityError::Token(TokenError::WrongAudience) => AuthError::WrongAudience,
        IdentityError::Token(_) | IdentityError::BadRequest => AuthError::InvalidToken,
        // The issuer's keys could not be fetched, or the hub is misconfigured: not the caller's doing, and not a verdict on
        // the token.
        _ => AuthError::Unavailable,
    }
}

/// [`TokenVerifier`] for the hub: Entra access tokens (MCP) and Google ID tokens (agent join).
#[derive(Debug)]
pub struct HubTokenVerifier {
    entra: EntraAccessTokens,
    google: GoogleIdTokens,
}

impl HubTokenVerifier {
    pub fn new(entra: EntraAccessTokens, google: GoogleIdTokens) -> Self {
        Self { entra, google }
    }
}

#[async_trait]
impl TokenVerifier for HubTokenVerifier {
    async fn entra_access_token(&self, jwt: &str) -> Result<VerifiedUser, AuthError> {
        self.entra.verify(jwt).await
    }

    async fn google_id_token(&self, jwt: &str, aud: &str) -> Result<GoogleIdentity, AuthError> {
        self.google.verify(jwt, aud).await
    }
}

/// MCP authentication: a valid Entra access token whose user is active and has at least the role a tool needs.
pub struct McpAuthenticator {
    tokens: Arc<dyn TokenVerifier>,
    users: Arc<dyn Users>,
}

impl std::fmt::Debug for McpAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAuthenticator").finish_non_exhaustive()
    }
}

impl McpAuthenticator {
    pub fn new(tokens: Arc<dyn TokenVerifier>, users: Arc<dyn Users>) -> Self {
        Self { tokens, users }
    }

    /// The user behind `jwt`, if the token is valid and they are active with at least `min`. An unknown user is
    /// [`AuthError::UnknownUser`]: the directory is filled by the web sign-in only.
    pub async fn authenticate(&self, jwt: &str, min: Role) -> Result<User, AuthError> {
        let verified = self.tokens.entra_access_token(jwt).await?;
        self.users.require_active(&verified.id, min).await
    }
}
