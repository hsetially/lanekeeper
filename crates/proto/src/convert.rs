//! Validating conversions between the generated messages and the domain wire types (Q4, S11).
//!
//! This is the one place where wire data becomes typed: paths become [`domain::NfsPath`], hashes
//! [`domain::ContentHash`], names [`domain::ShortText`] and so on, and every size limit in [`crate::limits`]
//! is checked. The hub, the agent and the sentinel use these conversions and never read a generated message
//! directly, so a raw `String` path cannot cross into a port.
//!
//! - Wire to domain is fallible (`TryFrom`, or `from_proto` for a whole stream message). Errors name the
//!   field and never contain the offending value, so a secret-looking value cannot reach a log (S21).
//! - Domain to wire is infallible (`From`, or `into_proto`).
//! - A stream message whose oneof is empty or unknown (a newer peer, or a reserved number) converts to
//!   `Ok(None)`: the caller skips it and keeps the stream open.

use bytes::Bytes;
use domain::{
    AgentConfig, AgentReply, AppName, AuditOperation, AuditRecord, AuditRecordBatch, ChannelName,
    ClusterReport, ContentHash, DeploymentInfo, EnvValue, Expected, Heartbeat, Hello, HubCommand, IdError,
    JobRef, NfsPath, OpError, OpResult, PodInfo, ReleaseHint, RequestId, ScanDelta, ScanEntry, Secret,
    SentinelConfig, SentinelHello, ServeRequest, ServiceRef, ShortText, SkippedEntry, SpoolGap, SwimlaneId,
    SyncWindowEvent, SyncWindowKind, TenantId, Timestamp,
};

use crate::limits;
use crate::pb;

/// Why a wire message was rejected. `&'static str` names the field, for example `scan_delta.entries.path`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    /// A required field or oneof variant is absent or empty.
    #[error("required field `{0}` is missing")]
    Missing(&'static str),
    /// The value is malformed: a bad path, a hash that is not 32 bytes, an unknown enum value.
    #[error("field `{0}` is not valid")]
    Invalid(&'static str),
    /// A size or count limit from [`crate::limits`] is exceeded.
    #[error("field `{0}` is too large")]
    TooLarge(&'static str),
}

type Result<T> = std::result::Result<T, ConvertError>;

// ---------------------------------------------------------------- helpers

fn hash(field: &'static str, raw: &[u8]) -> Result<ContentHash> {
    let bytes: [u8; 32] = raw.try_into().map_err(|_| ConvertError::Invalid(field))?;
    Ok(ContentHash::from_bytes(bytes))
}

fn opt_hash(field: &'static str, raw: Option<&Bytes>) -> Result<Option<ContentHash>> {
    raw.map(|b| hash(field, b)).transpose()
}

fn nfs_path(field: &'static str, raw: &str) -> Result<NfsPath> {
    NfsPath::parse(raw).map_err(|_| ConvertError::Invalid(field))
}

fn short(field: &'static str, raw: &str) -> Result<ShortText> {
    ShortText::parse(raw).map_err(|_| ConvertError::Invalid(field))
}

fn request_id(field: &'static str, raw: &str) -> Result<RequestId> {
    RequestId::parse(raw).map_err(|_: IdError| ConvertError::Invalid(field))
}

fn at_most(field: &'static str, len: usize, max: usize) -> Result<()> {
    if len > max {
        Err(ConvertError::TooLarge(field))
    } else {
        Ok(())
    }
}

fn file_bytes(field: &'static str, raw: Bytes) -> Result<Bytes> {
    at_most(field, raw.len(), limits::MAX_FILE_BYTES)?;
    Ok(raw)
}

/// Converts every item of a repeated field, after checking the count (so an oversized list is cheap to refuse).
fn list<T, U>(
    field: &'static str,
    items: Vec<T>,
    max: usize,
    convert: impl FnMut(T) -> Result<U>,
) -> Result<Vec<U>> {
    at_most(field, items.len(), max)?;
    items.into_iter().map(convert).collect()
}

/// An HTTP status the config-server answered with (100 to 599).
fn http_status(field: &'static str, raw: u32) -> Result<u16> {
    u16::try_from(raw)
        .ok()
        .filter(|status| (100..=599).contains(status))
        .ok_or(ConvertError::Invalid(field))
}

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(ms)
}

fn service_ref(field: &'static str, raw: Option<pb::ServiceRef>) -> Result<ServiceRef> {
    let raw = raw.ok_or(ConvertError::Missing(field))?;
    ServiceRef::new(&raw.namespace, &raw.name).map_err(|_| ConvertError::Invalid(field))
}

fn job_ref(field: &'static str, raw: Option<pb::JobRef>) -> Result<JobRef> {
    let raw = raw.ok_or(ConvertError::Missing(field))?;
    JobRef::new(&raw.name, &raw.uid).map_err(|_| ConvertError::Invalid(field))
}

fn pb_service(s: &ServiceRef) -> pb::ServiceRef {
    pb::ServiceRef {
        namespace: s.namespace().to_owned(),
        name: s.name().to_owned(),
    }
}

fn pb_job(j: &JobRef) -> pb::JobRef {
    pb::JobRef {
        name: j.name().to_owned(),
        uid: j.uid().to_owned(),
    }
}

fn pb_hash(h: &ContentHash) -> Bytes {
    Bytes::copy_from_slice(h.as_bytes())
}

// ---------------------------------------------------------------- scan entries and deltas

impl TryFrom<pb::ScanEntry> for ScanEntry {
    type Error = ConvertError;

    fn try_from(e: pb::ScanEntry) -> Result<Self> {
        if e.denied && e.content.is_some() {
            // A denied file's bytes must never leave the NFS mount (D79).
            return Err(ConvertError::Invalid("scan_delta.entries.content"));
        }
        Ok(Self {
            path: nfs_path("scan_delta.entries.path", &e.path)?,
            hash: hash("scan_delta.entries.hash", &e.hash)?,
            size: e.size,
            mtime: ts(e.mtime_ms),
            observed_at: ts(e.observed_at_ms),
            denied: e.denied,
            bytes: e.content,
        })
    }
}

impl From<ScanEntry> for pb::ScanEntry {
    fn from(e: ScanEntry) -> Self {
        Self {
            path: e.path.as_str().to_owned(),
            hash: pb_hash(&e.hash),
            size: e.size,
            mtime_ms: e.mtime.unix_millis(),
            observed_at_ms: e.observed_at.unix_millis(),
            denied: e.denied,
            content: e.bytes,
        }
    }
}

impl TryFrom<pb::SkippedEntry> for SkippedEntry {
    type Error = ConvertError;

    fn try_from(s: pb::SkippedEntry) -> Result<Self> {
        Ok(Self {
            path: nfs_path("scan_delta.skipped.path", &s.path)?,
            reason: short("scan_delta.skipped.reason", &s.reason)?,
        })
    }
}

impl From<SkippedEntry> for pb::SkippedEntry {
    fn from(s: SkippedEntry) -> Self {
        Self {
            path: s.path.as_str().to_owned(),
            reason: s.reason.as_str().to_owned(),
        }
    }
}

impl TryFrom<pb::SpoolGap> for SpoolGap {
    type Error = ConvertError;

    /// A gap that ends before it starts is malformed: the receiver would otherwise compare roots over a
    /// negative range and could decide that no history was lost.
    fn try_from(g: pb::SpoolGap) -> Result<Self> {
        if g.to_ms < g.from_ms {
            return Err(ConvertError::Invalid("scan_delta.gap"));
        }
        Ok(Self {
            from: ts(g.from_ms),
            to: ts(g.to_ms),
            lost_entries: g.lost_entries,
        })
    }
}

impl From<SpoolGap> for pb::SpoolGap {
    fn from(g: SpoolGap) -> Self {
        Self {
            from_ms: g.from.unix_millis(),
            to_ms: g.to.unix_millis(),
            lost_entries: g.lost_entries,
        }
    }
}

impl TryFrom<pb::ScanDelta> for ScanDelta {
    type Error = ConvertError;

    fn try_from(d: pb::ScanDelta) -> Result<Self> {
        let delta = Self {
            seq: d.seq,
            base_root: opt_hash("scan_delta.base_root", d.base_root.as_ref())?,
            new_root: hash("scan_delta.new_root", &d.new_root)?,
            entries: list(
                "scan_delta.entries",
                d.entries,
                limits::MAX_ENTRIES,
                ScanEntry::try_from,
            )?,
            removed: list("scan_delta.removed", d.removed, limits::MAX_ENTRIES, |p| {
                nfs_path("scan_delta.removed", &p)
            })?,
            skipped: list(
                "scan_delta.skipped",
                d.skipped,
                limits::MAX_ENTRIES,
                SkippedEntry::try_from,
            )?,
            during_job: d
                .during_job
                .map(|j| job_ref("scan_delta.during_job", Some(j)))
                .transpose()?,
            more: d.more,
            part: d.part,
            gap: d.gap.map(SpoolGap::try_from).transpose()?,
        };
        at_most(
            "scan_delta.entries.content",
            delta.payload_bytes(),
            limits::MAX_SCAN_DELTA_BYTES,
        )?;
        Ok(delta)
    }
}

impl From<ScanDelta> for pb::ScanDelta {
    fn from(d: ScanDelta) -> Self {
        Self {
            seq: d.seq,
            base_root: d.base_root.as_ref().map(pb_hash),
            new_root: pb_hash(&d.new_root),
            entries: d.entries.into_iter().map(Into::into).collect(),
            removed: d.removed.iter().map(|p| p.as_str().to_owned()).collect(),
            skipped: d.skipped.into_iter().map(Into::into).collect(),
            during_job: d.during_job.as_ref().map(pb_job),
            more: d.more,
            part: d.part,
            gap: d.gap.map(Into::into),
        }
    }
}

// ---------------------------------------------------------------- hello, heartbeat

impl TryFrom<pb::Hello> for Hello {
    type Error = ConvertError;

    fn try_from(h: pb::Hello) -> Result<Self> {
        Ok(Self {
            agent_version: short("hello.agent_version", &h.agent_version)?,
            swimlane: SwimlaneId::parse(&h.swimlane).map_err(|_| ConvertError::Invalid("hello.swimlane"))?,
            cluster: short("hello.cluster", &h.cluster)?,
            project: short("hello.project", &h.project)?,
            nfs_server: short("hello.nfs_server", &h.nfs_server)?,
            export: short("hello.export", &h.export)?,
            mount_root: short("hello.mount_root", &h.mount_root)?,
        })
    }
}

impl From<Hello> for pb::Hello {
    fn from(h: Hello) -> Self {
        Self {
            agent_version: h.agent_version.as_str().to_owned(),
            swimlane: h.swimlane.as_str().to_owned(),
            cluster: h.cluster.as_str().to_owned(),
            project: h.project.as_str().to_owned(),
            nfs_server: h.nfs_server.as_str().to_owned(),
            export: h.export.as_str().to_owned(),
            mount_root: h.mount_root.as_str().to_owned(),
        }
    }
}

impl TryFrom<pb::Heartbeat> for Heartbeat {
    type Error = ConvertError;

    fn try_from(h: pb::Heartbeat) -> Result<Self> {
        Ok(Self {
            scan_seq: h.scan_seq,
            merkle_root: hash("heartbeat.merkle_root", &h.merkle_root)?,
            file_count: h.file_count,
        })
    }
}

impl From<Heartbeat> for pb::Heartbeat {
    fn from(h: Heartbeat) -> Self {
        Self {
            scan_seq: h.scan_seq,
            merkle_root: pb_hash(&h.merkle_root),
            file_count: h.file_count,
        }
    }
}

// ---------------------------------------------------------------- operation results and replies

fn op_error(raw: i32) -> Option<OpError> {
    match pb::OpErrorCode::try_from(raw) {
        Ok(pb::OpErrorCode::Conflict) => Some(OpError::Conflict),
        Ok(pb::OpErrorCode::NotFound) => Some(OpError::NotFound),
        Ok(pb::OpErrorCode::Denied) => Some(OpError::Denied),
        Ok(pb::OpErrorCode::Io) => Some(OpError::Io),
        Ok(pb::OpErrorCode::Unsupported) => Some(OpError::Unsupported),
        // Unspecified or a code this build does not know: the caller decides (see `TryFrom<pb::OpResult>`).
        Ok(pb::OpErrorCode::Unspecified) | Err(_) => None,
    }
}

fn pb_op_error(e: Option<OpError>) -> pb::OpErrorCode {
    match e {
        None => pb::OpErrorCode::Unspecified,
        Some(OpError::Conflict) => pb::OpErrorCode::Conflict,
        Some(OpError::NotFound) => pb::OpErrorCode::NotFound,
        Some(OpError::Denied) => pb::OpErrorCode::Denied,
        Some(OpError::Io) => pb::OpErrorCode::Io,
        Some(OpError::Unsupported) => pb::OpErrorCode::Unsupported,
    }
}

impl TryFrom<pb::OpResult> for OpResult {
    type Error = ConvertError;

    fn try_from(r: pb::OpResult) -> Result<Self> {
        let error = op_error(r.error_code);
        // Success has no code; failure has exactly one, and a code this build cannot name is a failure we
        // could not explain, so it is refused rather than guessed.
        let consistent = if r.ok {
            r.error_code == pb::OpErrorCode::Unspecified as i32
        } else {
            error.is_some()
        };
        if !consistent {
            return Err(ConvertError::Invalid("op_result.error_code"));
        }
        Ok(Self {
            request_id: request_id("op_result.request_id", &r.request_id)?,
            ok: r.ok,
            error,
            current_hash: opt_hash("op_result.current_hash", r.current_hash.as_ref())?,
        })
    }
}

impl From<OpResult> for pb::OpResult {
    fn from(r: OpResult) -> Self {
        Self {
            request_id: r.request_id.as_str().to_owned(),
            ok: r.ok,
            error_code: pb_op_error(r.error) as i32,
            current_hash: r.current_hash.as_ref().map(pb_hash),
        }
    }
}

// ---------------------------------------------------------------- cluster report

impl TryFrom<pb::DeploymentInfo> for DeploymentInfo {
    type Error = ConvertError;

    fn try_from(d: pb::DeploymentInfo) -> Result<Self> {
        Ok(Self {
            service: service_ref("cluster_report.deployments.service", d.service)?,
            pods: list(
                "cluster_report.deployments.pods",
                d.pods,
                limits::MAX_ENTRIES,
                |p| {
                    Ok(PodInfo {
                        name: short("cluster_report.deployments.pods.name", &p.name)?,
                        started_at: ts(p.started_at_ms),
                    })
                },
            )?,
            env_values: list(
                "cluster_report.deployments.env_values",
                d.env_values,
                limits::MAX_ENTRIES,
                |v| {
                    Ok(EnvValue {
                        name: short("cluster_report.deployments.env_values.name", &v.name)?,
                        value: short("cluster_report.deployments.env_values.value", &v.value)?,
                    })
                },
            )?,
            env_names: list(
                "cluster_report.deployments.env_names",
                d.env_names,
                limits::MAX_ENTRIES,
                |n| short("cluster_report.deployments.env_names", &n),
            )?,
            removed: d.removed,
        })
    }
}

impl From<DeploymentInfo> for pb::DeploymentInfo {
    fn from(d: DeploymentInfo) -> Self {
        Self {
            service: Some(pb_service(&d.service)),
            pods: d
                .pods
                .into_iter()
                .map(|p| pb::PodInfo {
                    name: p.name.as_str().to_owned(),
                    started_at_ms: p.started_at.unix_millis(),
                })
                .collect(),
            env_values: d
                .env_values
                .into_iter()
                .map(|v| pb::EnvValue {
                    name: v.name.as_str().to_owned(),
                    value: v.value.as_str().to_owned(),
                })
                .collect(),
            env_names: d.env_names.iter().map(|n| n.as_str().to_owned()).collect(),
            removed: d.removed,
        }
    }
}

fn sync_kind(raw: i32) -> Result<SyncWindowKind> {
    match pb::SyncWindowKind::try_from(raw) {
        Ok(pb::SyncWindowKind::Opened) => Ok(SyncWindowKind::Opened),
        Ok(pb::SyncWindowKind::Closed) => Ok(SyncWindowKind::Closed),
        Ok(pb::SyncWindowKind::Unspecified) | Err(_) => {
            Err(ConvertError::Invalid("cluster_report.sync_windows.kind"))
        }
    }
}

impl TryFrom<pb::ClusterReport> for ClusterReport {
    type Error = ConvertError;

    /// The `request_id` is not part of the domain report; [`FromAgent::from_proto`] reads it.
    fn try_from(c: pb::ClusterReport) -> Result<Self> {
        Ok(Self {
            full: c.full,
            deployments: list(
                "cluster_report.deployments",
                c.deployments,
                limits::MAX_ENTRIES,
                DeploymentInfo::try_from,
            )?,
            release_hints: list(
                "cluster_report.release_hints",
                c.release_hints,
                limits::MAX_ENTRIES,
                |h| {
                    Ok(ReleaseHint {
                        service: service_ref("cluster_report.release_hints.service", h.service)?,
                        key: short("cluster_report.release_hints.key", &h.key)?,
                        value: short("cluster_report.release_hints.value", &h.value)?,
                    })
                },
            )?,
            config_server_started_at: c.config_server_started_at_ms.map(ts),
            sync_windows: list(
                "cluster_report.sync_windows",
                c.sync_windows,
                limits::MAX_ENTRIES,
                |w| {
                    Ok(SyncWindowEvent {
                        kind: sync_kind(w.kind)?,
                        job: job_ref("cluster_report.sync_windows.job", w.job)?,
                        at: ts(w.at_ms),
                    })
                },
            )?,
        })
    }
}

fn pb_cluster(c: ClusterReport, request_id: String) -> pb::ClusterReport {
    pb::ClusterReport {
        full: c.full,
        deployments: c.deployments.into_iter().map(Into::into).collect(),
        release_hints: c
            .release_hints
            .into_iter()
            .map(|h| pb::ReleaseHint {
                service: Some(pb_service(&h.service)),
                key: h.key.as_str().to_owned(),
                value: h.value.as_str().to_owned(),
            })
            .collect(),
        config_server_started_at_ms: c.config_server_started_at.map(Timestamp::unix_millis),
        sync_windows: c
            .sync_windows
            .into_iter()
            .map(|w| pb::SyncWindowEvent {
                kind: match w.kind {
                    SyncWindowKind::Opened => pb::SyncWindowKind::Opened,
                    SyncWindowKind::Closed => pb::SyncWindowKind::Closed,
                } as i32,
                job: Some(pb_job(&w.job)),
                at_ms: w.at.unix_millis(),
            })
            .collect(),
        request_id,
    }
}

impl From<ClusterReport> for pb::ClusterReport {
    /// An unsolicited report (empty `request_id`).
    fn from(c: ClusterReport) -> Self {
        pb_cluster(c, String::new())
    }
}

// ---------------------------------------------------------------- agent -> hub stream

/// What an agent sends on a `Connect` stream, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromAgent {
    Hello(Hello),
    Heartbeat(Heartbeat),
    /// One message of a scan delta (`more` says whether others follow).
    Delta(ScanDelta),
    /// A cluster report the agent sent on its own.
    Cluster(ClusterReport),
    /// The answer to a [`HubCommand`]: an operation result, file content, a served response, a notify
    /// result, or a cluster report that carries the request id.
    Reply(AgentReply),
    /// The agent asks for a new certificate before the current one expires (S5).
    CertRenewal {
        csr_der: Bytes,
    },
}

fn token(field: &'static str, raw: String) -> Result<String> {
    if raw.is_empty() {
        return Err(ConvertError::Missing(field));
    }
    at_most(field, raw.len(), limits::MAX_TOKEN_BYTES)?;
    Ok(raw)
}

fn csr(field: &'static str, raw: Bytes) -> Result<Bytes> {
    if raw.is_empty() {
        return Err(ConvertError::Missing(field));
    }
    at_most(field, raw.len(), limits::MAX_CSR_BYTES)?;
    Ok(raw)
}

impl FromAgent {
    /// `Ok(None)` when the oneof is empty or holds a variant this build does not know.
    pub fn from_proto(m: pb::AgentMessage) -> Result<Option<Self>> {
        use pb::agent_message::Kind;
        let Some(kind) = m.kind else { return Ok(None) };
        Ok(Some(match kind {
            Kind::Hello(h) => Self::Hello(h.try_into()?),
            Kind::Heartbeat(h) => Self::Heartbeat(h.try_into()?),
            Kind::ScanDelta(d) => Self::Delta(d.try_into()?),
            Kind::OpResult(r) => Self::Reply(AgentReply::Op(r.try_into()?)),
            Kind::ClusterReport(c) => {
                let id = c.request_id.clone();
                let report = ClusterReport::try_from(c)?;
                if id.is_empty() {
                    Self::Cluster(report)
                } else {
                    Self::Reply(AgentReply::Cluster {
                        request_id: request_id("cluster_report.request_id", &id)?,
                        report,
                    })
                }
            }
            Kind::CertRenewalRequest(r) => Self::CertRenewal {
                csr_der: csr("cert_renewal_request.csr_der", r.csr_der)?,
            },
            Kind::FileContent(f) => Self::Reply(AgentReply::File {
                request_id: request_id("file_content.request_id", &f.request_id)?,
                path: nfs_path("file_content.path", &f.path)?,
                hash: hash("file_content.hash", &f.hash)?,
                bytes: file_bytes("file_content.content", f.content)?,
            }),
            Kind::ServedResponse(s) => Self::Reply(AgentReply::Served {
                request_id: request_id("served_response.request_id", &s.request_id)?,
                status: http_status("served_response.status", s.status)?,
                bytes: file_bytes("served_response.body", s.body)?,
            }),
            Kind::NotifyResult(n) => Self::Reply(AgentReply::Notify {
                request_id: request_id("notify_result.request_id", &n.request_id)?,
                status: http_status("notify_result.status", n.status)?,
            }),
        }))
    }

    pub fn into_proto(self) -> pb::AgentMessage {
        use pb::agent_message::Kind;
        let kind = match self {
            Self::Hello(h) => Kind::Hello(h.into()),
            Self::Heartbeat(h) => Kind::Heartbeat(h.into()),
            Self::Delta(d) => Kind::ScanDelta(d.into()),
            Self::Cluster(c) => Kind::ClusterReport(c.into()),
            Self::CertRenewal { csr_der } => Kind::CertRenewalRequest(pb::CertRenewalRequest { csr_der }),
            Self::Reply(AgentReply::Op(r)) => Kind::OpResult(r.into()),
            Self::Reply(AgentReply::Cluster { request_id, report }) => {
                Kind::ClusterReport(pb_cluster(report, request_id.as_str().to_owned()))
            }
            Self::Reply(AgentReply::File {
                request_id,
                path,
                hash,
                bytes,
            }) => Kind::FileContent(pb::FileContent {
                request_id: request_id.as_str().to_owned(),
                path: path.as_str().to_owned(),
                hash: pb_hash(&hash),
                content: bytes,
            }),
            Self::Reply(AgentReply::Served {
                request_id,
                status,
                bytes,
            }) => Kind::ServedResponse(pb::ServedResponse {
                request_id: request_id.as_str().to_owned(),
                status: u32::from(status),
                body: bytes,
            }),
            Self::Reply(AgentReply::Notify { request_id, status }) => Kind::NotifyResult(pb::NotifyResult {
                request_id: request_id.as_str().to_owned(),
                status: u32::from(status),
            }),
        };
        pb::AgentMessage { kind: Some(kind) }
    }
}

// ---------------------------------------------------------------- hub -> agent stream

/// A certificate chain issued by the hub (S5), as a join response or a renewal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCert {
    /// DER, leaf first.
    pub cert_chain_der: Vec<Bytes>,
    pub not_after: Timestamp,
}

fn issued(field: &'static str, chain: Vec<Bytes>, not_after_ms: i64) -> Result<IssuedCert> {
    if chain.is_empty() {
        return Err(ConvertError::Missing(field));
    }
    at_most(field, chain.len(), limits::MAX_CERT_CHAIN)?;
    if chain
        .iter()
        .any(|c| c.is_empty() || c.len() > limits::MAX_CERT_BYTES)
    {
        return Err(ConvertError::Invalid(field));
    }
    Ok(IssuedCert {
        cert_chain_der: chain,
        not_after: ts(not_after_ms),
    })
}

impl TryFrom<pb::JoinResponse> for IssuedCert {
    type Error = ConvertError;

    fn try_from(r: pb::JoinResponse) -> Result<Self> {
        issued("join_response.cert_chain_der", r.cert_chain_der, r.not_after_ms)
    }
}

impl From<IssuedCert> for pb::JoinResponse {
    fn from(c: IssuedCert) -> Self {
        Self {
            cert_chain_der: c.cert_chain_der,
            not_after_ms: c.not_after.unix_millis(),
        }
    }
}

impl TryFrom<pb::CertRenewalResponse> for IssuedCert {
    type Error = ConvertError;

    fn try_from(r: pb::CertRenewalResponse) -> Result<Self> {
        issued(
            "cert_renewal_response.cert_chain_der",
            r.cert_chain_der,
            r.not_after_ms,
        )
    }
}

impl From<IssuedCert> for pb::CertRenewalResponse {
    fn from(c: IssuedCert) -> Self {
        Self {
            cert_chain_der: c.cert_chain_der,
            not_after_ms: c.not_after.unix_millis(),
        }
    }
}

impl TryFrom<pb::AgentConfig> for AgentConfig {
    type Error = ConvertError;

    fn try_from(c: pb::AgentConfig) -> Result<Self> {
        // A zero interval would make the agent scan or beat in a tight loop.
        if c.scan_interval_secs == 0 {
            return Err(ConvertError::Invalid("agent_config.scan_interval_secs"));
        }
        if c.heartbeat_interval_secs == 0 {
            return Err(ConvertError::Invalid("agent_config.heartbeat_interval_secs"));
        }
        Ok(Self {
            scan_interval_secs: c.scan_interval_secs,
            heartbeat_interval_secs: c.heartbeat_interval_secs,
            max_file_bytes: c.max_file_bytes,
            deny_globs: list(
                "agent_config.deny_globs",
                c.deny_globs,
                limits::MAX_CONFIG_ITEMS,
                |g| short("agent_config.deny_globs", &g),
            )?,
            env_allowlist: list(
                "agent_config.env_allowlist",
                c.env_allowlist,
                limits::MAX_CONFIG_ITEMS,
                |n| short("agent_config.env_allowlist", &n),
            )?,
            tenants: list("agent_config.tenants", c.tenants, limits::MAX_CONFIG_ITEMS, |t| {
                TenantId::parse(&t).map_err(|_| ConvertError::Invalid("agent_config.tenants"))
            })?,
        })
    }
}

impl From<AgentConfig> for pb::AgentConfig {
    fn from(c: AgentConfig) -> Self {
        Self {
            scan_interval_secs: c.scan_interval_secs,
            heartbeat_interval_secs: c.heartbeat_interval_secs,
            max_file_bytes: c.max_file_bytes,
            deny_globs: c.deny_globs.iter().map(|g| g.as_str().to_owned()).collect(),
            env_allowlist: c.env_allowlist.iter().map(|n| n.as_str().to_owned()).collect(),
            tenants: c.tenants.iter().map(|t| t.as_str().to_owned()).collect(),
        }
    }
}

fn expected(field: &'static str, raw: Option<pb::Expected>) -> Result<Expected> {
    use pb::expected::State;
    match raw.and_then(|e| e.state) {
        None => Err(ConvertError::Missing(field)),
        Some(State::Absent(_)) => Ok(Expected::Absent),
        Some(State::Hash(h)) => Ok(Expected::Hash {
            hash: hash("write_file.expected.hash", &h)?,
        }),
    }
}

fn pb_expected(e: &Expected) -> pb::Expected {
    use pb::expected::State;
    pb::Expected {
        state: Some(match e {
            Expected::Absent => State::Absent(pb::expected::Absent {}),
            Expected::Hash { hash } => State::Hash(pb_hash(hash)),
        }),
    }
}

/// What the hub sends on a `Connect` stream, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToAgent {
    Config(AgentConfig),
    /// A request the agent must answer with [`FromAgent::Reply`] (or, for scans, with deltas).
    Command(HubCommand),
    /// The hub durably applied the scan delta message with this `seq`.
    Ack(u64),
    /// The answer to [`FromAgent::CertRenewal`].
    CertRenewal(IssuedCert),
}

impl ToAgent {
    /// `Ok(None)` when the oneof is empty or holds a variant this build does not know.
    pub fn from_proto(m: pb::HubMessage) -> Result<Option<Self>> {
        use pb::hub_message::Kind;
        let Some(kind) = m.kind else { return Ok(None) };
        Ok(Some(match kind {
            Kind::AgentConfig(c) => Self::Config(c.try_into()?),
            Kind::Ack(a) => Self::Ack(a.seq),
            Kind::CertRenewalResponse(r) => Self::CertRenewal(r.try_into()?),
            Kind::RequestDelta(r) => Self::Command(HubCommand::RequestDelta {
                since_root: hash("request_delta.since_root", &r.since_root)?,
            }),
            Kind::RequestFullScan(_) => Self::Command(HubCommand::RequestFullScan),
            Kind::ReadFile(r) => Self::Command(HubCommand::ReadFile {
                request_id: request_id("read_file.request_id", &r.request_id)?,
                path: nfs_path("read_file.path", &r.path)?,
            }),
            Kind::WriteFile(w) => Self::Command(HubCommand::WriteFile {
                request_id: request_id("write_file.request_id", &w.request_id)?,
                path: nfs_path("write_file.path", &w.path)?,
                expected: expected("write_file.expected", w.expected)?,
                bytes: file_bytes("write_file.content", w.content)?,
            }),
            Kind::DeleteFile(d) => Self::Command(HubCommand::DeleteFile {
                request_id: request_id("delete_file.request_id", &d.request_id)?,
                path: nfs_path("delete_file.path", &d.path)?,
                expected: hash("delete_file.expected_hash", &d.expected_hash)?,
            }),
            Kind::RestartDeployment(r) => Self::Command(HubCommand::RestartDeployment {
                request_id: request_id("restart_deployment.request_id", &r.request_id)?,
                service: service_ref("restart_deployment.service", r.service)?,
            }),
            Kind::RequestClusterReport(r) => Self::Command(HubCommand::RequestClusterReport {
                request_id: request_id("request_cluster_report.request_id", &r.request_id)?,
            }),
            Kind::NotifyConfigServer(n) => Self::Command(HubCommand::NotifyConfigServer {
                request_id: request_id("notify_config_server.request_id", &n.request_id)?,
                paths: list(
                    "notify_config_server.paths",
                    n.paths,
                    limits::MAX_NOTIFY_PATHS,
                    |p| nfs_path("notify_config_server.paths", &p),
                )?,
            }),
            Kind::FetchServed(f) => Self::Command(HubCommand::FetchServed {
                request_id: request_id("fetch_served.request_id", &f.request_id)?,
                request: ServeRequest {
                    application: AppName::parse(&f.application)
                        .map_err(|_| ConvertError::Invalid("fetch_served.application"))?,
                    tenant: TenantId::parse(&f.tenant)
                        .map_err(|_| ConvertError::Invalid("fetch_served.tenant"))?,
                    channel: f
                        .channel
                        .map(|c| ChannelName::parse(&c))
                        .transpose()
                        .map_err(|_| ConvertError::Invalid("fetch_served.channel"))?,
                    file: nfs_path("fetch_served.file", &f.file)?,
                },
            }),
        }))
    }

    pub fn into_proto(self) -> pb::HubMessage {
        use pb::hub_message::Kind;
        let kind = match self {
            Self::Config(c) => Kind::AgentConfig(c.into()),
            Self::Ack(seq) => Kind::Ack(pb::Ack { seq }),
            Self::CertRenewal(c) => Kind::CertRenewalResponse(c.into()),
            Self::Command(HubCommand::RequestDelta { since_root }) => Kind::RequestDelta(pb::RequestDelta {
                since_root: pb_hash(&since_root),
            }),
            Self::Command(HubCommand::RequestFullScan) => Kind::RequestFullScan(pb::RequestFullScan {}),
            Self::Command(HubCommand::ReadFile { request_id, path }) => Kind::ReadFile(pb::ReadFile {
                request_id: request_id.as_str().to_owned(),
                path: path.as_str().to_owned(),
            }),
            Self::Command(HubCommand::WriteFile {
                request_id,
                path,
                expected,
                bytes,
            }) => Kind::WriteFile(pb::WriteFile {
                request_id: request_id.as_str().to_owned(),
                path: path.as_str().to_owned(),
                expected: Some(pb_expected(&expected)),
                content: bytes,
            }),
            Self::Command(HubCommand::DeleteFile {
                request_id,
                path,
                expected,
            }) => Kind::DeleteFile(pb::DeleteFile {
                request_id: request_id.as_str().to_owned(),
                path: path.as_str().to_owned(),
                expected_hash: pb_hash(&expected),
            }),
            Self::Command(HubCommand::RestartDeployment { request_id, service }) => {
                Kind::RestartDeployment(pb::RestartDeployment {
                    request_id: request_id.as_str().to_owned(),
                    service: Some(pb_service(&service)),
                })
            }
            Self::Command(HubCommand::RequestClusterReport { request_id }) => {
                Kind::RequestClusterReport(pb::RequestClusterReport {
                    request_id: request_id.as_str().to_owned(),
                })
            }
            Self::Command(HubCommand::NotifyConfigServer { request_id, paths }) => {
                Kind::NotifyConfigServer(pb::NotifyConfigServer {
                    request_id: request_id.as_str().to_owned(),
                    paths: paths.iter().map(|p| p.as_str().to_owned()).collect(),
                })
            }
            Self::Command(HubCommand::FetchServed { request_id, request }) => {
                Kind::FetchServed(pb::FetchServed {
                    request_id: request_id.as_str().to_owned(),
                    application: request.application.as_str().to_owned(),
                    tenant: request.tenant.as_str().to_owned(),
                    channel: request.channel.map(|c| c.as_str().to_owned()),
                    file: request.file.as_str().to_owned(),
                })
            }
        };
        pb::HubMessage { kind: Some(kind) }
    }
}

// ---------------------------------------------------------------- sentinel

impl TryFrom<pb::SentinelHello> for SentinelHello {
    type Error = ConvertError;

    fn try_from(h: pb::SentinelHello) -> Result<Self> {
        Ok(Self {
            vm: short("sentinel_hello.vm", &h.vm)?,
            export_root: short("sentinel_hello.export_root", &h.export_root)?,
            version: short("sentinel_hello.version", &h.version)?,
        })
    }
}

impl From<SentinelHello> for pb::SentinelHello {
    fn from(h: SentinelHello) -> Self {
        Self {
            vm: h.vm.as_str().to_owned(),
            export_root: h.export_root.as_str().to_owned(),
            version: h.version.as_str().to_owned(),
        }
    }
}

fn audit_operation(raw: i32) -> Result<AuditOperation> {
    match pb::AuditOperation::try_from(raw) {
        Ok(pb::AuditOperation::Create) => Ok(AuditOperation::Create),
        Ok(pb::AuditOperation::Write) => Ok(AuditOperation::Write),
        Ok(pb::AuditOperation::Delete) => Ok(AuditOperation::Delete),
        Ok(pb::AuditOperation::Rename) => Ok(AuditOperation::Rename),
        Ok(pb::AuditOperation::Attribute) => Ok(AuditOperation::Attribute),
        Ok(pb::AuditOperation::Unspecified) | Err(_) => {
            Err(ConvertError::Invalid("audit_record_batch.records.operation"))
        }
    }
}

impl TryFrom<pb::AuditRecord> for AuditRecord {
    type Error = ConvertError;

    fn try_from(r: pb::AuditRecord) -> Result<Self> {
        Ok(Self {
            time: ts(r.time_ms),
            path: nfs_path("audit_record_batch.records.path", &r.path)?,
            operation: audit_operation(r.operation)?,
            success: r.success,
            login_user: short("audit_record_batch.records.login_user", &r.login_user)?,
            effective_user: short("audit_record_batch.records.effective_user", &r.effective_user)?,
            exe: short("audit_record_batch.records.exe", &r.exe)?,
            comm: short("audit_record_batch.records.comm", &r.comm)?,
        })
    }
}

impl From<AuditRecord> for pb::AuditRecord {
    fn from(r: AuditRecord) -> Self {
        Self {
            time_ms: r.time.unix_millis(),
            path: r.path.as_str().to_owned(),
            operation: match r.operation {
                AuditOperation::Create => pb::AuditOperation::Create,
                AuditOperation::Write => pb::AuditOperation::Write,
                AuditOperation::Delete => pb::AuditOperation::Delete,
                AuditOperation::Rename => pb::AuditOperation::Rename,
                AuditOperation::Attribute => pb::AuditOperation::Attribute,
            } as i32,
            success: r.success,
            login_user: r.login_user.as_str().to_owned(),
            effective_user: r.effective_user.as_str().to_owned(),
            exe: r.exe.as_str().to_owned(),
            comm: r.comm.as_str().to_owned(),
        }
    }
}

impl TryFrom<pb::AuditRecordBatch> for AuditRecordBatch {
    type Error = ConvertError;

    fn try_from(b: pb::AuditRecordBatch) -> Result<Self> {
        Ok(Self {
            seq: b.seq,
            records: list(
                "audit_record_batch.records",
                b.records,
                limits::MAX_ENTRIES,
                AuditRecord::try_from,
            )?,
        })
    }
}

impl From<AuditRecordBatch> for pb::AuditRecordBatch {
    fn from(b: AuditRecordBatch) -> Self {
        Self {
            seq: b.seq,
            records: b.records.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<pb::SentinelSettings> for SentinelConfig {
    type Error = ConvertError;

    fn try_from(s: pb::SentinelSettings) -> Result<Self> {
        let batch_ok =
            usize::try_from(s.max_batch_records).is_ok_and(|n| (1..=limits::MAX_ENTRIES).contains(&n));
        if !batch_ok {
            return Err(ConvertError::Invalid("sentinel_settings.max_batch_records"));
        }
        if s.flush_interval_secs == 0 {
            return Err(ConvertError::Invalid("sentinel_settings.flush_interval_secs"));
        }
        Ok(Self {
            max_batch_records: s.max_batch_records,
            flush_interval_secs: s.flush_interval_secs,
        })
    }
}

impl From<SentinelConfig> for pb::SentinelSettings {
    fn from(c: SentinelConfig) -> Self {
        Self {
            max_batch_records: c.max_batch_records,
            flush_interval_secs: c.flush_interval_secs,
        }
    }
}

/// What the sentinel sends on a `Report` stream, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromSentinel {
    Hello(SentinelHello),
    Batch(AuditRecordBatch),
    Heartbeat { sent_at: Timestamp },
}

impl FromSentinel {
    /// `Ok(None)` when the oneof is empty or holds a variant this build does not know.
    pub fn from_proto(m: pb::SentinelMessage) -> Result<Option<Self>> {
        use pb::sentinel_message::Kind;
        let Some(kind) = m.kind else { return Ok(None) };
        Ok(Some(match kind {
            Kind::Hello(h) => Self::Hello(h.try_into()?),
            Kind::Batch(b) => Self::Batch(b.try_into()?),
            Kind::Heartbeat(h) => Self::Heartbeat {
                sent_at: ts(h.sent_at_ms),
            },
        }))
    }

    pub fn into_proto(self) -> pb::SentinelMessage {
        use pb::sentinel_message::Kind;
        let kind = match self {
            Self::Hello(h) => Kind::Hello(h.into()),
            Self::Batch(b) => Kind::Batch(b.into()),
            Self::Heartbeat { sent_at } => Kind::Heartbeat(pb::SentinelHeartbeat {
                sent_at_ms: sent_at.unix_millis(),
            }),
        };
        pb::SentinelMessage { kind: Some(kind) }
    }
}

/// What the hub sends on a `Report` stream, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToSentinel {
    Settings(SentinelConfig),
    /// Every batch up to and including this `seq` is durably applied; the sentinel may drop its spool (D74).
    Ack(u64),
}

impl ToSentinel {
    /// `Ok(None)` when the oneof is empty or holds a variant this build does not know.
    pub fn from_proto(m: pb::SentinelAck) -> Result<Option<Self>> {
        use pb::sentinel_ack::Kind;
        let Some(kind) = m.kind else { return Ok(None) };
        Ok(Some(match kind {
            Kind::AckedSeq(seq) => Self::Ack(seq),
            Kind::Settings(s) => Self::Settings(s.try_into()?),
        }))
    }

    pub fn into_proto(self) -> pb::SentinelAck {
        use pb::sentinel_ack::Kind;
        let kind = match self {
            Self::Ack(seq) => Kind::AckedSeq(seq),
            Self::Settings(s) => Kind::Settings(s.into()),
        };
        pb::SentinelAck { kind: Some(kind) }
    }
}

// ---------------------------------------------------------------- join

/// Who is joining. The kind decides the certificate SAN (Q11), so an agent subject can never be issued a
/// sentinel identity or the reverse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinSubject {
    /// An in-cluster agent, for this swimlane: `spiffe://lanekeeper/swimlane/<id>`.
    Agent(SwimlaneId),
    /// The sentinel on an NFS VM, by name: `spiffe://lanekeeper/sentinel/<name>`.
    Sentinel(ShortText),
}

/// The credential presented to `Join`. Always a [`Secret`] (S7, S21): it prints as `[redacted]`.
#[derive(Debug)]
pub enum JoinCredential {
    /// The node service account's Google ID token.
    GoogleIdToken(Secret<String>),
    /// A one-time join token issued by an Admin.
    JoinToken(Secret<String>),
}

impl JoinCredential {
    /// The token, for the verifier. Compare with `Secret::ct_eq_bytes`, never `==`.
    pub fn secret(&self) -> &Secret<String> {
        match self {
            Self::GoogleIdToken(s) | Self::JoinToken(s) => s,
        }
    }
}

/// A validated `JoinRequest`.
#[derive(Debug)]
pub struct JoinParams {
    pub subject: JoinSubject,
    /// PKCS#10 certificate signing request, DER. The caller parses and verifies it.
    pub csr_der: Bytes,
    pub credential: JoinCredential,
}

impl TryFrom<pb::JoinRequest> for JoinParams {
    type Error = ConvertError;

    fn try_from(r: pb::JoinRequest) -> Result<Self> {
        let kind = match pb::PeerKind::try_from(r.kind) {
            Ok(pb::PeerKind::Agent) => pb::PeerKind::Agent,
            Ok(pb::PeerKind::Sentinel) => pb::PeerKind::Sentinel,
            Ok(pb::PeerKind::Unspecified) | Err(_) => return Err(ConvertError::Invalid("join.kind")),
        };
        let credential = match r.credential {
            Some(pb::join_request::Credential::GoogleIdToken(t)) => {
                JoinCredential::GoogleIdToken(Secret::new(token("join.credential", t)?))
            }
            Some(pb::join_request::Credential::JoinToken(t)) => {
                JoinCredential::JoinToken(Secret::new(token("join.credential", t)?))
            }
            None => return Err(ConvertError::Missing("join.credential")),
        };
        let subject = match kind {
            pb::PeerKind::Sentinel => {
                let name = short("join.swimlane_id", &r.swimlane_id)?;
                if name.as_str().is_empty() {
                    return Err(ConvertError::Invalid("join.swimlane_id"));
                }
                JoinSubject::Sentinel(name)
            }
            _ => JoinSubject::Agent(
                SwimlaneId::parse(&r.swimlane_id).map_err(|_| ConvertError::Invalid("join.swimlane_id"))?,
            ),
        };
        Ok(Self {
            subject,
            csr_der: csr("join.csr_der", r.csr_der)?,
            credential,
        })
    }
}

impl From<JoinParams> for pb::JoinRequest {
    /// For the joining side. The plaintext is copied into the message; drop the message once it is sent.
    fn from(p: JoinParams) -> Self {
        let (swimlane_id, kind) = match &p.subject {
            JoinSubject::Agent(s) => (s.as_str().to_owned(), pb::PeerKind::Agent),
            JoinSubject::Sentinel(n) => (n.as_str().to_owned(), pb::PeerKind::Sentinel),
        };
        let credential = match &p.credential {
            JoinCredential::GoogleIdToken(t) => {
                pb::join_request::Credential::GoogleIdToken(t.expose().clone())
            }
            JoinCredential::JoinToken(t) => pb::join_request::Credential::JoinToken(t.expose().clone()),
        };
        Self {
            swimlane_id,
            csr_der: p.csr_der,
            credential: Some(credential),
            kind: kind as i32,
        }
    }
}
