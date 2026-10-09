//! `ShortText`: a bounded, control-character-free string for names, titles and hints.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

/// Maximum length of a [`ShortText`] in bytes.
pub const SHORT_TEXT_MAX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    #[error("text is too long")]
    TooLong,
    #[error("text contains a control character")]
    ControlChar,
}

/// Text of at most 256 bytes with no control characters. Empty is allowed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ShortText(String);

impl ShortText {
    pub fn parse(s: &str) -> Result<Self, TextError> {
        if s.len() > SHORT_TEXT_MAX {
            return Err(TextError::TooLong);
        }
        if s.chars().any(char::is_control) {
            return Err(TextError::ControlChar);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ShortText {
    type Err = TextError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for ShortText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ShortText {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}
