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

/// The longest public origin accepted.
const MAX_ORIGIN_LEN: usize = 253 + 8;

/// `scheme://host[:port]`: the origin users and Cursor see (the same value as the identity crate's `public_origin`). It builds the
/// URLs the server publishes (the resource-metadata link, the web links) and is what `Host` and `Origin` are checked against.
///
/// `https` for any host; `http` only for a loopback host, for development. No user information, path, query or fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOrigin {
    origin: String,
    authority: String,
}

impl PublicOrigin {
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `public_origin` for anything that is not an origin.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        const NAME: &str = "public_origin";
        let invalid = || ConfigError::Invalid(NAME);
        if text.is_empty()
            || text.len() > MAX_ORIGIN_LEN
            || text.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(invalid());
        }
        let (scheme, rest) = text.split_once("://").ok_or_else(invalid)?;
        if scheme != "https" && scheme != "http" {
            return Err(invalid());
        }
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.is_empty() || authority.contains(['/', '?', '#', '@', '\\']) {
            return Err(invalid());
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(':') && !host.contains(':') => (host, Some(port)),
            Some(_) => return Err(invalid()),
            None => (authority, None),
        };
        if let Some(port) = port {
            match port.parse::<u16>() {
                Ok(n) if n != 0 && !port.starts_with('0') => {}
                _ => return Err(invalid()),
            }
        }
        let label_ok = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if host.is_empty() || host.len() > 253 || !host.split('.').all(label_ok) {
            return Err(invalid());
        }
        let loopback = host == "localhost" || host == "127.0.0.1";
        if scheme == "http" && !loopback {
            return Err(invalid());
        }
        Ok(Self {
            origin: format!("{scheme}://{authority}"),
            authority: authority.to_owned(),
        })
    }

    /// `scheme://host[:port]`, no trailing slash.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.origin
    }

    /// `host[:port]`, as a `Host` header carries it.
    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// The `Host` values that name this origin: the authority as written, and the same host with or without the scheme's default
    /// port (`:443` for `https`, `:80` for `http`), which a client may or may not write.
    #[must_use]
    pub fn host_values(&self) -> Vec<String> {
        let default = if self.origin.starts_with("https://") {
            ":443"
        } else {
            ":80"
        };
        let mut values = vec![self.authority.clone()];
        match self.authority.strip_suffix(default) {
            Some(bare) => values.push(bare.to_owned()),
            // Only an authority without a port gains the default one; `host:8443` names one host and no other.
            None if !self.authority.contains(':') => values.push(format!("{}{default}", self.authority)),
            None => {}
        }
        values
    }

    /// `origin` followed by `path`, which starts with `/`.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }
}

/// The caps on what a result may weigh (D58, P9): 16 KB for a read tool and 4 KB for a write tool, as the contract says. A
/// deployment may lower them (never raise them past the contract); the tests lower them to page small data sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSettings {
    pub read_cap_bytes: usize,
    pub write_cap_bytes: usize,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            read_cap_bytes: crate::schemas::READ_CAP_BYTES,
            write_cap_bytes: crate::schemas::WRITE_CAP_BYTES,
        }
    }
}

impl OutputSettings {
    /// The smallest cap that can hold an envelope with nothing in it.
    pub const MIN_CAP_BYTES: usize = 512;

    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `output` for a cap below [`Self::MIN_CAP_BYTES`] or above the contract's.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let ok = |cap: usize, max: usize| (Self::MIN_CAP_BYTES..=max).contains(&cap);
        if ok(self.read_cap_bytes, crate::schemas::READ_CAP_BYTES)
            && ok(self.write_cap_bytes, crate::schemas::WRITE_CAP_BYTES)
        {
            Ok(())
        } else {
            Err(ConfigError::Invalid("output"))
        }
    }
}

/// Switches of single tools (`tools.<name>.enabled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolSettings {
    /// `notify_services` reaches the config-server's unauthenticated `/update-resources` (Q37). Off until Q37 is answered: the tool
    /// is then absent from `tools/list` and a call to it is `forbidden` (the contract's text).
    pub notify_services_enabled: bool,
}
