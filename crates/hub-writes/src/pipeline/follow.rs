//! The state changes that commit with a write (rule 8): the transaction that records the audit event of an action also records
//! the decision on the proposal the action carries out, or the draft it applies. They commit together or not at all, so a
//! proposal is never "applied" without its audit event and an audit event is never recorded for a proposal that stays pending.
//!
//! A proposal that is no longer pending when this runs (an admin rejected it, or its time ran out, while the command was on its
//! way) has had its file written already: the audit event is still recorded, because the change happened, and the proposal is
//! left as it is. That is logged; it cannot happen between two approvals (a lease serialises deciders) and is a few milliseconds
//! wide for the others.

use domain::{DomainEvent, ProposalStatus};
use ports::Tx;
use sqlx::{Postgres, Transaction};

use crate::drafts;
use crate::error::{Internal, Res};
use crate::idempotency::Follow;
use crate::proposals::store::{self, NewDecision};
use crate::service::Inner;

pub(crate) async fn apply(hub: &Inner, tx: &mut Transaction<'_, Postgres>, follow: &Follow) -> Res<()> {
    match follow {
        Follow::Draft { id } => drafts::mark(&mut *tx, id, ProposalStatus::Applied).await,
        Follow::Decided {
            proposal,
            approver,
            comment,
            rechecked,
            draft,
        } => {
            let status = store::lock_status(&mut *tx, *proposal).await?;
            if status != Some(ProposalStatus::Pending) {
                tracing::warn!(
                    proposal = proposal.get(),
                    "an approved proposal was no longer pending when its write was recorded"
                );
                return Ok(());
            }
            let now = hub.now_ms();
            store::insert_decision(
                &mut *tx,
                &NewDecision {
                    proposal: *proposal,
                    approved: true,
                    approver,
                    comment: comment.as_deref(),
                    rechecked: *rechecked,
                    at_ms: now,
                },
            )
            .await?;
            store::set_status(&mut *tx, *proposal, ProposalStatus::Applied, now).await?;
            if let Some(draft) = draft {
                drafts::mark(&mut *tx, draft, ProposalStatus::Applied).await?;
            }
            hub.bus
                .publish_in_tx(
                    &mut Tx::from_pg(&mut *tx),
                    DomainEvent::ProposalChanged {
                        id: *proposal,
                        status: ProposalStatus::Applied,
                    },
                )
                .await
                .map_err(|_| Internal::new("commit", "event outbox"))?;
            Ok(())
        }
    }
}
