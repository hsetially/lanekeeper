//! Asking for a pull request (T9, D77): the request is checked, and accepted as a persisted job that is answered at once with its id.
//!
//! The steps, in the order the write pipeline has (authorize, claim, check, decide):
//!
//! 1. The user is active and an Editor, read from the directory (S4).
//! 2. The idempotency key is claimed. A retry with the same key and the same request is answered with the same job; the same key
//!    with another request is refused (D68).
//! 3. The request is routed to its destination and the hash of each NFS file is read ([`PrService::preflight`]): that is the change
//!    the pull request is linked to. Nothing goes to GitHub.
//! 4. **One open pull request per change.** If any of the (path, hash) pairs is linked to an open pull request, nothing is made
//!    and the caller gets that link (409 with the link). The same check is made again under a lock when the job is inserted, and
//!    again by the job against the plan's hashes; the unique index on `pr_links` is the last guard.
//! 5. A user who `requires_approval` gets a pending proposal and no job (D37); the job is made when an admin approves.
//! 6. Otherwise the job is inserted, `PrJobProgress` is published, and the key's response is stored, all in one transaction. A
//!    job that the same person already has unfinished for the same paths is that job again.
//!
//! No audit event is written for the acceptance: it changes nothing but the job's own bookkeeping. The pull request is audited when
//! it exists (the event records its address), in the transaction that makes its links.

use std::fmt::Write as _;
use std::time::Duration;

use async_trait::async_trait;
use domain::{
    AuditId, ContentHash, NfsPath, Paged, PrJobId, PrJobState, PrRequest, ProposalId, RejectReason, Role,
    User, WriteCtx, WriteOutcome,
};
use ports::WriteError;
use serde_json::json;
use tokio::time::Instant;
use tracing::Instrument;

use super::PrError;
use super::job::{self, NewJob, PrJobView, StepData};
use super::links::{self, LinkQuery, PrLinkView};
use super::runner::{PrJobRunner, view_of};
use crate::error::{Internal, Res};
use crate::idempotency::{RequestHash, Response, finish_job_in_tx, lock_claim_any};
use crate::pipeline::Done;
use crate::pipeline::approval::{Proposal, propose};
use crate::pipeline::authorize::{authorize, authorize_user};
use crate::pipeline::claim::{Claim, ClaimedAny, claim_any};

/// What asking for a pull request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raised {
    /// Accepted as this job (a new one, or the one an earlier try with the same key or the same request already made). Follow it
    /// over SSE (`PrJobProgress`) or `GET /pr-jobs/{id}`.
    Job(PrJobView),
    /// An open pull request already covers the change; here is its link (409).
    Duplicate(PrLinkView),
    /// The user requires approval: a proposal was made instead (D37).
    Proposal(ProposalId),
    /// Nothing was made: the request cannot become a pull request. The outcome says why (422).
    Refused(WriteOutcome),
}

/// The pull-request routes call this. [`PrJobRunner`] implements it.
#[async_trait]
pub trait PrDesk: Send + Sync + 'static {
    /// Ask for a pull request. Editor and above.
    async fn raise(&self, ctx: &WriteCtx, r: PrRequest) -> Result<Raised, WriteError>;

    /// A job. Viewer and above.
    async fn job(&self, user: &User, id: PrJobId) -> Result<Option<PrJobView>, WriteError>;

    /// A page of pull-request links, newest first. Viewer and above.
    async fn links(&self, user: &User, q: LinkQuery) -> Result<Paged<PrLinkView>, WriteError>;

    /// Run the job in the background if a slot is free. Returns whether it was started; the scan finds a job that was not.
    fn start(&self, id: PrJobId) -> bool;
}

#[async_trait]
impl PrDesk for PrJobRunner {
    async fn raise(&self, ctx: &WriteCtx, r: PrRequest) -> Result<Raised, WriteError> {
        let span = tracing::info_span!(
            "pr.raise",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
            paths = r.paths.len(),
        );
        submit(self, ctx, r).instrument(span).await
    }

    async fn job(&self, user: &User, id: PrJobId) -> Result<Option<PrJobView>, WriteError> {
        let hub = self.hub();
        authorize_user(hub, &user.id, Role::Viewer).await?;
        Ok(hub.within("pr_job_view", view_of(hub, id)).await??)
    }

    async fn links(&self, user: &User, q: LinkQuery) -> Result<Paged<PrLinkView>, WriteError> {
        let hub = self.hub();
        authorize_user(hub, &user.id, Role::Viewer).await?;
        links::page(hub, &q).await
    }

    fn start(&self, id: PrJobId) -> bool {
        PrJobRunner::start(self, id)
    }
}

impl PrJobRunner {
    /// Ask for a pull request and wait for the job, for a caller that answers with an outcome (`WriteService::raise_pr`): the job is
    /// driven in this call, up to `WriteConfig::pr_wait` (30 s: a pull request is five or six GitHub calls), and a step still
    /// running at the end is stopped. `Applied` is the pull request, recorded; `Conflict` is a second pull request for the change;
    /// a job that failed is `Rejected`. A job that is still running when the wait ends is `Unavailable`: it goes on, and a retry
    /// with the same key finds it.
    pub(crate) async fn raise_and_wait(
        &self,
        ctx: &WriteCtx,
        r: PrRequest,
    ) -> Result<WriteOutcome, WriteError> {
        match self.raise(ctx, r).await? {
            Raised::Proposal(id) => Ok(WriteOutcome::ProposalCreated { id }),
            Raised::Duplicate(_) => Ok(WriteOutcome::Conflict { current: None }),
            Raised::Refused(outcome) => Ok(outcome),
            Raised::Job(view) => self.wait_for(PrJobId::from(view.id)).await,
        }
    }

    async fn wait_for(&self, id: PrJobId) -> Result<WriteOutcome, WriteError> {
        let hub = self.hub();
        let deadline = Instant::now() + hub.cfg.pr_wait;
        let mut pause = Duration::from_millis(250);
        loop {
            self.drive_until(id, Some(deadline)).await;
            let job = async {
                let mut conn = hub
                    .pool
                    .acquire()
                    .await
                    .map_err(|e| Internal::db("pr_job_get", &e))?;
                job::get(&mut conn, id).await
            };
            let Some(job) = hub.within("pr_job_get", job).await?? else {
                return Err(WriteError::Unavailable);
            };
            match job.state {
                PrJobState::Completed => {
                    let audit = job.step.audit.ok_or(WriteError::Unavailable)?;
                    return Ok(WriteOutcome::Applied {
                        hash: None,
                        audit_id: AuditId::from(audit),
                        warnings: Vec::new(),
                    });
                }
                PrJobState::Failed => {
                    return Ok(WriteOutcome::Rejected {
                        reason: RejectReason::NotAllowed,
                        issues: Vec::new(),
                    });
                }
                _ => {}
            }
            if Instant::now() + pause >= deadline {
                return Err(WriteError::Unavailable);
            }
            let jitter = fastrand::u64(0..=pause.as_millis().try_into().unwrap_or(250) / 2);
            tokio::time::sleep(pause + Duration::from_millis(jitter)).await;
            pause = (pause * 2).min(Duration::from_secs(4));
        }
    }
}

async fn submit(runner: &PrJobRunner, ctx: &WriteCtx, r: PrRequest) -> Result<Raised, WriteError> {
    let hub = runner.hub();
    authorize(hub, ctx, Role::Editor).await?;
    let claim = match claim_any(hub, ctx, RequestHash::of_raise_pr(&r)).await? {
        ClaimedAny::Mine(claim) => claim,
        ClaimedAny::Job(id) => return replayed_job(runner, id).await,
        ClaimedAny::Replay(WriteOutcome::ProposalCreated { id }) => return Ok(Raised::Proposal(id)),
        ClaimedAny::Replay(other) => return Ok(Raised::Refused(other)),
    };
    let result = accept(runner, ctx, &claim, &r).await;
    // Only a stored response keeps the key; a duplicate, a refusal and a failure are not outcomes of the request.
    if !matches!(result, Ok((_, true))) {
        let hub = runner.hub();
        if !matches!(
            hub.within("release", hub.idem.release(&claim.key)).await,
            Ok(Ok(()))
        ) {
            tracing::warn!("could not release an idempotency claim");
        }
    }
    result.map(|(raised, _)| raised)
}

async fn replayed_job(runner: &PrJobRunner, id: PrJobId) -> Result<Raised, WriteError> {
    let hub = runner.hub();
    match hub.within("pr_job_view", view_of(hub, id)).await?? {
        Some(view) => Ok(Raised::Job(view)),
        None => Err(WriteError::Unavailable),
    }
}

/// What a PR error at the pre-flight means to the caller.
fn refused(e: PrError) -> Result<Raised, WriteError> {
    let rejected = |reason, issues| Ok(Raised::Refused(WriteOutcome::Rejected { reason, issues }));
    match e {
        PrError::Forbidden => Err(WriteError::Forbidden),
        PrError::UnknownSwimlane => rejected(RejectReason::UnknownSwimlane, Vec::new()),
        PrError::Rejected { reason, issues, .. } => rejected(reason, issues),
        PrError::Target(_) | PrError::NotOnNfs { .. } => rejected(RejectReason::InvalidContent, Vec::new()),
        _ => Err(WriteError::Unavailable),
    }
}

/// The request, checked and decided: `(what to answer, whether the key's response is stored)`.
async fn accept(
    runner: &PrJobRunner,
    ctx: &WriteCtx,
    claim: &Claim,
    r: &PrRequest,
) -> Result<(Raised, bool), WriteError> {
    let hub = runner.hub();
    let pre = match runner.service().preflight(ctx, r).await {
        Ok(p) => p,
        Err(e) => return refused(e).map(|raised| (raised, false)),
    };
    // Sorted, so that two requests for the same files are the same request.
    let mut rows: Vec<(String, ContentHash)> = r
        .paths
        .iter()
        .map(|p| p.as_str().to_owned())
        .zip(pre.hashes.iter().copied())
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let existing = async {
        let mut conn = hub
            .pool
            .acquire()
            .await
            .map_err(|e| Internal::db("pr_link_open", &e))?;
        links::find_open(&mut conn, &r.swimlane, &rows).await
    };
    if let Some(link) = hub.within("pr_link_open", existing).await?? {
        return Ok((Raised::Duplicate(link.view()), false));
    }

    if pre.user.requires_approval {
        return propose_pr(runner, ctx, &pre.user, claim, r, &rows).await;
    }
    let made = hub
        .within("pr_job_make", make_job(runner, ctx, &pre.user, claim, r, &rows))
        .await??;
    // The checks under the swimlane's lock can find a pull request opened since the first look: that is a duplicate, which keeps
    // no key, like the one found before. Every other answer of the transaction is the key's stored response.
    let stored = !matches!(made, Raised::Duplicate(_));
    Ok((made, stored))
}

/// A pending proposal in place of a job, for a user who `requires_approval`.
async fn propose_pr(
    runner: &PrJobRunner,
    ctx: &WriteCtx,
    user: &User,
    claim: &Claim,
    r: &PrRequest,
    rows: &[(String, ContentHash)],
) -> Result<(Raised, bool), WriteError> {
    let hub = runner.hub();
    let first = rows
        .first()
        .and_then(|(p, _)| NfsPath::parse(p).ok())
        .ok_or(WriteError::BadRequest)?;
    let base_hashes: serde_json::Map<String, serde_json::Value> = rows
        .iter()
        .map(|(p, h)| (p.clone(), serde_json::Value::String(h.to_string())))
        .collect();
    let mut diff = format!("pull request \"{}\" for {} file(s)\n", r.title, rows.len());
    for (p, h) in rows {
        let _ = writeln!(diff, "{p} @ {h}");
    }
    if diff.len() > 60 * 1024 {
        let mut end = 60 * 1024;
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
    }
    let proposal = Proposal {
        ctx,
        user,
        claim,
        swimlane: &r.swimlane,
        action: "pr",
        path: Some(&first),
        paths: rows.iter().map(|(p, _)| p.clone()).collect(),
        base_hashes: serde_json::Value::Object(base_hashes),
        hash_before: None,
        content: None,
        params: json!({
            "title": r.title.as_str(),
            "body": r.body,
            "target": r.target,
            "via": ctx.via.as_str(),
        }),
        diff,
        draft: None,
    };
    match propose(hub, proposal).await {
        Ok(Done::Stored(WriteOutcome::ProposalCreated { id })) => Ok((Raised::Proposal(id), true)),
        Ok(Done::Stored(other) | Done::Store(other) | Done::Release(other)) => {
            Ok((Raised::Refused(other), false))
        }
        Err(abort) => Err(abort.error),
    }
}

/// The transaction that accepts the request: under a lock on the swimlane's pull requests, the checks are made again, the job is
/// inserted with its first progress event, and the key's response is stored.
async fn make_job(
    runner: &PrJobRunner,
    ctx: &WriteCtx,
    user: &User,
    claim: &Claim,
    r: &PrRequest,
    rows: &[(String, ContentHash)],
) -> Res<Raised> {
    let hub = runner.hub();
    let mut tx = hub
        .pool
        .begin()
        .await
        .map_err(|e| Internal::db("pr_job_make", &e))?;
    match lock_claim_any(&mut tx, &claim.key).await? {
        None => {}
        // Finished by another request with this key while this one worked: its answer stands.
        Some(Response::Job(id)) => {
            let _ = tx.rollback().await;
            return replayed(hub, id).await;
        }
        Some(Response::Outcome(WriteOutcome::ProposalCreated { id })) => {
            let _ = tx.rollback().await;
            return Ok(Raised::Proposal(id));
        }
        Some(Response::Outcome(other)) => {
            let _ = tx.rollback().await;
            return Ok(Raised::Refused(other));
        }
    }
    // One at a time per swimlane: two requests for the same change cannot both pass the checks below.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("pr:{}", r.swimlane))
        .execute(&mut *tx)
        .await
        .map_err(|e| Internal::db("pr_job_make", &e))?;
    if let Some(link) = links::find_open(&mut tx, &r.swimlane, rows).await? {
        let _ = tx.rollback().await;
        return Ok(Raised::Duplicate(link.view()));
    }
    let paths: Vec<String> = rows.iter().map(|(p, _)| p.clone()).collect();
    let id = if let Some(id) = job::active_for(&mut tx, &user.id, &r.swimlane, &paths).await? {
        id
    } else {
        let step = StepData::new(ctx.via, r.target.clone(), None);
        let id = job::insert(
            &mut tx,
            &NewJob {
                author: &user.id,
                swimlane: &r.swimlane,
                paths,
                title: r.title.as_str(),
                body: r.body.as_deref(),
                step: &step,
                request_id: &ctx.request_id,
                now_ms: hub.now_ms(),
            },
        )
        .await?;
        job::publish_progress(hub, &mut tx, id, PrJobState::Pending).await?;
        id
    };
    finish_job_in_tx(&mut tx, &claim.key, id).await?;
    tx.commit().await.map_err(|e| Internal::db("pr_job_make", &e))?;
    replayed(hub, id).await
}

async fn replayed(hub: &crate::service::Inner, id: PrJobId) -> Res<Raised> {
    view_of(hub, id)
        .await?
        .map(Raised::Job)
        .ok_or_else(|| Internal::new("pr_job_make", "job"))
}
