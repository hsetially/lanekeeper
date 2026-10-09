//! Who changed a file and how sure we are (D73). Upgraded later, never downgraded.

use serde::{Deserialize, Serialize};

use crate::{AttributionSource, Confidence, JobRef, ShortText, Timestamp, UserId};

/// The party an attribution points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    /// An application user (a tool write, or an OS Login user mapped to one, Q32).
    User { user: UserId },
    /// An OS Login user with no mapped application user.
    OsLogin { login: ShortText },
    /// A sync Job.
    Job { job: JobRef },
}

/// The facts behind an attribution: what the sentinel or the hub saw.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Evidence {
    pub executable: Option<ShortText>,
    /// The original login identity, when it differs from the effective user.
    pub login_identity: Option<ShortText>,
    pub observed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub source: AttributionSource,
    pub confidence: Confidence,
    pub actor: Option<Actor>,
    pub evidence: Evidence,
}

impl Attribution {
    /// No evidence at all.
    pub fn unknown() -> Self {
        Self {
            source: AttributionSource::Unknown,
            confidence: Confidence::Low,
            actor: None,
            evidence: Evidence::default(),
        }
    }

    /// True when `later` may replace `self`: attribution is only ever upgraded.
    pub fn may_upgrade_to(&self, later: &Attribution) -> bool {
        later.confidence > self.confidence
    }
}
