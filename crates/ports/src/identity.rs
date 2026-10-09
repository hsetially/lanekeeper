//! Who is calling: verified identities handed to ports by the edge (mTLS, Entra, Google).
//!
//! These are produced by the code that verifies a certificate or token, and consumed by the ports.
//! They carry no secrets and no raw credentials.

use serde::{Deserialize, Serialize};

use domain::{ShortText, SwimlaneId, UserId};

/// An agent whose client certificate was verified. The certificate SAN is
/// `spiffe://lanekeeper/agent/<swimlane>` (S5, Q11), so an agent can only ever speak for its own swimlane.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentIdentity {
    swimlane: SwimlaneId,
}

impl AgentIdentity {
    /// Only the mTLS edge should call this, after verifying the certificate chain and SAN.
    pub fn new(swimlane: SwimlaneId) -> Self {
        Self { swimlane }
    }

    pub fn swimlane(&self) -> &SwimlaneId {
        &self.swimlane
    }
}

/// The sentinel daemon on an NFS VM, from its verified client certificate
/// (`spiffe://lanekeeper/sentinel/<name>`, Q11). A sentinel identity can never act as an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SentinelIdentity {
    name: ShortText,
}

impl SentinelIdentity {
    /// Only the mTLS edge should call this, after verifying the certificate chain and SAN.
    pub fn new(name: ShortText) -> Self {
        Self { name }
    }

    pub fn name(&self) -> &ShortText {
        &self.name
    }
}

/// The sequence number of a spooled batch the hub has durably applied (D74). Senders delete their
/// spool up to and including this number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AckSeq(u64);

impl AckSeq {
    pub const fn new(seq: u64) -> Self {
        Self(seq)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A user whose Entra access token was verified for MCP: signature, issuer, audience, expiry, the
/// `mcp.access` scope and the tenant. Whether the user is active is decided later by `Users`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedUser {
    pub id: UserId,
    pub email: Option<ShortText>,
    pub display_name: Option<ShortText>,
}

/// A verified Google ID token, presented by an agent when it joins (S5). The caller matches `email`
/// (the node service account) against the swimlane's registered service account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleIdentity {
    pub subject: ShortText,
    pub email: ShortText,
    /// The audience that was verified, echoed for audit.
    pub audience: ShortText,
}
