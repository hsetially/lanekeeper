//! The registry's metrics (S10): rates and durations with labels from closed sets. A swimlane id, a path, a hash or a
//! file's content is never a label; the numbers say how much, the logs say which.

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

/// The message of an agent that was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum MessageKind {
    Hello,
    Heartbeat,
    Delta,
    Cluster,
}

/// How a message ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum Outcome {
    Ok,
    /// A part of a delta was staged and acknowledged; the delta is not applied yet.
    Staged,
    HelloRequired,
    IdentityMismatch,
    Invalid,
    TooLarge,
    Unavailable,
}

/// What became of one file version of a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum VersionResult {
    /// A new current version of a file: the state moved.
    Recorded,
    /// Recorded in the history only: an older version than the stored one.
    HistoryOnly,
    /// Equal to the current version, so nothing to do.
    Unchanged,
    /// Already recorded by an earlier delivery (a replay).
    Replayed,
    /// A removal that moved the state.
    Removed,
}

/// What became of one blob offered to the settings index (T2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum IndexResult {
    /// Parsed, and its rows and marker written.
    Indexed,
    /// Did not parse (or is over a limit): marked, with no rows, so it is parsed once and not again.
    Unparseable,
    /// Had its marker already: nothing to do.
    AlreadyIndexed,
    /// The blob store does not have it.
    Missing,
    /// Could not be indexed now (the database or the pool failed); no marker, so the sweeper tries again.
    Failed,
    /// Offered to a full queue and dropped; the sweeper finds it.
    Dropped,
    /// Already waiting in the queue or being indexed on this replica.
    Duplicate,
}

/// What the Git index did with a moved head (T2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum MoveResult {
    /// Some swimlane has paths to recompute.
    Affected,
    /// The branch is not deployed by any swimlane (a template, the docs branch, an unknown branch) or nothing changed.
    Unconcerned,
    /// More paths than `max_affected_paths`: the swimlanes are recomputed whole.
    Whole,
}

/// A lookup in the cache of normalised Git hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum CacheResult {
    Hit,
    Miss,
}

/// What the pipeline did with a swimlane in one batch (T3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum LaneResult {
    /// Only the affected paths were recomputed.
    Incremental,
    /// The whole swimlane was loaded and computed (first sight, a rules change, too many paths, a nightly run).
    Whole,
    /// The swimlane has no inventory yet, so there is nothing to compare.
    NoInventory,
    /// The batch for the swimlane failed; its memory is dropped and the next batch starts from the database.
    Failed,
}

/// An in-process signal to the pipeline (T3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SignalResult {
    Accepted,
    /// The queue was full: the reconcile finds the change.
    Dropped,
}

/// How a `record_sync` ended (T4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SyncResult {
    /// Baselines set, the deployment recorded.
    Recorded,
    /// The swimlane has no confirmed baseline yet: only the deployment was recorded.
    DeploymentOnly,
    /// The report named a swimlane, branch, tenant or commit the hub does not accept.
    Rejected,
    /// The agent is not connected, so no scan could be asked for.
    AgentNotConnected,
    /// The scan was asked for and not applied in time.
    ScanTimeout,
    /// The database, the mirror, the audit log or the outbox failed.
    Failed,
}

/// How the processing of a closed sync window ended (T8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum WindowResult {
    /// The sync was recorded and the alerts evaluated.
    Processed,
    /// The swimlane has no confirmed baseline yet: only the deployments were recorded, and nothing was alerted.
    DeploymentOnly,
    /// The processing could not be done now (the agent is not connected, the scan did not come, the database failed) and is tried again.
    Deferred,
    /// The window was given up after `sync_close_give_up`: its alerts were evaluated without the sync's own processing.
    GivenUp,
    /// The window names a swimlane the hub no longer has.
    Dropped,
    /// Nobody processed the window for `sync_window_expire_after`: it was marked done without anything being said.
    Expired,
}

/// What became of a drift alert (T8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum AlertResult {
    /// The notifier took the notification.
    Sent,
    /// The notifier answered with an error.
    Failed,
    /// The notifier did not answer within `notify_timeout`.
    TimedOut,
    /// A sync window was open for the swimlane: the alert waits for the window's evaluation (D75).
    Held,
    /// The swimlane has no confirmed baseline: there is nothing to call drift yet.
    NotAdopted,
    /// More alerts were in flight than `alert_in_flight`: this one was dropped.
    Dropped,
}

/// What a tenant-suffix review did (T4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RematchResult {
    /// A new review was raised.
    Raised,
    /// The open review was refreshed with the new suffixes.
    Refreshed,
    /// The same suffixes were seen before: nothing to do.
    Unchanged,
    /// An admin is working on the open review, so it was left as it is; the next batch looks again.
    Deferred,
    /// The ranking could not be made (the mirror, the database).
    Failed,
}

/// What the pickup computation did to a swimlane (T10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum PickupResult {
    /// The state of at least one service changed (and `PickupChanged` was published for it).
    Changed,
    /// Every service already had the state the computation gave.
    Unchanged,
    /// The swimlane could not be computed (the database, the outbox); the sweeper looks at it again.
    Failed,
}

/// What the structure checks (C9 to C11) did to a swimlane (T10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum StructureResult {
    /// The checks were computed and the findings are what they say (written or already so).
    Computed,
    /// Nothing the checks read had moved since they were last computed: nothing was read or written.
    Unchanged,
    /// The swimlane is not ready (no inventory yet, or the pipeline has not computed it): there is nothing to check.
    NotReady,
    /// The swimlane could not be computed (the database, the blob store, the outbox); the next run looks at it again.
    Failed,
}

/// What the "as served" verification did to a swimlane (T10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ServedSwimlane {
    /// Its agent was asked.
    Verified,
    /// Its agent is not connected: skipped.
    Offline,
    /// The config-server has to restart before it serves what is on NFS (an open C11): skipped.
    RestartPending,
    /// It could not be set up, or its findings could not be written; the next run looks at it again.
    Failed,
}

/// What the "as served" verification came to for one combination of application, tenant, channel and file (T10). A closed set: no
/// file, tenant or channel is ever a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ServedResult {
    /// The config-server serves what the engine renders.
    Match,
    /// It serves something else.
    Mismatch,
    /// It answered "not found" for a file the engine has.
    NotServed,
    /// The agent did not answer in time.
    Timeout,
    /// The agent refused the path (its deny list).
    Denied,
    /// No slot of the replica's served requests came free in time.
    Busy,
    /// The agent or the config-server answered something that cannot be compared (another status, a reply that is not a file, a
    /// connection that went away).
    Agent,
    /// The engine had no rendering to hold the answer against.
    Engine,
}

/// What became of a tool-write registration, or of an observation that one could explain (T1, A10). A closed set: the labels never
/// grow with the traffic, and no path, hash or user is ever one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ToolWriteResult {
    /// `register` or `register_delete` stored a new registration.
    Registered,
    /// It repeated a registration the registry holds: nothing was stored.
    Repeated,
    /// The queue was full: the registration was refused.
    Full,
    /// The registration could not be taken (a lock wait, a failed statement, a transaction that sqlx did not open): the caller's
    /// transaction is untouched.
    Unavailable,
    /// The transaction was not a Postgres one.
    WrongTx,
    /// An observation applied while the registration was visible was explained by it (ingestion).
    MatchedOnArrival,
    /// An observation applied before the registration committed was explained by it (the matcher: the late commit).
    MatchedLate,
    /// A newer observation of the path contradicted a pending registration, which was dropped.
    Invalidated,
    /// A registration passed its time to live and was dropped by the sweep.
    Expired,
    /// A stored registration could not be read back (a user or request id of a shape this code does not know) and was skipped.
    Unreadable,
}

/// What a call of the adoption service was (T5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum AdoptionAction {
    Start,
    ChooseBranches,
    ChooseBase,
    Resolve,
    ScanSecrets,
    Confirm,
    Cancel,
    Mark,
    Clear,
}

/// How a call of the adoption service ended (T5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum AdoptionOutcome {
    Ok,
    /// The caller is not an active user with the role the call needs (S4).
    Forbidden,
    /// The call was understood and refused: a wrong step, an unresolved file, a file that changed, an unknown branch.
    Rejected,
    /// The database, the mirror, the blob store, the audit log or the outbox failed.
    Failed,
}

/// One operation of the read API (T6). A closed set of fifteen: the labels never grow with the traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ReadOp {
    Swimlanes,
    Swimlane,
    Tree,
    Drift,
    Content,
    History,
    Compare,
    CompareTree,
    Grid,
    SearchSettings,
    SearchText,
    Findings,
    Effective,
    PendingRestarts,
    Served,
    /// The port's `auto_notify` (a swimlane's setting, read alone).
    AutoNotify,
}

/// How a read ended (T6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ReadOutcome {
    Ok,
    /// The caller's `If-None-Match` matched: no body.
    NotModified,
    Forbidden,
    NotFound,
    BadRequest,
    /// The database, the blob store, the mirror, the agent or a deadline failed.
    Unavailable,
}

/// A cache of the read side (T6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum CacheName {
    Rows,
    Effective,
    Diff,
    Files,
    /// The text-search snapshot as a whole: a hit is a search that needed no build.
    Corpus,
    /// The shards of a snapshot that is built: a hit is a shard kept from the one before, a miss one that was made again.
    CorpusShard,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ReadLabels {
    op: ReadOp,
    outcome: ReadOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ReadOpLabels {
    op: ReadOp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ReadCacheLabels {
    cache: CacheName,
    result: CacheResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct AdoptionLabels {
    action: AdoptionAction,
    outcome: AdoptionOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct SyncLabels {
    result: SyncResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct WindowLabels {
    result: WindowResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct AlertLabels {
    result: AlertResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RematchLabels {
    result: RematchResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct PickupLabels {
    result: PickupResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct StructureLabels {
    result: StructureResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ServedSwimlaneLabels {
    result: ServedSwimlane,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ServedResultLabels {
    result: ServedResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct LaneLabels {
    result: LaneResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct SignalLabels {
    result: SignalResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct IndexLabels {
    result: IndexResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct MoveLabels {
    result: MoveResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct CacheLabels {
    result: CacheResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct MessageLabels {
    kind: MessageKind,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct VersionLabels {
    result: VersionResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ToolWriteLabels {
    result: ToolWriteResult,
}

/// The message of a sentinel that was handled (T7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SentinelKind {
    Hello,
    Records,
}

/// What became of one record of a sentinel's batch (T7). A closed set: no path, user or executable is ever a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SentinelRecord {
    /// Stored in `sentinel_records`.
    Stored,
    /// A record of a batch the hub had applied already (a resend): nothing was stored again.
    Resent,
    /// A chmod or chown (A16): acknowledged and counted, not stored, because it changes no content.
    Attribute,
    /// A path outside the swimlane's tree, or equal to its root (A17): dropped, never stored.
    Outside,
    /// The sentinel serves no swimlane, or the paths of its swimlane cannot be mapped (a root or an export that is not a clean absolute
    /// path): dropped, never stored.
    Unserved,
    /// A time more than `max_clock_skew` ahead of the hub's clock: dropped (it could never be pruned from the default partition, and no
    /// change can be explained by it).
    Future,
}

/// What became of an observation that the attribution looked at (T7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum AttributionResult {
    /// Became `sentinel_login`, by a record that arrived after the observation was applied (the sink's own transaction).
    SentinelOnRecord,
    /// Became `sentinel_login`, by a record that was stored before the observation was applied (the job).
    SentinelByJob,
    /// Became `nfs_client` after the grace, because a sentinel that has been seen since reported nothing for it.
    NfsClient,
    /// Chosen, but already as strong or stronger when the write ran (another writer was first): left as it was.
    Raced,
    /// A swimlane whose observations the job could not handle; the next run looks at them again.
    LaneFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct SentinelLabels {
    kind: SentinelKind,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct SentinelRecordLabels {
    result: SentinelRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct AttributionLabels {
    result: AttributionResult,
}

/// What became of the blobs a retention run looked at (T9). A count of blobs, never which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RetentionBlobs {
    /// Deleted from `blobs`.
    Deleted,
    /// A dry run found them collectable and left them.
    WouldDelete,
    /// Left for the next run, as they should be: touched, locked by a writer or referenced between the plan and the delete.
    Skipped,
    /// A delete transaction that a reference or a lock stopped (the foreign key, `lock_timeout`): left for the next run.
    Contended,
}

/// The rows of the two tables whose old rows the retention job also removes (T9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RetentionTable {
    SyncWindows,
    DeltaStaging,
}

/// Whether the rows were deleted or a dry run counted them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RetentionRows {
    Deleted,
    WouldDelete,
}

/// A step of a retention run that failed (the rest of the run still ran).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RetentionStep {
    Settings,
    Plan,
    Collect,
    SyncWindows,
    DeltaStaging,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RetentionBlobLabels {
    result: RetentionBlobs,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RetentionRowLabels {
    table: RetentionTable,
    result: RetentionRows,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RetentionStepLabels {
    step: RetentionStep,
}

/// Cloning shares the counters.
#[derive(Debug, Clone)]
pub struct RegistryMetrics {
    messages: Family<MessageLabels, Counter>,
    versions: Family<VersionLabels, Counter>,
    out_of_band: Counter,
    streams: Gauge,
    skipped_entries: Counter,
    dropped_env: Counter,
    apply_seconds: Histogram,
    index_blobs: Family<IndexLabels, Counter>,
    index_rows: Counter,
    index_redacted: Counter,
    index_queue: Gauge,
    index_seconds: Histogram,
    head_moves: Family<MoveLabels, Counter>,
    git_hashes: Family<CacheLabels, Counter>,
    tree_rows: Counter,
    pipeline_lanes: Family<LaneLabels, Counter>,
    pipeline_paths: Counter,
    pipeline_rows: Counter,
    pipeline_findings: Counter,
    pipeline_drift_paths: Counter,
    pipeline_baselines: Counter,
    pipeline_marks_cleared: Counter,
    pipeline_divergences: Counter,
    pipeline_reconciles: Counter,
    pipeline_signals: Family<SignalLabels, Counter>,
    pipeline_lanes_in_memory: Gauge,
    pipeline_seconds: Histogram,
    syncs: Family<SyncLabels, Counter>,
    sync_overwritten: Counter,
    sync_baselines: Counter,
    sync_seconds: Histogram,
    windows: Family<WindowLabels, Counter>,
    window_stale_warnings: Counter,
    hold_budget_exhaustions: Counter,
    alerts: Family<AlertLabels, Counter>,
    alert_evaluations: Counter,
    rematches: Family<RematchLabels, Counter>,
    adoptions: Family<AdoptionLabels, Counter>,
    adoption_rank_seconds: Histogram,
    adoption_secret_hits: Counter,
    adoption_baselines: Counter,
    reads: Family<ReadLabels, Counter>,
    read_seconds: Family<ReadOpLabels, Histogram, fn() -> Histogram>,
    read_cache: Family<ReadCacheLabels, Counter>,
    read_search_truncated: Counter,
    blob_withheld: Counter,
    severity_rules_skipped: Counter,
    severity_rules_active: Gauge,
    pickup_lanes: Family<PickupLabels, Counter>,
    pickup_services_changed: Counter,
    pickup_truncated: Counter,
    structure_lanes: Family<StructureLabels, Counter>,
    structure_findings: Counter,
    structure_c10_evaluated: Counter,
    structure_c10_cached: Counter,
    structure_blobs_read: Counter,
    served_swimlanes: Family<ServedSwimlaneLabels, Counter>,
    served_combinations: Family<ServedResultLabels, Counter>,
    tool_writes: Family<ToolWriteLabels, Counter>,
    sentinel_messages: Family<SentinelLabels, Counter>,
    sentinel_records: Family<SentinelRecordLabels, Counter>,
    sentinel_gaps: Counter,
    attributions: Family<AttributionLabels, Counter>,
    retention_blobs: Family<RetentionBlobLabels, Counter>,
    retention_blob_bytes: Family<RetentionBlobLabels, Counter>,
    retention_rows: Family<RetentionRowLabels, Counter>,
    retention_failures: Family<RetentionStepLabels, Counter>,
    retention_runs: Counter,
    retention_dry_runs: Counter,
    retention_truncated: Counter,
}

/// 1 ms to about 8 s per read: the budgets are tens of milliseconds, a cold text search is hundreds.
fn read_histogram() -> Histogram {
    Histogram::new(exponential_buckets(0.001, 2.0, 14))
}

impl Default for RegistryMetrics {
    fn default() -> Self {
        Self {
            messages: Family::default(),
            versions: Family::default(),
            out_of_band: Counter::default(),
            streams: Gauge::default(),
            skipped_entries: Counter::default(),
            dropped_env: Counter::default(),
            // 1 ms to about 8 s.
            apply_seconds: Histogram::new(exponential_buckets(0.001, 2.0, 14)),
            index_blobs: Family::default(),
            index_rows: Counter::default(),
            index_redacted: Counter::default(),
            index_queue: Gauge::default(),
            // 1 ms to about 8 s per batch of blobs.
            index_seconds: Histogram::new(exponential_buckets(0.001, 2.0, 14)),
            head_moves: Family::default(),
            git_hashes: Family::default(),
            tree_rows: Counter::default(),
            pipeline_lanes: Family::default(),
            pipeline_paths: Counter::default(),
            pipeline_rows: Counter::default(),
            pipeline_findings: Counter::default(),
            pipeline_drift_paths: Counter::default(),
            pipeline_baselines: Counter::default(),
            pipeline_marks_cleared: Counter::default(),
            pipeline_divergences: Counter::default(),
            pipeline_reconciles: Counter::default(),
            pipeline_signals: Family::default(),
            pipeline_lanes_in_memory: Gauge::default(),
            // 1 ms to about 8 s; a whole swimlane takes seconds, an incremental batch milliseconds.
            pipeline_seconds: Histogram::new(exponential_buckets(0.001, 2.0, 14)),
            syncs: Family::default(),
            sync_overwritten: Counter::default(),
            sync_baselines: Counter::default(),
            // 10 ms to about 80 s: most of a sync's time is the agent's scan.
            sync_seconds: Histogram::new(exponential_buckets(0.01, 2.0, 14)),
            windows: Family::default(),
            window_stale_warnings: Counter::default(),
            hold_budget_exhaustions: Counter::default(),
            alerts: Family::default(),
            alert_evaluations: Counter::default(),
            rematches: Family::default(),
            adoptions: Family::default(),
            // 1 ms to about 8 s: the ranking of one swimlane over 60 tree indexes is milliseconds, the budget is 5 s.
            adoption_rank_seconds: Histogram::new(exponential_buckets(0.001, 2.0, 14)),
            adoption_secret_hits: Counter::default(),
            adoption_baselines: Counter::default(),
            reads: Family::default(),
            read_seconds: Family::new_with_constructor(read_histogram as fn() -> Histogram),
            read_cache: Family::default(),
            read_search_truncated: Counter::default(),
            blob_withheld: Counter::default(),
            severity_rules_skipped: Counter::default(),
            severity_rules_active: Gauge::default(),
            pickup_lanes: Family::default(),
            pickup_services_changed: Counter::default(),
            pickup_truncated: Counter::default(),
            structure_lanes: Family::default(),
            structure_findings: Counter::default(),
            structure_c10_evaluated: Counter::default(),
            structure_c10_cached: Counter::default(),
            structure_blobs_read: Counter::default(),
            served_swimlanes: Family::default(),
            served_combinations: Family::default(),
            tool_writes: Family::default(),
            sentinel_messages: Family::default(),
            sentinel_records: Family::default(),
            sentinel_gaps: Counter::default(),
            attributions: Family::default(),
            retention_blobs: Family::default(),
            retention_blob_bytes: Family::default(),
            retention_rows: Family::default(),
            retention_failures: Family::default(),
            retention_runs: Counter::default(),
            retention_dry_runs: Counter::default(),
            retention_truncated: Counter::default(),
        }
    }
}

impl RegistryMetrics {
    #[allow(clippy::too_many_lines)] // one registration per metric
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "registry_ingest_messages",
            "Messages from agents handled, by kind and outcome",
            self.messages.clone(),
        );
        registry.register(
            "registry_ingest_versions",
            "File versions of scan deltas, by what became of them",
            self.versions.clone(),
        );
        registry.register(
            "registry_ingest_out_of_band",
            "Changes on NFS that no tool write and no sync explains",
            self.out_of_band.clone(),
        );
        registry.register(
            "registry_ingest_streams",
            "Swimlanes that said hello on this replica",
            self.streams.clone(),
        );
        registry.register(
            "registry_ingest_skipped_entries",
            "Files the agent skipped (too large to read)",
            self.skipped_entries.clone(),
        );
        registry.register(
            "registry_ingest_dropped_env_values",
            "Environment values dropped because the variable is not on the allowlist",
            self.dropped_env.clone(),
        );
        registry.register(
            "registry_ingest_apply_seconds",
            "Time to apply one logical scan delta",
            self.apply_seconds.clone(),
        );
        registry.register(
            "registry_index_blobs",
            "Blobs offered to the settings index, by what became of them",
            self.index_blobs.clone(),
        );
        registry.register(
            "registry_index_rows",
            "Settings rows written to the settings index",
            self.index_rows.clone(),
        );
        registry.register(
            "registry_index_redacted_values",
            "Setting values the secret scan flagged, stored redacted",
            self.index_redacted.clone(),
        );
        registry.register(
            "registry_index_queue",
            "Blob hashes waiting for the settings index on this replica",
            self.index_queue.clone(),
        );
        registry.register(
            "registry_index_batch_seconds",
            "Time to index one batch of blobs",
            self.index_seconds.clone(),
        );
        registry.register(
            "registry_git_head_moves",
            "Moved Git heads handled, by outcome",
            self.head_moves.clone(),
        );
        registry.register(
            "registry_git_hash_cache",
            "Lookups of normalised Git hashes, by hit or miss",
            self.git_hashes.clone(),
        );
        registry.register(
            "registry_tree_rows_written",
            "Rows of effective_tree_hashes and effective_files written or deleted",
            self.tree_rows.clone(),
        );
        registry.register(
            "registry_pipeline_lanes",
            "Swimlanes handled by the pipeline, by how much of each was recomputed",
            self.pipeline_lanes.clone(),
        );
        registry.register(
            "registry_pipeline_paths",
            "Affected paths the pipeline recomputed",
            self.pipeline_paths.clone(),
        );
        registry.register(
            "registry_pipeline_rows_written",
            "file_state rows the pipeline wrote or deleted",
            self.pipeline_rows.clone(),
        );
        registry.register(
            "registry_pipeline_findings_written",
            "Findings the pipeline opened, changed or resolved",
            self.pipeline_findings.clone(),
        );
        registry.register(
            "registry_pipeline_drift_paths",
            "Paths named in DriftChanged events",
            self.pipeline_drift_paths.clone(),
        );
        registry.register(
            "registry_pipeline_baselines_set",
            "Baselines set because NFS and Git converged",
            self.pipeline_baselines.clone(),
        );
        registry.register(
            "registry_pipeline_marks_cleared",
            "Intentional-divergence marks cleared because the file moved on",
            self.pipeline_marks_cleared.clone(),
        );
        registry.register(
            "registry_pipeline_divergences",
            "Paths where a full recompute or the reconcile found the stored state wrong and repaired it",
            self.pipeline_divergences.clone(),
        );
        registry.register(
            "registry_pipeline_reconciles",
            "Reconcile scans run",
            self.pipeline_reconciles.clone(),
        );
        registry.register(
            "registry_pipeline_signals",
            "In-process signals to the pipeline, by outcome",
            self.pipeline_signals.clone(),
        );
        registry.register(
            "registry_pipeline_lanes_in_memory",
            "Swimlane states the pipeline holds in memory",
            self.pipeline_lanes_in_memory.clone(),
        );
        registry.register(
            "registry_pipeline_batch_seconds",
            "Time to bring one batch of swimlanes up to date",
            self.pipeline_seconds.clone(),
        );
        registry.register(
            "registry_sync_reports",
            "Sync reports handled by record_sync, by how they ended",
            self.syncs.clone(),
        );
        registry.register(
            "registry_sync_overwritten_changes",
            "Changes made only on NFS that a sync replaced (C7)",
            self.sync_overwritten.clone(),
        );
        registry.register(
            "registry_sync_baselines_set",
            "Baselines set by a sync report because NFS equals Git at the reported commits",
            self.sync_baselines.clone(),
        );
        registry.register(
            "registry_sync_seconds",
            "Time to record one sync report, including the wait for the scan",
            self.sync_seconds.clone(),
        );
        registry.register(
            "registry_sync_windows",
            "Closed sync windows handled by the watch job, by how they ended",
            self.windows.clone(),
        );
        registry.register(
            "registry_sync_window_stale_warnings",
            "Sync windows open for more than the stale limit that were warned about (once each)",
            self.window_stale_warnings.clone(),
        );
        registry.register(
            "registry_sync_hold_budget_exhausted",
            "Episodes in which the budget for holding a swimlane's alerts was spent (said once for each episode)",
            self.hold_budget_exhaustions.clone(),
        );
        registry.register(
            "registry_drift_alerts",
            "Drift alerts and window evaluations offered to the notifier, by what became of them",
            self.alerts.clone(),
        );
        registry.register(
            "registry_alert_evaluations",
            "Alert evaluations made when a sync window closed (one for the whole window, whatever the number of files)",
            self.alert_evaluations.clone(),
        );
        registry.register(
            "registry_tenant_rematches",
            "Reviews raised or refreshed because the tenant suffixes on NFS changed, by outcome",
            self.rematches.clone(),
        );
        registry.register(
            "registry_adoption_calls",
            "Calls of the adoption service and of the intentional-divergence marks, by action and outcome",
            self.adoptions.clone(),
        );
        registry.register(
            "registry_adoption_rank_seconds",
            "Time to rank the deployable branches against one swimlane",
            self.adoption_rank_seconds.clone(),
        );
        registry.register(
            "registry_adoption_secret_hits",
            "Places the adoption secret scan flagged (locations only, never values)",
            self.adoption_secret_hits.clone(),
        );
        registry.register(
            "registry_adoption_baselines",
            "Baselines written by adoption confirmations",
            self.adoption_baselines.clone(),
        );
        registry.register(
            "registry_reads",
            "Reads of the read API, by operation and outcome",
            self.reads.clone(),
        );
        registry.register(
            "registry_read_seconds",
            "Time of a read, by operation",
            self.read_seconds.clone(),
        );
        registry.register(
            "registry_read_cache",
            "Lookups of the read caches, by cache and hit or miss",
            self.read_cache.clone(),
        );
        registry.register(
            "registry_read_search_truncated",
            "Text searches whose result was cut by a cap",
            self.read_search_truncated.clone(),
        );
        registry.register(
            "registry_blob_withheld",
            "Blob requests refused because the hash is the content of a denied file",
            self.blob_withheld.clone(),
        );
        registry.register(
            "registry_severity_rules_skipped",
            "Rows of severity_rules that were not applied (a condition or a glob the engine refuses, or the id of a default rule), counted each time a set of rules is compiled",
            self.severity_rules_skipped.clone(),
        );
        registry.register(
            "registry_severity_rules_active",
            "Rules from severity_rules in effect on top of the defaults",
            self.severity_rules_active.clone(),
        );
        registry.register(
            "registry_pickup_lanes",
            "Swimlanes whose pickup state was computed, by whether any service's state changed",
            self.pickup_lanes.clone(),
        );
        registry.register(
            "registry_pickup_services_changed",
            "Services whose pickup state changed and for which PickupChanged was published",
            self.pickup_services_changed.clone(),
        );
        registry.register(
            "registry_pickup_truncated",
            "Computations of a swimlane's pickup state that read only the newest changed files because there were more than pickup_max_files",
            self.pickup_truncated.clone(),
        );
        registry.register(
            "registry_structure_lanes",
            "Swimlanes whose structure checks (C9 to C11) were looked at, by whether they were computed, unchanged, not ready or failed",
            self.structure_lanes.clone(),
        );
        registry.register(
            "registry_structure_findings",
            "Findings of C9 to C11 written (new, changed or resolved)",
            self.structure_findings.clone(),
        );
        registry.register(
            "registry_structure_c10_evaluated",
            "Resource files whose placeholders C10 evaluated (their result was not cached for this content, these views and these environment names)",
            self.structure_c10_evaluated.clone(),
        );
        registry.register(
            "registry_structure_c10_cached",
            "Resource files whose C10 result came from the cache, by content",
            self.structure_c10_cached.clone(),
        );
        registry.register(
            "registry_structure_blobs_read",
            "Blobs the structure checks read from the blob store",
            self.structure_blobs_read.clone(),
        );
        registry.register(
            "registry_served_verify_swimlanes",
            "Swimlanes the nightly as-served verification looked at, by whether they were asked, offline, waiting for a config-server restart or failed",
            self.served_swimlanes.clone(),
        );
        registry.register(
            "registry_served_verify_combinations",
            "Combinations of application, tenant, channel and file the as-served verification asked for, by what came of them",
            self.served_combinations.clone(),
        );
        registry.register(
            "registry_tool_writes",
            "Tool-write registrations and what became of them: registered, repeated, refused, matched on arrival or late, dropped",
            self.tool_writes.clone(),
        );
        registry.register(
            "registry_sentinel_messages",
            "Messages from sentinels handled, by kind and outcome",
            self.sentinel_messages.clone(),
        );
        registry.register(
            "registry_sentinel_records",
            "Records of sentinel batches by what became of them: stored, resent, attribute, outside the swimlane, unserved, in the future",
            self.sentinel_records.clone(),
        );
        registry.register(
            "registry_sentinel_gaps",
            "Batches from a sentinel whose sequence number skipped one (the sentinel's own spool dropped batches)",
            self.sentinel_gaps.clone(),
        );
        registry.register(
            "registry_attributions",
            "Observations the attribution upgraded or left, by what became of them",
            self.attributions.clone(),
        );
        registry.register(
            "registry_retention_blobs",
            "Blobs the retention job looked at, by what became of them (counts only)",
            self.retention_blobs.clone(),
        );
        registry.register(
            "registry_retention_blob_bytes",
            "Bytes of the blobs the retention job looked at, by what became of them",
            self.retention_blob_bytes.clone(),
        );
        registry.register(
            "registry_retention_rows",
            "Rows of sync_windows and delta_staging that the retention job deleted, or that a dry run counted",
            self.retention_rows.clone(),
        );
        registry.register(
            "registry_retention_failures",
            "Steps of a retention run that failed (the other steps still ran)",
            self.retention_failures.clone(),
        );
        registry.register(
            "registry_retention_runs",
            "Retention runs that deleted (a dry run is counted apart)",
            self.retention_runs.clone(),
        );
        registry.register(
            "registry_retention_dry_runs",
            "Retention runs that only reported",
            self.retention_dry_runs.clone(),
        );
        registry.register(
            "registry_retention_truncated",
            "Retention runs whose plan was cut by the batch or the byte limit, so that more is left for the next run",
            self.retention_truncated.clone(),
        );
    }

    pub(crate) fn tool_write(&self, result: ToolWriteResult, n: u64) {
        if n > 0 {
            self.tool_writes
                .get_or_create(&ToolWriteLabels { result })
                .inc_by(n);
        }
    }

    /// The count of one tool-write result, for tests.
    pub fn tool_write_count(&self, result: ToolWriteResult) -> u64 {
        self.tool_writes.get_or_create(&ToolWriteLabels { result }).get()
    }

    pub(crate) fn sentinel_message(&self, kind: SentinelKind, outcome: Outcome) {
        self.sentinel_messages
            .get_or_create(&SentinelLabels { kind, outcome })
            .inc();
    }

    /// The count of the messages of one kind and outcome from sentinels, for tests.
    pub fn sentinel_message_count(&self, kind: SentinelKind, outcome: Outcome) -> u64 {
        self.sentinel_messages
            .get_or_create(&SentinelLabels { kind, outcome })
            .get()
    }

    pub(crate) fn sentinel_record(&self, result: SentinelRecord, n: u64) {
        if n > 0 {
            self.sentinel_records
                .get_or_create(&SentinelRecordLabels { result })
                .inc_by(n);
        }
    }

    /// The count of the records of sentinel batches with one result, for tests.
    pub fn sentinel_record_count(&self, result: SentinelRecord) -> u64 {
        self.sentinel_records
            .get_or_create(&SentinelRecordLabels { result })
            .get()
    }

    pub(crate) fn sentinel_gap(&self) {
        self.sentinel_gaps.inc();
    }

    /// The batches that skipped a sequence number, for tests.
    pub fn sentinel_gap_count(&self) -> u64 {
        self.sentinel_gaps.get()
    }

    pub(crate) fn attribution(&self, result: AttributionResult, n: u64) {
        if n > 0 {
            self.attributions
                .get_or_create(&AttributionLabels { result })
                .inc_by(n);
        }
    }

    /// The count of observations with one attribution result, for tests.
    pub fn attribution_count(&self, result: AttributionResult) -> u64 {
        self.attributions
            .get_or_create(&AttributionLabels { result })
            .get()
    }

    /// Blobs a retention run looked at, with their bytes (T9). Counts only: which blobs is never a label and never a log field.
    pub(crate) fn retention_blobs(&self, result: RetentionBlobs, blobs: u64, bytes: u64) {
        let labels = RetentionBlobLabels { result };
        if blobs > 0 {
            self.retention_blobs.get_or_create(&labels).inc_by(blobs);
        }
        if bytes > 0 {
            self.retention_blob_bytes.get_or_create(&labels).inc_by(bytes);
        }
    }

    /// Rows of one table that a retention run deleted or a dry run counted (T9).
    pub(crate) fn retention_rows(&self, table: RetentionTable, result: RetentionRows, n: u64) {
        if n > 0 {
            self.retention_rows
                .get_or_create(&RetentionRowLabels { table, result })
                .inc_by(n);
        }
    }

    /// A step of a retention run failed (T9).
    pub(crate) fn retention_failure(&self, step: RetentionStep) {
        self.retention_failures
            .get_or_create(&RetentionStepLabels { step })
            .inc();
    }

    /// A retention run ended, deleting or not (T9).
    pub(crate) fn retention_run(&self, dry_run: bool, truncated: bool) {
        if dry_run {
            self.retention_dry_runs.inc();
        } else {
            self.retention_runs.inc();
        }
        if truncated {
            self.retention_truncated.inc();
        }
    }

    /// Blobs with one retention result, for tests.
    pub fn retention_blob_count(&self, result: RetentionBlobs) -> u64 {
        self.retention_blobs
            .get_or_create(&RetentionBlobLabels { result })
            .get()
    }

    /// Bytes of the blobs with one retention result, for tests.
    pub fn retention_blob_bytes_count(&self, result: RetentionBlobs) -> u64 {
        self.retention_blob_bytes
            .get_or_create(&RetentionBlobLabels { result })
            .get()
    }

    /// Rows of one table with one retention result, for tests.
    pub fn retention_row_count(&self, table: RetentionTable, result: RetentionRows) -> u64 {
        self.retention_rows
            .get_or_create(&RetentionRowLabels { table, result })
            .get()
    }

    /// Failures of one step of a retention run, for tests.
    pub fn retention_failure_count(&self, step: RetentionStep) -> u64 {
        self.retention_failures
            .get_or_create(&RetentionStepLabels { step })
            .get()
    }

    /// Retention runs that deleted, and (second) dry runs, and (third) runs whose plan was cut, for tests.
    pub fn retention_run_counts(&self) -> (u64, u64, u64) {
        (
            self.retention_runs.get(),
            self.retention_dry_runs.get(),
            self.retention_truncated.get(),
        )
    }

    pub(crate) fn message(&self, kind: MessageKind, outcome: Outcome) {
        self.messages
            .get_or_create(&MessageLabels { kind, outcome })
            .inc();
    }

    pub(crate) fn version(&self, result: VersionResult, n: u64) {
        if n > 0 {
            self.versions.get_or_create(&VersionLabels { result }).inc_by(n);
        }
    }

    pub(crate) fn out_of_band(&self, n: u64) {
        if n > 0 {
            self.out_of_band.inc_by(n);
        }
    }

    pub(crate) fn streams(&self, n: usize) {
        self.streams.set(i64::try_from(n).unwrap_or(i64::MAX));
    }

    pub(crate) fn skipped(&self, n: usize) {
        if n > 0 {
            self.skipped_entries.inc_by(n as u64);
        }
    }

    pub(crate) fn dropped_env(&self, n: usize) {
        if n > 0 {
            self.dropped_env.inc_by(n as u64);
        }
    }

    pub(crate) fn applied(&self, seconds: f64) {
        self.apply_seconds.observe(seconds);
    }

    pub(crate) fn index_blob(&self, result: IndexResult, n: u64) {
        if n > 0 {
            self.index_blobs.get_or_create(&IndexLabels { result }).inc_by(n);
        }
    }

    pub(crate) fn index_rows(&self, n: u64) {
        if n > 0 {
            self.index_rows.inc_by(n);
        }
    }

    pub(crate) fn index_redacted(&self, n: u64) {
        if n > 0 {
            self.index_redacted.inc_by(n);
        }
    }

    pub(crate) fn index_queue(&self, n: usize) {
        self.index_queue.set(i64::try_from(n).unwrap_or(i64::MAX));
    }

    pub(crate) fn index_batch_seconds(&self, seconds: f64) {
        self.index_seconds.observe(seconds);
    }

    pub(crate) fn head_move(&self, result: MoveResult) {
        self.head_moves.get_or_create(&MoveLabels { result }).inc();
    }

    pub(crate) fn git_hash(&self, result: CacheResult) {
        self.git_hashes.get_or_create(&CacheLabels { result }).inc();
    }

    pub(crate) fn tree_rows(&self, n: u64) {
        if n > 0 {
            self.tree_rows.inc_by(n);
        }
    }

    pub(crate) fn pipeline_lane(&self, result: LaneResult) {
        self.pipeline_lanes.get_or_create(&LaneLabels { result }).inc();
    }

    pub(crate) fn pipeline_paths(&self, n: u64) {
        if n > 0 {
            self.pipeline_paths.inc_by(n);
        }
    }

    pub(crate) fn pipeline_rows(&self, n: u64) {
        if n > 0 {
            self.pipeline_rows.inc_by(n);
        }
    }

    pub(crate) fn pipeline_findings(&self, n: u64) {
        if n > 0 {
            self.pipeline_findings.inc_by(n);
        }
    }

    pub(crate) fn pipeline_drift_paths(&self, n: u64) {
        if n > 0 {
            self.pipeline_drift_paths.inc_by(n);
        }
    }

    pub(crate) fn pipeline_baselines(&self, n: u64) {
        if n > 0 {
            self.pipeline_baselines.inc_by(n);
        }
    }

    pub(crate) fn pipeline_marks_cleared(&self, n: u64) {
        if n > 0 {
            self.pipeline_marks_cleared.inc_by(n);
        }
    }

    pub(crate) fn pipeline_divergences(&self, n: u64) {
        if n > 0 {
            self.pipeline_divergences.inc_by(n);
        }
    }

    pub(crate) fn pipeline_reconcile(&self) {
        self.pipeline_reconciles.inc();
    }

    pub(crate) fn pipeline_signal(&self, result: SignalResult) {
        self.pipeline_signals
            .get_or_create(&SignalLabels { result })
            .inc();
    }

    pub(crate) fn pipeline_lanes_in_memory(&self, n: usize) {
        self.pipeline_lanes_in_memory
            .set(i64::try_from(n).unwrap_or(i64::MAX));
    }

    pub(crate) fn pipeline_batch_seconds(&self, seconds: f64) {
        self.pipeline_seconds.observe(seconds);
    }

    pub(crate) fn sync(&self, result: SyncResult, seconds: f64) {
        self.syncs.get_or_create(&SyncLabels { result }).inc();
        self.sync_seconds.observe(seconds);
    }

    pub(crate) fn sync_overwritten(&self, n: u64) {
        if n > 0 {
            self.sync_overwritten.inc_by(n);
        }
    }

    pub(crate) fn sync_baselines(&self, n: u64) {
        if n > 0 {
            self.sync_baselines.inc_by(n);
        }
    }

    pub(crate) fn window(&self, result: WindowResult) {
        self.windows.get_or_create(&WindowLabels { result }).inc();
    }

    pub(crate) fn window_stale_warning(&self) {
        self.window_stale_warnings.inc();
    }

    pub(crate) fn hold_budget_exhaustion(&self) {
        self.hold_budget_exhaustions.inc();
    }

    pub(crate) fn alert(&self, result: AlertResult, n: u64) {
        if n > 0 {
            self.alerts.get_or_create(&AlertLabels { result }).inc_by(n);
        }
    }

    pub(crate) fn alert_evaluation(&self) {
        self.alert_evaluations.inc();
    }

    pub(crate) fn rematch(&self, result: RematchResult) {
        self.rematches.get_or_create(&RematchLabels { result }).inc();
    }

    pub(crate) fn adoption(&self, action: AdoptionAction, outcome: AdoptionOutcome) {
        self.adoptions
            .get_or_create(&AdoptionLabels { action, outcome })
            .inc();
    }

    pub(crate) fn adoption_ranked(&self, seconds: f64) {
        self.adoption_rank_seconds.observe(seconds);
    }

    pub(crate) fn adoption_secrets(&self, n: u64) {
        if n > 0 {
            self.adoption_secret_hits.inc_by(n);
        }
    }

    pub(crate) fn adoption_baselines(&self, n: u64) {
        if n > 0 {
            self.adoption_baselines.inc_by(n);
        }
    }

    /// Adoption calls by action and outcome, for tests.
    pub fn adoption_count(&self, action: AdoptionAction, outcome: AdoptionOutcome) -> u64 {
        self.adoptions
            .get_or_create(&AdoptionLabels { action, outcome })
            .get()
    }

    /// Places the secret scans flagged, for tests.
    pub fn adoption_secret_hits_count(&self) -> u64 {
        self.adoption_secret_hits.get()
    }

    /// Baselines written by confirmations, for tests.
    pub fn adoption_baselines_count(&self) -> u64 {
        self.adoption_baselines.get()
    }

    pub(crate) fn read(&self, op: ReadOp, outcome: ReadOutcome, seconds: f64) {
        self.reads.get_or_create(&ReadLabels { op, outcome }).inc();
        self.read_seconds
            .get_or_create(&ReadOpLabels { op })
            .observe(seconds);
    }

    pub(crate) fn read_cache(&self, cache: CacheName, result: CacheResult) {
        self.read_cache
            .get_or_create(&ReadCacheLabels { cache, result })
            .inc();
    }

    pub(crate) fn read_search_truncated(&self) {
        self.read_search_truncated.inc();
    }

    pub(crate) fn blob_withheld(&self) {
        self.blob_withheld.inc();
    }

    /// A set of severity rules was compiled: `active` rows are in effect and `skipped` were not (T9, D103).
    pub(crate) fn severity_rules(&self, active: usize, skipped: usize) {
        self.severity_rules_active
            .set(i64::try_from(active).unwrap_or(i64::MAX));
        self.severity_rules_skipped.inc_by(skipped as u64);
    }

    /// A swimlane's pickup state was computed (T10).
    pub(crate) fn pickup_lane(&self, result: PickupResult) {
        self.pickup_lanes.get_or_create(&PickupLabels { result }).inc();
    }

    /// Services whose pickup state changed (T10).
    pub(crate) fn pickup_services_changed(&self, n: usize) {
        if n > 0 {
            self.pickup_services_changed.inc_by(n as u64);
        }
    }

    /// A computation of the pickup state read only the newest changed files (T10, rule 5).
    pub(crate) fn pickup_truncated(&self) {
        self.pickup_truncated.inc();
    }

    /// A swimlane's structure checks were looked at (T10).
    pub(crate) fn structure_lane(&self, result: StructureResult) {
        self.structure_lanes
            .get_or_create(&StructureLabels { result })
            .inc();
    }

    /// Findings of C9 to C11 written, and the C10 work of a computation: results evaluated and results from the cache, and blobs read
    /// (T10). Counts only (S10).
    pub(crate) fn structure_work(&self, findings: usize, evaluated: usize, cached: usize, blobs: usize) {
        self.structure_findings.inc_by(findings as u64);
        self.structure_c10_evaluated.inc_by(evaluated as u64);
        self.structure_c10_cached.inc_by(cached as u64);
        self.structure_blobs_read.inc_by(blobs as u64);
    }

    /// Swimlanes whose structure checks were looked at, by result, for tests.
    pub fn structure_lane_count(&self, result: StructureResult) -> u64 {
        self.structure_lanes
            .get_or_create(&StructureLabels { result })
            .get()
    }

    /// Blobs the structure checks read, for tests.
    pub fn structure_blobs_read_count(&self) -> u64 {
        self.structure_blobs_read.get()
    }

    /// A swimlane was looked at by the as-served verification (T10).
    pub(crate) fn served_verify_swimlane(&self, result: ServedSwimlane) {
        self.served_swimlanes
            .get_or_create(&ServedSwimlaneLabels { result })
            .inc();
    }

    /// A combination was asked for by the as-served verification (T10).
    pub(crate) fn served_verify_combination(&self, result: ServedResult) {
        self.served_combinations
            .get_or_create(&ServedResultLabels { result })
            .inc();
    }

    /// Swimlanes looked at by the as-served verification, by result, for tests.
    pub fn served_verify_swimlane_count(&self, result: ServedSwimlane) -> u64 {
        self.served_swimlanes
            .get_or_create(&ServedSwimlaneLabels { result })
            .get()
    }

    /// Combinations asked for by the as-served verification, by result, for tests.
    pub fn served_verify_combination_count(&self, result: ServedResult) -> u64 {
        self.served_combinations
            .get_or_create(&ServedResultLabels { result })
            .get()
    }

    /// Swimlanes whose pickup state was computed, by result, for tests.
    pub fn pickup_lane_count(&self, result: PickupResult) -> u64 {
        self.pickup_lanes.get_or_create(&PickupLabels { result }).get()
    }

    /// Services whose pickup state changed, for tests.
    pub fn pickup_services_changed_count(&self) -> u64 {
        self.pickup_services_changed.get()
    }

    /// Truncated pickup computations, for tests.
    pub fn pickup_truncated_count(&self) -> u64 {
        self.pickup_truncated.get()
    }

    /// Rows of `severity_rules` that were not applied, over all the sets compiled, for tests.
    pub fn severity_rules_skipped_count(&self) -> u64 {
        self.severity_rules_skipped.get()
    }

    /// Rules from `severity_rules` in effect in the last set compiled, for tests.
    pub fn severity_rules_active_count(&self) -> i64 {
        self.severity_rules_active.get()
    }

    /// Reads by operation and outcome, for tests.
    pub fn read_count(&self, op: ReadOp, outcome: ReadOutcome) -> u64 {
        self.reads.get_or_create(&ReadLabels { op, outcome }).get()
    }

    /// Lookups of one read cache, for tests.
    pub fn read_cache_count(&self, cache: CacheName, result: CacheResult) -> u64 {
        self.read_cache
            .get_or_create(&ReadCacheLabels { cache, result })
            .get()
    }

    /// Blob requests refused for a denied file's hash, for tests.
    pub fn blob_withheld_count(&self) -> u64 {
        self.blob_withheld.get()
    }

    /// Text searches that were cut by a cap, for tests.
    pub fn read_search_truncated_count(&self) -> u64 {
        self.read_search_truncated.get()
    }

    /// Sync reports by how they ended, for tests.
    pub fn sync_count(&self, result: SyncResult) -> u64 {
        self.syncs.get_or_create(&SyncLabels { result }).get()
    }

    /// Changes a sync overwrote (C7), for tests.
    pub fn sync_overwritten_count(&self) -> u64 {
        self.sync_overwritten.get()
    }

    /// Baselines set by sync reports, for tests.
    pub fn sync_baselines_count(&self) -> u64 {
        self.sync_baselines.get()
    }

    /// Closed sync windows by how they ended, for tests.
    pub fn window_count(&self, result: WindowResult) -> u64 {
        self.windows.get_or_create(&WindowLabels { result }).get()
    }

    /// Stale-window warnings sent, for tests.
    pub fn window_stale_warning_count(&self) -> u64 {
        self.window_stale_warnings.get()
    }

    /// Episodes of an exhausted hold budget that were said, for tests.
    pub fn hold_budget_exhaustion_count(&self) -> u64 {
        self.hold_budget_exhaustions.get()
    }

    /// Drift alerts by what became of them, for tests.
    pub fn alert_count(&self, result: AlertResult) -> u64 {
        self.alerts.get_or_create(&AlertLabels { result }).get()
    }

    /// Window evaluations made, for tests.
    pub fn alert_evaluation_count(&self) -> u64 {
        self.alert_evaluations.get()
    }

    /// Tenant-suffix reviews by outcome, for tests.
    pub fn rematch_count(&self, result: RematchResult) -> u64 {
        self.rematches.get_or_create(&RematchLabels { result }).get()
    }

    /// Swimlanes handled by the pipeline, by result, for tests.
    pub fn pipeline_lane_count(&self, result: LaneResult) -> u64 {
        self.pipeline_lanes.get_or_create(&LaneLabels { result }).get()
    }

    /// Paths recomputed by the pipeline, for tests.
    pub fn pipeline_paths_count(&self) -> u64 {
        self.pipeline_paths.get()
    }

    /// `file_state` rows written or deleted by the pipeline, for tests.
    pub fn pipeline_rows_count(&self) -> u64 {
        self.pipeline_rows.get()
    }

    /// Paths named in `DriftChanged` events, for tests.
    pub fn pipeline_drift_paths_count(&self) -> u64 {
        self.pipeline_drift_paths.get()
    }

    /// Baselines set by convergence, for tests.
    pub fn pipeline_baselines_count(&self) -> u64 {
        self.pipeline_baselines.get()
    }

    /// Marks cleared, for tests.
    pub fn pipeline_marks_cleared_count(&self) -> u64 {
        self.pipeline_marks_cleared.get()
    }

    /// Paths repaired by a full recompute or the reconcile, for tests.
    pub fn pipeline_divergences_count(&self) -> u64 {
        self.pipeline_divergences.get()
    }

    /// Reconcile scans, for tests.
    pub fn pipeline_reconciles_count(&self) -> u64 {
        self.pipeline_reconciles.get()
    }

    /// Signals by outcome, for tests.
    pub fn pipeline_signal_count(&self, result: SignalResult) -> u64 {
        self.pipeline_signals
            .get_or_create(&SignalLabels { result })
            .get()
    }

    /// Swimlane states in memory, for tests.
    pub fn pipeline_lanes_in_memory_count(&self) -> i64 {
        self.pipeline_lanes_in_memory.get()
    }

    /// The count of blobs by what became of them in the settings index, for tests.
    pub fn index_count(&self, result: IndexResult) -> u64 {
        self.index_blobs.get_or_create(&IndexLabels { result }).get()
    }

    /// The settings rows written, for tests.
    pub fn index_rows_count(&self) -> u64 {
        self.index_rows.get()
    }

    /// The values stored redacted, for tests.
    pub fn index_redacted_count(&self) -> u64 {
        self.index_redacted.get()
    }

    /// The moved heads handled, by outcome, for tests.
    pub fn head_move_count(&self, result: MoveResult) -> u64 {
        self.head_moves.get_or_create(&MoveLabels { result }).get()
    }

    /// Lookups of normalised Git hashes, by hit or miss, for tests.
    pub fn git_hash_count(&self, result: CacheResult) -> u64 {
        self.git_hashes.get_or_create(&CacheLabels { result }).get()
    }

    /// The rows of the effective tables written or deleted, for tests.
    pub fn tree_rows_count(&self) -> u64 {
        self.tree_rows.get()
    }

    /// The count of one message kind and outcome, for tests.
    pub fn message_count(&self, kind: MessageKind, outcome: Outcome) -> u64 {
        self.messages
            .get_or_create(&MessageLabels { kind, outcome })
            .get()
    }

    /// The count of one version result, for tests.
    pub fn version_count(&self, result: VersionResult) -> u64 {
        self.versions.get_or_create(&VersionLabels { result }).get()
    }

    /// The count of out-of-band changes, for tests.
    pub fn out_of_band_count(&self) -> u64 {
        self.out_of_band.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pickup_counters_count_and_register() {
        let m = RegistryMetrics::default();
        m.pickup_lane(PickupResult::Changed);
        m.pickup_services_changed(2);
        m.pickup_services_changed(0);
        m.pickup_truncated();
        assert_eq!(m.pickup_lane_count(PickupResult::Changed), 1);
        assert_eq!(m.pickup_lane_count(PickupResult::Failed), 0);
        assert_eq!(
            m.pickup_services_changed_count(),
            2,
            "a count of zero adds nothing"
        );
        assert_eq!(m.pickup_truncated_count(), 1);
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        assert!(text.contains("registry_pickup_lanes_total"), "{text}");
        assert!(text.contains("registry_pickup_services_changed_total"), "{text}");
        assert!(text.contains("registry_pickup_truncated_total"), "{text}");
        assert!(
            text.contains("result=\"Changed\""),
            "a closed set of labels: {text}"
        );
    }

    #[test]
    fn the_structure_counters_count_and_register_with_closed_labels() {
        let m = RegistryMetrics::default();
        m.structure_lane(StructureResult::Computed);
        m.structure_lane(StructureResult::Computed);
        m.structure_lane(StructureResult::Unchanged);
        m.structure_work(3, 2, 5, 1);
        assert_eq!(m.structure_lane_count(StructureResult::Computed), 2);
        assert_eq!(m.structure_lane_count(StructureResult::Failed), 0);
        assert_eq!(m.structure_blobs_read_count(), 1);
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        for name in [
            "registry_structure_lanes_total",
            "registry_structure_findings_total",
            "registry_structure_c10_evaluated_total",
            "registry_structure_c10_cached_total",
            "registry_structure_blobs_read_total",
        ] {
            assert!(text.contains(name), "{name}: {text}");
        }
        assert!(
            text.contains("result=\"Computed\""),
            "a closed set of labels: {text}"
        );
    }

    #[test]
    fn the_served_verification_counters_count_and_register_with_closed_labels() {
        let m = RegistryMetrics::default();
        m.served_verify_swimlane(ServedSwimlane::Verified);
        m.served_verify_swimlane(ServedSwimlane::Offline);
        m.served_verify_combination(ServedResult::Match);
        m.served_verify_combination(ServedResult::Match);
        m.served_verify_combination(ServedResult::Timeout);
        assert_eq!(m.served_verify_swimlane_count(ServedSwimlane::Verified), 1);
        assert_eq!(m.served_verify_swimlane_count(ServedSwimlane::Failed), 0);
        assert_eq!(m.served_verify_combination_count(ServedResult::Match), 2);
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        for name in [
            "registry_served_verify_swimlanes_total",
            "registry_served_verify_combinations_total",
        ] {
            assert!(text.contains(name), "{name}: {text}");
        }
        assert!(
            text.contains("result=\"Timeout\""),
            "a closed set of labels: {text}"
        );
    }

    #[test]
    fn the_sentinel_counters_count_and_register_with_closed_labels() {
        let m = RegistryMetrics::default();
        m.sentinel_message(SentinelKind::Records, Outcome::Ok);
        m.sentinel_message(SentinelKind::Records, Outcome::Ok);
        m.sentinel_message(SentinelKind::Hello, Outcome::IdentityMismatch);
        m.sentinel_record(SentinelRecord::Stored, 3);
        m.sentinel_record(SentinelRecord::Outside, 0);
        m.sentinel_gap();
        m.attribution(AttributionResult::SentinelOnRecord, 2);
        m.attribution(AttributionResult::NfsClient, 0);
        assert_eq!(m.sentinel_message_count(SentinelKind::Records, Outcome::Ok), 2);
        assert_eq!(
            m.sentinel_message_count(SentinelKind::Hello, Outcome::IdentityMismatch),
            1
        );
        assert_eq!(m.sentinel_message_count(SentinelKind::Hello, Outcome::Ok), 0);
        assert_eq!(m.sentinel_record_count(SentinelRecord::Stored), 3);
        assert_eq!(
            m.sentinel_record_count(SentinelRecord::Outside),
            0,
            "a count of zero adds nothing"
        );
        assert_eq!(m.sentinel_gap_count(), 1);
        assert_eq!(m.attribution_count(AttributionResult::SentinelOnRecord), 2);
        assert_eq!(m.attribution_count(AttributionResult::NfsClient), 0);
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        for name in [
            "registry_sentinel_messages_total",
            "registry_sentinel_records_total",
            "registry_sentinel_gaps_total",
            "registry_attributions_total",
        ] {
            assert!(text.contains(name), "{name}: {text}");
        }
        assert!(
            text.contains("result=\"Stored\""),
            "a closed set of labels: {text}"
        );
    }

    #[test]
    fn counters_count_and_register() {
        let m = RegistryMetrics::default();
        m.message(MessageKind::Delta, Outcome::Ok);
        m.message(MessageKind::Delta, Outcome::Ok);
        m.version(VersionResult::Recorded, 3);
        m.version(VersionResult::Replayed, 0);
        m.out_of_band(2);
        assert_eq!(m.message_count(MessageKind::Delta, Outcome::Ok), 2);
        assert_eq!(m.message_count(MessageKind::Delta, Outcome::Invalid), 0);
        assert_eq!(m.version_count(VersionResult::Recorded), 3);
        assert_eq!(m.version_count(VersionResult::Replayed), 0);
        assert_eq!(m.out_of_band_count(), 2);
        m.index_blob(IndexResult::Indexed, 2);
        m.index_blob(IndexResult::Dropped, 0);
        m.index_rows(5);
        m.head_move(MoveResult::Whole);
        m.git_hash(CacheResult::Hit);
        m.tree_rows(3);
        m.sync(SyncResult::Recorded, 0.5);
        m.sync_overwritten(2);
        m.sync_overwritten(0);
        m.sync_baselines(7);
        m.rematch(RematchResult::Raised);
        m.window(WindowResult::Processed);
        m.window_stale_warning();
        m.hold_budget_exhaustion();
        m.alert(AlertResult::Held, 3);
        m.alert(AlertResult::Sent, 0);
        m.alert_evaluation();
        m.adoption(AdoptionAction::Confirm, AdoptionOutcome::Ok);
        m.adoption(AdoptionAction::Confirm, AdoptionOutcome::Forbidden);
        m.adoption_ranked(0.01);
        m.adoption_secrets(3);
        m.adoption_secrets(0);
        m.adoption_baselines(9);
        m.read(ReadOp::Grid, ReadOutcome::Ok, 0.01);
        m.read(ReadOp::Grid, ReadOutcome::Forbidden, 0.0);
        m.read_cache(CacheName::Rows, CacheResult::Hit);
        m.read_search_truncated();
        m.blob_withheld();
        assert_eq!(m.blob_withheld_count(), 1);
        assert_eq!(m.read_count(ReadOp::Grid, ReadOutcome::Ok), 1);
        assert_eq!(m.read_count(ReadOp::Tree, ReadOutcome::Ok), 0);
        assert_eq!(m.read_cache_count(CacheName::Rows, CacheResult::Hit), 1);
        assert_eq!(m.read_search_truncated_count(), 1);
        assert_eq!(m.adoption_count(AdoptionAction::Confirm, AdoptionOutcome::Ok), 1);
        assert_eq!(m.adoption_count(AdoptionAction::Start, AdoptionOutcome::Ok), 0);
        assert_eq!(m.adoption_secret_hits_count(), 3);
        assert_eq!(m.adoption_baselines_count(), 9);
        assert_eq!(m.sync_count(SyncResult::Recorded), 1);
        assert_eq!(m.sync_count(SyncResult::ScanTimeout), 0);
        assert_eq!(m.sync_overwritten_count(), 2);
        assert_eq!(m.sync_baselines_count(), 7);
        assert_eq!(m.rematch_count(RematchResult::Raised), 1);
        assert_eq!(m.window_count(WindowResult::Processed), 1);
        assert_eq!(m.window_count(WindowResult::GivenUp), 0);
        assert_eq!(m.window_stale_warning_count(), 1);
        assert_eq!(m.hold_budget_exhaustion_count(), 1);
        assert_eq!(m.alert_count(AlertResult::Held), 3);
        assert_eq!(
            m.alert_count(AlertResult::Sent),
            0,
            "a count of zero adds nothing"
        );
        assert_eq!(m.alert_evaluation_count(), 1);
        assert_eq!(m.index_count(IndexResult::Indexed), 2);
        assert_eq!(m.index_count(IndexResult::Dropped), 0);
        assert_eq!(m.index_rows_count(), 5);
        assert_eq!(m.head_move_count(MoveResult::Whole), 1);
        assert_eq!(m.git_hash_count(CacheResult::Hit), 1);
        assert_eq!(m.tree_rows_count(), 3);
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        assert!(text.contains("registry_ingest_messages_total"), "{text}");
        assert!(text.contains("registry_index_blobs_total"), "{text}");
        assert!(text.contains("registry_sync_reports_total"), "{text}");
        assert!(text.contains("registry_tenant_rematches_total"), "{text}");
        assert!(text.contains("registry_sync_windows_total"), "{text}");
        assert!(
            text.contains("registry_sync_window_stale_warnings_total"),
            "{text}"
        );
        assert!(text.contains("registry_drift_alerts_total"), "{text}");
        assert!(text.contains("registry_alert_evaluations_total"), "{text}");
        assert!(text.contains("registry_adoption_calls_total"), "{text}");
        assert!(text.contains("registry_adoption_rank_seconds"), "{text}");
        assert!(text.contains("registry_reads_total"), "{text}");
        assert!(text.contains("registry_read_seconds"), "{text}");
        assert!(text.contains("registry_read_cache_total"), "{text}");
        assert!(!text.contains("sit1"), "no swimlane id is ever a label");
    }

    #[test]
    fn the_retention_counters_count_register_and_carry_no_identity() {
        let m = RegistryMetrics::default();
        m.retention_blobs(RetentionBlobs::Deleted, 3, 3_000);
        m.retention_blobs(RetentionBlobs::WouldDelete, 0, 0);
        m.retention_rows(RetentionTable::SyncWindows, RetentionRows::Deleted, 2);
        m.retention_rows(RetentionTable::DeltaStaging, RetentionRows::WouldDelete, 5);
        m.retention_failure(RetentionStep::Plan);
        m.retention_run(false, true);
        m.retention_run(true, false);
        assert_eq!(m.retention_blob_count(RetentionBlobs::Deleted), 3);
        assert_eq!(m.retention_blob_bytes_count(RetentionBlobs::Deleted), 3_000);
        assert_eq!(
            m.retention_blob_count(RetentionBlobs::WouldDelete),
            0,
            "a count of zero adds nothing"
        );
        assert_eq!(
            m.retention_row_count(RetentionTable::SyncWindows, RetentionRows::Deleted),
            2
        );
        assert_eq!(
            m.retention_row_count(RetentionTable::DeltaStaging, RetentionRows::WouldDelete),
            5
        );
        assert_eq!(m.retention_failure_count(RetentionStep::Plan), 1);
        assert_eq!(m.retention_run_counts(), (1, 1, 1));
        let mut registry = Registry::default();
        m.register(&mut registry);
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap_or_default();
        for name in [
            "registry_retention_blobs_total",
            "registry_retention_blob_bytes_total",
            "registry_retention_rows_total",
            "registry_retention_failures_total",
            "registry_retention_runs_total",
            "registry_retention_dry_runs_total",
            "registry_retention_truncated_total",
        ] {
            assert!(text.contains(name), "{name}: {text}");
        }
    }
}
