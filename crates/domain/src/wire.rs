//! Domain mirrors of the agent and sentinel wire messages (`proto/agent.proto`).
//!
//! These are the validated forms the ports exchange (Q4): paths are [`NfsPath`], hashes are
//! [`ContentHash`], and so on. `crates/proto` converts to and from the generated messages with
//! validating `TryFrom` impls, so a raw `String` path never crosses into a port (S11).
//! They are not `Serialize`: the wire format is protobuf, and several carry file bytes.

use bytes::Bytes;

use crate::{
    ContentHash, Expected, JobRef, NfsPath, RequestId, ServeRequest, ServiceRef, ShortText, SwimlaneId,
    TenantId, Timestamp,
};

// ---------------------------------------------------------------- agent -> hub

/// First message on a `Connect` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub agent_version: ShortText,
    pub swimlane: SwimlaneId,
    pub cluster: ShortText,
    pub project: ShortText,
    pub nfs_server: ShortText,
    pub export: ShortText,
    pub mount_root: ShortText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heartbeat {
    pub scan_seq: u64,
    /// Merkle root of the NFS tree. A changed root makes the hub request a delta.
    pub merkle_root: ContentHash,
    pub file_count: u64,
}

/// What the hub asks for in reply to a heartbeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatAction {
    None,
    RequestDelta { since_root: ContentHash },
    RequestFullScan,
}

/// A changed file in a scan delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub path: NfsPath,
    pub hash: ContentHash,
    pub size: u64,
    pub mtime: Timestamp,
    pub observed_at: Timestamp,
    /// Denied entries (D79) carry name, size and hash but no bytes.
    pub denied: bool,
    pub bytes: Option<Bytes>,
}

/// A file the agent did not read, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    pub path: NfsPath,
    pub reason: ShortText,
}

/// One message of a (possibly multi-message) scan delta, at most 3 MiB (`proto::limits`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanDelta {
    /// Acknowledged by the hub with `Ack{seq}`.
    pub seq: u64,
    /// `None` for a full scan.
    pub base_root: Option<ContentHash>,
    pub new_root: ContentHash,
    pub entries: Vec<ScanEntry>,
    pub removed: Vec<NfsPath>,
    pub skipped: Vec<SkippedEntry>,
    /// The sync Job that was running when these changes were observed (D72).
    pub during_job: Option<JobRef>,
    /// More messages of the same logical delta follow; the hub applies it only when this is false (Q12).
    pub more: bool,
    pub part: u32,
}

impl ScanDelta {
    /// The most file bytes one message may carry (3 MiB), under gRPC's 4 MiB message limit.
    pub const MAX_BYTES: usize = 3 * 1024 * 1024;
    /// The most entries one message may carry.
    pub const MAX_ENTRIES: usize = 10_000;

    /// Total size of the file bytes in this message.
    pub fn payload_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|e| e.bytes.as_ref().map_or(0, Bytes::len))
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncWindowKind {
    Opened,
    Closed,
}

/// A sync Job started or finished (D72).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncWindowEvent {
    pub kind: SyncWindowKind,
    pub job: JobRef,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodInfo {
    pub name: ShortText,
    pub started_at: Timestamp,
}

/// An allowlisted environment variable. Only allowlisted names ever carry a value (D88).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvValue {
    pub name: ShortText,
    pub value: ShortText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentInfo {
    pub service: ServiceRef,
    pub pods: Vec<PodInfo>,
    /// Values of allowlisted variables, such as `CONFIG_CLIENT_CACHE_TTL`.
    pub env_values: Vec<EnvValue>,
    /// Names, never values, of every other environment variable.
    pub env_names: Vec<ShortText>,
    /// In a delta report: the deployment no longer exists.
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseHint {
    pub service: ServiceRef,
    pub key: ShortText,
    pub value: ShortText,
}

/// Cluster state from the agent, full or as a delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterReport {
    pub full: bool,
    pub deployments: Vec<DeploymentInfo>,
    pub release_hints: Vec<ReleaseHint>,
    /// When the config-server pod started (C11).
    pub config_server_started_at: Option<Timestamp>,
    pub sync_windows: Vec<SyncWindowEvent>,
}

// ---------------------------------------------------------------- hub -> agent

/// Settings the hub gives an agent after `Hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub scan_interval_secs: u32,
    pub heartbeat_interval_secs: u32,
    pub max_file_bytes: u64,
    /// Files matching these globs are recorded by name, size and hash only (D79).
    pub deny_globs: Vec<ShortText>,
    /// Environment variables whose values the agent may report (D88).
    pub env_allowlist: Vec<ShortText>,
    pub tenants: Vec<TenantId>,
}

/// A request/response command for `AgentGateway::request`. The stream-only messages (`AgentConfig`,
/// `Ack`, certificate renewal) are handled inside the gateway and are not commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubCommand {
    RequestDelta {
        since_root: ContentHash,
    },
    RequestFullScan,
    ReadFile {
        request_id: RequestId,
        path: NfsPath,
    },
    /// Never a blind write: the agent compares `expected` with the file and refuses on a mismatch.
    WriteFile {
        request_id: RequestId,
        path: NfsPath,
        expected: Expected,
        bytes: Bytes,
    },
    DeleteFile {
        request_id: RequestId,
        path: NfsPath,
        expected: ContentHash,
    },
    RestartDeployment {
        request_id: RequestId,
        service: ServiceRef,
    },
    RequestClusterReport {
        request_id: RequestId,
    },
    /// The agent POSTs these paths to the config-server's `/update-resources` (D86).
    NotifyConfigServer {
        request_id: RequestId,
        paths: Vec<NfsPath>,
    },
    /// The agent GETs the config-server's answer for this request (D88).
    FetchServed {
        request_id: RequestId,
        request: ServeRequest,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpError {
    /// The expected hash did not match.
    Conflict,
    NotFound,
    Denied,
    Io,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpResult {
    pub request_id: RequestId,
    pub ok: bool,
    pub error: Option<OpError>,
    /// The hash found on NFS after the operation, or on a conflict.
    pub current_hash: Option<ContentHash>,
}

/// What an agent answers to a [`HubCommand`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentReply {
    Op(OpResult),
    File {
        request_id: RequestId,
        path: NfsPath,
        hash: ContentHash,
        bytes: Bytes,
    },
    Served {
        request_id: RequestId,
        status: u16,
        bytes: Bytes,
    },
    Cluster {
        request_id: RequestId,
        report: ClusterReport,
    },
}

// ---------------------------------------------------------------- sentinel

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentinelHello {
    pub vm: ShortText,
    pub export_root: ShortText,
    pub version: ShortText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentinelConfig {
    pub max_batch_records: u32,
    pub flush_interval_secs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOperation {
    Create,
    Write,
    Delete,
    Rename,
    Attribute,
}

/// One local audit record from the NFS VM (D72).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub time: Timestamp,
    pub path: NfsPath,
    pub operation: AuditOperation,
    pub success: bool,
    pub login_user: ShortText,
    pub effective_user: ShortText,
    pub exe: ShortText,
    pub comm: ShortText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecordBatch {
    pub seq: u64,
    pub records: Vec<AuditRecord>,
}
