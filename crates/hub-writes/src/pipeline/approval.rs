//! Step 5: approval (D37). A user who `requires_approval` does not write: the change is validated, stored as a pending proposal
//! that lives 72 hours, and the admins are told. Nothing is sent to the agent, and the file is not touched.

use domain::{
    DomainEvent, Expected, NfsPath, ProposalId, ProposalStatus, Severity, ShortText, SwimlaneId, User,
    WriteCtx, WriteOutcome,
};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Notification, NotificationKind, Tx};

use crate::error::{Internal, Res};
use crate::idempotency::{ClaimKey, finish_in_tx, lock_claim};
use crate::pipeline::claim::Claim;
use crate::pipeline::execute::store_blob;
use crate::pipeline::{Abort, Done, Step};
use crate::proposals::store::{NewProposal, insert};
use crate::service::{Inner, millis};

/// A change that waits for approval: what an admin will be asked to apply (T4). The proposal keeps the version it proposes (a
/// blob), the hashes it expects to find, and the few parameters its action needs.
pub(crate) struct Proposal<'a> {
    pub ctx: &'a WriteCtx,
    pub user: &'a User,
    pub claim: &'a Claim,
    pub swimlane: &'a SwimlaneId,
    /// One of the table's actions: `edit`, `upload`, `delete`, `revert`, `restart`.
    pub action: &'static str,
    /// The file the audit event names; none for a restart.
    pub path: Option<&'a NfsPath>,
    /// What the proposal lists: the file, or `namespace/name` for a restart. At least one, at most 100.
    pub paths: Vec<String>,
    /// Path to the hash the author expected (hex), or `"absent"`.
    pub base_hashes: serde_json::Value,
    /// The hash the audit event records as the file's before.
    pub hash_before: Option<domain::ContentHash>,
    /// The version that would be written: kept as a blob, and named by the proposal's `new_hash`.
    pub content: Option<bytes::Bytes>,
    /// What the action needs besides that: the version a revert restores, the service a restart names.
    pub params: serde_json::Value,
    /// The diff the audit event keeps. Flagged values are redacted already.
    pub diff: String,
    /// The draft that became this proposal, when one did: it waits with the proposal.
    pub draft: Option<&'a str>,
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

/// `{path: expected hash or "absent"}`, the `base_hashes` of a proposal.
pub(crate) fn base_hash(path: &NfsPath, expected: &Expected) -> serde_json::Value {
    serde_json::json!({
        path.as_str(): expected_hash(expected).map_or_else(|| "absent".to_owned(), |h| h.to_string()),
    })
}

/// Create the proposal, its audit event, its event and the claim's response in one transaction, then tell the admins.
pub(crate) async fn propose(hub: &Inner, p: Proposal<'_>) -> Step<Done> {
    let new_hash = match &p.content {
        Some(bytes) => Some(store_blob(hub, bytes.clone()).await?),
        None => None,
    };
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

async fn create(hub: &Inner, p: &Proposal<'_>, new_hash: Option<domain::ContentHash>) -> Res<Created> {
    let key: &ClaimKey = &p.claim.key;
    let mut tx = hub.pool.begin().await.map_err(|e| Internal::db("proposal", &e))?;
    if let Some(done) = lock_claim(&mut tx, key).await? {
        let _ = tx.rollback().await;
        return Ok(Created::Existing(done));
    }
    let now = hub.now_ms();
    let id = insert(
        &mut tx,
        &NewProposal {
            author: &p.user.id,
            action: p.action,
            swimlane: p.swimlane,
            paths: p.paths.clone(),
            base_hashes: p.base_hashes.clone(),
            new_hash,
            params: p.params.clone(),
            via: p.ctx.via,
            request_id: &p.ctx.request_id,
            created_ms: now,
            expires_ms: now.saturating_add(millis(hub.cfg.proposal_ttl)),
        },
    )
    .await?;
    if let Some(draft) = p.draft {
        crate::drafts::mark(&mut tx, draft, ProposalStatus::Pending).await?;
    }
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
    event.path = p.path.cloned();
    event.hash_before = p.hash_before;
    event.hash_after = new_hash;
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
    let summary = pending_summary(author.display_name.as_str(), swimlane);
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

/// "`name` proposed a change in `swimlane`", as a [`ShortText`] can hold it: a name that would make the line longer is cut (at a
/// character boundary, with an ellipsis) so that the words that say what happened, and where, are always there. Without the cut a
/// user whose display name is the longest the type allows would make the notice unparseable and the admins would hear nothing.
fn pending_summary(name: &str, swimlane: &SwimlaneId) -> String {
    const ELLIPSIS: &str = "\u{2026}";
    let tail = format!(" proposed a change in {swimlane}");
    let room = SHORT_TEXT_MAX.saturating_sub(tail.len());
    if name.len() <= room {
        return format!("{name}{tail}");
    }
    let mut end = room.saturating_sub(ELLIPSIS.len());
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}{tail}", &name[..end])
}

/// The longest [`ShortText`], in bytes.
const SHORT_TEXT_MAX: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_of_a_proposal_fits_a_short_text_whatever_the_name_of_its_author() {
        let lane = SwimlaneId::parse("sit1").unwrap();
        // The longest name a user can have is the longest ShortText; two-byte letters make the cut land mid-character.
        for name in [
            "Ada".to_owned(),
            "x".repeat(256),
            "é".repeat(128),
            "ü".repeat(127) + "ab",
        ] {
            assert!(ShortText::parse(&name).is_ok(), "{} bytes", name.len());
            let summary = pending_summary(&name, &lane);
            assert!(
                ShortText::parse(&summary).is_ok(),
                "{} bytes for a name of {}",
                summary.len(),
                name.len()
            );
            assert!(summary.ends_with(" proposed a change in sit1"), "{summary}");
        }
        // A name that fits is not touched.
        assert_eq!(pending_summary("Ada", &lane), "Ada proposed a change in sit1");
    }
}
