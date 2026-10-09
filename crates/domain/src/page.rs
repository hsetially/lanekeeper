//! Keyset pagination (never OFFSET). The cursor is opaque to clients.

use serde::{Deserialize, Deserializer, Serialize};

/// Why a page request was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PageError {
    #[error("page limit must be between 1 and 500")]
    BadLimit,
    #[error("cursor is not valid")]
    BadCursor,
}

/// An opaque keyset cursor: 1-512 characters of `[A-Za-z0-9_=.-]` (base64url plus padding).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Cursor(String);

impl Cursor {
    pub fn parse(s: &str) -> Result<Self, PageError> {
        if s.is_empty() || s.len() > 512 {
            return Err(PageError::BadCursor);
        }
        if !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'=' | b'.'))
        {
            return Err(PageError::BadCursor);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A page request: a limit of 1-500 and an optional cursor from the previous page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Page {
    limit: u32,
    cursor: Option<Cursor>,
}

impl Page {
    pub const MAX_LIMIT: u32 = 500;
    pub const DEFAULT_LIMIT: u32 = 100;

    /// Strict constructor, for REST where an out-of-range limit is a 400.
    pub fn new(limit: u32, cursor: Option<Cursor>) -> Result<Self, PageError> {
        if limit == 0 || limit > Self::MAX_LIMIT {
            return Err(PageError::BadLimit);
        }
        Ok(Self { limit, cursor })
    }

    /// Lenient constructor, for MCP where a limit is clamped into range.
    pub fn clamped(limit: u32) -> Self {
        Self {
            limit: limit.clamp(1, Self::MAX_LIMIT),
            cursor: None,
        }
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    pub fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: Self::DEFAULT_LIMIT,
            cursor: None,
        }
    }
}

impl<'de> Deserialize<'de> for Page {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            limit: u32,
            cursor: Option<Cursor>,
        }
        let raw = Raw::deserialize(d)?;
        Self::new(raw.limit, raw.cursor).map_err(serde::de::Error::custom)
    }
}

/// One page of results. `next` is `None` on the last page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub next: Option<Cursor>,
}

impl<T> Paged<T> {
    /// A final page.
    pub fn last(items: Vec<T>) -> Self {
        Self { items, next: None }
    }
}
