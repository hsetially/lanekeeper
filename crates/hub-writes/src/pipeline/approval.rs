//! Step 5: approval (D37). A user who `requires_approval` does not write: the change is validated, stored as a pending proposal
//! that lives 72 hours, and the admins are told. Nothing is sent to the agent, and the file is not touched.

use domain::{
    DomainEvent, Expected, NfsPath, ProposalId, ProposalStatus, Severity, ShortText, SwimlaneId, User,
    WriteCtx, WriteOutcome,
};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Notification, NotificationKind, Tx};
use serde_json::json;

use crate::error::{Internal, Res};
use crate::idempotency::{ClaimKey, finish_in_tx, lock_claim};
use crate::pipeline::claim::Claim;
use crate::pipeline::execute::store_blob;
use crate::pipeline::{Abort, Done, Step};
use crate::proposals::store::{NewProposal, insert};
use crate::service::{Inner, millis};

/// An edit that waits for approval.
pub(crate) struct EditProposal<'a> {
    pub ctx: &'a WriteCtx,
    pub user: &'a User,
    pub claim: &'a Claim,
    pub swimlane: &'a SwimlaneId,
    pub path: &'a NfsPath,
    pub expected: &'a Expected,
    pub content: bytes::Bytes,
    pub diff: String,
}

pub(crate) fn via_of(v: domain::Via) -> AuditVia {
    match v {
        domain::Via::Ui => AuditVia::Ui,
        domain::Via::Mcp => AuditVia::Mcp,
    }
}

pub(crate) fn expected_hash(e: &Expected) -> Option<domain::ContentHash> {
    match e {
        Expected::Absent => None,
        Expected::Hash { hash } => Some(*hash),
    }
}

/// Create the proposal, its audit event, its event and the claim's response in one transaction, then tell the admins.
pub(crate) async fn propose_edit(hub: &Inner, p: EditProposal<'_>) -> Step<Done> {
    let new_hash = store_blob(hub, p.content.clone()).await?;
    match hub.within("proposal", create(hub, &p, new_hash)).await? {
        Ok(Created::New(outcome)) => {
            if let WriteOutcome::ProposalCreated { id } = &outcome {
                tell_admins(hub, p.user, p.swimlane, *id).await;
            }
            Ok(Done::Stored(outcome))
        }
        // Finished by another request while this one worked: its response stands, and its request told the admins.
        Ok(Created::Existing(outcome)) => Ok(Done::Stored(outcome)),
        Err(e) => Err(Abort::release(e)),
    }
}

enum Created {
    New(WriteOutcome),
    Existing(WriteOutcome),
}

async fn create(hub: &Inner, p: &EditProposal<'_>, new_hash: domain::ContentHash) -> Res<Created> {
    let key: &ClaimKey = &p.claim.key;
    let mut tx = hub.pool.begin().await.map_err(|e| Internal::db("proposal", &e))?;
    if let Some(done) = lock_claim(&mut tx, key).await? {
        let _ = tx.rollback().await;
        return Ok(Created::Existing(done));
    }
    let now = hub.now_ms();
    let before = expected_hash(p.expected);
    let id = insert(
        &mut tx,
        &NewProposal {
            author: &p.user.id,
            action: "edit",
            swimlane: p.swimlane,
            paths: vec![p.path.as_str().to_owned()],
            base_hashes: json!({
                p.path.as_str(): before.map_or_else(|| "absent".to_owned(), |h| h.to_string()),
            }),
            new_hash: Some(new_hash),
            params: json!({}),
            via: p.ctx.via,
            request_id: &p.ctx.request_id,
            created_ms: now,
            expires_ms: now.saturating_add(millis(hub.cfg.proposal_ttl)),
        },
    )
    .await?;
    hub.bus
        .publish_in_tx(
            &mut Tx::from_pg(&mut tx),
            DomainEvent::ProposalChanged {
                id,
                status: ProposalStatus::Pending,
            },
        )
        .await
        .map_err(|_| Internal::new("proposal", "event outbox"))?;
    let mut event = AuditEvent::new(
        hub.clock.now(),
        AuditActor::User {
            user: p.user.id.clone(),
        },
        AuditAction::ProposalCreated,
        via_of(p.ctx.via),
    );
    event.swimlane = Some(p.swimlane.clone());
    event.path = Some(p.path.clone());
    event.hash_before = before;
    event.hash_after = Some(new_hash);
    event.diff = Some(p.diff.clone());
    event.approval = Some(id);
    event.request_id = Some(p.ctx.request_id.clone());
    hub.audit
        .record(&mut Tx::from_pg(&mut tx), event)
        .await
        .map_err(|_| Internal::new("proposal", "audit log"))?;
    let outcome = WriteOutcome::ProposalCreated { id };
    finish_in_tx(&mut tx, key, &outcome).await?;
    tx.commit().await.map_err(|e| Internal::db("proposal", &e))?;
    Ok(Created::New(outcome))
}

/// Tell the admins (D49). A failure is logged and never fails the request that caused it.
async fn tell_admins(hub: &Inner, author: &User, swimlane: &SwimlaneId, id: ProposalId) {
    let summary = format!(
        "{} proposed a change in {}",
        author.display_name.as_str(),
        swimlane
    );
    let (Ok(title), Ok(summary), Ok(link)) = (
        ShortText::parse("Change awaiting approval"),
        ShortText::parse(&summary),
        ShortText::parse(&format!("/proposals/{id}")),
    ) else {
        tracing::warn!("a proposal notification could not be built");
        return;
    };
    let note = Notification {
        kind: NotificationKind::ProposalPending,
        severity: Severity::Medium,
        swimlane: Some(swimlane.clone()),
        title,
        summary,
        link: Some(link),
    };
    let sent = hub.within("notify_admins", hub.notifier.notify(note)).await;
    if !matches!(sent, Ok(Ok(()))) {
        tracing::warn!(proposal = id.get(), "admins could not be notified of a proposal");
    }
}
