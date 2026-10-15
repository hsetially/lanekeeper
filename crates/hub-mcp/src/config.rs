//! The three deployment switches of the MCP server, read once at start-up (plan 07, T0; D52, A4, A6).
//!
//! Until a human records the Cursor 3.22.12 results (`crates/hub-mcp/t0/README.md`, D91) the defaults are the design's: OAuth,
//! elicitation and the summary rendering. Each switch has a typed value; a malformed value is an error that names the variable
//! and never prints the value (S21, the same rule as the hub's own settings).
//!
//! | Variable | Values | Default |
//! |---|---|---|
//! | `LK_MCP_AUTH` | `oauth`, `personal_token` | `oauth` |
//! | `LK_MCP_CONFIRM` | `elicitation`, `draft` | `elicitation` |
//! | `LK_MCP_CONTENT` | `summary`, `data_block` | `summary` |
//!
//! `LK_MCP_AUTH` is read as a list so that a mistaken "both" is an error and not a silent pick of one (D52: personal-token mode
//! is a separate mode and is never combined with OAuth). The type that comes out can hold one mode only.

use std::fmt;

const VAR_AUTH: &str = "LK_MCP_AUTH";
const VAR_CONFIRM: &str = "LK_MCP_CONFIRM";
const VAR_CONTENT: &str = "LK_MCP_CONTENT";

/// The most names `LK_MCP_AUTH` may list. There are two modes, so a longer list is a mistake and is refused unread.
const MAX_AUTH_NAMES: usize = 4;

/// A setting that could not be read. Carries the variable name only, never the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The variable holds something that is not one of its documented values.
    #[error("{0} is not valid")]
    Invalid(&'static str),
    /// `LK_MCP_AUTH` names both authentication modes (D52).
    #[error("LK_MCP_AUTH names both oauth and personal_token; a deployment uses exactly one")]
    BothAuthModes,
}

/// How a caller proves who it is (S14). Exactly one per deployment (D52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthMode {
    /// Entra v2 access tokens, with OAuth discovery metadata published.
    #[default]
    Oauth,
    /// Seven-day personal tokens sent in a header. No OAuth discovery metadata is published in this mode.
    PersonalToken,
}

/// How a write is confirmed (D52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmMode {
    /// The server asks the user through MCP elicitation and proceeds only on an exact accept.
    #[default]
    Elicitation,
    /// A write tool does nothing and returns a link where the user confirms in the web UI.
    Draft,
}

/// How file and doc text reaches the model (A6, S15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentMode {
    /// `content` is the summary and a data notice; the text sits in `structuredContent` only (the contract's text).
    #[default]
    Summary,
    /// `content` also carries the text in a delimited block, for a client that shows the model only `content`.
    DataBlock,
}

impl AuthMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "oauth" => Some(Self::Oauth),
            "personal_token" => Some(Self::PersonalToken),
            _ => None,
        }
    }
}

impl ConfirmMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "elicitation" => Some(Self::Elicitation),
            "draft" => Some(Self::Draft),
            _ => None,
        }
    }
}

impl ContentMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "summary" => Some(Self::Summary),
            "data_block" => Some(Self::DataBlock),
            _ => None,
        }
    }
}

macro_rules! display_as_setting_value {
    ($($ty:ty { $($variant:ident => $text:literal),+ $(,)? })+) => {$(
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self { $(Self::$variant => $text),+ })
            }
        }
    )+};
}

display_as_setting_value! {
    AuthMode { Oauth => "oauth", PersonalToken => "personal_token" }
    ConfirmMode { Elicitation => "elicitation", Draft => "draft" }
    ContentMode { Summary => "summary", DataBlock => "data_block" }
}

/// The one authentication mode a deployment runs, given the modes its settings switch on. Two modes at once is
/// [`ConfigError::BothAuthModes`] (D52); no mode, or the same mode named twice, is invalid.
///
/// # Errors
///
/// [`ConfigError::BothAuthModes`] when both modes are named; [`ConfigError::Invalid`] for `LK_MCP_AUTH` when none is named or a
/// mode is named twice.
pub fn validate_auth_modes(on: &[AuthMode]) -> Result<AuthMode, ConfigError> {
    let oauth = on.contains(&AuthMode::Oauth);
    let personal_token = on.contains(&AuthMode::PersonalToken);
    match (oauth, personal_token, on) {
        (true, true, _) => Err(ConfigError::BothAuthModes),
        (true, false, [mode]) | (false, true, [mode]) => Ok(*mode),
        _ => Err(ConfigError::Invalid(VAR_AUTH)),
    }
}

/// The deployment switches (plan 07, T0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct McpConfig {
    pub auth_mode: AuthMode,
    pub confirm_mode: ConfirmMode,
    pub content_mode: ContentMode,
}

impl McpConfig {
    /// Read the switches through `lookup`, which is `std::env::var` in `main` and a map in tests. An unset or empty variable
    /// takes its default; anything else must be exactly one of the documented values.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] for a value that is not documented or holds a control character, and
    /// [`ConfigError::BothAuthModes`] when `LK_MCP_AUTH` names both modes.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        // An unset or empty variable is "not given". Every value that is given is matched against the documented values exactly
        // (no case folding, no trimming beyond the spaces around the commas of `LK_MCP_AUTH`), so a value with a control
        // character or anything else unexpected matches nothing and is invalid. No value is ever printed.
        let get = |name: &'static str| lookup(name).filter(|v| !v.is_empty());

        let auth_mode = match get(VAR_AUTH) {
            None => AuthMode::default(),
            Some(list) => {
                let mut modes = Vec::with_capacity(MAX_AUTH_NAMES);
                for name in list.split(',') {
                    if modes.len() == MAX_AUTH_NAMES {
                        return Err(ConfigError::Invalid(VAR_AUTH));
                    }
                    let mode =
                        AuthMode::parse(name.trim_matches(' ')).ok_or(ConfigError::Invalid(VAR_AUTH))?;
                    modes.push(mode);
                }
                validate_auth_modes(&modes)?
            }
        };
        let confirm_mode = match get(VAR_CONFIRM) {
            None => ConfirmMode::default(),
            Some(value) => ConfirmMode::parse(&value).ok_or(ConfigError::Invalid(VAR_CONFIRM))?,
        };
        let content_mode = match get(VAR_CONTENT) {
            None => ContentMode::default(),
            Some(value) => ContentMode::parse(&value).ok_or(ConfigError::Invalid(VAR_CONTENT))?,
        };
        Ok(Self {
            auth_mode,
            confirm_mode,
            content_mode,
        })
    }
}
