//! The three deployment switches of the MCP server (plan 07, T0; D52, A4, A6). The defaults are the ones T0 leaves in place
//! until a human records the Cursor results: OAuth, elicitation and the summary rendering.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use hub_mcp::config::{AuthMode, ConfigError, ConfirmMode, ContentMode, McpConfig, validate_auth_modes};
use proptest::prelude::*;

fn from_pairs(pairs: &[(&str, &str)]) -> Result<McpConfig, ConfigError> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    McpConfig::from_lookup(|name| map.get(name).cloned())
}

#[test]
fn defaults_are_oauth_elicitation_summary() {
    let config = from_pairs(&[]).unwrap();
    assert_eq!(config.auth_mode, AuthMode::Oauth);
    assert_eq!(config.confirm_mode, ConfirmMode::Elicitation);
    assert_eq!(config.content_mode, ContentMode::Summary);
    assert_eq!(config, McpConfig::default());
}

#[test]
fn an_empty_value_is_the_same_as_unset() {
    let config = from_pairs(&[
        ("LK_MCP_AUTH", ""),
        ("LK_MCP_CONFIRM", ""),
        ("LK_MCP_CONTENT", ""),
    ])
    .unwrap();
    assert_eq!(config, McpConfig::default());
}

#[test]
fn every_documented_value_is_accepted() {
    let config = from_pairs(&[
        ("LK_MCP_AUTH", "personal_token"),
        ("LK_MCP_CONFIRM", "draft"),
        ("LK_MCP_CONTENT", "data_block"),
    ])
    .unwrap();
    assert_eq!(config.auth_mode, AuthMode::PersonalToken);
    assert_eq!(config.confirm_mode, ConfirmMode::Draft);
    assert_eq!(config.content_mode, ContentMode::DataBlock);
    let config = from_pairs(&[
        ("LK_MCP_AUTH", "oauth"),
        ("LK_MCP_CONFIRM", "elicitation"),
        ("LK_MCP_CONTENT", "summary"),
    ])
    .unwrap();
    assert_eq!(config, McpConfig::default());
}

#[test]
fn both_auth_modes_at_once_is_a_config_error() {
    // D52: personal-token mode is a separate mode and is never combined with OAuth. A list that names both is an error, not a
    // silent pick of one: an operator who writes it has misunderstood the deployment.
    for value in [
        "oauth,personal_token",
        "personal_token,oauth",
        " oauth , personal_token ",
        "oauth,personal_token,oauth",
    ] {
        let error = from_pairs(&[("LK_MCP_AUTH", value)]).unwrap_err();
        assert_eq!(error, ConfigError::BothAuthModes, "value {value:?}");
    }
    assert_eq!(
        validate_auth_modes(&[AuthMode::Oauth, AuthMode::PersonalToken]),
        Err(ConfigError::BothAuthModes)
    );
    assert_eq!(
        validate_auth_modes(&[AuthMode::PersonalToken, AuthMode::Oauth]),
        Err(ConfigError::BothAuthModes)
    );
}

#[test]
fn exactly_one_auth_mode_is_valid() {
    assert_eq!(validate_auth_modes(&[AuthMode::Oauth]), Ok(AuthMode::Oauth));
    assert_eq!(
        validate_auth_modes(&[AuthMode::PersonalToken]),
        Ok(AuthMode::PersonalToken)
    );
    // Nothing switched on is not "OAuth by default" here: the default is applied where the setting is read.
    assert_eq!(validate_auth_modes(&[]), Err(ConfigError::Invalid("LK_MCP_AUTH")));
    // The same mode named twice is ambiguous, not harmless.
    assert_eq!(
        validate_auth_modes(&[AuthMode::Oauth, AuthMode::Oauth]),
        Err(ConfigError::Invalid("LK_MCP_AUTH"))
    );
}

#[test]
fn unknown_value_is_a_config_error() {
    for (name, value) in [
        ("LK_MCP_AUTH", "basic"),
        ("LK_MCP_AUTH", "OAuth"),
        ("LK_MCP_AUTH", "oauth,basic"),
        ("LK_MCP_AUTH", "oauth,"),
        ("LK_MCP_AUTH", ","),
        ("LK_MCP_CONFIRM", "maybe"),
        ("LK_MCP_CONFIRM", "Draft"),
        ("LK_MCP_CONFIRM", "elicitation,draft"),
        ("LK_MCP_CONTENT", "raw"),
        ("LK_MCP_CONTENT", "data-block"),
    ] {
        assert_eq!(
            from_pairs(&[(name, value)]).unwrap_err(),
            ConfigError::Invalid(name),
            "{name}={value:?}"
        );
    }
}

#[test]
fn a_control_character_is_never_a_valid_value() {
    for value in ["oauth\n", "oauth\0", "\toauth", "oau\u{7f}th"] {
        assert_eq!(
            from_pairs(&[("LK_MCP_AUTH", value)]).unwrap_err(),
            ConfigError::Invalid("LK_MCP_AUTH"),
            "value {value:?}"
        );
    }
}

#[test]
fn errors_name_the_variable_and_never_print_the_value() {
    let secret_looking = "sk-live-0123456789abcdef";
    for name in ["LK_MCP_AUTH", "LK_MCP_CONFIRM", "LK_MCP_CONTENT"] {
        let error = from_pairs(&[(name, secret_looking)]).unwrap_err();
        let shown = format!("{error} / {error:?}");
        assert!(shown.contains(name), "{shown}");
        assert!(!shown.contains(secret_looking), "{shown}");
        assert!(!shown.contains("0123456789"), "{shown}");
    }
    let both = ConfigError::BothAuthModes;
    assert!(both.to_string().contains("LK_MCP_AUTH"));
}

#[test]
fn a_bad_value_in_one_setting_is_not_masked_by_a_good_one() {
    assert_eq!(
        from_pairs(&[("LK_MCP_AUTH", "oauth"), ("LK_MCP_CONFIRM", "nope")]).unwrap_err(),
        ConfigError::Invalid("LK_MCP_CONFIRM")
    );
}

proptest! {
    /// Whatever the three variables hold, reading them never panics, and an accepted configuration holds exactly one auth
    /// mode: the type cannot represent both, and the reader never produces a mode the setting did not name.
    #[test]
    fn from_lookup_never_panics_and_accepts_only_exact_values(
        auth in ".{0,40}",
        confirm in ".{0,40}",
        content in ".{0,40}",
    ) {
        let result = from_pairs(&[
            ("LK_MCP_AUTH", auth.as_str()),
            ("LK_MCP_CONFIRM", confirm.as_str()),
            ("LK_MCP_CONTENT", content.as_str()),
        ]);
        if let Ok(config) = result {
            let auth_ok = auth.is_empty()
                || (config.auth_mode == AuthMode::Oauth && auth.trim_matches(' ') == "oauth")
                || (config.auth_mode == AuthMode::PersonalToken && auth.trim_matches(' ') == "personal_token");
            prop_assert!(auth_ok, "auth {:?} gave {:?}", auth, config.auth_mode);
            let confirm_ok = confirm.is_empty()
                || (config.confirm_mode == ConfirmMode::Elicitation && confirm == "elicitation")
                || (config.confirm_mode == ConfirmMode::Draft && confirm == "draft");
            prop_assert!(confirm_ok, "confirm {:?} gave {:?}", confirm, config.confirm_mode);
            let content_ok = content.is_empty()
                || (config.content_mode == ContentMode::Summary && content == "summary")
                || (config.content_mode == ContentMode::DataBlock && content == "data_block");
            prop_assert!(content_ok, "content {:?} gave {:?}", content, config.content_mode);
        }
    }
}
