# Interfaces

Prompt 01 creates these in `crates/ports`, with types from `crates/domain`. They are the seams between workstreams. Agents implement or consume them exactly as written. Changes go through a contract-change PR.

## Conventions

- Object-safe async traits use `#[async_trait]`. All traits are `Send + Sync + 'static`.
- Every port has an in-memory fake in `crates/ports/src/fakes/`, behind the `fakes` feature. Tests in other crates use these fakes, never a sibling crate's real implementation.
- Errors are per-port `thiserror` enums.
- Timeouts are passed in explicitly. Nothing waits indefinitely.
- `Bytes` means `bytes::Bytes`.

The signatures below show the shape. Exact generic details are settled in prompt 01.

## Platform ports (implemented by 03b)

```rust
#[async_trait] pub trait AgentGateway {
    /// Routed to whichever replica holds the agent's stream.
    async fn request(&self, s: &SwimlaneId, cmd: HubCommand, timeout: Duration)
        -> Result<AgentReply, GatewayError>;
    async fn status(&self, s: &SwimlaneId) -> AgentStatus;
}

#[async_trait] pub trait ReportSink {               // implemented by 05
    async fn hello(&self, id: &AgentIdentity, h: Hello) -> Result<AgentConfig, SinkError>;
    async fn heartbeat(&self, id: &AgentIdentity, hb: Heartbeat) -> Result<HeartbeatAction, SinkError>;
    async fn delta(&self, id: &AgentIdentity, d: ScanDelta) -> Result<(), SinkError>;
    async fn cluster(&self, id: &AgentIdentity, c: ClusterReport) -> Result<(), SinkError>;
}

#[async_trait] pub trait BlobStore {
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError>;   // idempotent; an existing blob has its age refreshed
    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError>; // cached
    async fn get_many(&self, hs: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError>;
}
// Storing a blob that already exists refreshes its age (migration `0008`, `blobs.touched_at`), at most once an hour per blob so that a
// hot blob does not churn (`hub_platform::blobs::store::TOUCH_INTERVAL`). Why: a writer stores a blob BEFORE the transaction that
// references it commits (an ingest delta, a staged part of one waiting up to 24 hours, a draft), and the registry's retention deletes
// blobs that nothing references and whose age is beyond a grace period (48 hours). A re-store that kept the old age would let an old,
// unreferenced blob whose content came back (a file reverted to a version from months ago) be collected between the store and the commit
// of its reference, and `file_state`, `file_observations` and `baselines` have no foreign key to `blobs` to stop the commit. The age
// trails the last `put` by at most one interval, plus the wait for a row lock when the blob is being refreshed or collected at that
// moment (bounded by the put's own timeout, 5 s by default), and is never ahead of it. A read (`get`, `get_many`) never refreshes.
// The port has no accessor for the age and gains no method: it is the store's own column, read by retention (05), so the conformance
// suite cannot pin the refresh and only the Postgres store's tests do. `PgBlobStore::put` is one statement, `PUT_SQL`, and runs
// outside any caller transaction: `now()` is the transaction start time, so inside a long transaction it would write an older
// `touched_at`.
//
// The grace period of retention must be at least `MIN_RETENTION_GRACE`: the interval plus the longest wait for the reference. That
// wait is a constant, `MAX_PUT_TO_REFERENCE` (24 hours), and the registry's `staging_ttl` is configurable, so `MIN_RETENTION_GRACE` is
// the floor for the default only: the registry must range-check `staging_ttl <= MAX_PUT_TO_REFERENCE`, or compute its floor as
// `TOUCH_INTERVAL + staging_ttl`. Anything that stores a blob and references it more than 24 hours later (for example a prompt-06
// upload preview that puts a blob and creates its draft or proposal row later) must create the row when it stores the blob, or `put`
// the blob again when it references it; a draft or a proposal is safe once its row exists, because `drafts.new_hash` and
// `proposals.new_hash` are RESTRICT foreign keys.
//
// Rules for a collector (retention) and for the code that references blobs:
// A collector must test `touched_at < now() - grace` in the same statement that locks or deletes the row (`DELETE ... WHERE touched_at
// < ...`, or `SELECT ... FOR UPDATE SKIP LOCKED` with the same condition), never from an earlier read, so that a refresh that commits in
// between wins. A job that locks first (`FOR UPDATE [SKIP LOCKED]`) must delete in the same transaction while it still holds the lock;
// a lock that is released before the delete protects nothing. Keep the retention transaction short and set `lock_timeout` on it,
// because a `put` of an old blob that the transaction has locked waits behind it, up to the put's own timeout. A reference taken by an
// existence check without a `put` (the registry's `sync/record.rs`, `adoption/confirm.rs` and the guarded insert in `baselines.rs`
// check that the blob exists and do not store it) is not protected by the refresh: only a `blob_refs` row protects such a reference
// (its foreign key is RESTRICT), and `baselines.blob_hash`, `file_state.nfs_hash` and `pr_links.nfs_hash` have no foreign key (nor do
// the other hash columns of the registry's tables), so the retention's reference check must read all of those columns. Any other
// writer of `blobs` must refresh it the same way, and any code that references a blob without storing it must be covered by the
// retention's reference check.
//
// Rollout: migration `0008` first, then the code. With a rolling deploy the old replicas still run the old `ON CONFLICT DO NOTHING` put,
// which refreshes nothing. The migration covers that overlap: it makes every existing blob count as touched when it ran, and a blob that
// an old replica stores gets `now()` from the column's default, so nothing is collectable for the first 48 hours after the migration,
// far longer than the overlap. The retention job must not be enabled before the last old replica is gone.

pub trait GitReader {                                    // sync; call via spawn_blocking
    fn head(&self, r: RepoKind, branch: &str) -> Result<CommitId, GitError>;
    fn branches(&self, r: RepoKind) -> Result<Vec<BranchInfo>, GitError>;
    fn tags(&self, r: RepoKind) -> Result<Vec<TagInfo>, GitError>;
    fn tree_index(&self, r: RepoKind, c: &CommitId) -> Result<Arc<TreeIndex>, GitError>;
    fn diff_trees(&self, r: RepoKind, from: &CommitId, to: &CommitId) -> Result<Vec<PathChange>, GitError>;
    fn read(&self, r: RepoKind, c: &CommitId, path: &RepoPath) -> Result<Option<Bytes>, GitError>;
    fn history(&self, r: RepoKind, branch: &str, path: &RepoPath, limit: usize) -> Result<Vec<CommitInfo>, GitError>;
    fn describe(&self, r: RepoKind, c: &CommitId) -> Result<VersionLabel, GitError>;
}

#[async_trait] pub trait EventBus {
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError>;  // fans out across replicas
    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent>; // bounded; lag => Resync
}

#[async_trait] pub trait Leases {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError>;
}

#[async_trait] pub trait KmsSigner {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError>;
}

#[async_trait] pub trait KmsEnvelope {
    async fn wrap(&self, dek: &Secret<[u8; 32]>, aad: &[u8]) -> Result<WrappedKey, KmsError>;
    async fn unwrap(&self, w: &WrappedKey, aad: &[u8]) -> Result<Secret<[u8; 32]>, KmsError>;
}

#[async_trait] pub trait SecretSource {
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError>;
}

#[async_trait] pub trait Notifier {
    async fn notify(&self, n: Notification) -> Result<(), NotifyError>; // never fails the caller's action
}
```

## Identity ports (implemented by 03a)

```rust
#[async_trait] pub trait TokenVerifier {
    async fn entra_access_token(&self, jwt: &str) -> Result<VerifiedUser, AuthError>;  // MCP
    async fn google_id_token(&self, jwt: &str, aud: &str) -> Result<GoogleIdentity, AuthError>; // agent join
}

#[async_trait] pub trait Users {
    async fn get(&self, id: &UserId) -> Result<Option<User>, UserError>;
    async fn require_active(&self, id: &UserId, min: Role) -> Result<User, AuthError>;
}

#[async_trait] pub trait AuditLog {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError>; // chained
    async fn record_many(&self, tx: &mut Tx<'_>, events: Vec<AuditEvent>) -> Result<Vec<AuditId>, AuditError>; // chained, batched
}
```

`AuditLog::record_many` (additive, P15; it has a default that calls `record` once per event, so an implementation that does not override it keeps working):

- **Same chain.** It writes exactly the chain that one `record` call per event writes: one entry per event (rule 8 is unchanged), the same links and hashes, the same checks per event. The ids come back in input order, and they are the entries' ids.
- **Ordering.** The events are chained in input order, and the batch is consecutive in the chain: another writer's entry never falls inside it, because the chain lock is held from the batch to the end of the caller's transaction. Calls of `record` and `record_many` interleave freely, in one transaction and across transactions, into one valid chain.
- **Atomic with the transaction.** The entries exist only if `tx` commits. A rollback leaves no entry and no chain advance: the next entry chains to the old head.
- **Size: two limits.** At most `MAX_AUDIT_BATCH` (200) events, and at most `MAX_AUDIT_BATCH_BYTES` (1 MiB) of canonical JSON summed over the events. A bigger batch is refused whole with `AuditError::BatchTooLarge`, never truncated, before the chain lock is taken, and nothing is recorded. The bytes are what the work under the lock depends on, not the count: canonical JSON writes a control character as six bytes and an event's `diff` is untrusted text of up to 64 KiB, so one event is up to `MAX_AUDIT_EVENT_BYTES` (398,673 bytes) and 200 of the largest would be about 76 MiB (79,734,600 bytes). (The real log stores at most 398,669 of them: its `timestamptz` column holds no time as long as `i64::MIN`.) The budget is at least one such event, so every event that `record` accepts is accepted alone. `canonical_size(&event)` is the size of one event and `split_batches(events)` cuts a list into batches that respect both limits, in order and without dropping any; the 50 paths of one agent message are one batch unless their diffs are large. An empty batch is `Ok` with no ids; it takes no lock and does not look at `tx`.
- **The chain lock is held to the end of the caller's transaction,** not to the end of the call: every other audited write waits until the caller commits or rolls back (their lock timeout is 5 s). So call it last in the transaction, do nothing slow after it, and do not pile several batches into one transaction, because the transaction then holds the lock for all of them.
- **Failure.** An event that `record` would refuse (`TooLarge`, `Encode`) is refused the same way before anything of the batch is recorded: the transaction stays usable, and committing it keeps nothing of the batch. `WrongTx` is also returned before any database work and does not alter the supplied transaction. Lock timeouts and store failures (`Unavailable`) leave the transaction in an unknown state, so the caller rolls back, as after a failed `record`. The real log returns `Unavailable`, and does not return ids, if the rows it inserted did not land in chain order; the rows are in the caller's transaction until it rolls back.
- **Where the work runs.** The real log checks and encodes the events, and hashes the links, on the blocking pool and not on the async runtime (AGENTS.md rule 4); the encoding stops as soon as the bytes pass the budget.
- **Cost.** A batch is three round trips whatever its size, against three for every event, so a batch of one event is no faster than `record`: use `record` for one event. Measured in a release build on a 4-vCPU Intel Xeon 2.8 GHz VM, Postgres 16.15 on the same machine over loopback (fsync on), with other builds running on it (`crates/hub-identity/tests/audit_batch.rs`, run with `--release --nocapture`, the first with `--ignored`):
  - one transaction (BEGIN, the audit, COMMIT), median of 15, `record` once per event against `record_many`: 1 event 1.18 and 1.29 ms; 10 events 6.5 and 2.5 ms; 50 events 29.3 and 5.2 ms; 200 events 106 and 14.8 ms (`report_the_time_of_a_batch_against_the_same_events_one_by_one`).
  - the time another writer waits for the chain lock behind a batch, from the grant of the lock to the commit of the batch, 5 samples in each of 3 runs: 3 events at the budget (1 MiB of control characters) 19 to 39 ms; the two largest events and a rest at the budget 19 to 35 ms; 200 events at the budget 26 to 58 ms; 50 typical events (25 KB) 3.4 to 15 ms. A debug build takes about 80 ms at the budget. The other writers' lock timeout is 5 s (`the_chain_lock_is_held_for_a_short_time_by_a_batch_at_the_byte_budget`).
- **A wrapper or decorator of an `AuditLog` must forward `record_many`** (and override it too if it changes what `record` records or checks): the default calls `record`, so a wrapper that forwards only `record` still works, but it silently falls back to one lock, head read and insert per event.

## Domain services (implemented by 05, 06 and 14; used by REST and MCP)

```rust
#[async_trait] pub trait RegistryRead {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError>;
    async fn tree(&self, u: &User, s: &SwimlaneId, prefix: &NfsPath, page: Page) -> Result<Paged<FileEntry>, ReadError>;
    async fn content(&self, u: &User, s: &SwimlaneId, p: &NfsPath, src: ContentSource) -> Result<FileContent, ReadError>;
    async fn history(&self, u: &User, s: &SwimlaneId, p: &NfsPath, page: Page) -> Result<Paged<Version>, ReadError>;
    async fn compare(&self, u: &User, l: CompareRef, r: CompareRef, p: Option<LogicalFile>) -> Result<Comparison, ReadError>;
    async fn compare_tree(&self, u: &User, l: CompareRef, r: CompareRef, page: Page) -> Result<TreeComparison, ReadError>;
    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError>;
    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError>;
    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError>;
    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError>;
    async fn effective(&self, u: &User, s: &SwimlaneId, f: &LogicalFile) -> Result<EffectiveConfig, ReadError>;
    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError>;
}

#[async_trait] pub trait WriteService {
    async fn edit(&self, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError>;
    async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError>;
    async fn delete(&self, ctx: &WriteCtx, r: DeleteRequest) -> Result<WriteOutcome, WriteError>;
    async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError>;
    async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError>;
    async fn raise_pr(&self, ctx: &WriteCtx, r: PrRequest) -> Result<WriteOutcome, WriteError>;
    async fn propose_draft(&self, ctx: &WriteCtx, r: EditRequest) -> Result<Draft, WriteError>; // no side effects
    async fn apply_draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<WriteOutcome, WriteError>;
    async fn preview_effect(&self, ctx: &WriteCtx, r: PreviewRequest) -> Result<ChangePreview, WriteError>; // no side effects
    async fn notify(&self, ctx: &WriteCtx, r: NotifyRequest) -> Result<WriteOutcome, WriteError>; // Operator
}
// WriteCtx carries the user, via (Ui | Mcp), an idempotency key and a request id.
// WriteOutcome is one of Applied{hash: Option<ContentHash>, audit_id, warnings} (`hash` is the new content hash and
// is None for a delete or a restart), ProposalCreated, Conflict{current}, Locked or Rejected{reason, issues}.
// `warnings` and `issues` are ValidationIssue{kind, line, rule}: a place (`line` is 1-based, zero is refused) and a
// rule name, never the text of the line.
// `change_eol` (lf | crlf) is optional on EditRequest and RevertRequest (D17). Absent, an edit (and an upload)
// preserves an existing file's style (the dominant one, if its endings are mixed), and a new file takes the dominant
// style of its folder (the content as it is, when the folder has no line endings). Absent, a revert writes the stored
// version as it was. Only an explicit value converts, and a lone CR counts as a line ending in a conversion.
// UploadRequest and DeleteRequest carry the `preview` that preview_effect issued: without a valid one they are
// Rejected{not_allowed} (D46). PrRequest carries an optional `target` (a tenant branch, or the base).
// Content of an edit, a draft, an upload or a preview is at most 2 MiB (the engine's `Limits::input_bytes`); the
// service enforces min(the agent's max_file_bytes, 2 MiB).
// ChangePreview.changes is a list of PreviewedChange{change: SettingChange, flagged: bool}. A value the secret scan
// flags is masked (`[REDACTED: N chars, rule]`) and `flagged` is true; revealing stays the audited /files/reveal (D79).
// `WriteService::notify` (tell the config-server that files changed, audited, Operator only) has nothing to do with
// the `Notifier::notify` port (Teams and other notifications): they share a verb and no behaviour.

/// Called by 06 inside the transaction that carries a write's audit event; implemented by 05 (D73).
/// A registration becomes visible at commit and 06 commits after the file is written (`WriteFile`), so 05 also matches an
/// observation that arrives before the commit is visible, and an unmatched registration expires after `ToolWriteEntry::TTL_SECS` (15 minutes, on the registry's clock, from when `register` ran), or at once when a newer observation of the path has another hash or says the file is gone (see the port's docs).
#[async_trait] pub trait ToolWrites {
    async fn register(&self, tx: &mut Tx<'_>, swimlane: &SwimlaneId, path: &NfsPath, new_hash: &ContentHash,
                      user: &UserId, request_id: &RequestId) -> Result<(), ToolWriteError>;
    /// Provided: the default registers nothing and says Ok (see below). `deleted_hash` is the hash of the file that was deleted.
    async fn register_delete(&self, tx: &mut Tx<'_>, swimlane: &SwimlaneId, path: &NfsPath, deleted_hash: &ContentHash,
                             user: &UserId, request_id: &RequestId) -> Result<(), ToolWriteError> { Ok(()) }
}
// A failed registration must not abort the caller's transaction. Both methods run inside the caller's open transaction, and in
// Postgres any failed statement aborts it: every later statement, the caller's audit insert included, fails with SQLSTATE 25P02
// until a ROLLBACK. So an implementation MUST run every statement of a registration inside a savepoint (a nested transaction) that
// it opens and releases itself, so that any failure rolls back only its own work, leaves the caller's transaction usable, and is
// returned as `Unavailable`. A caller that gets `Full` or `Unavailable` after the file operation has already happened therefore
// goes on and records the operation's own audit event in the same transaction (it must not roll it back or hide it: a failed
// registration only means the change may appear as an out-of-band change, attributed by the other means, the sentinel or the NFS
// client). The MUST is the implementation's because it is the contract, and the conformance suites cannot test it: the fakes have
// no failed state, so an implementation that forgets the savepoint passes every suite and fails only in production. Prompt 05
// tests it against a real Postgres: a failing registration statement, then the audit insert and the commit still succeed, and the
// registration is absent. Callers (prompt 06) SHOULD ALSO wrap the call in a savepoint of their own (`Tx::from_pg(&mut *savepoint)`,
// rolled back on `Err`) as defence in depth, because that does not depend on each implementor. A lost connection is not covered:
// the transaction is gone with the connection, the change appears as an out-of-band change, and the audit event cannot be recorded
// in that transaction either (the caller's problem, as for any lost connection). A pitfall: a `SET LOCAL lock_timeout` made inside
// the savepoint is reverted by a rollback to it but PERSISTS after `RELEASE` (300 ms were measured leaking into the caller's later
// audit insert); a registration needs a bounded wait (AGENTS rule 5), so the implementation restores the previous value after it
// releases the savepoint, or uses another bound. And `sqlx::Connection::begin` opens a savepoint only inside a transaction opened
// by sqlx (as every production caller does, through `Tx::from_pg(&mut tx)`); after a raw `BEGIN` it issues a second `BEGIN`, so an
// implementation that cannot rely on that issues `SAVEPOINT`, `RELEASE` and `ROLLBACK TO` itself.
//
// Order, time and keys (rules of the registry, which the port cannot observe, so 05 tests them and the conformance suites do
// not). "Newer" and "older" mean later and earlier in the PATH'S OBSERVATION ORDER (the order in which the registry applied
// observations of that swimlane and path), never wall-clock or ingestion time compared across paths. The registry stamps a
// registration with its own injected clock when `register` or `register_delete` runs, never with a time from a caller, and clamps
// it to that clock (a registration cannot be dated into the future and never age out of the sweep); the stamp is for the TTL only.
// The TTL of a registration runs from that stamp and the TTL of an observation from the moment the registry applied it, never from
// the agent's `observed_at`. With several hub replicas each stamps with its own clock; skew is harmless for a 15-minute TTL and is
// assumed small relative to it. A registry cannot know the commit time and a caller-owned transaction has no commit hook, so a registration
// expires at most a few milliseconds early, the safe direction. The key is (swimlane, path, hash, request), without the user: the
// same key from another user is a no-op that keeps the first user. That is an accepted risk that rests on the uniqueness of
// hub-made request ids, which nothing enforces; the registry's threat model records it. The bound (`Full`) is the registry's rule,
// soft by at most the number of registering transactions in flight.
//
// Which observation a registration explains (both kinds). An observation is eligible only if it is not already attributed as a
// tool write: a registration may UPGRADE a weaker attribution (unknown, nfs_client, sentinel_login, fs_owner_hint, sync_job) to
// tool_write/certain, and never replaces another tool-write attribution. When several registrations are pending for one
// observation (different requests, one path), the OLDEST consumes it, so an observation never has two actors: the one with the
// lowest `id` (assigned by the registry's store, in the order the registrations were inserted; stamps can disagree across
// replicas); the others stay pending until their own match, the TTL or an invalidation. A registration that commits after
// observations attributes at most one.
//
// Matching a hash registration: one that is visible when its hash is first observed at its path is consumed by that observation;
// one that commits after observations attributes the newest eligible observation of that exact hash at that path, within the TTL of
// when the registry applied it. A newer observation of the path that has another hash, or says the file is gone, invalidates a
// pending hash registration at once (the state that replaced the one it was for).
//
// A delete is a registration of its own kind, pinned to content like a write is: it carries `deleted_hash`, the hash of the file
// that was deleted (the expected hash the delete carried anyway, AGENTS rule 11). It exists only if `tx` commits (a rollback leaves
// nothing), the same (swimlane, path, deleted hash, request) again is a no-op and never `Full` (the same key from another user keeps
// the first user), it takes a place in the same bounded queue as a hash registration (`Full` is the answer of `register` and
// `register_delete`, never of the commit), it is distinct from a hash registration of the same path, hash and request, and a delete
// of another version of the file is another registration. Its matching is defined by the path's observation order:
//   (a) a deletion observation of the path matches a delete registration only if the state of the path immediately before that
//       deletion, in the path's observation order, was the hash `deleted_hash`; a deletion of any other version never matches;
//   (b) a delete registration that is visible when such a deletion is first observed is consumed by it (attributed `tool_write`,
//       certain, with this user as the actor; the oldest pending one, if several);
//   (c) one that commits AFTER observations attributes AT MOST ONE observation: the newest observation of that path in that
//       swimlane, which must be a deletion matching as in (a), that no hash observation of the path follows, that is eligible,
//       and that is within the TTL of when the registry applied it; older deletions stay exactly as they were (a person's earlier
//       delete over NFS is never taken for the tool's, however old the later one is);
//   (d) a hash observation later in the path's order than the deletion invalidates it (the file came back); with no deletion
//       observed yet, one applied after the registration became visible does; a hash observation earlier in the path's order
//       never does;
//   (e) it expires after the TTL, as a hash registration does. Dropping one is not audited.
// Where the registry reads "the state immediately before a deletion": the path's observation order is the registry's apply order for
// the path (`file_observations.id`), and that state is the hash of the closest earlier observation of that path in that order that
// has a hash (the deletion observation's own `previous_hash`, where the registry recorded it, must agree). Ingestion and the late
// matcher MUST read it from the same place. A deletion with no earlier observation of the path never matches (safe), and a registry
// whose last-known state is stale (the tool deleted a version the agent had not yet reported) gets a safe false negative.
//
// What remains (the registry's threat model, prompt 05 T10, carries these): (1) the same content recreated by someone else inside
// the window is indistinguishable from the tool's: a delete of it, recreated and deleted again, is attributed to the tool's
// registration; and for hash registrations too: if someone else writes H, then H', then H again inside the window, a late
// registration takes the newest eligible observation of H; (2) concurrent writers of identical content: if the later registration
// commits first, it takes the observation and the earlier one gets nothing ("oldest pending" only orders registrations that are
// visible together); both are tool users, so it is a swap between tool users and not a misattribution to a non-tool actor; (3) the
// assumption that the newest deletion at commit time is the tool's holds inside the duration of the registering transaction.
//
// The default of `register_delete` registers nothing and answers `Ok`, so that the method is additive: an implementation that has
// only `register` keeps compiling, and a delete made through the tool stays an out-of-band change (author unknown, next to the
// write path's own audit event), which is the safe degradation. The default does not check `tx`, where `register` answers `WrongTx`
// for a transaction of another store: an implementation that registers deletes must refuse one. A wrapper or decorator of `ToolWrites`
// MUST forward `register_delete`, or the deletes sent through it are dropped silently. An implementation that overrides
// `register_delete` MUST make its `ToolWritesProbe` return `Some(..)` from `registered_deletes` and run
// `ports::conformance::tool_writes_with_deletes` (and `tool_writes_bounded`): this is a convention, and a probe left at the default
// `None` passes `tool_writes` with every delete rule unchecked. The fake does, and the registry's probe must.
// `tool_writes_bounded` checks the `Full` rule over both kinds, exactly, one transaction at a time.
// `ToolDeleteEntry { swimlane, path, deleted_hash, user, request_id }` is what the probe reads back; `ToolWriteEntry` is unchanged.
//
// Storage is the implementor's, not the port's. Prompt 05's is the table `tool_write_registrations` (migration `0007`): one row
// per registration, `hash` NOT NULL (the written hash, or the deleted hash) and `is_delete` NOT NULL (no default), `UNIQUE
// (swimlane_id, path, hash, request_id, is_delete)` as the idempotency key (so the repeat of either kind is `INSERT 0 0`), the user
// and the registry's clock beside it, an age index for the expiry sweep and the bound, SELECT, INSERT and DELETE for the app (a
// registration is never updated, and the app holds no row locks: a sweep or a claim uses `DELETE ... RETURNING`), `ON DELETE
// CASCADE` from `swimlanes` and no foreign key to `users` (`db/migrations/README.md`).

#[async_trait] pub trait DocSearch {
    async fn search(&self, u: &User, q: &str, limit: usize) -> Result<Vec<DocHit>, DocError>;
    async fn related(&self, u: &User, f: &LogicalFile) -> Result<Vec<DocHit>, DocError>;
    async fn read(&self, u: &User, path: &DocPath, lines: Option<LineRange>) -> Result<DocText, DocError>;
    async fn grep(&self, u: &User, pattern: &str, regex: bool, context: u8) -> Result<Vec<DocGrepHit>, DocError>;
    async fn feature_status(&self, u: &User, q: FeatureQuery) -> Result<FeatureStatus, DocError>;
}
```

## Domain events

Published on the `EventBus`, and delivered to SSE subscribers filtered by what each user can see.

```rust
enum DomainEvent {
    FileObserved { swimlane, path, hash, source },        // scan | tool_write | sync
    DriftChanged { swimlane, paths },
    FindingsChanged { swimlane },
    PendingRestartChanged { swimlane, services },         // superseded by PickupChanged (D85); kept, no longer published once 05 has pickup state
    ProposalChanged { id, status },
    AgentStatusChanged { swimlane, status },
    GitHeadMoved { repo, branch, from, to },
    AccessRequestCreated { user },
    DocIndexed { doc },
    Resync,                                               // the subscriber lagged; refetch state
}
```

## Engine API (04, pure functions)

- `classify`
- `parse` and `validate`
- `flatten`
- `index_settings`, which returns setting rows with their line ranges
- `locate`
- `map_path` and `reverse_map`
- `effective`
- `effective_tree_hash`
- `drift_state`
- `text_diff`, `semantic_diff` and `binary_compare`
- `grid`
- `search_text`
- `checks_c1_to_c8`
- `effect_preview`
- `secret_scan`
- `apply_eol` and `normalise_eol`
- `version_label`
- batch variants that use rayon internally

The batch functions must be called from a blocking context.

## Additions (D72–D80)

```rust
// EventBus gains outbox publishing (D80); plain publish() remains, for events with no database change.
#[async_trait] pub trait EventBus {
    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError>; // fanned out after commit
    async fn publish_many_in_tx(&self, tx: &mut Tx<'_>, events: Vec<DomainEvent>) -> Result<(), BusError>; // the same, batched
    // ...existing methods
}

#[async_trait] pub trait SentinelSink {                    // implemented by 05
    async fn hello(&self, id: &SentinelIdentity, h: SentinelHello) -> Result<SentinelConfig, SinkError>;
    async fn records(&self, id: &SentinelIdentity, batch: AuditRecordBatch) -> Result<AckSeq, SinkError>;
}

pub struct Attribution { source: AttributionSource, confidence: Confidence, actor: Option<Actor>, evidence: Evidence }
// The engine gains severity(&ChangeContext, &[SeverityRule]) -> Severity, and inline change ranges in diff hunks.
```

`EventBus::publish_many_in_tx` (additive, P15; it has a default that calls `publish_in_tx` once per event):

- **Same events.** Each event of the batch gets what `publish_in_tx` gives it: the same payload bound (the outbox bus stores an event over its size cap as a `Resync` at its place, and counts it), the same filters and swimlane rule for its subscribers (internal events included), fan-out only after the commit and never on a rollback, and at-least-once delivery.
- **Ordering.** The events of a batch take outbox positions in input order, and in order with the other `publish_in_tx` and `publish_many_in_tx` calls of the same transaction. A batch commits atomically and the relay delivers what it reads in id order, so a batch is heard in input order. The one exception belongs to the relay and not to batching (see the module doc of `events::bus`): while the relay works through a backlog of more than its read size, ids that another transaction drew between the rows of the batch can make the lower rows arrive in the next poll, after the higher ones. An event is a prompt to refetch, so the order within a batch does not decide what a client shows.
- **Atomic with the transaction,** as `publish_in_tx`: parked until the commit, gone on a rollback.
- **Size.** At most `MAX_EVENT_BATCH` (200) events, each at most 4 KiB in the outbox bus; a bigger batch is `BusError::BatchTooLarge`, refused whole, nothing published. There is no byte budget as there is for the audit chain: nothing here is held under a lock that other writers wait for. An empty batch is `Ok`, writes nothing and does not look at `tx`. On any other error the caller rolls back, as after a failed `publish_in_tx`; a batch refused before it is written (too many, or a transaction of another store) writes none of it.
- **Notifications.** The `outbox_notify` trigger is `FOR EACH ROW` with the row id as payload, so a batch of N rows sends N notifications when its transaction commits, as N single publishes do. The relay treats a notification only as a prompt to look and delivers what the table holds, each id once, so nothing is lost, repeated or reordered by there being N of them. One notification per transaction would take a migration.
- **Cost.** One statement instead of one round trip for each event, so a batch of one event is no faster than `publish_in_tx`: use `publish_in_tx` for one event. Measured in a release build on the machine described under `record_many` (`crates/hub-platform/tests/outbox_batch.rs`, `report_the_time_of_a_batch_against_the_same_events_one_by_one`, run with `--release --ignored --nocapture`), one transaction (BEGIN, the publishes, COMMIT), median of 15, `publish_in_tx` once per event against `publish_many_in_tx`: 1 event 1.06 and 1.20 ms; 10 events 2.8 and 1.4 ms; 50 events 10.0 and 2.0 ms; 200 events 35.9 and 3.7 ms.
- **A wrapper or decorator of an `EventBus` must forward `publish_many_in_tx`** (and override it too if it changes what `publish_in_tx` stores or checks): the default calls `publish_in_tx`, so a wrapper that forwards only `publish_in_tx` still works, but it silently falls back to one statement per event.

New domain events:

- `AttributionUpdated { swimlane, path, hash }`
- `PrStateChanged { link_id, state }`
- `PrJobProgress { job_id, state }`
- `SyncWindowOpened { swimlane, job }`
- `SyncWindowClosed { swimlane, job }`
- `UserSessionsRevoked { user }`: internal, for replica coordination (03a). `DomainEvent::is_internal()` is true for it; the SSE edge drops it, and it is not in the OpenAPI `EventKind` list.
- `PickupChanged { swimlane, services }`: D85 (prompt 05). The pickup state of a change changed for these services; it carries ids only, and the state itself (with "live by HH:MM") is read from the pending-restarts endpoint. It supersedes `PendingRestartChanged`, which stays in the contract so that old rows and subscribers still parse. In the OpenAPI `EventKind` as `pickup_changed`.
