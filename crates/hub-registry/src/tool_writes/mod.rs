//! The registry's side of `ports::ToolWrites` (T1, A10, D73, D107): prompt 06 tells the registry which hash it is about to put on NFS
//! (or which version of a file it is about to delete), and the registry explains the next observation of it as a tool write, with the
//! user as the actor and `certain` as the confidence, instead of an out-of-band change.
//!
//! # Where the pieces are
//!
//! | Piece | Module | What it does |
//! |---|---|---|
//! | `register`, `register_delete` | `register` | One statement in a savepoint of its own, in the caller's transaction: a repeat is a no-op, the bound is checked before the insert, the stamp is the registry's clock |
//! | matching on arrival | `matching` (pure), `crate::ingest` (the SQL) | An observation applied while a registration is visible is explained by the oldest matching one, and the registrations it contradicts are dropped, in the same transaction |
//! | the late commit | `matcher` | A leased job explains an observation that was applied before the registration committed, and drops what expired |
//!
//! The storage is the table `tool_write_registrations` (its own migration, `db/migrations/README.md`), a table of its own: no reader of history can mistake a registration for an
//! observation (`file_observations` has none of it, and `is_version_of` never sees a registered hash).
//!
//! # The rules, and where each one is held
//!
//! - **The savepoint** (the port's MUST): every registration runs inside a `SAVEPOINT` that it opens and releases itself. A statement that
//!   fails is rolled back to it and answers `Unavailable`, and the caller's transaction stays usable for its own audit insert. The
//!   savepoint is raw (`SAVEPOINT`, `RELEASE`, `ROLLBACK TO`) and not sqlx's nested transaction: sqlx 0.8.6 rolls back the **caller's whole
//!   transaction** when a `begin()` inside one is dropped half-way (its guard issues a plain `ROLLBACK` at depth one), and a caller that
//!   began with a raw `BEGIN` gets a second `BEGIN`, so neither is acceptable for something that runs inside a transaction that is not ours.
//! - **A bounded wait** (rule 5): `lock_timeout` and `statement_timeout` are set for the statements of the registration
//!   (`tool_write_timeout`) and **restored before the savepoint is released**, because a `SET LOCAL` outlives a `RELEASE` and would leak
//!   into the caller's next statement; a client-side timeout sits above them.
//! - **The bound** (`tool_write_max_pending`): registrations younger than the TTL are counted up to the bound, `Full` is answered before
//!   anything is inserted, and a repeat of a key that is held is `Ok` without a count. It is soft by at most the registering transactions
//!   in flight (a count, then an insert): a hard bound needs a lock held until the caller's commit, whose order against the audit chain's
//!   lock the registry cannot control from here. `Full` is never the commit's answer: nothing is reserved at commit.
//! - **The stamp** is the registry's injected clock when the call runs, and nothing else (the port takes no time), so there is nothing to
//!   clamp and a registration cannot be dated into the future. The TTL (`ToolWriteEntry::TTL_SECS`) runs from it. Which registration is the
//!   oldest is decided by `id` (the order of insertion), never by the stamp (replicas stamp with their own clocks).
//! - **Order** ("newer", "older") is the path's observation order: `file_observations.id`. Ingestion and the late matcher read the state
//!   immediately before a deletion from the same place (the closest earlier observation of the path that has a hash, [`state_before`]).
//! - **A registration that nothing matches expires** after the TTL (the sweep), is dropped at once by a newer observation of the path that
//!   contradicts it, and a drop is a `DELETE` that is not audited (the write's own audit event is).
//!
//! No path, hash or user is ever in a log line, an error or a metric (S10): the logs say the operation and a fixed word, the metric
//! `registry_tool_writes` a result from a closed set.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{ContentHash, Guid, NfsPath, RequestId, SwimlaneId, UserId};
use ports::{AuditLog, EventBus, ToolWriteError, ToolWrites, Tx};
use sqlx::PgPool;

use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::metrics::RegistryMetrics;

/// The state of a path immediately before a deletion, in the path's observation order: the hash of the closest earlier observation of
/// the path (`$bound` is the deletion's own id, or `i64::MAX` for a deletion that is about to be recorded) that has a hash. A deletion
/// with no earlier observation of the path has no such state and never matches a delete registration.
///
/// Ingestion and the late matcher both embed this text, so that they read it from the same place by construction. The deletion's own
/// `previous_hash` is not consulted: the registry never records one for a deletion (a sync does not delete, Q3), so there is nothing to
/// agree with.
macro_rules! state_before {
    // For a delete registration that ingestion reads: before the next observation to be recorded, so before every one that exists.
    (of_registration) => {
        state_before!(@text "r.swimlane_id", "r.path", "$5")
    };
    // For a deletion observation that the matcher reads: before that observation's own id.
    (of_observation) => {
        state_before!(@text "o.swimlane_id", "o.path", "o.id")
    };
    (@text $swimlane:literal, $path:literal, $bound:literal) => {
        concat!(
            "(SELECT p.hash::bytea FROM file_observations p WHERE p.swimlane_id = ",
            $swimlane,
            " AND p.path = ",
            $path,
            " AND p.id < ",
            $bound,
            " AND p.hash IS NOT NULL ORDER BY p.id DESC LIMIT 1)"
        )
    };
}

mod matcher;
pub(crate) mod matching;
mod register;

pub use matcher::{
    ATTRIBUTE_SQL, LANE_PENDING_SQL, LANES_SQL, MatchReport, OBSERVATIONS_SQL, SWEEP_SQL, ToolWriteMatcher,
};
pub use register::REGISTER_SQL;

/// `ToolWriteEntry::TTL_SECS` in milliseconds: how long a registration waits for its observation, and how long an observation waits for
/// its registration. Pinned to the port's constant by a test.
pub(crate) const TTL_MS: i64 = 15 * 60 * 1000;

/// How much earlier than a registration's stamp the registry may have applied an observation that the registration then explains late.
///
/// **Decided by the implementer (D107), narrower than the port's text.** A registration that commits late takes the newest eligible
/// observation of its hash applied within the TTL; the tool registers *before* it writes, so an observation applied before the registration
/// was even stamped is not the write that it announced, and without this floor a revert to content that someone else wrote ten minutes ago
/// would credit that older observation to the editor (and leave the editor's own change out of band). The floor is the skew allowed between
/// the clocks of two replicas (the stamp is one replica's, the apply time another's); a skew larger than that is a false negative, which is
/// the safe direction.
pub(crate) const LATE_SKEW_MS: i64 = 1_000;

/// The most registrations one read of a chunk's or a swimlane's pending registrations returns (rule 5). The queue's bound is far below it
/// (it is soft by the registering transactions in flight, a few at most); a read that reaches it sees a prefix, which is a safe false
/// negative.
pub(crate) const MAX_READ: i64 = 1_100_000;

/// The pending registrations of some paths of a swimlane, for ingestion: `$1` the swimlane, `$2` the paths, `$3` the cutoff (Unix
/// milliseconds: a registration stamped at or before it has expired), `$4` the most rows, `$5` the bound of [`state_before`] for a delete
/// registration (`i64::MAX`). Oldest first within a path (`id`). The columns are those of [`PendingRow`]; the last one is the state before
/// a deletion, read only for a delete registration. Served by the unique key (`swimlane_id`, `path`, ...).
pub const PENDING_OF_PATHS_SQL: &str = concat!(
    "SELECT r.id, r.path, r.hash::bytea, r.is_delete, r.user_tid, r.user_oid, r.request_id, \
            (EXTRACT(EPOCH FROM r.registered_at) * 1000)::bigint, \
            CASE WHEN r.is_delete THEN ",
    state_before!(of_registration),
    " END \
     FROM tool_write_registrations r \
     WHERE r.swimlane_id = $1 AND r.path = ANY($2::text[]) \
       AND r.registered_at > (timestamptz 'epoch' + ($3::bigint::text || ' milliseconds')::interval) \
     ORDER BY r.path, r.id LIMIT $4"
);

/// One row of [`PENDING_OF_PATHS_SQL`]: id, path, hash, `is_delete`, user tid, user oid, request id, stamp (ms), state before a deletion.
pub(crate) type PendingRow = (
    i64,
    String,
    Vec<u8>,
    bool,
    String,
    String,
    String,
    i64,
    Option<Vec<u8>>,
);

/// Drop registrations by id: the consumed ones and the ones a newer observation contradicts (`$1` the swimlane, `$2` the ids). The id is
/// the primary key, the swimlane makes it a statement about one swimlane's rows only.
pub const DROP_SQL: &str =
    "DELETE FROM tool_write_registrations WHERE swimlane_id = $1 AND id = ANY($2::bigint[])";

/// A registration that no observation has explained yet, as ingestion and the matcher read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pending {
    /// The order of insertion: the lowest id is the oldest registration.
    pub id: i64,
    /// The hash the tool wrote (a write) or the hash of the file it deleted (a delete).
    pub hash: ContentHash,
    pub is_delete: bool,
    pub user: UserId,
    pub request_id: RequestId,
    /// The registry's clock when the registration was made. For the TTL only.
    pub registered_at_ms: i64,
}

fn hash_of(bytes: Vec<u8>) -> Option<ContentHash> {
    <[u8; 32]>::try_from(bytes).ok().map(ContentHash::from_bytes)
}

/// The registration a row holds, or `None` when the row cannot be read back (a user or request id of a shape this code does not know:
/// nothing can be attributed to it, and the sweep drops it at its TTL).
pub(crate) fn pending_of(row: PendingRow) -> Option<(NfsPath, Pending, Option<ContentHash>)> {
    let (id, path, hash, is_delete, tid, oid, request, registered_at_ms, before) = row;
    let user = UserId::new(Guid::parse(&tid).ok()?, Guid::parse(&oid).ok()?);
    Some((
        NfsPath::parse(&path).ok()?,
        Pending {
            id,
            hash: hash_of(hash)?,
            is_delete,
            user,
            request_id: RequestId::parse(&request).ok()?,
            registered_at_ms,
        },
        before.and_then(hash_of),
    ))
}

/// Everything the registry's tool-write side needs from the rest of the hub.
pub struct ToolWriteDeps {
    /// The pool of the `lanekeeper_app` role (the matcher's; `register` runs in the caller's transaction and uses none).
    pub pool: PgPool,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for ToolWriteDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolWriteDeps").finish_non_exhaustive()
    }
}

pub(crate) struct Inner {
    pub db: Db,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

/// `ports::ToolWrites` over Postgres: what prompt 06 calls, in its own transaction, before it sends `WriteFile` or `DeleteFile` to the
/// agent. Cloning shares everything.
#[derive(Clone)]
pub struct PgToolWrites {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PgToolWrites {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgToolWrites").finish_non_exhaustive()
    }
}

impl PgToolWrites {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range.
    pub fn new(deps: ToolWriteDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                bus: deps.bus,
                audit: deps.audit,
                clock: deps.clock,
                config: deps.config,
                metrics: deps.metrics,
            }),
        })
    }

    /// The leased job that explains the observations a registration committed after, and drops the registrations that expired.
    pub fn matcher(&self) -> ToolWriteMatcher {
        ToolWriteMatcher::new(Arc::clone(&self.inner))
    }
}

#[async_trait]
impl ToolWrites for PgToolWrites {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.inner
            .register_one(tx, swimlane, path, new_hash, false, user, request_id)
            .await
    }

    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.inner
            .register_one(tx, swimlane, path, deleted_hash, true, user, request_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ms, ts};

    #[test]
    fn the_ttl_is_the_ports() {
        assert_eq!(
            TTL_MS,
            i64::try_from(ports::ToolWriteEntry::TTL_SECS).unwrap_or(i64::MAX) * 1000
        );
    }

    #[test]
    fn the_statements_use_the_exact_time_expressions_of_the_db_helpers() {
        // The consts are literal text (a const cannot call `ts`), so they are held to the helpers' output here: an exact conversion,
        // integer arithmetic only.
        assert!(PENDING_OF_PATHS_SQL.contains(&ts("$3")));
        assert!(PENDING_OF_PATHS_SQL.contains(&ms("r.registered_at")));
    }

    #[test]
    fn the_state_before_a_deletion_is_read_by_the_path_order() {
        let sql = state_before!(of_observation);
        assert!(sql.contains("p.id < o.id"), "earlier in the path's order: {sql}");
        assert!(
            sql.contains("ORDER BY p.id DESC LIMIT 1"),
            "the closest one: {sql}"
        );
        assert!(sql.contains("p.hash IS NOT NULL"), "one that has a hash: {sql}");
        assert!(PENDING_OF_PATHS_SQL.contains("ORDER BY p.id DESC LIMIT 1"));
    }

    fn row(user_tid: &str, request: &str, hash: Vec<u8>) -> PendingRow {
        (
            7,
            "svc/a.yml".to_owned(),
            hash,
            false,
            user_tid.to_owned(),
            "00000000-0000-4000-8000-000000000001".to_owned(),
            request.to_owned(),
            1_000,
            None,
        )
    }

    #[test]
    fn a_row_that_cannot_be_read_back_is_skipped_and_not_an_error() {
        let tid = "72f988bf-86f1-41af-91ab-2d7cd011db47";
        assert!(pending_of(row(tid, "req-1", vec![1; 32])).is_some());
        assert!(pending_of(row("not-a-guid", "req-1", vec![1; 32])).is_none());
        assert!(pending_of(row(tid, "a request id with spaces", vec![1; 32])).is_none());
        assert!(pending_of(row(tid, "req-1", vec![1; 31])).is_none());
    }
}
