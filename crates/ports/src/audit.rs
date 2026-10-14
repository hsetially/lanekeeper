//! The tamper-evident audit log port (S9, D38, D56).
//!
//! Every state-changing action writes exactly one [`AuditEvent`], through [`AuditLog::record`] (or, for a
//! change that audits many paths at once, [`AuditLog::record_many`]), in the same transaction as the change. The log is hash-chained: `hash = SHA-256(prev_hash || canonical event JSON)`.
//! [`canonical_json`] and [`chain_hash`] are the single definition of that chain, so the real log (03a), the
//! nightly verifier and the fakes agree.

use async_trait::async_trait;
use domain::{
    Attribution, AuditId, ContentHash, NfsPath, ProposalId, RequestId, ShortText, SwimlaneId, Timestamp,
    UserId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Tx;

/// Who performed the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditActor {
    User {
        user: UserId,
    },
    /// An agent, acting for its swimlane (for example a detected NFS change).
    Agent {
        swimlane: SwimlaneId,
    },
    Sentinel {
        name: ShortText,
    },
    /// A background job or the hub itself.
    System {
        component: ShortText,
    },
}

/// How the action reached the hub (D38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditVia {
    Ui,
    Mcp,
    Sync,
    AgentDetected,
    System,
}

/// What happened. Reads are not audited. New variants are added by contract change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    FileEdited,
    FileUploaded,
    FileDeleted,
    FileReverted,
    /// A change made on NFS outside the tool, seen by a scan (author unknown or attributed, D39, D73).
    FileChangeDetected,
    AttributionUpgraded,
    ServiceRestarted,
    ConfigServerNotified,
    PrRaised,
    ProposalCreated,
    ProposalApproved,
    ProposalRejected,
    ProposalExpired,
    DraftApplied,
    BaselineAdopted,
    DriftMarkedIntentional,
    DriftMarkCleared,
    FlaggedValueRevealed,
    RoleChanged,
    UserStatusChanged,
    AccessRequested,
    AccessApproved,
    AccessDenied,
    GithubTokenStored,
    GithubTokenDeleted,
    DocCreated,
    DocUpdated,
    DocDeleted,
    AgentJoined,
    AgentCertificateRenewed,
    SentinelEnrolled,
    SettingsChanged,
    AutoNotifyChanged,
    SessionSignedIn,
    SessionSignedOut,
    /// A pull request's state changed on GitHub (opened, merged or closed, D77), learned from a `pull_request` webhook and
    /// recorded with the `pr_links` update in one transaction.
    PrStateChanged,
}

/// The most events one [`AuditLog::record_many`] call takes. A bigger batch is refused whole with
/// [`AuditError::BatchTooLarge`], never truncated.
///
/// The chain lock is held from the batch to the end of the caller's transaction, so every other writer waits at
/// least as long as the batch takes. 200 is four times the 50 versions of one agent message, the largest unit of
/// work that audits many paths in one transaction (D93), so a caller never needs to split a unit, and it keeps the
/// longest hold short: the real log writes 200 typical events in about 12 ms (50 in about 4 ms, `record` once per event
/// takes 22 ms for 50 and 104 ms for 200; a release build against a local Postgres, `tests/audit_batch.rs` in
/// `hub-identity` reports it). The one statement carries at most 200 events of at most [`AuditEvent::MAX_DIFF_BYTES`]
/// each: 12.8 MiB at the very worst, a few hundred KiB for real events.
pub const MAX_AUDIT_BATCH: usize = 200;

/// The facts of one audited action. Contains no secret and no file content other than the bounded `diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub at: Timestamp,
    pub actor: AuditActor,
    pub action: AuditAction,
    pub via: AuditVia,
    pub swimlane: Option<SwimlaneId>,
    pub path: Option<NfsPath>,
    pub hash_before: Option<ContentHash>,
    pub hash_after: Option<ContentHash>,
    /// The unified diff text, so that evidence survives garbage collection (D-retention). Flagged values
    /// must already be redacted. At most [`AuditEvent::MAX_DIFF_BYTES`].
    pub diff: Option<String>,
    /// The user's GitHub login, for PR actions.
    pub github_login: Option<ShortText>,
    /// The approving proposal, when the action followed an approval.
    pub approval: Option<ProposalId>,
    pub request_id: Option<RequestId>,
    pub attribution: Option<Attribution>,
}

impl AuditEvent {
    pub const MAX_DIFF_BYTES: usize = 64 * 1024;

    /// A minimal event; set the optional fields you have.
    pub fn new(at: Timestamp, actor: AuditActor, action: AuditAction, via: AuditVia) -> Self {
        Self {
            at,
            actor,
            action,
            via,
            swimlane: None,
            path: None,
            hash_before: None,
            hash_after: None,
            diff: None,
            github_login: None,
            approval: None,
            request_id: None,
            attribution: None,
        }
    }
}

/// An event as stored: its position in the chain and the two chain hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub id: AuditId,
    pub prev_hash: ContentHash,
    pub hash: ContentHash,
    pub event: AuditEvent,
}

/// The `prev_hash` of the first entry.
pub fn genesis_hash() -> ContentHash {
    ContentHash::from_bytes([0; 32])
}

/// The canonical JSON of an event: serde field order (declaration order), no whitespace. Changing the
/// field order or a wire name breaks every stored chain, so a golden test pins it.
pub fn canonical_json(e: &AuditEvent) -> Result<Vec<u8>, AuditError> {
    serde_json::to_vec(e).map_err(|_| AuditError::Encode)
}

/// `SHA-256(prev_hash || canonical event JSON)`.
pub fn chain_hash(prev: &ContentHash, canonical: &[u8]) -> ContentHash {
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update(canonical);
    ContentHash::from_bytes(h.finalize().into())
}

/// Recompute and check a sequence of entries in chain order. Returns the index of the first bad entry.
pub fn verify_chain(entries: &[AuditEntry]) -> Result<(), usize> {
    let mut prev = genesis_hash();
    for (i, e) in entries.iter().enumerate() {
        let Ok(json) = canonical_json(&e.event) else {
            return Err(i);
        };
        if e.prev_hash != prev || e.hash != chain_hash(&prev, &json) {
            return Err(i);
        }
        prev = e.hash;
    }
    Ok(())
}

/// Why an audit event could not be recorded. Carries no event content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuditError {
    /// The diff is larger than [`AuditEvent::MAX_DIFF_BYTES`].
    #[error("audit event is too large")]
    TooLarge,
    /// The transaction belongs to another store.
    #[error("transaction is not backed by the expected store")]
    WrongTx,
    /// The event could not be turned into canonical JSON.
    #[error("audit event could not be encoded")]
    Encode,
    #[error("audit log is unavailable")]
    Unavailable,
    /// `record_many` was given more than [`MAX_AUDIT_BATCH`] events. Nothing was recorded; split the batch.
    #[error("audit batch is too large")]
    BatchTooLarge,
}

#[async_trait]
pub trait AuditLog: Send + Sync + 'static {
    /// Append `e` to the chain inside `tx`. The entry exists only if `tx` commits. Appends are serialised,
    /// so the chain order is the commit order.
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError>;

    /// Append `events` to the chain inside `tx`, in order, as if `record` had been called once per event: the same links
    /// and hashes, one entry per event (rule 8 is unchanged), and the ids come back in input order. An implementation
    /// may do it with less work (one lock acquisition, one read of the chain head and one insert), but the chain that
    /// results must be byte for byte the one that `n` calls of `record` write. The entries exist only if `tx` commits,
    /// and the batch is not interleaved with another writer's entries: its entries are consecutive in the chain.
    ///
    /// - An empty batch is `Ok` with no ids; it takes no lock and does not look at `tx`.
    /// - More than [`MAX_AUDIT_BATCH`] events is [`AuditError::BatchTooLarge`], and nothing is recorded.
    /// - An event that `record` would refuse (`TooLarge`, `Encode`) is refused the same way, before anything of the
    ///   batch is recorded: the transaction stays usable and committing it keeps nothing of the batch. Any other
    ///   error (the store is unavailable) leaves the transaction in an unknown state, and the caller must roll it
    ///   back, as after a failed `record`.
    ///
    /// The default calls `record` once per event, after the checks above, so an implementation that does not override it
    /// keeps working. Overriding is for the cost of a call, not for a different result.
    async fn record_many(
        &self,
        tx: &mut Tx<'_>,
        events: Vec<AuditEvent>,
    ) -> Result<Vec<AuditId>, AuditError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        if events.len() > MAX_AUDIT_BATCH {
            return Err(AuditError::BatchTooLarge);
        }
        // `record` refuses an oversized diff; say so before the first event is recorded, not after some are.
        if events.iter().any(|e| {
            e.diff
                .as_ref()
                .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
        }) {
            return Err(AuditError::TooLarge);
        }
        let mut ids = Vec::with_capacity(events.len());
        for e in events {
            ids.push(self.record(tx, e).await?);
        }
        Ok(ids)
    }
}
