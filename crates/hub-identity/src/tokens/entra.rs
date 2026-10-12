//! Entra access tokens for MCP (S1): the hub API as audience, the `mcp.access` scope, the tenant, and a `tid` plus `oid`
//! identity.
//!
//! Deployment requirement: the hub API's app registration must issue v2 tokens (`accessTokenAcceptedVersion: 2`), whose
//! issuer is `https://login.microsoftonline.com/<tid>/v2.0`, the issuer this verifier accepts. A v1 token (`sts.windows.net`)
//! is refused, which fails closed.

use std::sync::Arc;

use domain::{Guid, ShortText, UserId};
use ports::{AuthError, VerifiedUser};

use super::auth_error;
use crate::config::EntraConfig;
use crate::oidc::jwt::{Claims, JwtVerifier, Profile};

/// The scope an MCP client's token must carry.
pub const MCP_SCOPE: &str = "mcp.access";

/// Verifies access tokens issued by the hub's tenant for the hub API.
pub struct EntraAccessTokens {
    verifier: Arc<JwtVerifier>,
    profile: Profile,
}

impl std::fmt::Debug for EntraAccessTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntraAccessTokens").finish_non_exhaustive()
    }
}

impl EntraAccessTokens {
    /// `verifier` is the tenant's: [`crate::oidc::login::OidcLogin::verifier`] shares its key set with the web sign-in.
    pub fn new(cfg: &EntraConfig, verifier: Arc<JwtVerifier>) -> Self {
        Self {
            verifier,
            profile: Profile {
                issuer: cfg.issuer(),
                audiences: cfg.accepted_api_audiences.clone(),
                tenant: Some(cfg.tenant_id.to_string()),
            },
        }
    }

    pub async fn verify(&self, jwt: &str) -> Result<VerifiedUser, AuthError> {
        let claims = self.verifier.verify(jwt, &self.profile).await.map_err(|e| {
            tracing::debug!(error = %e, "entra access token refused");
            auth_error(e)
        })?;
        identity_from_claims(&claims)
    }
}

/// The checks that follow a verified signature, issuer, audience, tenant and expiry: the scope, `nbf`, and a GUID `tid` and
/// `oid`. Email and name are for display only and never part of the identity.
pub fn identity_from_claims(claims: &Claims) -> Result<VerifiedUser, AuthError> {
    if !claims.has_scope(MCP_SCOPE) || claims.nbf.is_none() {
        return Err(AuthError::InvalidToken);
    }
    let guid = |v: &Option<String>| {
        v.as_deref()
            .and_then(|s| Guid::parse(s).ok())
            .ok_or(AuthError::InvalidToken)
    };
    let id = UserId::new(guid(&claims.tid)?, guid(&claims.oid)?);
    let text = |v: &Option<String>| v.as_deref().and_then(|s| ShortText::parse(s).ok());
    Ok(VerifiedUser {
        id,
        email: text(&claims.email).or_else(|| text(&claims.preferred_username)),
        display_name: text(&claims.name),
    })
}
