//! Authentication for `/mcp` (S1, S14): who is calling, and are they allowed in at all.
//!
//! **Two modes, never together (D52).** OAuth mode takes Entra v2 access tokens through [`ports::TokenVerifier`]; personal-token
//! mode takes `lkpat_` tokens that users make in the web UI ([`pat`]). A deployment runs exactly one, so a token of the other kind
//! is refused as not valid without being sent anywhere: a JWT in personal-token mode never reaches a verifier, and a `lkpat_` token
//! in OAuth mode never reaches the identity provider. Personal-token mode registers no discovery route at all.
//!
//! **Every call re-checks the user.** After the credential is good, [`ports::Users::require_active`] decides whether its owner is
//! active with at least the Viewer role, so a role change or a disabling bites on the next call (S1, D36). Nothing here caches that.
//!
//! The pieces: [`header`] reads the `Authorization` header, [`token`] is the syntax and the keyed hash of a personal token, [`store`]
//! and [`pg_store`] keep them, [`pat`] is the service, [`oauth`] and [`metadata`] are what OAuth mode publishes, and [`routes`]
//! are the REST routes through which a signed-in user makes, lists and revokes their tokens.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{Role, User, UserId};
use http::request::Parts;
use hub_platform::SessionAuth;
use ports::{AuthError, TokenVerifier, Users};

use crate::config::AuthMode;
use crate::limits::DEFAULT_READ_TIMEOUT;

pub mod header;
pub mod metadata;
pub mod oauth;
pub mod pat;
pub mod pg_store;
pub mod routes;
pub mod store;
pub mod token;

use pat::PersonalTokens;

/// Which credential this deployment accepts.
enum Backend {
    Oauth(Arc<dyn TokenVerifier>),
    Personal(Arc<PersonalTokens>),
}

/// Turns the `Authorization` header of a request into an active user (S14). A [`SessionAuth`], so that the hub's guard machinery
/// (and its `still_valid` check for long-lived streams) can use it as it uses the cookie-based one.
pub struct BearerAuth {
    backend: Backend,
    users: Arc<dyn Users>,
    /// How long the identity provider and the directory of users have, together, to vouch for a request (rule 5).
    timeout: Duration,
}

impl std::fmt::Debug for BearerAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BearerAuth")
            .field("mode", &self.mode())
            .finish_non_exhaustive()
    }
}

impl BearerAuth {
    /// OAuth mode: Entra access tokens.
    #[must_use]
    pub fn oauth(verifier: Arc<dyn TokenVerifier>, users: Arc<dyn Users>) -> Self {
        Self {
            backend: Backend::Oauth(verifier),
            users,
            timeout: DEFAULT_READ_TIMEOUT,
        }
    }

    /// Personal-token mode: `lkpat_` tokens.
    #[must_use]
    pub fn personal(tokens: Arc<PersonalTokens>, users: Arc<dyn Users>) -> Self {
        Self {
            backend: Backend::Personal(tokens),
            users,
            timeout: DEFAULT_READ_TIMEOUT,
        }
    }

    /// How long the ports behind a request may take to answer, in all (the deployment's read timeout). A port that is silent for
    /// longer is `Unavailable`, which the gate answers with 503: silence is never a pass.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `check`, within the timeout; a port that does not answer in time is [`AuthError::Unavailable`].
    async fn bounded<T>(&self, check: impl Future<Output = Result<T, AuthError>>) -> Result<T, AuthError> {
        tokio::time::timeout(self.timeout, check)
            .await
            .unwrap_or(Err(AuthError::Unavailable))
    }

    #[must_use]
    pub fn mode(&self) -> AuthMode {
        match self.backend {
            Backend::Oauth(_) => AuthMode::Oauth,
            Backend::Personal(_) => AuthMode::PersonalToken,
        }
    }

    /// The user's id behind a credential, before the user's status is looked at.
    async fn identify(&self, token: &str) -> Result<UserId, AuthError> {
        match &self.backend {
            Backend::Oauth(verifier) => {
                // A personal token is not a JWT: there is no use asking the identity provider about it.
                if token::looks_like_personal_token(token) {
                    return Err(AuthError::InvalidToken);
                }
                Ok(verifier.entra_access_token(token).await?.id)
            }
            Backend::Personal(tokens) => {
                if !token::looks_like_personal_token(token) {
                    return Err(AuthError::InvalidToken);
                }
                tokens.authenticate(token).await
            }
        }
    }
}

#[async_trait]
impl SessionAuth for BearerAuth {
    async fn authenticate(&self, parts: &Parts) -> Result<User, AuthError> {
        let token = header::bearer(&parts.headers).map_err(|_| AuthError::InvalidToken)?;
        // One bound for the whole verdict: the credential and then the user. The gate answers within it, whatever the ports do.
        self.bounded(async {
            let id = self.identify(token).await?;
            // Pending and disabled users never pass, whatever their role (D36, S4). Viewer is the floor of every tool; each tool
            // checks its own role again on every call.
            self.users.require_active(&id, Role::Viewer).await
        })
        .await
    }

    async fn still_valid(&self, user: &UserId) -> bool {
        // A user the directory cannot vouch for in time is not valid: a stream is cut rather than kept open on a guess.
        self.bounded(self.users.require_active(user, Role::Viewer))
            .await
            .is_ok()
    }
}
