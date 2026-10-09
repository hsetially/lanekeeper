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
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError>;   // idempotent
    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError>; // cached
    async fn get_many(&self, hs: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError>;
}

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
}
```

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
}
// WriteCtx carries the user, via (Ui | Mcp), an idempotency key and a request id.
// WriteOutcome is one of Applied, ProposalCreated, Conflict{current}, Locked or Rejected{reason}.

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
    PendingRestartChanged { swimlane, services },
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
    // ...existing methods
}

#[async_trait] pub trait SentinelSink {                    // implemented by 05
    async fn hello(&self, id: &SentinelIdentity, h: SentinelHello) -> Result<SentinelConfig, SinkError>;
    async fn records(&self, id: &SentinelIdentity, batch: AuditRecordBatch) -> Result<AckSeq, SinkError>;
}

pub struct Attribution { source: AttributionSource, confidence: Confidence, actor: Option<Actor>, evidence: Evidence }
// The engine gains severity(&ChangeContext, &[SeverityRule]) -> Severity, and inline change ranges in diff hunks.
```

New domain events:

- `AttributionUpdated { swimlane, path, hash }`
- `PrStateChanged { link_id, state }`
- `PrJobProgress { job_id, state }`
- `SyncWindowOpened { swimlane, job }`
- `SyncWindowClosed { swimlane, job }`
