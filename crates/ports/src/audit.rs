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
    /// A proposal's file changed before an admin decided, so it can no longer be applied (D37). Written by the
    /// approval re-check and recorded with the `stale` status in one transaction.
    ProposalStale,
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
/// [`AuditError::BatchTooLarge`], never truncated. The other bound on a batch is [`MAX_AUDIT_BATCH_BYTES`]; use
/// [`split_batches`] to respect both.
///
/// 200 is four times the 50 versions of one agent message, which is the registry's chunk (it audits the paths of one
/// message in one transaction), so a caller does not have to split a unit of that size by count. What bounds the time
/// the chain lock is held is the byte budget, not this number.
pub const MAX_AUDIT_BATCH: usize = 200;

/// The most bytes of canonical JSON ([`canonical_json`]), summed over the events, that one [`AuditLog::record_many`] call takes. A
/// batch over it is refused whole with [`AuditError::BatchTooLarge`], before the chain lock is taken and before anything is
/// written. [`canonical_size`] is the size of one event.
///
/// 1 MiB, for two reasons. It is at least [`MAX_AUDIT_EVENT_BYTES`] (399 KB, the largest single event: a 64 KiB diff
/// of control characters is six bytes per byte in canonical JSON), so every event that `record` accepts is accepted alone
/// by `record_many`; the const assertion below holds that. And it bounds the work done under the chain lock, which is held
/// from the batch to the end of the caller's transaction and which every other audited write waits for (their lock timeout
/// is 5 s): hashing the batch, one insert of it, and the commit. At the budget the real log held the lock for 19 to 58 ms,
/// whatever the shape of the batch (a release build on a 4-vCPU VM, Postgres 16.15 on the same machine; `docs/interfaces.md` has
/// the shapes and how it was measured), against the 5 s of the other writers. Without the budget, 200 events of control
/// characters were about 76 MiB and seconds of work under the lock.
pub const MAX_AUDIT_BATCH_BYTES: usize = 1024 * 1024;

/// The size in bytes of the canonical JSON of the largest event that can exist, derived from the limits of its fields; the
/// test `the_largest_event_is_what_the_constant_says` builds that event and checks the number to the byte.
///
/// - `diff` is untrusted text of at most [`AuditEvent::MAX_DIFF_BYTES`] bytes that may hold control characters, and
///   the canonical JSON writes most of them as `\u00XX`: six bytes for one. That is 393,218 of the bytes.
/// - Every other text field excludes control characters: a [`ShortText`] is at most 256 bytes, a path at most 1,024 bytes in
///   components of at most 255, a swimlane id 63, a request id 128, and a quote or a backslash costs two bytes, so the
///   worst text is all quotes or backslashes.
/// - Every `Option` is `Some`, a number is its longest form, and the names are the longest of their enum.
/// - A [`Timestamp`] is any `i64`, so this counts `-9223372036854775808` for the time. The real log's `timestamptz` column holds
///   no such time and refuses the event, and the earliest time it holds is four characters shorter: the largest event it stores
///   is 398,669 bytes.
pub const MAX_AUDIT_EVENT_BYTES: usize = max_event_json_bytes();

const _: () = assert!(
    MAX_AUDIT_BATCH_BYTES >= MAX_AUDIT_EVENT_BYTES,
    "a single event that `record` accepts must be accepted alone by `record_many`"
);

/// The bytes of a JSON string of `content` bytes, each of which costs at most `per_byte` bytes, with its quotes.
const fn json_string(content: usize, per_byte: usize) -> usize {
    2 + content * per_byte
}

/// The bytes of an object's braces, commas and `"key":` parts, without its values.
const fn object_overhead(keys: &[&str]) -> usize {
    let mut n = 2 + keys.len().saturating_sub(1);
    let mut i = 0;
    while i < keys.len() {
        n += keys[i].len() + 3;
        i += 1;
    }
    n
}

const fn max2(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

/// See [`MAX_AUDIT_EVENT_BYTES`].
const fn max_event_json_bytes() -> usize {
    // No control characters, so the worst character is a quote or a backslash, at two bytes for one.
    let short_text = json_string(256, 2);
    // `-9223372036854775808`.
    let int = 20;
    let user_id = json_string(36 + 1 + 36, 1);

    let actor = max2(
        max2(
            object_overhead(&["kind", "user"]) + json_string(4, 1) + user_id,
            object_overhead(&["kind", "swimlane"]) + json_string(5, 1) + json_string(63, 1),
        ),
        max2(
            object_overhead(&["kind", "name"]) + json_string(8, 1) + short_text,
            object_overhead(&["kind", "component"]) + json_string(6, 1) + short_text,
        ),
    );

    // A path of 1,024 bytes with components of at most 255: the fewest separators is one fewer than the fewest components,
    // and everything else is a quote.
    let components = (1024 + 1 + 255) / (255 + 1);
    let quotes = 1024 - (components - 1);
    let path = json_string(quotes * 2 + (components - 1), 1);

    let attribution_actor = max2(
        max2(
            object_overhead(&["kind", "user"]) + json_string(4, 1) + user_id,
            object_overhead(&["kind", "login"]) + json_string(8, 1) + short_text,
        ),
        object_overhead(&["kind", "job"])
            + json_string(3, 1)
            + object_overhead(&["name", "uid"])
            + json_string(253, 1)
            + json_string(64, 1),
    );
    let evidence =
        object_overhead(&["executable", "login_identity", "observed_at"]) + short_text + short_text + int;
    let attribution = object_overhead(&["source", "confidence", "actor", "evidence"])
        + json_string(14, 1)
        + json_string(7, 1)
        + attribution_actor
        + evidence;

    object_overhead(&[
        "at",
        "actor",
        "action",
        "via",
        "swimlane",
        "path",
        "hash_before",
        "hash_after",
        "diff",
        "github_login",
        "approval",
        "author",
        "request_id",
        "attribution",
    ]) + int
        + actor
        + json_string(25, 1)
        + json_string(14, 1)
        + json_string(63, 1)
        + path
        + 2 * json_string(64, 1)
        + json_string(AuditEvent::MAX_DIFF_BYTES, 6)
        + short_text
        + int
        + user_id
        + json_string(128, 1)
        + attribution
}

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
    /// Who proposed the change, when the action followed an approval: the event's `actor` is then the approver
    /// (D38). Omitted from the canonical JSON when `None`, so every event written before this field existed keeps
    /// its bytes and its hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<UserId>,
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
            author: None,
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
    /// `record_many` was given more than [`MAX_AUDIT_BATCH`] events, or events whose canonical JSON adds up to more than
    /// [`MAX_AUDIT_BATCH_BYTES`]. Nothing was recorded; split the batch ([`split_batches`] does it for both limits).
    ///
    /// New with the batch methods: an exhaustive `match` on `AuditError` (and on `BusError`, which gained a variant of the same
    /// name) needs an arm for it.
    #[error("audit batch is too large")]
    BatchTooLarge,
}

/// The bytes of the canonical JSON of `e` ([`canonical_json`]): what `record_many` counts against [`MAX_AUDIT_BATCH_BYTES`].
///
/// # Errors
/// [`AuditError::TooLarge`] when the diff is over [`AuditEvent::MAX_DIFF_BYTES`], the one limit `record` also checks first,
/// and [`AuditError::Encode`] when the event cannot be encoded.
pub fn canonical_size(e: &AuditEvent) -> Result<usize, AuditError> {
    if diff_too_large(e) {
        return Err(AuditError::TooLarge);
    }
    Ok(canonical_json(e)?.len())
}

fn diff_too_large(e: &AuditEvent) -> bool {
    e.diff
        .as_ref()
        .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
}

/// Split `events` into batches that `record_many` accepts, in order and without reordering or dropping any: each has at most
/// [`MAX_AUDIT_BATCH`] events and at most [`MAX_AUDIT_BATCH_BYTES`] of canonical JSON, and each is filled as far as both limits
/// allow before the next begins. No events give no batches. Pure: nothing is recorded, so an error here leaves nothing behind.
///
/// Call `record_many` once per batch, and keep the batches of one transaction few: the chain lock is held from the first call
/// to the end of the caller's transaction (see [`AuditLog::record_many`]), so the transaction is as long as all its batches.
///
/// This canonicalises each event once to count it, and `record_many` does it again to record it. That is the price of not
/// guessing the sizes; it is the work of one `record_many` call, done outside the chain lock.
///
/// **It is CPU-bound and synchronous**, and it encodes every event with no early stop (the events are already in memory, and
/// each is bounded: a few KB, or up to [`MAX_AUDIT_EVENT_BYTES`] for one made of control characters, so a list of 200 of the
/// largest is tens of milliseconds in a release build and seconds in a debug one). A caller on the async runtime calls it from
/// the blocking pool (`tokio::task::spawn_blocking`), as `record_many` does for its own encoding and hashing (AGENTS.md rule 4).
///
/// # Errors
/// What [`canonical_size`] says about the first event that cannot be recorded, whatever its place, and
/// [`AuditError::TooLarge`] for an event that is alone over [`MAX_AUDIT_BATCH_BYTES`] (no valid event is).
pub fn split_batches(events: Vec<AuditEvent>) -> Result<Vec<Vec<AuditEvent>>, AuditError> {
    let mut batches: Vec<Vec<AuditEvent>> = Vec::new();
    let mut bytes = 0_usize;
    for e in events {
        let size = canonical_size(&e)?;
        if size > MAX_AUDIT_BATCH_BYTES {
            return Err(AuditError::TooLarge);
        }
        match batches.last_mut() {
            Some(open) if open.len() < MAX_AUDIT_BATCH && bytes + size <= MAX_AUDIT_BATCH_BYTES => {
                open.push(e);
                bytes += size;
            }
            _ => {
                batches.push(vec![e]);
                bytes = size;
            }
        }
    }
    Ok(batches)
}

/// The checks of `record_many` that need no store, in the order they are made, for the default implementation: every event is
/// checked before any is recorded. An empty batch passes (the caller returns early), and so does a batch of valid events within
/// both limits.
fn check_batch(events: &[AuditEvent]) -> Result<(), AuditError> {
    if events.len() > MAX_AUDIT_BATCH {
        return Err(AuditError::BatchTooLarge);
    }
    let mut bytes = 0_usize;
    for e in events {
        bytes += canonical_size(e)?;
        if bytes > MAX_AUDIT_BATCH_BYTES {
            return Err(AuditError::BatchTooLarge);
        }
    }
    Ok(())
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
    /// - More than [`MAX_AUDIT_BATCH`] events, or canonical JSON that adds up to more than [`MAX_AUDIT_BATCH_BYTES`], is
    ///   [`AuditError::BatchTooLarge`], and nothing is recorded. Every event that `record` accepts is accepted alone.
    ///   [`split_batches`] cuts a list of events into batches that respect both limits.
    /// - An event that `record` would refuse (`TooLarge`, `Encode`) is refused the same way, before anything of the
    ///   batch is recorded: the transaction stays usable and committing it keeps nothing of the batch. So is `WrongTx`, which
    ///   is returned before any database work. Any other error (the store is unavailable) leaves the transaction in an
    ///   unknown state, and the caller must roll it back, as after a failed `record`.
    /// - All of this is checked before the chain lock is taken, so a refused batch never makes another writer wait.
    ///
    /// **The chain lock is held to the end of the caller's transaction,** as for `record`, not to the end of this call: every
    /// other audited write waits until the caller commits or rolls back (their lock timeout is 5 s). So call it last in the
    /// transaction, do nothing slow after it, and do not pile several batches into one transaction: the transaction holds
    /// the lock for all of them.
    ///
    /// A batch of one event is no faster than `record`: use `record` for a single event. The gain is for many events, in
    /// one lock acquisition, one read of the head and one insert instead of three round trips for each.
    ///
    /// **A wrapper or decorator of an `AuditLog` must forward this method** (and override it too, if it changes what
    /// `record` records or checks, so that the two stay one chain). The default calls `record`, so a wrapper that
    /// forwards only `record` still works, but it silently falls back to the slow loop of one lock, head read and insert per
    /// event.
    ///
    /// The default calls `record` once per event, after every event has been checked (on the blocking pool, so that encoding
    /// a full batch never holds an async worker), so an implementation that does not override it keeps working. Overriding is
    /// for the cost of a call, not for a different result.
    async fn record_many(
        &self,
        tx: &mut Tx<'_>,
        events: Vec<AuditEvent>,
    ) -> Result<Vec<AuditId>, AuditError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        // All of it before the first `record`: a batch with one bad event records none of its events. Encoding up to the byte
        // budget is CPU-bound, so it runs on the blocking pool and not on the async runtime (AGENTS.md rule 4), as
        // `record_many` of the real log does; the events go in and come back out, so nothing is cloned.
        let events = tokio::task::spawn_blocking(move || check_batch(&events).map(|()| events))
            .await
            .map_err(|_| AuditError::Unavailable)??;
        let mut ids = Vec::with_capacity(events.len());
        for e in events {
            ids.push(self.record(tx, e).await?);
        }
        Ok(ids)
    }
}
