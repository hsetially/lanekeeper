//! Approving a proposal for a pull request (T9, D37, D77).
//!
//! A pull request is not a write to NFS, so approving one does not write anything: it makes the job that raises the pull request,
//! as the proposal's author and with the author's own token (D41). The rules are those of any approval (an admin, never the
//! author, a pending proposal), and the same re-check: the proposal holds the hash of each NFS file its author saw, and a file
//! that has another hash now makes the proposal stale (409 with the hash found), because the pull request would carry something
//! nobody approved. A change that has an open pull request by now is refused: the proposal stays pending for the admin to reject.
//!
//! One transaction records the decision, the proposal's new status (`approved`; it is `applied` when the job has opened the pull
//! request), the job, their events and the audit event of the approval. The pull request's own audit event follows when it exists.

use domain::{
    ContentHash, ContentSource, NfsPath, PrJobState, PrTarget, ProposalStatus, RejectReason, ShortText, User,
    Via, WriteCtx, WriteOutcome,
};
use ports::{AuditAction, ReadError, Tx};

use super::{announce, event, mark_stale, publish};
use crate::error::{Internal, Res};
use crate::idempotency::{finish_in_tx, lock_claim};
use crate::ops::file::rejected;
use crate::pipeline::claim::Claim;
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::{Abort, Done, Step};
use crate::proposals::store::{self, Base, NewDecision, Stored};
use crate::prs::job::{self, NewJob, StepData};
use crate::prs::links;
use crate::service::Inner;

/// The request a proposal holds.
struct Asked {
    title: String,
    body: Option<String>,
    target: Option<PrTarget>,
    via: Via,
    /// The paths, each with the hash its author expected there.
    files: Vec<(NfsPath, ContentHash)>,
}

fn asked(p: &Stored) -> Option<Asked> {
    let title = p.params.get("title")?.as_str()?.to_owned();
    ShortText::parse(&title).ok()?;
    let body = match p.params.get("body") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(v.as_str()?.to_owned()),
    };
    let target = match p.params.get("target") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v.clone()).ok()?),
    };
    let via = match p.params.get("via").and_then(serde_json::Value::as_str) {
        Some("mcp") => Via::Mcp,
        _ => Via::Ui,
    };
    let files = p
        .paths
        .iter()
        .map(|text| {
            let path = NfsPath::parse(text).ok()?;
            match p.expected_at(text)? {
                Base::At(hash) => Some((path, hash)),
                Base::Absent => None,
            }
        })
        .collect::<Option<Vec<_>>>()?;
    (!files.is_empty()).then_some(Asked {
        title,
        body,
        target,
        via,
        files,
    })
}

/// What the transaction found.
enum Made {
    Done(WriteOutcome),
    Existing(WriteOutcome),
    NotPending,
    Duplicate,
    /// An unfinished job already covers one of the files: there is no pull request yet to make this one a duplicate of.
    Busy,
}

pub(super) async fn apply_pr(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: &Stored,
    comment: Option<String>,
) -> Step<Done> {
    let Some(ask) = asked(p) else {
        return Ok(Done::Store(rejected(RejectReason::InvalidContent)));
    };
    if matches!(check_lock(hub, &p.swimlane).await?, LockState::UnknownSwimlane) {
        return Ok(Done::Release(rejected(RejectReason::UnknownSwimlane)));
    }
    if ask.files.iter().any(|(path, _)| hub.deny.is_denied(path)) {
        return Ok(Done::Store(rejected(RejectReason::Denied)));
    }

    // The re-check: each file is as its author saw it, as the registry has it (what the pull request will read).
    for (path, expected) in &ask.files {
        let read = hub
            .within(
                "approve_pr_read",
                hub.registry.content(admin, &p.swimlane, path, ContentSource::Nfs),
            )
            .await?;
        let current = match read {
            Ok(content) => Some(content.hash),
            Err(ReadError::NotFound) => None,
            Err(_) => return Err(Abort::release(ports::WriteError::Unavailable)),
        };
        if current != Some(*expected) {
            return mark_stale(
                hub,
                ctx,
                claim,
                admin,
                p,
                path,
                Some(*expected),
                current,
                comment.as_deref(),
            )
            .await;
        }
    }

    match hub
        .within("approve_pr", approve_tx(hub, ctx, admin, claim, p, &ask, comment))
        .await?
    {
        Ok(Made::Done(outcome)) => {
            announce(hub, p, "was approved; its pull request is being made").await;
            Ok(Done::Stored(outcome))
        }
        Ok(Made::Existing(outcome)) => Ok(Done::Stored(outcome)),
        Ok(Made::NotPending | Made::Duplicate) => Ok(Done::Store(rejected(RejectReason::NotAllowed))),
        // Not stored: when the other job is done the same approval can be made.
        Ok(Made::Busy) => Ok(Done::Release(WriteOutcome::Locked { until: None })),
        Err(e) => Err(Abort::release(e)),
    }
}

async fn approve_tx(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: &Stored,
    ask: &Asked,
    comment: Option<String>,
) -> Res<Made> {
    let mut tx = hub
        .pool
        .begin()
        .await
        .map_err(|e| Internal::db("approve_pr", &e))?;
    if let Some(done) = lock_claim(&mut tx, &claim.key).await? {
        let _ = tx.rollback().await;
        return Ok(Made::Existing(done));
    }
    if store::lock_status(&mut tx, p.id).await? != Some(ProposalStatus::Pending) {
        let _ = tx.rollback().await;
        return Ok(Made::NotPending);
    }
    // The same lock the request side takes: no job is made for a change that has a pull request by now.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("pr:{}", p.swimlane))
        .execute(&mut *tx)
        .await
        .map_err(|e| Internal::db("approve_pr", &e))?;
    let pairs: Vec<(String, ContentHash)> = ask
        .files
        .iter()
        .map(|(path, hash)| (path.as_str().to_owned(), *hash))
        .collect();
    if links::find_open(&mut tx, &p.swimlane, &pairs).await?.is_some() {
        let _ = tx.rollback().await;
        return Ok(Made::Duplicate);
    }
    // A job that has not opened its pull request has no link yet. A second job for the same files would open a second pull
    // request; the proposal stays pending until that job is done.
    if job::covering(&mut tx, &p.swimlane, &p.paths).await?.is_some() {
        let _ = tx.rollback().await;
        return Ok(Made::Busy);
    }

    let now = hub.now_ms();
    store::insert_decision(
        &mut tx,
        &NewDecision {
            proposal: p.id,
            approved: true,
            approver: &admin.id,
            comment: comment.as_deref(),
            rechecked: None,
            at_ms: now,
        },
    )
    .await?;
    store::set_status(&mut tx, p.id, ProposalStatus::Approved, now).await?;
    let step = StepData::new(ask.via, ask.target.clone(), Some(p.id.get()));
    let job = job::insert(
        &mut tx,
        &NewJob {
            author: &p.author,
            swimlane: &p.swimlane,
            paths: p.paths.clone(),
            title: &ask.title,
            body: ask.body.as_deref(),
            step: &step,
            request_id: &ctx.request_id,
            now_ms: now,
        },
    )
    .await?;
    job::publish_progress(hub, &mut tx, job, PrJobState::Pending).await?;
    publish(hub, &mut tx, p.id, ProposalStatus::Approved).await?;

    let path = ask.files.first().map(|(path, _)| path.clone());
    let mut e = event(hub, ctx, admin, p, AuditAction::ProposalApproved, path.as_ref());
    e.diff = Some(format!("approved the pull request of job {job}"));
    let audit_id = hub
        .audit
        .record(&mut Tx::from_pg(&mut tx), e)
        .await
        .map_err(|_| Internal::new("approve_pr", "audit log"))?;
    let outcome = WriteOutcome::Applied {
        hash: None,
        audit_id,
        warnings: Vec::new(),
    };
    finish_in_tx(&mut tx, &claim.key, &outcome).await?;
    tx.commit().await.map_err(|e| Internal::db("approve_pr", &e))?;
    Ok(Made::Done(outcome))
}
