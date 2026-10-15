//! What the file holds now, as far as the writer can tell, for the diff of the audit event.
//!
//! The registry knows the file from the agent's scans, so it can be a few seconds behind a write that just landed: a second edit
//! carries the hash that the first one returned, and the registry still has the version before. So the registry's copy is used
//! only when it is the version the request expects; then the blob store, which keeps every version by hash (D47); then the agent,
//! which is the truth. A request is never refused because the registry is behind: the expected hash is checked by the agent.

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
    let hash = match expected {
        Expected::Absent => return Found::Old(Old::Absent),
        Expected::Hash { hash } => *hash,
    };
    if let Ok(Ok(fc)) = hub
        .within(
            "current_registry",
            hub.registry.content(user, swimlane, path, ContentSource::Nfs),
        )
        .await
    {
        if fc.withheld {
            return Found::Withheld;
        }
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
