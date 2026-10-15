//! The write pipeline (T1, D68). Every change goes through the same steps, in this order:
//!
//! 1. [`authorize`]: the user is active and has the role the action needs, read from the directory (never trusted from the
//!    request). Nothing is claimed, sent or recorded for a user who may not act (S4).
//! 2. [`claim`]: the idempotency key is claimed; a retry replays the stored response, another request with the key is refused.
//! 3. [`lock`]: the swimlane's baseline is confirmed (D2), or the answer is `Locked` (423).
//! 4. [`validate`]: size, deny globs, the engine's parse, the value-based secret scan (S13); parse errors and secrets block,
//!    duplicate keys are warnings.
//! 5. [`approval`]: a user who `requires_approval` gets a pending proposal and nothing is written (D37).
//! 6. [`execute`]: the agent is sent the change with the expected hash; then one transaction records the audit event, registers
//!    the tool write, publishes the events and stores the response. No database connection is held while the agent works.
//!
//! A step ends the request with a [`Done`] (an outcome and what to do with the claim) or an [`Abort`] (an error).

use domain::WriteOutcome;
use ports::WriteError;

pub(crate) mod approval;
pub(crate) mod authorize;
pub(crate) mod claim;
pub(crate) mod current;
pub(crate) mod execute;
pub(crate) mod follow;
pub(crate) mod lock;
pub(crate) mod validate;

/// How a request ended, and what becomes of its claim.
#[derive(Debug)]
pub(crate) enum Done {
    /// The claim already holds this response (the transaction that recorded the change stored it).
    Stored(WriteOutcome),
    /// Store this response with the claim: the same request would get the same answer (a conflict, a rejection).
    Store(WriteOutcome),
    /// Answer this, and free the key: it is not an outcome of the request (a lock, an agent that could not be reached).
    Release(WriteOutcome),
}

/// A request that ended with an error. `keep` is set when the change may have reached the agent: the claim stays with its
/// intent, and the retry with the same key or the reconcile job finds out whether it landed.
#[derive(Debug)]
pub(crate) struct Abort {
    pub error: WriteError,
    pub keep: bool,
}

impl Abort {
    pub fn release(error: impl Into<WriteError>) -> Self {
        Self {
            error: error.into(),
            keep: false,
        }
    }

    pub fn keep(error: impl Into<WriteError>) -> Self {
        Self {
            error: error.into(),
            keep: true,
        }
    }
}

impl From<crate::error::Internal> for Abort {
    fn from(e: crate::error::Internal) -> Self {
        Self::release(e)
    }
}

pub(crate) type Step<T> = Result<T, Abort>;
