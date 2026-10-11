//! Configuration of the identity component (S1, S7). Loaded by the hub binary; validated once at startup.
//!
//! The configuration names secrets (by their Secret Manager name) and files, never their values. `Debug` shows only names.

use domain::Guid;
use serde::Deserialize;
use url::Url;

/// The longest `redirect_path`, API audience, token file path.
const MAX_FIELD_LEN: usize = 512;
/// At most this many accepted API audiences (the hub API has one or two app-id URIs).
const MAX_AUDIENCES: usize = 8;

/// The host of the public Entra authority. Overridable for sovereign clouds and tests.
pub const DEFAULT_AUTHORITY_HOST: &str = "login.microsoftonline.com";

/// Why the configuration was refused. Names the setting, never its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("authority_host must be a bare lower-case host name")]
    AuthorityHost,
    #[error("public_origin must be an https origin with no path, query or credentials")]
    PublicOrigin,
    #[error(
        "redirect_path must start with one '/' and contain no control character, backslash, query or fragment"
    )]
    RedirectPath,
    #[error("a secret name is not valid")]
    SecretName,
    #[error("token_file must be an absolute path")]
    TokenFile,
    #[error("accepted_api_audiences must hold 1 to 8 non-empty values without control characters")]
    Audiences,
}

/// How the hub proves its identity to Entra at the token endpoint.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientAuth {
    /// Preferred (Q25): the client assertion is the projected Kubernetes service-account token that Entra trusts through a
    /// federated credential. The file is read again on every exchange, because the kubelet rotates it.
    Federated { token_file: String },
    /// A client secret from Secret Manager, by name.
    Secret { name: String },
}

impl std::fmt::Debug for ClientAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Federated { token_file } => f
                .debug_struct("Federated")
                .field("token_file", token_file)
                .finish(),
            Self::Secret { name } => f.debug_struct("Secret").field("name", name).finish(),
        }
    }
}

fn default_authority_host() -> String {
    DEFAULT_AUTHORITY_HOST.to_owned()
}

fn default_redirect_path() -> String {
    "/auth/callback".to_owned()
}

/// Entra sign-in for the web UI, and the audiences accepted on MCP access tokens.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntraConfig {
    /// The one tenant that may sign in (single-tenant app registration).
    pub tenant_id: Guid,
    /// The application (client) id of the hub's app registration.
    pub client_id: Guid,
    #[serde(default = "default_authority_host")]
    pub authority_host: String,
    /// `https://host[:port]`, the origin users see. The redirect URI and the CSRF `Origin` check derive from it.
    pub public_origin: String,
    #[serde(default = "default_redirect_path")]
    pub redirect_path: String,
    /// Accepted `aud` values of MCP access tokens (T4): the hub API's application id URI and client id.
    #[serde(default)]
    pub accepted_api_audiences: Vec<String>,
    pub client_auth: ClientAuth,
    /// Secret Manager name of the 32-byte key (base64url) that seals the login cookie. Every replica reads the same one.
    pub login_cookie_key_secret: String,
}

impl EntraConfig {
    /// Check every field. Call once at startup and refuse to start on an error.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let host = &self.authority_host;
        if host.is_empty()
            || host.len() > 253
            || host.starts_with(['.', '-'])
            || !host
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-'))
        {
            return Err(ConfigError::AuthorityHost);
        }
        self.origin_url().map_err(|()| ConfigError::PublicOrigin)?;
        let p = &self.redirect_path;
        let bytes = p.as_bytes();
        if bytes.first() != Some(&b'/')
            || matches!(bytes.get(1), Some(b'/' | b'\\'))
            || p.len() > MAX_FIELD_LEN
            || p.chars().any(|c| c.is_control() || matches!(c, '\\' | '?' | '#'))
        {
            return Err(ConfigError::RedirectPath);
        }
        match &self.client_auth {
            ClientAuth::Secret { name } if !ports::is_valid_secret_name(name) => {
                return Err(ConfigError::SecretName);
            }
            ClientAuth::Federated { token_file }
                if !token_file.starts_with('/') || token_file.len() > MAX_FIELD_LEN =>
            {
                return Err(ConfigError::TokenFile);
            }
            _ => {}
        }
        if !ports::is_valid_secret_name(&self.login_cookie_key_secret) {
            return Err(ConfigError::SecretName);
        }
        let a = &self.accepted_api_audiences;
        if a.is_empty()
            || a.len() > MAX_AUDIENCES
            || a.iter()
                .any(|v| v.is_empty() || v.len() > MAX_FIELD_LEN || v.chars().any(char::is_control))
        {
            return Err(ConfigError::Audiences);
        }
        Ok(())
    }

    fn origin_url(&self) -> Result<Url, ()> {
        let url = Url::parse(&self.public_origin).map_err(|_| ())?;
        let plain = url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.path(), "" | "/")
            && url.query().is_none()
            && url.fragment().is_none();
        if plain { Ok(url) } else { Err(()) }
    }

    /// The origin as a browser sends it in `Origin`: scheme, host and non-default port, no trailing slash.
    pub fn origin(&self) -> String {
        self.public_origin.trim_end_matches('/').to_owned()
    }

    /// `https://<origin><redirect_path>`, the redirect URI registered in the app registration.
    pub fn redirect_uri(&self) -> String {
        format!("{}{}", self.origin(), self.redirect_path)
    }

    /// The issuer Entra puts in v2 tokens for this tenant.
    pub fn issuer(&self) -> String {
        format!("https://{}/{}/v2.0", self.authority_host, self.tenant_id)
    }

    /// Where the discovery document lives.
    pub fn discovery_url(&self) -> String {
        format!("{}/.well-known/openid-configuration", self.issuer())
    }
}

/// Everything the identity component is configured with. T2 adds the bootstrap admins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    pub entra: EntraConfig,
}

impl IdentityConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.entra.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TID: &str = "11111111-1111-4111-8111-111111111111";
    const CID: &str = "22222222-2222-4222-8222-222222222222";

    fn json(extra: &str) -> String {
        format!(
            r#"{{"tenant_id":"{TID}","client_id":"{CID}","public_origin":"https://lanekeeper.example.com",
            "accepted_api_audiences":["api://lanekeeper"],"login_cookie_key_secret":"login-cookie-key",
            "client_auth":{{"mode":"secret","name":"entra-client-secret"}}{extra}}}"#
        )
    }

    fn parse(extra: &str) -> EntraConfig {
        serde_json::from_str(&json(extra)).unwrap()
    }

    #[test]
    fn defaults_and_derived_values() {
        let c = parse("");
        c.validate().unwrap();
        assert_eq!(c.authority_host, DEFAULT_AUTHORITY_HOST);
        assert_eq!(c.redirect_uri(), "https://lanekeeper.example.com/auth/callback");
        assert_eq!(
            c.issuer(),
            format!("https://login.microsoftonline.com/{TID}/v2.0")
        );
        assert_eq!(
            c.discovery_url(),
            format!("https://login.microsoftonline.com/{TID}/v2.0/.well-known/openid-configuration")
        );
    }

    #[test]
    fn unknown_fields_and_bad_ids_are_refused_at_load() {
        assert!(serde_json::from_str::<EntraConfig>(&json(r#","surprise":1"#)).is_err());
        assert!(serde_json::from_str::<EntraConfig>(&json("").replace(TID, "not-a-guid")).is_err());
    }

    #[test]
    fn public_origin_must_be_a_plain_https_origin() {
        for bad in [
            "http://lanekeeper.example.com",
            "https://lanekeeper.example.com/app",
            "https://user:pw@lanekeeper.example.com",
            "https://lanekeeper.example.com?x=1",
            "lanekeeper.example.com",
        ] {
            let mut c = parse("");
            c.public_origin = bad.to_owned();
            assert_eq!(c.validate(), Err(ConfigError::PublicOrigin), "{bad}");
        }
        let mut c = parse("");
        c.public_origin = "https://lanekeeper.example.com:8443/".to_owned();
        c.validate().unwrap();
        assert_eq!(c.origin(), "https://lanekeeper.example.com:8443");
    }

    #[test]
    fn redirect_path_and_hosts_and_secret_names_are_checked() {
        let mut c = parse("");
        for bad in [
            "auth/callback",
            "//evil.example.com/x",
            "/\\evil",
            "/a?b=1",
            "/a#b",
            "/a\nb",
        ] {
            c.redirect_path = bad.to_owned();
            assert_eq!(c.validate(), Err(ConfigError::RedirectPath), "{bad:?}");
        }
        let mut c = parse("");
        c.authority_host = "Login.Example".to_owned();
        assert_eq!(c.validate(), Err(ConfigError::AuthorityHost));
        let mut c = parse("");
        c.login_cookie_key_secret = "bad name".to_owned();
        assert_eq!(c.validate(), Err(ConfigError::SecretName));
        let mut c = parse("");
        c.client_auth = ClientAuth::Federated {
            token_file: "relative/token".to_owned(),
        };
        assert_eq!(c.validate(), Err(ConfigError::TokenFile));
        let mut c = parse("");
        c.accepted_api_audiences.clear();
        assert_eq!(c.validate(), Err(ConfigError::Audiences));
    }

    #[test]
    fn debug_output_names_secrets_but_holds_no_value() {
        let c = parse("");
        let text = format!("{c:?}");
        assert!(
            text.contains("entra-client-secret"),
            "the name is configuration, not a secret"
        );
    }
}
