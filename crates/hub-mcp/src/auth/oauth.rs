//! OAuth mode (D52, S14): what the hub tells a client about the authorization server, and the challenge a refused request carries.
//!
//! Cursor starts its sign-in from the `WWW-Authenticate` header of a 401: its `resource_metadata` parameter names the protected
//! resource metadata (RFC 9728), whose `authorization_servers` names Entra's v2 endpoint of the hub's tenant. Whether Cursor also
//! needs the static mirror of Entra's own metadata at `/.well-known/oauth-authorization-server` is a T0 observation, so the mirror
//! is off unless the settings turn it on (`C1.mirror_needed` in D91).

use domain::Guid;

use crate::config::{ConfigError, PublicOrigin};

/// The scope that grants access to the MCP server. Entra puts it in the `scp` claim of the access token.
pub const SCOPE_NAME: &str = "mcp.access";
/// Where the protected resource metadata for `/mcp` is published (RFC 9728 §3.1: the well-known prefix, then the resource's path).
pub const RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource/mcp";
/// The same document at the root, for a client that probes there.
pub const RESOURCE_METADATA_ROOT_PATH: &str = "/.well-known/oauth-protected-resource";
/// The static mirror of Entra's authorization server metadata (only when switched on).
pub const AUTHORIZATION_SERVER_PATH: &str = "/.well-known/oauth-authorization-server";

/// How OAuth mode talks to Entra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthSettings {
    /// The Entra tenant that issues the tokens: the only authorization server this resource names.
    pub tenant_id: Guid,
    /// The hub API's application ID URI, `api://<id>`. The scope is this plus `/mcp.access`.
    pub api_audience: String,
    /// The Entra host, `login.microsoftonline.com` unless the deployment is in a sovereign cloud.
    pub authority_host: String,
    /// Also serve a static mirror of Entra's v2 endpoints at [`AUTHORIZATION_SERVER_PATH`].
    pub authorization_server_mirror: bool,
}

impl OAuthSettings {
    /// The default Entra host.
    pub const DEFAULT_AUTHORITY_HOST: &'static str = "login.microsoftonline.com";

    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming the setting that is not usable.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let host = &self.authority_host;
        if host.is_empty()
            || host.len() > 253
            || host.starts_with(['.', '-'])
            || !host
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-'))
        {
            return Err(ConfigError::Invalid("authority_host"));
        }
        let Some(rest) = self.api_audience.strip_prefix("api://") else {
            return Err(ConfigError::Invalid("api_audience"));
        };
        if rest.is_empty()
            || rest.len() > 128
            || rest.starts_with('/')
            || rest.ends_with('/')
            || rest.contains("//")
            || !rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
        {
            return Err(ConfigError::Invalid("api_audience"));
        }
        Ok(())
    }

    /// The scope a client asks for: `api://<id>/mcp.access`.
    #[must_use]
    pub fn scope(&self) -> String {
        format!("{}/{SCOPE_NAME}", self.api_audience)
    }

    /// Entra's issuer for the tenant, which is the authorization server named in the metadata.
    #[must_use]
    pub fn issuer(&self) -> String {
        format!("https://{}/{}/v2.0", self.authority_host, self.tenant_id)
    }
}

/// The `WWW-Authenticate` header values for a 401. Built once at start-up from validated settings.
#[derive(Debug, Clone)]
pub struct Challenge {
    without_token: String,
    with_bad_token: String,
}

impl Challenge {
    /// OAuth mode: points at the protected resource metadata and names the scope. A request that carried a token that was not good
    /// also says `error="invalid_token"` (RFC 6750 §3); one that carried none does not.
    #[must_use]
    pub fn oauth(origin: &PublicOrigin, settings: &OAuthSettings) -> Self {
        let base = format!(
            "Bearer resource_metadata=\"{}\", scope=\"{}\"",
            origin.url(RESOURCE_METADATA_PATH),
            settings.scope()
        );
        Self {
            with_bad_token: format!("{base}, error=\"invalid_token\""),
            without_token: base,
        }
    }

    /// Personal-token mode publishes no discovery metadata, so its challenge points at none.
    #[must_use]
    pub fn personal_token() -> Self {
        Self {
            without_token: "Bearer".to_owned(),
            with_bad_token: "Bearer error=\"invalid_token\"".to_owned(),
        }
    }

    /// The header value, for a request that did or did not present a token.
    #[must_use]
    pub fn value(&self, token_presented: bool) -> &str {
        if token_presented {
            &self.with_bad_token
        } else {
            &self.without_token
        }
    }
}
