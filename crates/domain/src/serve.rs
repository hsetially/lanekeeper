//! What the config-server is asked for (D82): `(application, profile = "<tenant>,default", [channel], file)`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{IdError, NfsPath, TenantId};

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
