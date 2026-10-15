//! What the config-server is asked for (D82): `(application, profile = "<tenant>,default", [channel], file)`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ContentHash, EffectiveSource, FileRole, IdError, NfsPath, ShortText, SwimlaneId, TenantId};

macro_rules! name_token {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn parse(s: &str) -> Result<Self, IdError> {
                if s.is_empty() {
                    return Err(IdError::Empty);
                }
                if s.len() > 128 {
                    return Err(IdError::TooLong);
                }
                if !s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')) {
                    return Err(IdError::BadChar);
                }
                if s.starts_with('.') {
                    return Err(IdError::BadShape);
                }
                Ok(Self(s.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

name_token!(
    /// A config-server application name, such as `tx-infinity-api`.
    AppName
);
name_token!(
    /// A delivery channel from `channels.yml`, such as `remote-itm-teller`.
    ChannelName
);

/// One request to the config-server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServeRequest {
    pub application: AppName,
    pub tenant: TenantId,
    pub channel: Option<ChannelName>,
    pub file: NfsPath,
}

/// What the config-server actually serves for a [`ServeRequest`] (D82, D86), as the agent fetched it read-only: the "as served"
/// view, not the engine's computation. Same fields as the REST `ServedFile`.
///
/// `text` is the file as the config-server serves it, so it may hold a value the secret scan has not flagged; `Debug` prints
/// the file's name and size and never its text (S10, S21), and the MCP layer redacts it before it leaves (S14b).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServedFile {
    pub swimlane: SwimlaneId,
    pub application: AppName,
    pub tenant: TenantId,
    pub channel: Option<ChannelName>,
    pub file: NfsPath,
    pub role: FileRole,
    /// Hash of what was served.
    pub hash: ContentHash,
    /// True for a denied file (D79): `text` is `None` and the hash stays.
    pub content_withheld: bool,
    /// `None` when `content_withheld` is true or the served file is not text. Always serialised, as `null` when absent.
    pub text: Option<String>,
    /// Where the served content came from, highest precedence first.
    pub sources: Vec<EffectiveSource>,
    /// `${...}` left as written because their keys are not in the property view.
    pub unresolved_placeholders: Vec<ShortText>,
}

impl fmt::Debug for ServedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServedFile")
            .field("swimlane", &self.swimlane)
            .field("application", &self.application)
            .field("tenant", &self.tenant)
            .field("channel", &self.channel)
            .field("file", &self.file)
            .field("role", &self.role)
            .field("hash", &self.hash)
            .field("content_withheld", &self.content_withheld)
            .field("text_bytes", &self.text.as_ref().map(String::len))
            .field("sources", &self.sources.len())
            .field("unresolved_placeholders", &self.unresolved_placeholders.len())
            .finish()
    }
}
