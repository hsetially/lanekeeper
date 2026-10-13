//! What an event looks like in the outbox, and its id (D80, S11).
//!
//! An outbox row holds a versioned envelope, `{"v":1,"event":{...}}`, around the [`DomainEvent`] JSON the contract pins. The
//! event carries ids and hashes only, never file content (S12), and the whole envelope is at most [`MAX_EVENT_BYTES`]: a
//! publisher whose event does not fit stores a [`DomainEvent::Resync`] instead (the subscriber refetches), and a reader refuses
//! a row over the cap or of another version rather than guess at it. The id of the row, from the table's identity column, is
//! the one cluster-wide, ordered id of the event: it is the SSE event id, and `Last-Event-ID` resumes from it.

use std::fmt;

use domain::DomainEvent;
use serde::{Deserialize, Serialize};

/// The version of the envelope this build writes and reads.
pub const ENVELOPE_VERSION: u8 = 1;

/// The most an envelope may weigh, in bytes of JSON text (S11).
pub const MAX_EVENT_BYTES: usize = 4096;

/// The longest text a [`EventId`] is read from. `i64::MAX` has 19 digits; the header limit in the API is 64.
const MAX_ID_TEXT: usize = 64;

/// The id of an event: the `id` of its `outbox` row. Positive, and increasing in the order the rows were inserted (not
/// necessarily the order they committed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(i64);

impl EventId {
    /// `None` for zero and negative numbers: an identity column never produces them.
    #[must_use]
    pub fn new(id: i64) -> Option<Self> {
        (id > 0).then_some(Self(id))
    }

    #[must_use]
    pub fn get(self) -> i64 {
        self.0
    }

    /// Parse an id received from outside (the `Last-Event-ID` header): ASCII digits only, no sign, no spaces, at most the
    /// size of an `i64`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty() || text.len() > MAX_ID_TEXT || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse::<i64>().ok().and_then(Self::new)
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why an event could not be turned into an envelope, or an envelope into an event. Carries no event content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("the event is larger than the cap")]
    TooLarge,
    #[error("the envelope has version {0}, which this build does not read")]
    Version(u8),
    #[error("the envelope is not valid")]
    Malformed,
}

#[derive(Serialize)]
struct Outgoing<'a> {
    v: u8,
    event: &'a DomainEvent,
}

#[derive(Deserialize)]
struct VersionOnly {
    v: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Incoming {
    // Already checked by `VersionOnly`; the field has to be named for `deny_unknown_fields`.
    #[allow(dead_code)]
    v: u8,
    event: DomainEvent,
}

/// The envelope for `event`, as JSON text, if it fits in `max` bytes.
pub fn encode(event: &DomainEvent, max: usize) -> Result<String, EnvelopeError> {
    let text = serde_json::to_string(&Outgoing {
        v: ENVELOPE_VERSION,
        event,
    })
    .map_err(|_| EnvelopeError::Malformed)?;
    if text.len() > max {
        return Err(EnvelopeError::TooLarge);
    }
    Ok(text)
}

/// The event in an envelope, if the envelope is of this version, within `max` bytes and well formed.
pub fn decode(text: &str, max: usize) -> Result<DomainEvent, EnvelopeError> {
    if text.len() > max {
        return Err(EnvelopeError::TooLarge);
    }
    let VersionOnly { v } = serde_json::from_str(text).map_err(|_| EnvelopeError::Malformed)?;
    if v != ENVELOPE_VERSION {
        return Err(EnvelopeError::Version(v));
    }
    let Incoming { event, .. } = serde_json::from_str(text).map_err(|_| EnvelopeError::Malformed)?;
    Ok(event)
}
