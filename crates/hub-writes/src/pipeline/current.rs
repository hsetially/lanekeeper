//! What the file holds now, as far as the writer can tell, for the diff of the audit event.
//!
//! The registry knows the file from the agent's scans, so it can be a few seconds behind a write that just landed: a second edit
//! carries the hash that the first one returned, and the registry still has the version before. So the registry's copy is used
//! only when it is the version the request expects (and, for any request, to learn that the file is withheld); then the blob
//! store, which keeps every version by hash (D47); then the agent, which is the truth. A request is never refused because the registry is behind: the expected hash is checked by the agent.

use bytes::Bytes;
use domain::{AgentReply, ContentSource, Expected, NfsPath, RequestId, SwimlaneId, User};
use ports::content_hash;

use crate::pipeline::validate::Old;
use crate::service::Inner;

/// What the writer found.
pub(crate) enum Found {
    Old(Old),
    /// The registry says the file is withheld (a denied file, D79).
    Withheld,
}

pub(crate) async fn old_content(
    hub: &Inner,
    user: &User,
    swimlane: &SwimlaneId,
    path: &NfsPath,
    expected: &Expected,
) -> Found {
    // The registry is asked even when the request says the file is new: the agent's own deny globs may match a file that the hub's
    // do not, and such a file must not be written, or proposed for, because the request calls it new (D79). A file the registry
    // does not have, or cannot be asked about, is new as far as the writer can tell; the agent checks the expectation.
    let registry = hub
        .within(
            "current_registry",
            hub.registry.content(user, swimlane, path, ContentSource::Nfs),
        )
        .await;
    if let Ok(Ok(fc)) = &registry {
        if fc.withheld {
            return Found::Withheld;
        }
    }
    let hash = match expected {
        Expected::Absent => return Found::Old(Old::Absent),
        Expected::Hash { hash } => *hash,
    };
    if let Ok(Ok(fc)) = registry {
        if fc.hash == hash {
            if let Some(bytes) = fc.bytes {
                return Found::Old(Old::Known(bytes));
            }
        }
    }
    if let Ok(Ok(Some(bytes))) = hub.within("current_blob", hub.blobs.get(&hash)).await {
        if content_hash(&bytes) == hash {
            return Found::Old(Old::Known(bytes));
        }
    }
    Found::Old(match read_from_agent(hub, swimlane, path, hash).await {
        Some(bytes) => Old::Known(bytes),
        None => Old::Unknown,
    })
}

async fn read_from_agent(
    hub: &Inner,
    swimlane: &SwimlaneId,
    path: &NfsPath,
    hash: domain::ContentHash,
) -> Option<Bytes> {
    let request_id = RequestId::parse(&format!("read-{:016x}", fastrand::u64(..))).ok()?;
    let cmd = domain::HubCommand::ReadFile {
        request_id,
        path: path.clone(),
    };
    let reply = tokio::time::timeout(
        hub.cfg.agent_timeout,
        hub.gateway.request(swimlane, cmd, hub.cfg.agent_timeout),
    )
    .await
    .ok()?
    .ok()?;
    match reply {
        AgentReply::File {
            hash: found, bytes, ..
        } if found == hash && content_hash(&bytes) == hash => Some(bytes),
        _ => None,
    }
}

/// What the agent has at a path right now.
pub(crate) enum AgentFile {
    Present {
        hash: domain::ContentHash,
        bytes: Bytes,
    },
    Absent,
    /// The agent could not be asked, or answered something else.
    Unreachable,
}

/// Ask the agent for the file: the truth about NFS, for the actions that must not destroy a version nobody kept.
pub(crate) async fn agent_file(hub: &Inner, swimlane: &SwimlaneId, path: &NfsPath) -> AgentFile {
    let Ok(request_id) = RequestId::parse(&format!("read-{:016x}", fastrand::u64(..))) else {
        return AgentFile::Unreachable;
    };
    let cmd = domain::HubCommand::ReadFile {
        request_id,
        path: path.clone(),
    };
    let asked = tokio::time::timeout(
        hub.cfg.agent_timeout,
        hub.gateway.request(swimlane, cmd, hub.cfg.agent_timeout),
    )
    .await;
    match asked {
        Ok(Ok(AgentReply::File { hash, bytes, .. })) if content_hash(&bytes) == hash => {
            AgentFile::Present { hash, bytes }
        }
        Ok(Ok(AgentReply::Op(op))) if op.error == Some(domain::OpError::NotFound) => AgentFile::Absent,
        _ => AgentFile::Unreachable,
    }
}
