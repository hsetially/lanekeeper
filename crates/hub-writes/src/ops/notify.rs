//! `notify`: tell the config-server that files changed (T11, D85, D86). Operator only, audited, and one call to the agent.
//!
//! The config-server's `/update-resources` has no authentication (Q37), so this is the one door to it and it is kept narrow:
//!
//! - **Operator or above**, read from the directory like every other action. A user who needs approval proposes the notification
//!   and an admin's approval sends it (the admin is then the actor, the proposer the author).
//! - **Exactly the paths of the request,** as the root-relative [`domain::NfsPath`]s they are, once each, in the order given. The
//!   agent turns each into one form value; nothing else about the request is chosen by the caller.
//! - **One audit event** for a notification that happened, recorded in the transaction that also updates the pickup states
//!   ([`super::pickup`]) and the outbox. A notification the config-server refused, or that never reached it, changes nothing and
//!   records nothing, and its idempotency key stays free for a retry. One whose answer was lost may have gone out; the retry
//!   sends it again, which is harmless (a notification refreshes a cache), and the lost one is a residual risk in the threat model.
//! - The editing lock does not apply: a notification changes no file. The swimlane has to exist.
//! - **Bounded:** one to 100 paths ([`NotifyRequest::MAX_PATHS`]); the audit event names at most what fits in its diff.

use std::collections::BTreeSet;

use domain::{NfsPath, NotifyRequest, RejectReason, Role, SwimlaneId, User, WriteCtx, WriteOutcome};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, WriteError};

use super::file::rejected;
use super::pickup::PickupUpdate;
use super::restart::ByApproval;
use crate::idempotency::RequestHash;
use crate::pipeline::approval::{Proposal, propose, via_of};
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::execute::{Committed, Recording, Sent, record, send_notify};
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::{Abort, Done, Step};
use crate::service::Inner;

pub(crate) async fn notify(
    hub: &Inner,
    ctx: &WriteCtx,
    r: NotifyRequest,
) -> Result<WriteOutcome, WriteError> {
    notify_as(hub, ctx, r, via_of(ctx.via)).await
}

/// [`notify`] with the `via` its audit event records: `System` for the batcher of auto-notify, which makes the call for a user
/// whose writes it holds.
pub(crate) async fn notify_as(
    hub: &Inner,
    ctx: &WriteCtx,
    r: NotifyRequest,
    via: AuditVia,
) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Operator).await?;
    // A request of no path, or of more than the contract allows, is not a request the typed API can make.
    let paths = distinct(r.paths);
    if paths.is_empty() || paths.len() > NotifyRequest::MAX_PATHS {
        return Err(WriteError::BadRequest);
    }
    let claim = match claim(hub, ctx, RequestHash::of_notify(&r.swimlane, &paths)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, r.swimlane, paths, via).await;
    settle(hub, &claim, result).await
}

/// The paths, each once, in the order they were given.
fn distinct(paths: Vec<NfsPath>) -> Vec<NfsPath> {
    let mut seen = BTreeSet::new();
    paths.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

async fn run(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &User,
    claim: &Claim,
    swimlane: SwimlaneId,
    paths: Vec<NfsPath>,
    via: AuditVia,
) -> Step<Done> {
    if matches!(check_lock(hub, &swimlane).await?, LockState::UnknownSwimlane) {
        return Ok(Done::Release(rejected(RejectReason::UnknownSwimlane)));
    }
    // A user who needs approval proposes; nothing is sent.
    if user.requires_approval {
        return propose(
            hub,
            Proposal {
                ctx,
                user,
                claim,
                swimlane: &swimlane,
                action: "notify",
                path: None,
                paths: paths.iter().map(|p| p.as_str().to_owned()).collect(),
                base_hashes: serde_json::json!({}),
                hash_before: None,
                content: None,
                params: serde_json::json!({}),
                diff: summary(&paths),
                draft: None,
            },
        )
        .await;
    }
    execute(hub, ctx, user, claim, &swimlane, paths, via, None).await
}

/// What the audit event says: how many paths, and which. The diff of an event holds at most [`AuditEvent::MAX_DIFF_BYTES`]; the
/// paths that do not fit are counted, not named.
pub(crate) fn summary(paths: &[NfsPath]) -> String {
    let mut text = format!(
        "notify config-server of {} path{}",
        paths.len(),
        if paths.len() == 1 { "" } else { "s" }
    );
    let mut left = paths.len();
    for p in paths {
        // One line each; the closing line, if any, needs room too.
        if text.len() + p.as_str().len() + 1 + 40 > AuditEvent::MAX_DIFF_BYTES {
            break;
        }
        text.push('\n');
        text.push_str(p.as_str());
        left -= 1;
    }
    if left > 0 {
        text.push_str("\n(and ");
        text.push_str(&left.to_string());
        text.push_str(" more)");
    }
    text
}

/// Step 6 for a notification: send it, and record it with the pickup states it changed. `by` is the proposal that an admin's
/// approval carries out.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &User,
    claim: &Claim,
    swimlane: &SwimlaneId,
    paths: Vec<NfsPath>,
    via: AuditVia,
    by: Option<ByApproval>,
) -> Step<Done> {
    // Read before the command goes out, and not after the answer: see [`PickupUpdate::now`].
    let began = hub.clock.now();
    match send_notify(hub, swimlane, paths.clone(), &ctx.request_id).await {
        Sent::Undelivered => Ok(Done::Release(rejected(RejectReason::AgentUnavailable))),
        Sent::Refused { outcome, transient } => Ok(if transient {
            Done::Release(outcome)
        } else {
            Done::Store(outcome)
        }),
        // The notification may or may not have gone out. It is not recorded and the key is freed: a retry notifies again.
        Sent::Ambiguous => {
            tracing::warn!(swimlane = %swimlane, "a notification was sent and its answer was lost");
            Err(Abort::release(WriteError::Unavailable))
        }
        Sent::Landed(_) => {
            let mut event = AuditEvent::new(
                hub.clock.now(),
                AuditActor::User {
                    user: user.id.clone(),
                },
                AuditAction::ConfigServerNotified,
                via,
            );
            event.swimlane = Some(swimlane.clone());
            event.diff = Some(summary(&paths));
            event.request_id = Some(ctx.request_id.clone());
            let follow = by.map(|by| {
                event.author = Some(by.author);
                event.approval = Some(by.proposal);
                by.follow
            });
            // The facts that decide which files are property sources are read before the transaction opens.
            let facts = match crate::preview::effect::facts(hub, swimlane).await {
                Ok(facts) => facts,
                // The notification went out; without the facts the pickup states are left for the registry to recompute.
                Err(e) => {
                    tracing::warn!(%e, "the pickup states were not updated after a notification");
                    return record_without_pickups(hub, claim, swimlane, event, follow).await;
                }
            };
            let recorded = record(
                hub,
                Recording {
                    key: &claim.key,
                    swimlane,
                    event,
                    tool_write: None,
                    events: Vec::new(),
                    keep: Vec::new(),
                    applied_hash: None,
                    warnings: Vec::new(),
                    follow,
                    pickups: Some(PickupUpdate {
                        facts,
                        paths,
                        now: began,
                    }),
                },
            )
            .await;
            settled(recorded)
        }
    }
}

async fn record_without_pickups(
    hub: &Inner,
    claim: &Claim,
    swimlane: &SwimlaneId,
    event: AuditEvent,
    follow: Option<crate::idempotency::Follow>,
) -> Step<Done> {
    let recorded = record(
        hub,
        Recording {
            key: &claim.key,
            swimlane,
            event,
            tool_write: None,
            events: Vec::new(),
            keep: Vec::new(),
            applied_hash: None,
            warnings: Vec::new(),
            follow,
            pickups: None,
        },
    )
    .await;
    settled(recorded)
}

fn settled(recorded: crate::error::Res<Committed>) -> Step<Done> {
    match recorded {
        Ok(Committed::Done(outcome) | Committed::Lost(outcome)) => Ok(Done::Stored(outcome)),
        // The config-server was told and the record failed. The key is freed; the store logged the failure.
        Err(e) => Err(Abort::release(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` distinct paths of about `len` characters (at most 1,000), in four folders, so no component is too long.
    fn paths(n: usize, len: usize) -> Vec<NfsPath> {
        let folder = "d".repeat(len.saturating_sub(20) / 4);
        (0..n)
            .map(|i| {
                let tree = if folder.is_empty() {
                    "d".to_owned()
                } else {
                    format!("{folder}/{folder}/{folder}/{folder}")
                };
                NfsPath::parse(&format!("{tree}/f{i:04}.yml")).unwrap()
            })
            .collect()
    }

    #[test]
    fn the_summary_names_the_paths_and_counts_them() {
        let p = paths(2, 1);
        assert_eq!(
            summary(&p),
            "notify config-server of 2 paths\nd/f0000.yml\nd/f0001.yml"
        );
        assert_eq!(summary(&p[..1]), "notify config-server of 1 path\nd/f0000.yml");
    }

    #[test]
    fn a_summary_fits_the_audit_diff_whatever_the_paths() {
        // 100 paths of about 1,000 characters would be 100 KB.
        let p = paths(100, 990);
        assert!(p.iter().all(|p| p.as_str().len() > 900));
        let text = summary(&p);
        assert!(text.len() <= AuditEvent::MAX_DIFF_BYTES, "{} bytes", text.len());
        assert!(text.starts_with("notify config-server of 100 paths\n"));
        let named = text.lines().skip(1).filter(|l| !l.starts_with('(')).count();
        if named < 100 {
            assert!(
                text.ends_with(&format!("(and {} more)", 100 - named)),
                "{}",
                &text[text.len() - 40..]
            );
        }
    }

    #[test]
    fn a_path_named_twice_is_sent_once_in_the_order_given() {
        let a = NfsPath::parse("b.yml").unwrap();
        let b = NfsPath::parse("a.yml").unwrap();
        assert_eq!(
            distinct(vec![a.clone(), b.clone(), a.clone(), b.clone()]),
            vec![a, b]
        );
    }
}
