//! The MCP server's settings (plan 18, T6, G12; S14, S15, D52, D91): how a caller proves who it is, how a write is confirmed, how
//! file and document text reaches the model, and two switches that a person turns on.
//!
//! Nothing here is a secret. The three switches are the library's own (`hub_mcp::config`, D91) and keep its names and defaults:
//! `LK_MCP_AUTH` is `oauth` or `personal_token` and never both (D52), `LK_MCP_CONFIRM` is `elicitation` or `draft`, and
//! `LK_MCP_CONTENT` is `summary` or `data_block`. The library reads them through a lookup; the lookup this group gives it passes
//! those three names and nothing else, and goes through the same [`Reader`] as every other setting, so an error names the
//! variable and never holds the value (S7, S10).
//!
//! The two switches that are the hub's: `LK_MCP_NOTIFY_SERVICES` turns on the one tool that reaches the config-server's
//! unauthenticated `/update-resources` (Q37), and `LK_MCP_AS_MIRROR` serves a static copy of Entra's authorization-server metadata
//! (D91's `C1.mirror_needed`). Both are `true` or `false` and nothing else, and both are off.
//!
//! **What comes from the Entra settings.** OAuth mode needs the tenant, the application id URI the scope belongs to, the authority
//! host and the public origin. They are the identity group's (one app registration, one origin), so there are no MCP variables for
//! them: [`McpSettings::for_identity`] builds the library's settings from the two groups, and `HubSettings` calls it when it reads
//! them, so a mismatch is an exit 2 that names a variable and not a failure at the first request.
//!
//! **The secret.** Personal-token mode needs one: the Secret Manager secret `mcp-token-hmac-key`, a constant of `hub-mcp` that the
//! hub reads at start (see [`crate::mcp`]). OAuth mode needs none.

use hub_mcp::auth::oauth::OAuthSettings;
use hub_mcp::config::{AuthMode, ConfigError as McpConfigError, McpConfig, PublicOrigin};

use super::ConfigError;
use super::identity::IdentitySettings;
use super::reader::Reader;

/// The switches `hub_mcp::config` reads. They are literals here so that `tests/hardening.rs` sees every variable the hub reads.
const SWITCHES: [&str; 3] = ["LK_MCP_AUTH", "LK_MCP_CONFIRM", "LK_MCP_CONTENT"];

/// The MCP server's settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpSettings {
    /// The authentication, confirmation and content modes (D52, D91). The defaults are the design's.
    pub config: McpConfig,
    /// `tools.notify_services.enabled`: off until Q37 (the config-server's `/update-resources` has no authentication) is answered.
    pub notify_services: bool,
    /// Also serve a static mirror of Entra's v2 endpoints at `/.well-known/oauth-authorization-server`.
    pub authorization_server_mirror: bool,
}

impl McpSettings {
    pub(super) fn read(reader: &Reader<'_>) -> Result<Self, ConfigError> {
        // The library asks for its three names; it is given those and nothing else.
        let lookup = |name: &str| {
            SWITCHES
                .iter()
                .find(|known| **known == name)
                .and_then(|known| reader.get(known))
        };
        let config = McpConfig::from_lookup(lookup).map_err(|error| {
            ConfigError::Invalid(match error {
                McpConfigError::Invalid(name) => SWITCHES
                    .iter()
                    .find(|known| **known == name)
                    .copied()
                    .unwrap_or("LK_MCP_AUTH"),
                // Both modes at once: D52 says never, and the variable that names them is the one to fix.
                McpConfigError::BothAuthModes => "LK_MCP_AUTH",
            })
        })?;
        Ok(Self {
            config,
            notify_services: reader.flag("LK_MCP_NOTIFY_SERVICES", false)?,
            authorization_server_mirror: reader.flag("LK_MCP_AS_MIRROR", false)?,
        })
    }

    /// The library's settings for a deployment with these Entra settings. The public origin is the identity group's, and in OAuth
    /// mode so are the tenant, the authority and the audience: the first accepted audience that is an application id URI
    /// (`api://...`), because the scope a client asks for is `<that URI>/mcp.access`. A list with none is refused by the variable
    /// that holds it, since a token for the client id alone has no scope to ask for.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming the variable that is at fault.
    pub fn for_identity(&self, identity: &IdentitySettings) -> Result<hub_mcp::McpSettings, ConfigError> {
        let entra = &identity.config.entra;
        let public_origin =
            PublicOrigin::parse(&entra.origin()).map_err(|_| ConfigError::Invalid("LK_PUBLIC_ORIGIN"))?;
        let oauth = match self.config.auth_mode {
            AuthMode::Oauth => Some(OAuthSettings {
                tenant_id: entra.tenant_id.clone(),
                api_audience: entra
                    .accepted_api_audiences
                    .iter()
                    .find(|audience| audience.starts_with("api://"))
                    .cloned()
                    .ok_or(ConfigError::Invalid("LK_ENTRA_API_AUDIENCES"))?,
                authority_host: entra.authority_host.clone(),
                authorization_server_mirror: self.authorization_server_mirror,
            }),
            // D52: the modes are never combined, so the one that is off has no settings at all.
            AuthMode::PersonalToken => None,
        };
        let mut settings = hub_mcp::McpSettings::new(self.config, public_origin, oauth);
        settings.tools.notify_services_enabled = self.notify_services;
        settings.validate().map_err(|error| {
            ConfigError::Invalid(match error {
                McpConfigError::Invalid("public_origin") => "LK_PUBLIC_ORIGIN",
                McpConfigError::Invalid("authority_host") => "LK_ENTRA_AUTHORITY_HOST",
                McpConfigError::Invalid("api_audience" | "oauth") => "LK_ENTRA_API_AUDIENCES",
                // What is left is a limit or a template that is the library's default: no variable of the hub's is behind it.
                McpConfigError::Invalid(_) | McpConfigError::BothAuthModes => "LK_MCP_AUTH",
            })
        })?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hub_mcp::config::AuthMode;

    use super::*;
    use crate::config::IdentitySettings;

    const TENANT: &str = "11111111-1111-4111-8111-111111111111";
    const CLIENT: &str = "22222222-2222-4222-8222-222222222222";
    const KEY: &str = "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/audit-checkpoint/cryptoKeyVersions/1";

    fn reader_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn read(pairs: &[(&str, &str)]) -> Result<McpSettings, ConfigError> {
        let lookup = reader_of(pairs);
        let read_file = |_: &std::path::Path| Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        McpSettings::read(&Reader::new(&lookup, &read_file))
    }

    fn identity(extra: &[(&str, &str)]) -> IdentitySettings {
        let mut pairs = vec![
            ("LK_ENTRA_TENANT_ID", TENANT),
            ("LK_ENTRA_CLIENT_ID", CLIENT),
            ("LK_PUBLIC_ORIGIN", "https://lanekeeper.example.com"),
            ("LK_ENTRA_CLIENT_AUTH_REF", "lanekeeper-entra-client"),
            ("LK_ENTRA_COOKIE_KEY_REF", "lanekeeper-login-cookie"),
            ("LK_AUDIT_CHECKPOINT_BUCKET", "lanekeeper-audit-checkpoints"),
            ("LK_AUDIT_CHECKPOINT_KMS_REF", KEY),
        ];
        pairs.extend_from_slice(extra);
        let lookup = reader_of(&pairs);
        let read_file = |_: &std::path::Path| Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        IdentitySettings::read(&Reader::new(&lookup, &read_file))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn the_defaults_are_the_designs_and_the_riskier_switches_are_off() {
        let settings = read(&[]).unwrap();
        assert_eq!(settings.config.auth_mode, AuthMode::Oauth);
        assert_eq!(settings.config.confirm_mode.to_string(), "elicitation");
        assert_eq!(settings.config.content_mode.to_string(), "summary");
        // The tool that reaches the config-server's unauthenticated endpoint (Q37) and the static copy of Entra's metadata are each
        // a person's decision.
        assert!(!settings.notify_services);
        assert!(!settings.authorization_server_mirror);
    }

    #[test]
    fn each_switch_takes_its_documented_values() {
        let settings = read(&[
            ("LK_MCP_AUTH", "personal_token"),
            ("LK_MCP_CONFIRM", "draft"),
            ("LK_MCP_CONTENT", "data_block"),
            ("LK_MCP_NOTIFY_SERVICES", "true"),
            ("LK_MCP_AS_MIRROR", "true"),
        ])
        .unwrap();
        assert_eq!(settings.config.auth_mode, AuthMode::PersonalToken);
        assert_eq!(settings.config.confirm_mode.to_string(), "draft");
        assert_eq!(settings.config.content_mode.to_string(), "data_block");
        assert!(settings.notify_services && settings.authorization_server_mirror);
    }

    #[test]
    fn both_authentication_modes_at_once_refuse_to_start_and_name_the_variable() {
        // D52: personal-token mode is never combined with OAuth. The library says so in its own words; the hub says which variable.
        for both in [
            "oauth,personal_token",
            "personal_token, oauth",
            "oauth,personal_token,oauth",
        ] {
            assert_eq!(
                read(&[("LK_MCP_AUTH", both)]).unwrap_err(),
                ConfigError::Invalid("LK_MCP_AUTH"),
                "{both}"
            );
        }
    }

    #[test]
    fn a_bad_value_is_refused_by_the_variable_and_the_value_is_never_printed() {
        let planted = "planted-bad-value";
        for name in [
            "LK_MCP_AUTH",
            "LK_MCP_CONFIRM",
            "LK_MCP_CONTENT",
            "LK_MCP_NOTIFY_SERVICES",
            "LK_MCP_AS_MIRROR",
        ] {
            let error = read(&[(name, planted)]).unwrap_err();
            assert_eq!(error, ConfigError::Invalid(name), "{name}");
            assert!(!format!("{error} {error:?}").contains(planted));
        }
        // No case folding and no stray control character: a switch is an exact word.
        for value in ["OAuth", "oauth\n", "personal-token", " "] {
            assert!(read(&[("LK_MCP_AUTH", value)]).is_err(), "{value:?}");
        }
        for value in ["True", "1", "yes"] {
            assert!(read(&[("LK_MCP_NOTIFY_SERVICES", value)]).is_err(), "{value}");
        }
    }

    #[test]
    fn oauth_mode_takes_the_tenant_audience_origin_and_authority_from_the_entra_settings() {
        let settings = read(&[("LK_MCP_AS_MIRROR", "true")]).unwrap();
        let built = settings.for_identity(&identity(&[])).unwrap();
        let oauth = built.oauth.as_ref().unwrap();
        assert_eq!(oauth.tenant_id.to_string(), TENANT);
        // The default audiences are the application id URI and the client id; the first is the scope's resource.
        assert_eq!(oauth.api_audience, format!("api://{CLIENT}"));
        assert_eq!(oauth.authority_host, "login.microsoftonline.com");
        assert!(oauth.authorization_server_mirror);
        assert_eq!(built.public_origin.as_str(), "https://lanekeeper.example.com");
        assert!(!built.tools.notify_services_enabled);
    }

    #[test]
    fn a_named_audience_is_the_first_application_id_uri_in_the_list() {
        let named = identity(&[(
            "LK_ENTRA_API_AUDIENCES",
            "other-client-id, api://hub-api, api://second",
        )]);
        let built = read(&[]).unwrap().for_identity(&named).unwrap();
        assert_eq!(built.oauth.as_ref().unwrap().api_audience, "api://hub-api");
    }

    #[test]
    fn oauth_mode_without_an_application_id_uri_is_refused_by_the_audiences_variable() {
        let bare = identity(&[("LK_ENTRA_API_AUDIENCES", CLIENT)]);
        assert_eq!(
            read(&[]).unwrap().for_identity(&bare).unwrap_err(),
            ConfigError::Invalid("LK_ENTRA_API_AUDIENCES")
        );
        // Personal-token mode has no scope to name, so it does not need one.
        let personal = read(&[("LK_MCP_AUTH", "personal_token")]).unwrap();
        assert!(personal.for_identity(&bare).unwrap().oauth.is_none());
    }

    #[test]
    fn personal_token_mode_carries_no_oauth_settings() {
        // D52: the two modes are never combined, so the settings of the one that is off do not exist.
        let settings = read(&[("LK_MCP_AUTH", "personal_token")]).unwrap();
        let built = settings.for_identity(&identity(&[])).unwrap();
        assert!(built.oauth.is_none());
        assert_eq!(built.config.auth_mode, AuthMode::PersonalToken);
    }

    #[test]
    fn the_notify_services_switch_reaches_the_tool_setting() {
        let settings = read(&[("LK_MCP_NOTIFY_SERVICES", "true")]).unwrap();
        assert!(
            settings
                .for_identity(&identity(&[]))
                .unwrap()
                .tools
                .notify_services_enabled
        );
    }

    #[test]
    fn the_whole_settings_refuse_an_mcp_value_that_does_not_fit_the_entra_settings() {
        // Read together, as the process reads them: a bad audience is an exit 2 naming the variable, not a failure at the first request.
        let env = [
            ("LK_ENTRA_TENANT_ID", TENANT),
            ("LK_ENTRA_CLIENT_ID", CLIENT),
            ("LK_PUBLIC_ORIGIN", "https://lanekeeper.example.com"),
            ("LK_ENTRA_CLIENT_AUTH_REF", "lanekeeper-entra-client"),
            ("LK_ENTRA_COOKIE_KEY_REF", "lanekeeper-login-cookie"),
            ("LK_AUDIT_CHECKPOINT_BUCKET", "lanekeeper-audit-checkpoints"),
            ("LK_AUDIT_CHECKPOINT_KMS_REF", KEY),
            ("LK_ENTRA_API_AUDIENCES", CLIENT),
        ];
        let error = crate::HubSettings::from_lookup(reader_of(&env)).unwrap_err();
        assert_eq!(error, ConfigError::Invalid("LK_ENTRA_API_AUDIENCES"));
        let mut personal = env.to_vec();
        personal.push(("LK_MCP_AUTH", "personal_token"));
        assert!(crate::HubSettings::from_lookup(reader_of(&personal)).is_ok());
    }

    proptest::proptest! {
        /// S7, S10: whatever a switch holds, an error names the variable and holds none of the value.
        #[test]
        fn no_value_of_any_switch_ever_reaches_an_error(value in "[ -~]{1,40}") {
            let marker = format!("zz{value}zz");
            for name in ["LK_MCP_AUTH", "LK_MCP_CONFIRM", "LK_MCP_CONTENT", "LK_MCP_NOTIFY_SERVICES", "LK_MCP_AS_MIRROR"] {
                if let Err(error) = read(&[(name, &marker)]) {
                    let shown = format!("{error} {error:?}");
                    proptest::prop_assert!(!shown.contains(&marker), "{}", shown);
                }
            }
        }
    }
}
