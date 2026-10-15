//! Running a pull-request job (T9, D77): one durable step at a time, resumable after a crash in any state, idempotent through the
//! job's id.
//!
//! **A step** reads the job, does the work of its state, and moves the job (and says what it produced) in one transaction guarded
//! by the state it ran in. What happens outside the database is made repeatable by what T6 built: the plan is kept in the row, so
//! a resumed `committing` builds the same tree on the same head; the branch it finds already there with that tree is its own
//! earlier work; and a pull request that is open for the branch is the answer of an earlier `opening`. The one thing that cannot be
//! repeated, the audit event, is written once, in the transaction that makes the links, and `step_data.audit` says it was.
//!
//! **Crash points.** After any commit the job is in the next state with its product stored, and a runner resumes from there. A
//! crash inside a step, between the GitHub call and the commit that records it, repeats that one step against GitHub, which finds
//! its own earlier work. The audit event is the only record of the pull request that is not repeatable, and it is written
//! with the links: both exist or neither does.
//!
//! **Who runs.** A request starts the job in the background ([`PrDesk::start`](super::submit::PrDesk::start)); the leased scan
//! ([`PrJobRunner::run_once`]) takes up what a crash or a restart left: `pending` jobs at once and any other unfinished job nobody
//! has touched for a while. A lease on the job's name keeps two replicas from driving one job. The job runs as its author: their
//! own token is opened for each GitHub call (S8), after the directory is asked again whether they are still active and an Editor,
//! so a user who is disabled while a job waits has the job fail instead of run.
//!
//! **Failure.** What the user can fix, or what will not change by waiting, fails the job at once with a sentence that names no
//! path, content or token. What may pass (GitHub unavailable or rate limiting, the store not answering) is counted, and the job
//! is tried again by the scan until it has used [`MAX_ATTEMPTS`] tries. One failure is not counted: a pull request that is open
//! on GitHub and could not be recorded (a failed job would leave it with no record), which is tried again until it is recorded.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use domain::{
    AuditId, ContentHash, DomainEvent, IdempotencyKey, NfsPath, PrJobId, PrJobState, ProposalStatus,
    RejectReason, RequestId, ShortText, SwimlaneId, UserId, Via, WriteCtx,
};
use ports::{AuditAction, AuditActor, AuditEvent, Tx};
use sqlx::Row;
use tokio::sync::Semaphore;

use super::job::{self, JobRow, PrJobView, StepData, StoredPlan};
use super::links::{self, EVENT_PATH_BYTES, NewLinks};
use super::plan::PrPlan;
use super::{Committed, Opened, PrError, PrService};
use crate::error::{Internal, Res};
use crate::github::GithubError;
use crate::pipeline::approval::via_of;
use crate::service::Inner;
use crate::vault::TokenError;

/// How many times a step that fails for a reason that may pass is tried before the job fails.
pub const MAX_ATTEMPTS: i32 = 5;

/// One step, including its calls to GitHub, may take this long.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// The lease on a job, renewed between its steps.
const JOB_LEASE: Duration = Duration::from_secs(90);

/// A job nobody has touched for this long is taken up by the scan (a runner moves or retries a job well inside it).
const STALE_AFTER: Duration = Duration::from_secs(30);

/// The most jobs one run of the scan takes up.
const SCAN_BATCH: i64 = 50;

/// The most steps one drive of a job takes: the states, and the sub-steps of `opening`, with room.
const MAX_STEPS: usize = 12;

/// The most jobs a process runs in the background at once. A request that would be the next is not started: the scan finds it.
const MAX_RUNNING: usize = 4;

/// What one step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advanced {
    /// The job is now in this state (or went on inside `opening`).
    Moved(PrJobState),
    /// The job is `completed` or `failed`. Nothing was done.
    Finished(PrJobState),
    /// The step failed for a reason that may pass; the try is counted and the job stays where it is.
    Retry,
    /// Another runner moved the job first.
    Raced,
    /// There is no such job.
    Gone,
    /// The store did not answer; nothing was changed.
    Unavailable,
}

/// How driving a job came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driven {
    /// The job finished in this state.
    Finished(PrJobState),
    /// The job waits for its next try (a step failed for a reason that may pass).
    Waiting,
    /// Another runner holds the job.
    Busy,
    /// There is no such job.
    Gone,
}

/// What one run of the scan did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunReport {
    pub driven: usize,
    pub finished: usize,
    pub waiting: usize,
    pub busy: usize,
}

/// Why a step stopped.
enum Stop {
    /// May pass: count the try and leave the job where it is.
    Transient,
    /// Will not change by waiting: fail the job with this sentence.
    Permanent(String),
    /// The store did not answer: counted like a transient failure.
    Store,
    /// The pull request is open on GitHub and its record did not reach the store. Not counted: failing the job would leave a pull
    /// request that nothing records; the next try finds it and records it.
    Unrecorded,
}

impl From<Internal> for Stop {
    fn from(_: Internal) -> Self {
        Self::Store
    }
}

fn permanent(text: &str) -> Stop {
    Stop::Permanent(text.to_owned())
}

/// A sentence for the person who raised the pull request, from the error of a step. Fixed words and numbers; never a path,
/// content, a token or what GitHub said.
fn stop_of(e: PrError) -> Stop {
    match e {
        PrError::Forbidden => permanent("you may no longer raise pull requests"),
        PrError::UnknownSwimlane => permanent("the swimlane does not exist"),
        PrError::Target(t) => Stop::Permanent(t.to_string()),
        PrError::Rejected {
            index,
            reason,
            issues,
        } => {
            let what = match reason {
                RejectReason::SecretDetected => "holds a value that looks like a secret",
                RejectReason::ParseError => "does not parse",
                RejectReason::TooLarge => "is too large",
                RejectReason::Denied => "is a protected file",
                _ => "cannot go into a pull request",
            };
            let mut text = format!("file {} {what}", index + 1);
            if let Some(line) = issues.iter().find_map(|i| i.line) {
                let _ = write!(text, " (line {line})");
            }
            Stop::Permanent(text)
        }
        PrError::NotOnNfs { index } => Stop::Permanent(format!("file {} is not on NFS any more", index + 1)),
        PrError::RepoFileUnreadable { index } => Stop::Permanent(format!(
            "the repository has no regular file for file {}",
            index + 1
        )),
        PrError::NothingToChange => {
            permanent("there is nothing to change: the repository already holds this content")
        }
        PrError::TargetBranchMissing => permanent("the destination branch does not exist"),
        PrError::BranchTaken => permanent("the pull request branch is taken by something else"),
        PrError::Token(t) => match t {
            TokenError::NoToken => permanent("store a GitHub token first"),
            TokenError::Expired => permanent("the stored GitHub token has expired"),
            TokenError::GithubUnavailable | TokenError::Unavailable => Stop::Transient,
            _ => permanent("the stored GitHub token cannot be used"),
        },
        PrError::Github(g) => match g {
            GithubError::Unavailable | GithubError::RateLimited => Stop::Transient,
            GithubError::Unauthorized => permanent("GitHub rejected the stored token"),
            GithubError::Forbidden => {
                permanent("GitHub refused the request: the token may lack access to the repository")
            }
            GithubError::NotFound => permanent("GitHub does not have the repository or the branch"),
            GithubError::Unprocessable => permanent("GitHub refused the change"),
            GithubError::Unexpected | GithubError::Config => {
                permanent("GitHub gave an answer that cannot be used")
            }
        },
        PrError::Unavailable => Stop::Transient,
    }
}

/// The name of a job's lease.
fn lease_name(id: PrJobId) -> String {
    format!("pr-job:{}", id.get())
}

/// Runs pull-request jobs. Cheap to clone: the clones share one pool of background slots.
#[derive(Clone)]
pub struct PrJobRunner {
    svc: PrService,
    slots: Arc<Semaphore>,
}

impl std::fmt::Debug for PrJobRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrJobRunner").finish_non_exhaustive()
    }
}

impl PrJobRunner {
    pub(crate) fn new(svc: PrService) -> Self {
        Self {
            svc,
            slots: Arc::new(Semaphore::new(MAX_RUNNING)),
        }
    }

    pub(crate) fn hub(&self) -> &Inner {
        &self.svc.hub
    }

    pub(crate) fn service(&self) -> &PrService {
        &self.svc
    }

    // ---------------------------------------------------------------------------------------------------- driving

    /// Start the job in the background if a slot is free. Returns whether it was started; a job that was not is found by the
    /// scan.
    pub fn start(&self, id: PrJobId) -> bool {
        let Ok(permit) = Arc::clone(&self.slots).try_acquire_owned() else {
            return false;
        };
        let runner = self.clone();
        tokio::spawn(async move {
            let _slot = permit;
            runner.drive(id).await;
        });
        true
    }

    /// Run the job until it finishes, waits for a retry, or another runner has it. Holds the job's lease.
    pub async fn drive(&self, id: PrJobId) -> Driven {
        let hub = self.hub();
        let mut lease = match hub
            .within("pr_job_lease", hub.leases.try_acquire(&lease_name(id), JOB_LEASE))
            .await
        {
            Ok(Ok(Some(guard))) => guard,
            Ok(Ok(None)) => return Driven::Busy,
            Ok(Err(_)) | Err(_) => return Driven::Waiting,
        };
        let mut end = Driven::Waiting;
        for _ in 0..MAX_STEPS {
            match self.advance(id).await {
                Advanced::Moved(_) => {
                    if lease.renew(JOB_LEASE).await.is_err() {
                        // The lease is lost: somebody else may be running the job now.
                        return Driven::Busy;
                    }
                }
                Advanced::Finished(state) => {
                    end = Driven::Finished(state);
                    break;
                }
                Advanced::Retry | Advanced::Unavailable => break,
                Advanced::Raced => {
                    end = Driven::Busy;
                    break;
                }
                Advanced::Gone => {
                    end = Driven::Gone;
                    break;
                }
            }
        }
        if lease.release().await.is_err() {
            tracing::warn!("a pull request job lease could not be released; it runs out by itself");
        }
        end
    }

    /// The leased job (D63): take up the jobs a crash or a restart left. `still_held` is asked before each job; when it says no,
    /// the run stops (the lease is lost).
    pub async fn run_once(&self, still_held: &dyn Fn() -> bool) -> RunReport {
        let hub = self.hub();
        let mut report = RunReport::default();
        let cutoff = hub.now_ms().saturating_sub(crate::service::millis(STALE_AFTER));
        let ids = async {
            let mut conn = hub
                .pool
                .acquire()
                .await
                .map_err(|e| Internal::db("pr_job_resume", &e))?;
            job::needing_a_runner(&mut conn, cutoff, SCAN_BATCH).await
        };
        let Ok(Ok(ids)) = hub.within("pr_job_resume", ids).await else {
            return report;
        };
        for id in ids {
            if !still_held() {
                break;
            }
            report.driven += 1;
            match self.drive(id).await {
                Driven::Finished(_) => report.finished += 1,
                Driven::Waiting => report.waiting += 1,
                Driven::Busy => report.busy += 1,
                Driven::Gone => {}
            }
        }
        report
    }

    // ----------------------------------------------------------------------------------------------------- stepping

    /// One durable step of the job. Idempotent: a finished job does nothing, and a step another runner took first is `Raced`.
    pub async fn advance(&self, id: PrJobId) -> Advanced {
        let hub = self.hub();
        let found = async {
            let mut conn = hub
                .pool
                .acquire()
                .await
                .map_err(|e| Internal::db("pr_job_get", &e))?;
            job::get(&mut conn, id).await
        };
        let job = match hub.within("pr_job_get", found).await {
            Ok(Ok(Some(job))) => job,
            Ok(Ok(None)) => return Advanced::Gone,
            Ok(Err(_)) | Err(_) => return Advanced::Unavailable,
        };
        if job.is_finished() {
            return Advanced::Finished(job.state);
        }
        let span =
            tracing::info_span!("pr.job", job = id.get(), state = %job.state, swimlane = %job.swimlane);
        let result = {
            use tracing::Instrument;
            match tokio::time::timeout(STEP_TIMEOUT, self.step(&job))
                .instrument(span)
                .await
            {
                Ok(r) => r,
                Err(_) => Err(Stop::Transient),
            }
        };
        match result {
            Ok(advanced) => advanced,
            Err(Stop::Permanent(text)) => self.fail(&job, &text).await,
            Err(Stop::Unrecorded) => Advanced::Unavailable,
            // The store not answering may pass, like GitHub not answering, and is counted the same way: a store that never comes
            // back must not leave the job active for ever, to be picked up by every scan and to take a place in its batch. When
            // the database itself is down the count cannot be written either, and `retry` answers `Unavailable` as before.
            Err(Stop::Transient | Stop::Store) => self.retry(&job).await,
        }
    }

    async fn step(&self, job: &JobRow) -> Result<Advanced, Stop> {
        match job.state {
            PrJobState::Pending => self.persist(job, PrJobState::Preparing, &job.step, &[]).await,
            PrJobState::Preparing => self.prepare(job).await,
            PrJobState::Committing => self.commit(job).await,
            PrJobState::Opening => self.open(job).await,
            PrJobState::Completed | PrJobState::Failed => Ok(Advanced::Finished(job.state)),
        }
    }

    /// `preparing`: the plan, kept for the steps after it.
    async fn prepare(&self, job: &JobRow) -> Result<Advanced, Stop> {
        let hub = self.hub();
        let ctx = self.ctx_for(job).await?;
        let request = request_of(job)?;
        let plan = self
            .svc
            .plan(&ctx, &request)
            .await
            .map_err(stop_of)?
            .for_job(job.id);
        if let Some(proposal) = job.step.proposal {
            self.check_approved(domain::ProposalId::from(proposal), &plan)
                .await?;
        }

        // The change as it is now: a pull request that is open for any of these files, at these hashes, makes this one a second.
        let pairs: Vec<(String, ContentHash)> = plan
            .files
            .iter()
            .map(|f| (f.nfs.as_str().to_owned(), f.nfs_hash))
            .collect();
        let existing = async {
            let mut conn = hub
                .pool
                .acquire()
                .await
                .map_err(|e| Internal::db("pr_link_open", &e))?;
            links::find_open(&mut conn, &plan.swimlane, &pairs).await
        };
        if let Some(link) = hub.within("pr_link_open", existing).await?? {
            return Err(Stop::Permanent(format!(
                "an open pull request (#{}) already covers this change",
                link.number
            )));
        }

        // The files, in the form they will be committed, kept as blobs: the row holds their hashes.
        let mut blobs = Vec::with_capacity(plan.files.len());
        for f in &plan.files {
            let hash = hub
                .within("pr_job_blob", hub.blobs.put(f.content.clone()))
                .await?
                .map_err(|_| Stop::Store)?;
            blobs.push(hash);
        }
        let mut step = job.step.clone();
        step.plan = Some(StoredPlan::of(&plan, &blobs));
        self.persist(job, PrJobState::Committing, &step, &blobs).await
    }

    /// A job that carries out an approved proposal makes its pull request from the files the admin approved, at the hashes their
    /// author saw. The plan has just read each file again; one with another hash is not what was approved, and the pull request
    /// would carry something nobody looked at.
    async fn check_approved(&self, id: domain::ProposalId, plan: &PrPlan) -> Result<(), Stop> {
        let hub = self.hub();
        let found = async {
            let mut conn = hub
                .pool
                .acquire()
                .await
                .map_err(|e| Internal::db("pr_job_proposal", &e))?;
            crate::proposals::store::get(&mut conn, id).await
        };
        let Some(proposal) = hub.within("pr_job_proposal", found).await?? else {
            return Err(permanent("the proposal of the job is gone"));
        };
        for f in &plan.files {
            match proposal.expected_at(f.nfs.as_str()) {
                Some(crate::proposals::store::Base::At(approved)) if approved == f.nfs_hash => {}
                _ => return Err(permanent("a file changed after the proposal was approved")),
            }
        }
        Ok(())
    }

    /// `committing`: the commit and the branch.
    async fn commit(&self, job: &JobRow) -> Result<Advanced, Stop> {
        let ctx = self.ctx_for(job).await?;
        let plan = self.plan_of(job, true).await?;
        let committed = self.svc.commit(&ctx, &plan).await.map_err(stop_of)?;
        let mut step = job.step.clone();
        step.committed = Some(committed);
        self.persist(job, PrJobState::Opening, &step, &[]).await
    }

    /// `opening`: the pull request; then it is recorded (links, audit event); then its body gets the audit id; then the job is done.
    async fn open(&self, job: &JobRow) -> Result<Advanced, Stop> {
        let ctx = self.ctx_for(job).await?;
        let plan = self.plan_of(job, false).await?;
        let Some(committed) = job.step.committed.clone() else {
            return Err(permanent("the job lost its branch"));
        };
        let Some(audit) = job.step.audit else {
            let opened = match &job.step.opened {
                Some(o) => o.clone(),
                None => self.svc.open(&ctx, &plan, &committed).await.map_err(stop_of)?,
            };
            return self.record(job, &plan, &committed, opened).await;
        };
        let Some(opened) = job.step.opened.clone() else {
            return Err(permanent("the job lost its pull request"));
        };
        if let Err(e) = self
            .svc
            .note_audit(&ctx, &plan, &opened, AuditId::from(audit))
            .await
        {
            // The pull request exists and is recorded; only its body lacks the audit id. Try again a few times, then let it go.
            if job.attempts + 1 < MAX_ATTEMPTS {
                tracing::warn!(
                    "the audit id could not be put in the pull request body; it will be tried again"
                );
                return Err(Stop::Transient);
            }
            tracing::warn!(error = %e, "the audit id was never put in the pull request body");
        }
        self.persist(job, PrJobState::Completed, &job.step, &[]).await
    }

    // ----------------------------------------------------------------------------------------------------- helpers

    /// The job's author as a write context. The directory is asked again by every step that uses the token.
    async fn ctx_for(&self, job: &JobRow) -> Result<WriteCtx, Stop> {
        let hub = self.hub();
        let user = hub
            .within("pr_job_user", hub.users.get(&job.author))
            .await?
            .map_err(|_| Stop::Store)?;
        let Some(user) = user else {
            return Err(permanent("the author of the pull request is no longer known"));
        };
        let request_id = job
            .request_id
            .as_deref()
            .and_then(|r| RequestId::parse(r).ok())
            .or_else(|| RequestId::parse(&format!("pr-job-{}", job.id)).ok())
            .ok_or(Stop::Store)?;
        Ok(WriteCtx {
            user,
            via: job.step.via.unwrap_or(Via::Ui),
            idempotency_key: IdempotencyKey::parse(&format!("pr-job-{}", job.id)).map_err(|_| Stop::Store)?,
            request_id,
        })
    }

    /// The plan the row keeps, with the files' bytes read from their blobs when the step needs them.
    async fn plan_of(&self, job: &JobRow, with_content: bool) -> Result<PrPlan, Stop> {
        let hub = self.hub();
        let Some(stored) = job.step.plan.clone() else {
            return Err(permanent("the job lost its plan"));
        };
        let contents = if with_content {
            let mut out = Vec::with_capacity(stored.files.len());
            for hash in stored.blob_hashes() {
                match hub.within("pr_job_blob", hub.blobs.get(&hash)).await? {
                    Ok(Some(bytes)) if ports::content_hash(&bytes) == hash => out.push(bytes),
                    Ok(Some(_) | None) => {
                        return Err(permanent(
                            "the prepared files are gone: raise the pull request again",
                        ));
                    }
                    Err(_) => return Err(Stop::Store),
                }
            }
            Some(out)
        } else {
            None
        };
        stored
            .into_plan(job, contents)
            .ok_or_else(|| permanent("the job lost its plan"))
    }

    /// Move the job to `to` with what the step produced, and publish the progress.
    async fn persist(
        &self,
        job: &JobRow,
        to: PrJobState,
        step: &StepData,
        keep: &[ContentHash],
    ) -> Result<Advanced, Stop> {
        let hub = self.hub();
        let moved = hub
            .within("pr_job_move", async {
                let mut tx = hub
                    .pool
                    .begin()
                    .await
                    .map_err(|e| Internal::db("pr_job_move", &e))?;
                if !job::advance_in_tx(hub, &mut tx, job.id, job.state, to, step, None).await? {
                    let _ = tx.rollback().await;
                    return Ok::<bool, Internal>(false);
                }
                if !keep.is_empty() {
                    job::keep_blobs(&mut tx, job.id, keep).await?;
                }
                tx.commit().await.map_err(|e| Internal::db("pr_job_move", &e))?;
                Ok(true)
            })
            .await??;
        Ok(if moved {
            Advanced::Moved(to)
        } else {
            Advanced::Raced
        })
    }

    /// End the job as failed, with a sentence for the person.
    async fn fail(&self, job: &JobRow, text: &str) -> Advanced {
        self.fail_with(job, text, &job.step).await
    }

    async fn fail_with(&self, job: &JobRow, text: &str, step: &StepData) -> Advanced {
        let hub = self.hub();
        let text = sentence(text);
        let done = hub
            .within("pr_job_fail", async {
                let mut tx = hub
                    .pool
                    .begin()
                    .await
                    .map_err(|e| Internal::db("pr_job_fail", &e))?;
                let moved = job::advance_in_tx(
                    hub,
                    &mut tx,
                    job.id,
                    job.state,
                    PrJobState::Failed,
                    step,
                    Some(&text),
                )
                .await?;
                if !moved {
                    let _ = tx.rollback().await;
                    return Ok::<bool, Internal>(false);
                }
                tx.commit().await.map_err(|e| Internal::db("pr_job_fail", &e))?;
                Ok(true)
            })
            .await;
        match done {
            Ok(Ok(true)) => {
                tracing::warn!(job = job.id.get(), "a pull request job failed");
                Advanced::Finished(PrJobState::Failed)
            }
            Ok(Ok(false)) => Advanced::Raced,
            Ok(Err(_)) | Err(_) => Advanced::Unavailable,
        }
    }

    /// Count a try that failed for a reason that may pass; the job fails when the tries are used.
    async fn retry(&self, job: &JobRow) -> Advanced {
        let hub = self.hub();
        match hub.within("pr_job_bump", job::bump(hub, job.id, job.state)).await {
            Ok(Ok(Some(tries))) if tries >= MAX_ATTEMPTS => {
                self.fail(job, "GitHub or the store did not answer after several tries: raise the pull request again later")
                    .await
            }
            Ok(Ok(Some(_))) => Advanced::Retry,
            Ok(Ok(None)) => Advanced::Raced,
            Ok(Err(_)) | Err(_) => Advanced::Unavailable,
        }
    }

    /// The pull request exists: make its links, record its audit event and the events, in one transaction. A second pull request
    /// for one of its files (a race that the check before the job could not see) fails the job; the pull request stays on GitHub
    /// and the sentence says so.
    async fn record(
        &self,
        job: &JobRow,
        plan: &PrPlan,
        committed: &Committed,
        opened: Opened,
    ) -> Result<Advanced, Stop> {
        let hub = self.hub();
        let result = hub
            .within("pr_record", record_tx(hub, job, plan, committed, &opened))
            .await
            .map_err(|_| Stop::Unrecorded)?;
        match result {
            Ok(Recorded::Done) => Ok(Advanced::Moved(PrJobState::Opening)),
            Ok(Recorded::Raced) => Ok(Advanced::Raced),
            Ok(Recorded::Duplicate) => {
                let mut step = job.step.clone();
                step.opened = Some(opened.clone());
                Ok(self
                    .fail_with(
                        job,
                        &format!(
                            "pull request #{} was opened, but an open pull request already covers one of its files",
                            opened.number
                        ),
                        &step,
                    )
                    .await)
            }
            Err(_) => Err(Stop::Unrecorded),
        }
    }
}

/// A failure sentence that fits `ShortText` (it is shown as the job's `error`).
fn sentence(text: &str) -> String {
    let mut out: String = text.chars().filter(|c| !c.is_control()).collect();
    if out.len() > 250 {
        let mut end = 250;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// The request a job was made from.
fn request_of(job: &JobRow) -> Result<domain::PrRequest, Stop> {
    let paths = job
        .paths
        .iter()
        .map(|p| NfsPath::parse(p))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| permanent("the job holds a path that is not valid"))?;
    Ok(domain::PrRequest {
        swimlane: job.swimlane.clone(),
        paths,
        title: ShortText::parse(&job.title)
            .map_err(|_| permanent("the job holds a title that is not valid"))?,
        body: job.body.clone(),
        target: job.step.target.clone(),
    })
}

enum Recorded {
    Done,
    Raced,
    /// `pr_links_one_open` refused a link.
    Duplicate,
}

fn is_one_open(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .is_some_and(|d| d.code().as_deref() == Some("23505") && d.constraint() == Some("pr_links_one_open"))
}

/// The login linked to the user (the audit event names it, as the vault's do).
async fn login_of(conn: &mut sqlx::PgConnection, user: &UserId) -> Res<Option<String>> {
    let row = sqlx::query("SELECT github_login FROM users WHERE tid = $1 AND oid = $2")
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_record", &e))?;
    row.map(|r| r.try_get::<Option<String>, _>("github_login"))
        .transpose()
        .map_err(|e| Internal::db("pr_record", &e))
        .map(Option::flatten)
}

/// What the audit event of a pull request says: where it is, where it came from, and what it carries. File names and hashes,
/// never content.
fn diff_text(plan: &PrPlan, committed: &Committed, opened: &Opened, job: PrJobId) -> String {
    let mut out = format!(
        "pull request #{} {}\nrepository {}, base branch {}, from branch {}\njob {}\n",
        opened.number, opened.url, plan.repo, plan.dest.branch, committed.branch, job
    );
    for f in &plan.files {
        let _ = writeln!(out, "{} -> {} @ {}", f.nfs, f.repo_path, f.nfs_hash);
    }
    if out.len() > AuditEvent::MAX_DIFF_BYTES {
        let mut end = AuditEvent::MAX_DIFF_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// `DriftChanged` for these paths, in events that stay inside the bus's cap.
pub(crate) fn drift_events(swimlane: &SwimlaneId, paths: &[NfsPath]) -> Vec<DomainEvent> {
    let mut events = Vec::new();
    let mut chunk: Vec<NfsPath> = Vec::new();
    let mut bytes = 0_usize;
    for p in paths {
        if !chunk.is_empty() && bytes + p.as_str().len() > EVENT_PATH_BYTES {
            events.push(DomainEvent::DriftChanged {
                swimlane: swimlane.clone(),
                paths: std::mem::take(&mut chunk),
            });
            bytes = 0;
        }
        bytes += p.as_str().len();
        chunk.push(p.clone());
    }
    if !chunk.is_empty() {
        events.push(DomainEvent::DriftChanged {
            swimlane: swimlane.clone(),
            paths: chunk,
        });
    }
    events
}

/// The audit event of a pull request: the author acts (the pull request is made with their token), the proposal that an admin
/// approved is its `approval`, and the diff records where the pull request is.
fn audit_event(
    hub: &Inner,
    job: &JobRow,
    plan: &PrPlan,
    committed: &Committed,
    opened: &Opened,
    login: Option<&str>,
) -> AuditEvent {
    let actor = AuditActor::User {
        user: job.author.clone(),
    };
    let via = via_of(job.step.via.unwrap_or(Via::Ui));
    let mut event = AuditEvent::new(hub.clock.now(), actor, AuditAction::PrRaised, via);
    event.swimlane = Some(job.swimlane.clone());
    event.path = plan.files.first().map(|f| f.nfs.clone());
    event.diff = Some(diff_text(plan, committed, opened, job.id));
    event.github_login = login.and_then(|l| ShortText::parse(l).ok());
    event.approval = job.step.proposal.map(domain::ProposalId::from);
    event.request_id = job.request_id.as_deref().and_then(|r| RequestId::parse(r).ok());
    event
}

/// The recording transaction (rule 8, D80): the links, the one audit event, the proposal the job carried out, the events, and the
/// job's own `opened` and `audit`, together.
async fn record_tx(
    hub: &Inner,
    job: &JobRow,
    plan: &PrPlan,
    committed: &Committed,
    opened: &Opened,
) -> Res<Recorded> {
    let mut tx = hub
        .pool
        .begin()
        .await
        .map_err(|e| Internal::db("pr_record", &e))?;
    // The job as it is now: another runner may have recorded it already.
    match job::lock(&mut tx, job.id).await? {
        Some(now) if now.state == PrJobState::Opening && now.step.audit.is_none() => {}
        _ => {
            let _ = tx.rollback().await;
            return Ok(Recorded::Raced);
        }
    }
    let login = login_of(&mut tx, &job.author).await?;
    let files: Vec<(String, ContentHash)> = plan
        .files
        .iter()
        .map(|f| (f.nfs.as_str().to_owned(), f.nfs_hash))
        .collect();
    let made = links::insert_all(
        &mut tx,
        &NewLinks {
            swimlane: &job.swimlane,
            repo: plan.dest.side.as_str(),
            branch: committed.branch.as_str(),
            number: opened.number,
            job: job.id,
            author: &job.author,
            login: login.as_deref(),
            url: &opened.url,
            files,
            now_ms: hub.now_ms(),
        },
    )
    .await;
    match made {
        Ok(_) => {}
        Err(e) if is_one_open(&e) => {
            let _ = tx.rollback().await;
            return Ok(Recorded::Duplicate);
        }
        Err(e) => return Err(Internal::db("pr_links", &e)),
    }

    if let Some(proposal) = job.step.proposal {
        let id = domain::ProposalId::from(proposal);
        if crate::proposals::store::mark_applied(&mut tx, id, hub.now_ms()).await? {
            hub.bus
                .publish_in_tx(
                    &mut Tx::from_pg(&mut tx),
                    DomainEvent::ProposalChanged {
                        id,
                        status: ProposalStatus::Applied,
                    },
                )
                .await
                .map_err(|_| Internal::new("pr_record", "event outbox"))?;
        }
    }
    let paths: Vec<NfsPath> = plan.files.iter().map(|f| f.nfs.clone()).collect();
    for e in drift_events(&job.swimlane, &paths) {
        hub.bus
            .publish_in_tx(&mut Tx::from_pg(&mut tx), e)
            .await
            .map_err(|_| Internal::new("pr_record", "event outbox"))?;
    }

    // The audit log last: its chain lock is held until the commit, and everything after it is one statement.
    let event = audit_event(hub, job, plan, committed, opened, login.as_deref());
    let audit = hub
        .audit
        .record(&mut Tx::from_pg(&mut tx), event)
        .await
        .map_err(|_| Internal::new("pr_record", "audit log"))?;

    let mut step = job.step.clone();
    step.opened = Some(opened.clone());
    step.audit = Some(audit.get());
    if !job::advance_in_tx(
        hub,
        &mut tx,
        job.id,
        PrJobState::Opening,
        PrJobState::Opening,
        &step,
        None,
    )
    .await?
    {
        let _ = tx.rollback().await;
        return Ok(Recorded::Raced);
    }
    tx.commit().await.map_err(|e| Internal::db("pr_record", &e))?;
    Ok(Recorded::Done)
}

/// The job as the API shows it.
pub(crate) async fn view_of(hub: &Inner, id: PrJobId) -> Res<Option<PrJobView>> {
    let mut conn = hub
        .pool
        .acquire()
        .await
        .map_err(|e| Internal::db("pr_job_get", &e))?;
    Ok(job::get(&mut conn, id).await?.and_then(|j| j.view()))
}

#[cfg(test)]
mod tests {
    use domain::{IssueKind, ValidationIssue};

    use super::*;
    use crate::prs::target::TargetError;

    fn path(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    #[test]
    fn the_paths_of_a_pull_request_are_announced_in_events_that_fit_the_bus() {
        let swimlane = SwimlaneId::parse("sit1").unwrap();
        assert!(drift_events(&swimlane, &[]).is_empty());
        let one = drift_events(&swimlane, &[path("a.yml")]);
        assert_eq!(one.len(), 1);

        // 100 paths of the longest names: every event stays under the cap of the bus (4 KiB for the whole envelope), no path is lost
        // or repeated, and the order is kept.
        let long = |i: usize| path(&format!("{}/{i:03}.yml", vec!["d".repeat(200); 4].join("/")));
        let paths: Vec<NfsPath> = (0..100).map(long).collect();
        let events = drift_events(&swimlane, &paths);
        assert!(events.len() > 1);
        let mut seen = Vec::new();
        for e in &events {
            let json = serde_json::to_string(e).unwrap();
            assert!(json.len() < 4_096 - 64, "{} bytes", json.len());
            let DomainEvent::DriftChanged {
                swimlane: s,
                paths: p,
            } = e
            else {
                panic!("{e:?}")
            };
            assert_eq!(s, &swimlane);
            assert!(!p.is_empty());
            seen.extend(p.iter().cloned());
        }
        assert_eq!(seen, paths);
    }

    #[test]
    fn a_failure_sentence_fits_the_error_field_and_has_no_control_characters() {
        assert_eq!(sentence("fine"), "fine");
        assert_eq!(sentence("a\nb\u{7}c"), "abc");
        let long = "é".repeat(400);
        let cut = sentence(&long);
        assert!(
            cut.len() <= 250 && ShortText::parse(&cut).is_ok(),
            "{}",
            cut.len()
        );
    }

    #[test]
    fn what_may_pass_is_retried_and_what_will_not_fails_the_job_with_fixed_words() {
        use crate::vault::TokenError;
        for transient in [
            PrError::Unavailable,
            PrError::Github(GithubError::Unavailable),
            PrError::Github(GithubError::RateLimited),
            PrError::Token(TokenError::GithubUnavailable),
            PrError::Token(TokenError::Unavailable),
        ] {
            assert!(
                matches!(stop_of(transient.clone()), Stop::Transient),
                "{transient:?}"
            );
        }
        let all = [
            PrError::Forbidden,
            PrError::UnknownSwimlane,
            PrError::Target(TargetError::MixedDestinations),
            PrError::Rejected {
                index: 2,
                reason: RejectReason::SecretDetected,
                issues: vec![ValidationIssue {
                    kind: IssueKind::SecretMatch,
                    line: std::num::NonZeroU32::new(14),
                    rule: ShortText::parse("aws_access_key").ok(),
                }],
            },
            PrError::Rejected {
                index: 0,
                reason: RejectReason::TooLarge,
                issues: Vec::new(),
            },
            PrError::NotOnNfs { index: 1 },
            PrError::RepoFileUnreadable { index: 1 },
            PrError::NothingToChange,
            PrError::TargetBranchMissing,
            PrError::BranchTaken,
            PrError::Token(TokenError::NoToken),
            PrError::Token(TokenError::Expired),
            PrError::Token(TokenError::Undecryptable),
            PrError::Github(GithubError::Unauthorized),
            PrError::Github(GithubError::Forbidden),
            PrError::Github(GithubError::NotFound),
            PrError::Github(GithubError::Unprocessable),
            PrError::Github(GithubError::Unexpected),
            PrError::Github(GithubError::Config),
        ];
        for e in all {
            let Stop::Permanent(text) = stop_of(e.clone()) else {
                panic!("{e:?} should fail the job");
            };
            assert!(
                !text.is_empty() && ShortText::parse(&text).is_ok(),
                "{e:?}: {text}"
            );
            // The place of a secret is the file's number and a line, never a name or the rule.
            assert!(!text.contains("aws_access_key") && !text.contains('/'), "{text}");
        }
        let Stop::Permanent(text) = stop_of(PrError::Rejected {
            index: 2,
            reason: RejectReason::SecretDetected,
            issues: vec![ValidationIssue {
                kind: IssueKind::SecretMatch,
                line: std::num::NonZeroU32::new(14),
                rule: None,
            }],
        }) else {
            panic!()
        };
        assert_eq!(text, "file 3 holds a value that looks like a secret (line 14)");
    }

    #[test]
    fn a_job_lease_name_is_a_valid_lease_name() {
        assert!(ports::is_valid_lease_name(&lease_name(PrJobId::from(i64::MAX))));
    }
}
