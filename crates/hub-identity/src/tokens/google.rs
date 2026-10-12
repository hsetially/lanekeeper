//! Google ID tokens for agent join (S1, S5): the Google issuer, the audience the caller names, and a verified `email` that the
//! caller matches against the swimlane's registered service account.

use std::sync::Arc;

use async_trait::async_trait;
use domain::ShortText;
use ports::{AuthError, GoogleIdentity};
use url::Url;

use super::auth_error;
use crate::clock::Clock;
use crate::error::IdentityError;
use crate::http::HttpsClient;
use crate::oidc::jwks::{JwksCache, JwksLocation};
use crate::oidc::jwt::{Claims, JwtVerifier, Profile};

/// The `iss` of Google ID tokens (also those the metadata server issues to a node's service account).
pub const GOOGLE_ISSUER: &str = "https://accounts.google.com";
/// Google's key set (JWK format). Fixed: nothing in a token chooses where keys come from.
pub const GOOGLE_CERTS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
/// The longest audience a caller may ask for.
const MAX_AUDIENCE_BYTES: usize = 512;

struct GoogleCerts;

#[async_trait]
impl JwksLocation for GoogleCerts {
    async fn jwks_url(&self) -> Result<Url, IdentityError> {
        Url::parse(GOOGLE_CERTS_URL).map_err(|_| IdentityError::Jwks)
    }
}

pub struct GoogleIdTokens {
    verifier: JwtVerifier,
}

impl std::fmt::Debug for GoogleIdTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleIdTokens").finish_non_exhaustive()
    }
}

impl GoogleIdTokens {
    /// `http` must be allowed to reach `www.googleapis.com`.
    pub fn new(http: Arc<dyn HttpsClient>, clock: Arc<dyn Clock>) -> Self {
        let keys = Arc::new(JwksCache::new(http, Arc::new(GoogleCerts), Arc::clone(&clock)));
        Self {
            verifier: JwtVerifier::new(keys, clock),
        }
    }

    /// `aud` is the audience the agent was told to request (the hub's join endpoint). An empty one matches nothing.
    pub async fn verify(&self, jwt: &str, aud: &str) -> Result<GoogleIdentity, AuthError> {
        if aud.is_empty() || aud.len() > MAX_AUDIENCE_BYTES {
            return Err(AuthError::InvalidToken);
        }
        let profile = Profile {
            issuer: GOOGLE_ISSUER.to_owned(),
            audiences: vec![aud.to_owned()],
            tenant: None,
        };
        let claims = self.verifier.verify(jwt, &profile).await.map_err(|e| {
            tracing::debug!(error = %e, "google id token refused");
            auth_error(e)
        })?;
        identity_from_claims(&claims, aud)
    }
}

/// The checks that follow a verified signature, issuer, audience and expiry: a subject, and an email that Google says it
/// verified. `aud` is echoed for the audit trail.
pub fn identity_from_claims(claims: &Claims, aud: &str) -> Result<GoogleIdentity, AuthError> {
    if claims.email_verified != Some(true) {
        return Err(AuthError::InvalidToken);
    }
    let text = |v: &Option<String>| {
        v.as_deref()
            .filter(|s| !s.is_empty())
            .and_then(|s| ShortText::parse(s).ok())
            .ok_or(AuthError::InvalidToken)
    };
    Ok(GoogleIdentity {
        subject: text(&claims.subject)?,
        email: text(&claims.email)?,
        audience: ShortText::parse(aud).map_err(|_| AuthError::InvalidToken)?,
    })
}
