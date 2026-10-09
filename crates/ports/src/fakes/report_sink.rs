use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{
    AgentConfig, ClusterReport, ContentHash, Heartbeat, HeartbeatAction, Hello, ScanDelta, ShortText,
    SwimlaneId,
};

use super::util::lock;
use crate::{AgentIdentity, ReportSink, SinkError, content_hash};

/// Most message parts of one logical delta the fake buffers.
const MAX_PARTS: usize = 64;
/// Most applied deltas the fake remembers per swimlane.
const KEEP_DELTAS: usize = 256;
/// Largest file the fake config allows an agent to send.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Default)]
struct AgentState {
    root: Option<ContentHash>,
    parts: Vec<ScanDelta>,
    applied: VecDeque<ScanDelta>,
    clusters: VecDeque<ClusterReport>,
}

/// Validates what agents send, tracks each agent's Merkle root and applies multi-part deltas only when
/// the last part arrives. It stores nothing else: ingestion into indexes is prompt 05.
#[derive(Clone, Default)]
pub struct FakeReportSink {
    agents: Arc<Mutex<HashMap<SwimlaneId, AgentState>>>,
}

impl std::fmt::Debug for FakeReportSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeReportSink").finish_non_exhaustive()
    }
}

impl FakeReportSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// The Merkle root of the last fully applied delta for `s`.
    pub fn root(&self, s: &SwimlaneId) -> Option<ContentHash> {
        lock(&self.agents).get(s).and_then(|a| a.root)
    }

    /// Fully applied deltas, oldest first (the last 256). Multi-part deltas appear once, as their last part.
    pub fn applied_deltas(&self, s: &SwimlaneId) -> Vec<ScanDelta> {
        lock(&self.agents)
            .get(s)
            .map(|a| a.applied.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Cluster reports received, oldest first (the last 256).
    pub fn cluster_reports(&self, s: &SwimlaneId) -> Vec<ClusterReport> {
        lock(&self.agents)
            .get(s)
            .map(|a| a.clusters.iter().cloned().collect())
            .unwrap_or_default()
    }
}

fn default_config() -> Option<AgentConfig> {
    let globs = [
        "*.jks",
        "*.p12",
        "*.pfx",
        "*.pem",
        "*.key",
        "*.keystore",
        "*private*",
    ];
    let env = [
        "SPRING_APPLICATION_NAME",
        "SPRING_PROFILES_ACTIVE",
        "CONFIG_CLIENT_CACHE_TTL",
        "CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED",
    ];
    let texts = |items: &[&str]| -> Option<Vec<ShortText>> {
        items.iter().map(|s| ShortText::parse(s).ok()).collect()
    };
    Some(AgentConfig {
        scan_interval_secs: 300,
        heartbeat_interval_secs: 30,
        max_file_bytes: MAX_FILE_BYTES,
        deny_globs: texts(&globs)?,
        env_allowlist: texts(&env)?,
        tenants: Vec::new(),
    })
}

fn validate_delta(d: &ScanDelta) -> Result<(), SinkError> {
    if d.entries.len() > ScanDelta::MAX_ENTRIES || d.payload_bytes() > ScanDelta::MAX_BYTES {
        return Err(SinkError::TooLarge);
    }
    for e in &d.entries {
        match (&e.bytes, e.denied) {
            // A denied file is reported by name, size and hash only (D79).
            (Some(_), true) => return Err(SinkError::Invalid),
            (Some(b), false) => {
                if content_hash(b) != e.hash || b.len() as u64 != e.size || e.size > MAX_FILE_BYTES {
                    return Err(SinkError::Invalid);
                }
            }
            (None, _) => {}
        }
    }
    Ok(())
}

#[async_trait]
impl ReportSink for FakeReportSink {
    async fn hello(&self, id: &AgentIdentity, h: Hello) -> Result<AgentConfig, SinkError> {
        if &h.swimlane != id.swimlane() {
            return Err(SinkError::IdentityMismatch);
        }
        let config = default_config().ok_or(SinkError::Unavailable)?;
        lock(&self.agents).entry(id.swimlane().clone()).or_default();
        Ok(config)
    }

    async fn heartbeat(&self, id: &AgentIdentity, hb: Heartbeat) -> Result<HeartbeatAction, SinkError> {
        let agents = lock(&self.agents);
        let a = agents.get(id.swimlane()).ok_or(SinkError::HelloRequired)?;
        Ok(match a.root {
            None => HeartbeatAction::RequestFullScan,
            Some(known) if known == hb.merkle_root => HeartbeatAction::None,
            Some(known) => HeartbeatAction::RequestDelta { since_root: known },
        })
    }

    async fn delta(&self, id: &AgentIdentity, d: ScanDelta) -> Result<(), SinkError> {
        validate_delta(&d)?;
        let mut agents = lock(&self.agents);
        let a = agents.get_mut(id.swimlane()).ok_or(SinkError::HelloRequired)?;
        if d.more {
            if a.parts.len() >= MAX_PARTS {
                a.parts.clear();
                return Err(SinkError::TooLarge);
            }
            a.parts.push(d);
            return Ok(());
        }
        a.parts.clear();
        a.root = Some(d.new_root);
        if a.applied.len() >= KEEP_DELTAS {
            a.applied.pop_front();
        }
        a.applied.push_back(d);
        Ok(())
    }

    async fn cluster(&self, id: &AgentIdentity, c: ClusterReport) -> Result<(), SinkError> {
        let mut agents = lock(&self.agents);
        let a = agents.get_mut(id.swimlane()).ok_or(SinkError::HelloRequired)?;
        if a.clusters.len() >= KEEP_DELTAS {
            a.clusters.pop_front();
        }
        a.clusters.push_back(c);
        Ok(())
    }
}
