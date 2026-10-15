//! The tool-write registration port (D73): the write path tells the registry which hash it is about to put on NFS, so
//! that the next observation of that hash is attributed as a tool write with certainty, and not guessed from NFS
//! activity.
//!
//! Called by prompt 06 (`hub-writes`), implemented by prompt 05 (`hub-registry`). Like [`crate::AuditLog::record`], it
//! takes the caller's open transaction: the registration exists only if the transaction that carries the write's
//! audit event commits.
//!
//! # A failed registration must not abort the caller's transaction: the savepoint is the implementation's
//!
//! `register` and `register_delete` run inside the caller's open transaction. In Postgres any failed statement aborts the
//! whole transaction: every later statement, the caller's audit insert included, fails with SQLSTATE `25P02` until a
//! `ROLLBACK`. A registration that fails (a deadlock, a serialization failure, a lock timeout) would therefore leave the
//! file operation done and its audit event unwritable, against the rule that every state change writes exactly one audit
//! event in the same transaction (AGENTS rule 8, S9).
//!
//! So an implementation **MUST run every statement of a registration inside a savepoint** (a nested transaction) that it
//! opens and releases itself, so that any failure rolls back only its own work, leaves the caller's transaction usable, and
//! is returned as `Unavailable`. With the savepoint, a caller that gets `Full` or `Unavailable` can go on and record the
//! operation's audit event in the same transaction (see [`ToolWriteError`]).
//!
//! - **The MUST is the implementation's, because it is the contract.** The conformance suites cannot test it: the in-memory
//!   fakes have no failed state, so an implementation that forgets the savepoint passes every suite and fails only in
//!   production. The registry (05) must test it against a real Postgres: a registration statement that fails (forced, for
//!   instance, by a lock timeout), then the audit insert and the commit still succeed in the same transaction, and the
//!   registration is absent.
//! - **Callers SHOULD ALSO wrap the call.** A caller that holds the sqlx transaction can open a savepoint of its own around
//!   the registration (`Tx::from_pg(&mut *savepoint)`) and roll it back on `Err`. Callers (prompt 06) should do that as
//!   defence in depth, because it does not depend on each implementor.
//! - **A lost connection is not covered.** The caller's transaction is gone with the connection, and no savepoint helps. The
//!   change then appears as an out-of-band change, and the audit event cannot be recorded in that transaction either (the
//!   caller's problem, as for any lost connection).
//! - **A pitfall: `SET LOCAL` inside the savepoint.** A `SET LOCAL lock_timeout` made inside the savepoint is reverted by a
//!   rollback to it but **persists after `RELEASE`**, into the caller's later statements (measured: 300 ms leaking into the
//!   caller's audit insert). A registration needs a bounded wait (AGENTS rule 5), so the implementation must restore the
//!   previous value after it releases the savepoint, or use another bound.
//!
//! # Order, time and who stamps what
//!
//! - **Order.** "Newer" and "older" in this port always mean later and earlier in the **path's observation order**: the
//!   order in which the registry applied observations of that swimlane and that path. They never mean wall-clock time or
//!   ingestion time compared across paths.
//! - **The clock.** The port takes no time. The registry stamps a registration with its own (injected) clock when `register`
//!   or `register_delete` runs, and never takes a time from a caller; it must clamp what it stores to that clock, so that a
//!   registration cannot be dated into the future, never age out of the sweep and take a place in the bounded queue for
//!   good. The stamp is used for the **TTL only**. Wherever this port speaks of the TTL ([`ToolWriteEntry::TTL_SECS`]) it is
//!   measured on the registry's clock: a registration's from its stamp, and an observation's from the moment the registry
//!   **applied** it, never from the agent's `observed_at`, which is the agent's clock. With several hub replicas each stamps
//!   with its own clock; skew between them is harmless for a 15-minute TTL, and is assumed small relative to it.
//! - **The TTL of a registration** therefore runs from the registry's clock when `register` or `register_delete` ran. A
//!   registry cannot know the commit time, and a caller-owned transaction has no commit hook, so a registration expires at
//!   most the time the transaction took (milliseconds) before `TTL_SECS` after its commit, which is the safe direction.
//! - **The key** of a registration is (swimlane, path, hash, request); a delete registration is a registration of its own
//!   kind, with the hash of the file it deleted (swimlane, path, deleted hash, request). It does not include the user: the
//!   same key from another user is a no-op that **keeps the first user**. This is an **accepted risk**: it holds only
//!   because a request id is made by the hub and is unique per request (never taken from a client), and nothing enforces
//!   that uniqueness; the registry's threat model records the risk.
//!
//! # Which observation a registration explains
//!
//! These rules hold for both kinds of registration.
//!
//! - **Eligibility.** An observation is eligible only if it is not already attributed as a tool write. A registration may
//!   **upgrade** a weaker attribution (`unknown`, `nfs_client`, `sentinel_login`, `fs_owner_hint`, `sync_job`) to
//!   `tool_write` with confidence `certain`, and never replaces another tool-write attribution.
//! - **One registration per observation.** When several registrations are pending for one observation (different requests,
//!   the same path), the **oldest** is the one that consumes it: the one with the lowest `id` (assigned by the registry's
//!   store, in the order the registrations were inserted; the stamp is for the TTL only, and stamps can disagree across
//!   replicas), so an observation never has two actors. The others stay pending until their own matching observation, the
//!   TTL or an invalidation.
//! - **One observation per late registration.** A registration that commits after observations attributes at most one.
//!
//! # Ordering: the observation can come first
//!
//! A registration becomes visible when its transaction commits, and 06 commits after the file is written (the
//! `WriteFile` command has returned). So the agent can see the new file, and report it, before the registration is
//! visible. The registry (05) must therefore also match an observation that arrives before the commit becomes visible
//! (a late-committed registration): it keeps an observation it could not attribute for at least
//! [`ToolWriteEntry::TTL_SECS`] and attributes it as a tool write when a matching registration commits within that
//! time. Matching is by swimlane, path and hash.
//!
//! - A registration that is already visible when its hash is first observed at its path is consumed by that observation
//!   (the oldest pending one, if several).
//! - A registration that commits after observations attributes **at most one** of them: the newest eligible observation of
//!   that exact hash at that path, still within [`ToolWriteEntry::TTL_SECS`] of when the registry applied it. Older
//!   observations of the same hash stay as they were.
//!
//! # A registration that nothing matches expires
//!
//! An unmatched registration is not kept forever, because it could be attributed to a write that was not the tool's and
//! because the queue is bounded:
//!
//! - the registry drops it [`ToolWriteEntry::TTL_SECS`] after its stamp (see "The clock" above);
//! - a newer observation of the same path (later in the path's observation order than the registration became visible)
//!   invalidates it at once, when that observation has another hash or says the file is gone: that state replaced the one
//!   the registration was for, so a later write of the registered hash is not the tool's write;
//! - dropping a registration is not an error and is not audited; the write's own audit event already is.
//!
//! These are rules the registry implements. The port has no way to observe, so they are tested where the registry is
//! implemented (05), and the conformance suites here cannot see them.
//!
//! # Deletes: [`ToolWrites::register_delete`]
//!
//! A delete is a registration of its own, with the same shape and the same `Full` rule as one for a write, pinned to content
//! like a write is: it carries `deleted_hash`, the hash of the file that was deleted (the expected hash that the delete
//! carried anyway, AGENTS rule 11). It has matching rules of its own:
//!
//! - it is made in the caller's transaction, becomes visible at the commit, and a rollback leaves nothing behind;
//! - the same (swimlane, path, deleted hash, request) again is a no-op, in the same transaction or a later one; a delete
//!   registration is **distinct** from a hash registration of the same path, hash and request (registering one never
//!   replaces, merges with or repeats the other), and a delete of another version of the file is another registration;
//! - **a deletion matches only the version it deleted.** A deletion observation of the path matches a delete registration
//!   only if the state of the path immediately before that deletion, in the path's observation order, was the hash
//!   `deleted_hash`. A deletion of any other version never matches;
//! - a delete registration that is already visible when such a deletion is first observed is **consumed by that deletion
//!   observation** (the oldest pending one, if several): the registry attributes it as `ObservationSource::ToolWrite`, with
//!   this user as the actor and confidence `certain`;
//! - a delete registration that commits **after** observations attributes **at most one** observation: the newest
//!   observation of that path in that swimlane, which must be a deletion matching as above, that no hash observation of the
//!   path follows, that is eligible, and that is still within [`ToolWriteEntry::TTL_SECS`] of when the registry applied it.
//!   Older deletions of the path stay exactly as they were (attribution is never downgraded, and never moved to another
//!   observation). Example: a person deletes `svc/a.yml` over NFS (deletion O0), someone recreates it out of band (O1, a
//!   hash), and an editor deletes it through the tool (O2), which the agent reports before the editor's transaction commits.
//!   The registration attributes O2 and nothing else, and O0 stays unattributed, whatever its age; had O2 deleted a version
//!   other than `deleted_hash`, nothing would be attributed;
//! - a hash observation that comes **later** in the path's observation order than the deletion invalidates a delete
//!   registration at once (the file came back, so a later disappearance is not the tool's delete). With no deletion
//!   observed yet, a hash observation that the registry applies after the registration became visible does. A hash
//!   observation that is **earlier** in the path's order never does;
//! - it expires [`ToolWriteEntry::TTL_SECS`] after its stamp, as a hash registration does (see "The clock" above);
//! - it takes a place in the same bounded queue as a hash registration, so `Full` counts both, and comes only from
//!   `register_delete`, never from the commit;
//! - dropping it is not an error and is not audited.
//!
//! **Where the registry reads "the state immediately before a deletion".** The path's observation order is the registry's apply
//! order for the path (`file_observations.id`). The state immediately before a deletion is the hash of the closest earlier
//! observation of that path in that order that has a hash; the deletion observation's own `previous_hash`, where the registry
//! recorded it, must agree. Ingestion and the late matcher MUST read it from the same place. A deletion with no earlier
//! observation of the path never matches (safe), and a registry whose last-known state is stale (the tool deleted a version
//! the agent had not yet reported) gets a safe false negative: nothing is attributed.
//!
//! **What remains.** The registry's threat model (prompt 05, T10) carries these residuals:
//!
//! - The same content, recreated by someone else inside the window, is indistinguishable from the tool's. For a delete: the
//!   same content recreated and deleted again inside the window is attributed to the tool's registration. It applies to hash
//!   registrations too: if someone else writes H, then H', then H again inside the window, a late registration takes the
//!   newest eligible observation of H.
//! - Concurrent writers of identical content: if the later registration commits first, it takes the observation and the
//!   earlier one gets nothing ("oldest pending" only orders registrations that are visible together). Both are tool users,
//!   so this is a swap between tool users, not a misattribution to a non-tool actor.
//! - The assumption that the newest deletion at commit time is the tool's holds inside the duration of the registering
//!   transaction.
//!
//! Like the rules above, the matching rules of a delete are the registry's, and the conformance suites cannot observe them.
//!
//! A registry that has not taken the method keeps today's behaviour: the delete is an out-of-band change, with the author
//! unknown, on top of the write path's own audit event. See the default of [`ToolWrites::register_delete`], which is
//! exactly that.

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};

use crate::Tx;

/// Why a registration could not be made. Carries no path and no hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ToolWriteError {
    /// The transaction belongs to another store.
    #[error("transaction is not backed by the expected store")]
    WrongTx,
    /// Too many registrations are waiting to be matched with an observation (every queue is bounded: this is the registry's
    /// rule, soft by at most the number of registering transactions in flight). It is the answer of `register` and
    /// `register_delete`, never of the commit: a registration the registry accepted is not dropped for lack of room, and a
    /// repeat of one it already holds is a no-op, not `Full`.
    ///
    /// A caller gets `Full` or `Unavailable` after the file operation has already happened. It must not roll back the
    /// operation's own audit event or hide it: a registration that failed only means that the change may appear as an
    /// out-of-band change and be attributed by the other means (the sentinel, the NFS client). It can rely on its
    /// transaction still being usable, because an implementation runs its statements in a savepoint of its own (see the
    /// module docs): the audit insert and the commit go on in the same transaction.
    #[error("too many tool writes are waiting to be observed")]
    Full,
    /// The registry could not take the registration (a deadlock, a serialization failure, a lock timeout...). The
    /// implementation has rolled back its own work to its savepoint, so, as for `Full`, the caller's transaction is usable
    /// and it keeps the operation's own audit event. A lost connection is not such a case: the transaction is gone with the
    /// connection, the change appears as an out-of-band change, and the audit event cannot be recorded in that transaction
    /// either (the caller's problem, as for any lost connection).
    #[error("tool write registry is unavailable")]
    Unavailable,
}

/// A registration as stored: what the registry will match the next observation against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolWriteEntry {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub new_hash: ContentHash,
    pub user: UserId,
    pub request_id: RequestId,
}

impl ToolWriteEntry {
    /// How long a registration waits for its observation, and how long an observation waits for its registration: 15
    /// minutes. Long enough for the agent to report a written file and for a commit that comes late; short enough that a
    /// stale entry cannot be attributed to a later write or fill the bounded queue.
    pub const TTL_SECS: u64 = 15 * 60;
}

/// A delete registration as stored: what the registry will match the next observation that `path` is gone against. It is
/// [`ToolWriteEntry`] with the hash of the file that was deleted instead of the hash that was written, and a separate type so
/// that `ToolWriteEntry::new_hash` keeps meaning "the hash that was written".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDeleteEntry {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    /// The hash of the file the tool deleted: the expected hash that every write and delete carries (AGENTS rule 11).
    pub deleted_hash: ContentHash,
    pub user: UserId,
    pub request_id: RequestId,
}

#[async_trait]
pub trait ToolWrites: Send + Sync + 'static {
    /// Register that `user` wrote `new_hash` to `path` in `swimlane`, through request `request_id`, inside `tx`.
    ///
    /// - The registration exists only if `tx` commits; a rolled-back transaction leaves nothing behind.
    /// - Registering the same (swimlane, path, hash, request) again is a no-op, so a retry that completes an
    ///   interrupted write does not register twice. The user is not part of the key: the same key from another user is
    ///   a no-op too, and keeps the first user (an accepted risk that rests on the uniqueness of hub-made request ids).
    /// - After the commit, the registry attributes the next observation of this hash at this path as
    ///   `ObservationSource::ToolWrite`, with this user as the actor and confidence `certain` (D73). The matching and the
    ///   expiry rules are in the module docs.
    /// - The registry stamps the registration with its own clock and clamps it to that clock; the time is never the
    ///   caller's.
    /// - **The implementation MUST run its statements inside a savepoint that it opens itself**, so that a failure rolls
    ///   back only its own work, leaves `tx` usable and is returned as `Unavailable` (module docs: a failed statement
    ///   aborts a Postgres transaction, the suites cannot test the savepoint because the fakes have no failed state, and
    ///   callers SHOULD ALSO wrap the call in a savepoint of their own).
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError>;

    /// Register that `user` deleted `path` in `swimlane`, through request `request_id`, inside `tx`, where `deleted_hash` is
    /// the hash of the file that was deleted (the expected hash the delete carried), so that the observation that this
    /// version of the file is gone is attributed to the tool and not to an unknown author.
    ///
    /// The rules are those of [`register`](Self::register), for a delete (see "Deletes" in the module docs):
    ///
    /// - the registration exists only if `tx` commits; a rolled-back transaction leaves nothing behind;
    /// - registering the same (swimlane, path, deleted hash, request) again is a no-op, and is never `Full`; a delete
    ///   registration is not the same registration as a hash registration of that path, hash and request, and a delete of
    ///   another version of the file is another registration. The user is not part of the key: the same key from another user
    ///   is a no-op too, and keeps the first user;
    /// - a deletion matches only the version it deleted: the state of the path immediately before the deletion, in the
    ///   path's observation order, must have been `deleted_hash`. A registration that is visible when such a deletion is
    ///   first observed is consumed by it; one that commits after observations attributes at most one, the newest
    ///   observation of the path, which must be such a deletion that no hash observation follows, within
    ///   [`ToolWriteEntry::TTL_SECS`] of when the registry applied it; a later hash observation invalidates it, an earlier
    ///   one never does (the module docs have the rules, an example and what remains);
    /// - it expires [`ToolWriteEntry::TTL_SECS`] after the registry stamped it with its own clock;
    /// - the implementation MUST run its statements inside a savepoint, as `register` must;
    /// - after the commit, the registry attributes that observation as `ObservationSource::ToolWrite`, with this user as
    ///   the actor and confidence `certain` (D73).
    ///
    /// # The default registers nothing, and says `Ok`
    ///
    /// The provided body does nothing and returns `Ok(())`. It exists so that this method is additive: an implementation
    /// that has only `register` (every one that existed before this method) keeps compiling and behaving as it did, which
    /// means a delete made through the tool stays an **out-of-band change** (a `FileChangeDetected` with an unknown author,
    /// on top of the write path's own audit event). That is the safe degradation: the delete is still seen and audited, it is
    /// just not attributed. The other defaults were rejected on purpose: an error would fail every delete of every
    /// implementation that has not taken the method, and a new `ToolWriteError` variant would break exhaustive matches in
    /// the callers.
    ///
    /// Two consequences, both silent, so both are tested:
    ///
    /// - The default does not look at `tx`: it answers `Ok(())` to a transaction of another store, where `register` answers
    ///   `WrongTx`. An implementation that registers deletes must check it.
    /// - **A wrapper or decorator of this trait must forward this method.** One that implements only `register` inherits
    ///   the default, and the deletes sent through it are dropped without a sound, whatever the wrapped implementation does.
    ///
    /// The conformance suite (feature `conformance`) holds an implementation that registers deletes to the rules above, but
    /// only by convention: **an implementation that overrides this method MUST make its probe return `Some(..)` from
    /// `ToolWritesProbe::registered_deletes` and run `tool_writes_with_deletes` (and `tool_writes_bounded`)**. A probe left at
    /// the default `None` passes `tool_writes` with every delete rule unchecked, and `tool_writes_with_deletes` is the one
    /// that fails for it, so that keeping the default by mistake cannot pass.
    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        let _ = (tx, swimlane, path, deleted_hash, user, request_id);
        Ok(())
    }
}

#[cfg(all(test, feature = "conformance"))]
mod tests {
    use super::*;
    use crate::conformance::sample::{hash, nfs, request_id, swimlane, user_id};

    /// An implementation with only `register`, as every one was before `register_delete`.
    struct Bare;

    #[async_trait]
    impl ToolWrites for Bare {
        async fn register(
            &self,
            _: &mut Tx<'_>,
            _: &SwimlaneId,
            _: &NfsPath,
            _: &ContentHash,
            _: &UserId,
            _: &RequestId,
        ) -> Result<(), ToolWriteError> {
            Err(ToolWriteError::Unavailable)
        }
    }

    /// The rustdoc says the default does not look at the transaction: it says `Ok` to one of another store, where `register`
    /// answers `WrongTx`. An implementation that registers deletes has to check it, and the conformance suite makes it.
    #[tokio::test]
    async fn the_default_register_delete_does_not_look_at_the_transaction() {
        let result = Bare
            .register_delete(
                &mut Tx::detached(),
                &swimlane("sit1"),
                &nfs("svc/a.yml"),
                &hash(b"a: 1\n"),
                &user_id(1),
                &request_id("req-1"),
            )
            .await;
        assert_eq!(result, Ok(()));
    }
}
