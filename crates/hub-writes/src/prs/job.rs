//! The `pr_jobs` table (T9, D77): a pull request is made by a persisted job that moves one state at a time, so that a crash at any
//! point is resumed from where the job stands and never repeats a step that took effect.
//!
//! ```text
//!   pending --> preparing --> committing --> opening --> completed
//!                   |             |             |
//!                   +-------------+-------------+--> failed
//! ```
//!
//! - `pending`: accepted. Nothing was read or sent.
//! - `preparing`: the plan is being made (the NFS files, the destination branch, the files in the form of the repository).
//! - `committing`: the plan is stored; the blobs, the tree, the commit and the branch are made.
//! - `opening`: the branch is stored; the pull request is opened, recorded (the links, the audit event, the events, in one
//!   transaction), and its body gets the audit id.
//!
//! What a step produced is kept in `step_data` in the transaction that moves the state. The files of the plan are not kept in the
//! row: they are blobs (content-addressed, so a retry stores nothing twice), and the row holds their hashes. A step is guarded by
//! `UPDATE ... WHERE state = <the state it ran in>`, so two runners racing on one job cannot both move it; what they do outside
//! the database (GitHub) is made idempotent by the branch and the pull request being found again (T6).
//!
//! Every move publishes `PrJobProgress` in the transaction that makes it (D80), which is what the screen follows over SSE.

use std::str::FromStr;

use bytes::Bytes;
use domain::{
    ContentHash, DomainEvent, Guid, NfsPath, PrJobId, PrJobState, PrTarget, RepoPath, RequestId, ShortText,
    SwimlaneId, Timestamp, UserId, Via,
};
use ports::Tx;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, Postgres, Row, Transaction};

use super::plan::{PlannedFile, PrPlan};
use super::target::Destination;
use super::{Committed, Opened};
use crate::db::{ms, ts};
use crate::error::{Internal, Res};
use crate::github::{BranchName, GitSha, RepoRef};
use crate::service::Inner;

/// The version of [`StepData`] this build writes. A row of another version is refused rather than guessed at.
pub const STEP_VERSION: u8 = 1;

/// What the job has produced so far.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct StepData {
    pub v: u8,
    /// The door the request came through.
    pub via: Option<Via>,
    /// Where the person asked the pull request to go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<PrTarget>,
    /// The proposal that an admin approved to make this job, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<i64>,
    /// Made by `preparing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<StoredPlan>,
    /// Made by `committing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed: Option<Committed>,
    /// Made by `opening`: the pull request exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened: Option<Opened>,
    /// The audit event of the pull request, recorded with its links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<i64>,
}

impl StepData {
    pub fn new(via: Via, target: Option<PrTarget>, proposal: Option<i64>) -> Self {
        Self {
            v: STEP_VERSION,
            via: Some(via),
            target,
            proposal,
            plan: None,
            committed: None,
            opened: None,
            audit: None,
        }
    }
}

/// A file of the plan as the row keeps it: the content is the hash of a blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredFile {
    pub nfs: NfsPath,
    pub nfs_hash: ContentHash,
    pub repo_path: RepoPath,
    pub content: ContentHash,
    pub exists: bool,
}

/// A [`PrPlan`] as the row keeps it. The title and the person's text are the row's own columns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredPlan {
    pub dest: Destination,
    pub repo: String,
    pub head: GitSha,
    pub tree: GitSha,
    pub branch: BranchName,
    pub files: Vec<StoredFile>,
    pub unchanged: Vec<NfsPath>,
    pub warnings: Vec<ShortText>,
    pub user: UserId,
    pub request_id: RequestId,
}

impl StoredPlan {
    /// The plan with its files' content replaced by the hashes of the blobs they were stored as (in file order).
    pub fn of(plan: &PrPlan, blobs: &[ContentHash]) -> Self {
        Self {
            dest: plan.dest.clone(),
            repo: plan.repo.to_string(),
            head: plan.head.clone(),
            tree: plan.tree.clone(),
            branch: plan.branch.clone(),
            files: plan
                .files
                .iter()
                .zip(blobs)
                .map(|(f, blob)| StoredFile {
                    nfs: f.nfs.clone(),
                    nfs_hash: f.nfs_hash,
                    repo_path: f.repo_path.clone(),
                    content: *blob,
                    exists: f.exists,
                })
                .collect(),
            unchanged: plan.unchanged.clone(),
            warnings: plan.warnings.clone(),
            user: plan.user.clone(),
            request_id: plan.request_id.clone(),
        }
    }

    /// The hashes of the blobs the plan's files are kept as.
    pub fn blob_hashes(&self) -> Vec<ContentHash> {
        self.files.iter().map(|f| f.content).collect()
    }

    /// The plan again. `contents` are the files' bytes in file order; a step that only names files (opening the pull request)
    /// passes none, and the plan carries empty content it never reads.
    pub fn into_plan(self, job: &JobRow, contents: Option<Vec<Bytes>>) -> Option<PrPlan> {
        if contents.as_ref().is_some_and(|c| c.len() != self.files.len()) {
            return None;
        }
        let mut contents = contents.map(Vec::into_iter);
        let files = self
            .files
            .into_iter()
            .map(|f| PlannedFile {
                nfs: f.nfs,
                nfs_hash: f.nfs_hash,
                repo_path: f.repo_path,
                content: contents.as_mut().and_then(Iterator::next).unwrap_or_default(),
                exists: f.exists,
            })
            .collect();
        Some(PrPlan {
            swimlane: job.swimlane.clone(),
            dest: self.dest,
            repo: RepoRef::parse(&self.repo).ok()?,
            head: self.head,
            tree: self.tree,
            branch: self.branch,
            title: ShortText::parse(&job.title).ok()?,
            text: job.body.clone(),
            files,
            unchanged: self.unchanged,
            warnings: self.warnings,
            user: self.user,
            request_id: self.request_id,
            job: Some(job.id),
        })
    }
}

/// A job as the table holds it.
#[derive(Debug, Clone)]
pub(crate) struct JobRow {
    pub id: PrJobId,
    pub author: UserId,
    pub swimlane: SwimlaneId,
    pub paths: Vec<String>,
    pub title: String,
    pub body: Option<String>,
    pub state: PrJobState,
    pub failure: Option<String>,
    pub attempts: i32,
    pub step: StepData,
    pub request_id: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

/// A job as the API shows it (the `PrJob` schema of `api/openapi.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrJobView {
    pub id: i64,
    pub swimlane: SwimlaneId,
    pub state: PrJobState,
    pub paths: Vec<NfsPath>,
    pub pr_url: Option<String>,
    pub error: Option<ShortText>,
    /// What the person should know about where this goes. Absent when there is nothing to say.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ShortText>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl JobRow {
    pub fn view(&self) -> Option<PrJobView> {
        Some(PrJobView {
            id: self.id.get(),
            swimlane: self.swimlane.clone(),
            state: self.state,
            paths: self
                .paths
                .iter()
                .map(|p| NfsPath::parse(p).ok())
                .collect::<Option<_>>()?,
            pr_url: self.step.opened.as_ref().map(|o| o.url.clone()),
            error: self.failure.as_deref().and_then(|f| ShortText::parse(f).ok()),
            warnings: self
                .step
                .plan
                .as_ref()
                .map(|p| p.warnings.clone())
                .unwrap_or_default(),
            created_at: Timestamp::from_unix_millis(self.created_ms),
            updated_at: Timestamp::from_unix_millis(self.updated_ms),
        })
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.state, PrJobState::Completed | PrJobState::Failed)
    }
}

// --------------------------------------------------------------------------------------------------------------- statements

/// The select list every read of a job uses.
macro_rules! select_job {
    () => {
        concat!(
            "SELECT id, author_tid, author_oid, swimlane_id, paths, title, body, state, failure, attempts, step_data, \
             request_id, ",
            ms!("created_at"),
            " AS created_ms, ",
            ms!("updated_at"),
            " AS updated_ms FROM pr_jobs"
        )
    };
}

/// One job by its key.
pub(crate) const GET_SQL: &str = concat!(select_job!(), " WHERE id = $1");

/// One job, locked until the transaction ends.
pub(crate) const LOCK_SQL: &str = concat!(select_job!(), " WHERE id = $1 FOR UPDATE");

pub(crate) const INSERT_SQL: &str = concat!(
    "INSERT INTO pr_jobs (author_tid, author_oid, swimlane_id, paths, title, body, state, step_data, request_id, created_at, \
     updated_at) VALUES ($1, $2, $3, $4::text[], $5, $6, 'pending', $7, $8, ",
    ts!("9"),
    ", ",
    ts!("9"),
    ") RETURNING id"
);

/// The unfinished jobs a person has already asked for with these paths in this swimlane. Read through the index on the unfinished
/// states.
pub(crate) const ACTIVE_SQL: &str = "SELECT id FROM pr_jobs WHERE state NOT IN ('completed', 'failed') \
     AND author_tid = $1 AND author_oid = $2 AND swimlane_id = $3 AND paths = $4::text[] ORDER BY state, id LIMIT 1";

/// The jobs that need a runner: `pending` ones at once, the others when nobody has touched them since `$1` (a runner that is
/// working on a job moves it, or renews its lease, well inside that time). `$2` is the most rows. Read through the index on the
/// unfinished states, in the order of that index (`state`, `id`): the finished jobs, which are nearly all of the table, are not in it.
pub(crate) const RESUME_SQL: &str = concat!(
    "SELECT id FROM pr_jobs WHERE state NOT IN ('completed', 'failed') AND (state = 'pending' OR updated_at < ",
    ts!("1"),
    ") ORDER BY state, id LIMIT $2"
);

const SET_SQL: &str = concat!(
    "UPDATE pr_jobs SET state = $3, step_data = $4, failure = $5, updated_at = ",
    ts!("6"),
    " WHERE id = $1 AND state = $2"
);

pub(crate) const BUMP_SQL: &str = concat!(
    "UPDATE pr_jobs SET attempts = attempts + 1, updated_at = ",
    ts!("3"),
    " WHERE id = $1 AND state = $2 RETURNING attempts"
);

/// Keep the blobs of a plan for as long as the job's pull request is kept (D78).
pub(crate) const BLOB_REFS_SQL: &str = "INSERT INTO blob_refs (hash, ref_kind, ref_id) \
     SELECT b.hash, 'pr_link', $2 FROM blobs b WHERE b.hash = ANY($1::bytea[]) ON CONFLICT DO NOTHING";

fn user_of(tid: &str, oid: &str) -> Option<UserId> {
    Some(UserId::new(Guid::parse(tid).ok()?, Guid::parse(oid).ok()?))
}

fn read(row: &sqlx::postgres::PgRow) -> Option<JobRow> {
    let id: i64 = row.try_get("id").ok()?;
    let state: String = row.try_get("state").ok()?;
    let step: serde_json::Value = row.try_get("step_data").ok()?;
    let step: StepData = serde_json::from_value(step).ok()?;
    if step.v != STEP_VERSION {
        return None;
    }
    Some(JobRow {
        id: PrJobId::from(id),
        author: user_of(
            &row.try_get::<String, _>("author_tid").ok()?,
            &row.try_get::<String, _>("author_oid").ok()?,
        )?,
        swimlane: SwimlaneId::parse(&row.try_get::<String, _>("swimlane_id").ok()?).ok()?,
        paths: row.try_get("paths").ok()?,
        title: row.try_get("title").ok()?,
        body: row.try_get("body").ok()?,
        state: PrJobState::from_str(&state).ok()?,
        failure: row.try_get("failure").ok()?,
        attempts: row.try_get("attempts").ok()?,
        step,
        request_id: row.try_get("request_id").ok()?,
        created_ms: row.try_get("created_ms").ok()?,
        updated_ms: row.try_get("updated_ms").ok()?,
    })
}

pub(crate) async fn get(conn: &mut PgConnection, id: PrJobId) -> Res<Option<JobRow>> {
    let row = sqlx::query(GET_SQL)
        .bind(id.get())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_job_get", &e))?;
    row.map(|r| read(&r).ok_or_else(|| Internal::new("pr_job_get", "row")))
        .transpose()
}

pub(crate) async fn lock(conn: &mut PgConnection, id: PrJobId) -> Res<Option<JobRow>> {
    let row = sqlx::query(LOCK_SQL)
        .bind(id.get())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_job_lock", &e))?;
    row.map(|r| read(&r).ok_or_else(|| Internal::new("pr_job_lock", "row")))
        .transpose()
}

/// A job to make.
pub(crate) struct NewJob<'a> {
    pub author: &'a UserId,
    pub swimlane: &'a SwimlaneId,
    /// Sorted, so that two requests for the same files are the same request.
    pub paths: Vec<String>,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub step: &'a StepData,
    pub request_id: &'a RequestId,
    pub now_ms: i64,
}

pub(crate) async fn insert(conn: &mut PgConnection, j: &NewJob<'_>) -> Res<PrJobId> {
    let step = serde_json::to_value(j.step).map_err(|_| Internal::new("pr_job_insert", "encode"))?;
    let id: i64 = sqlx::query_scalar(INSERT_SQL)
        .bind(j.author.tid().as_str())
        .bind(j.author.oid().as_str())
        .bind(j.swimlane.as_str())
        .bind(&j.paths)
        .bind(j.title)
        .bind(j.body)
        .bind(step)
        .bind(j.request_id.as_str())
        .bind(j.now_ms)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_job_insert", &e))?;
    Ok(PrJobId::from(id))
}

/// The unfinished job this person already asked for with exactly these paths, if there is one.
pub(crate) async fn active_for(
    conn: &mut PgConnection,
    author: &UserId,
    swimlane: &SwimlaneId,
    paths: &[String],
) -> Res<Option<PrJobId>> {
    let id: Option<i64> = sqlx::query_scalar(ACTIVE_SQL)
        .bind(author.tid().as_str())
        .bind(author.oid().as_str())
        .bind(swimlane.as_str())
        .bind(paths)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_job_active", &e))?;
    Ok(id.map(PrJobId::from))
}

/// The jobs that need a runner, oldest first, at most `limit`.
pub(crate) async fn needing_a_runner(
    conn: &mut PgConnection,
    untouched_since_ms: i64,
    limit: i64,
) -> Res<Vec<PrJobId>> {
    let ids: Vec<i64> = sqlx::query_scalar(RESUME_SQL)
        .bind(untouched_since_ms)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_job_resume", &e))?;
    Ok(ids.into_iter().map(PrJobId::from).collect())
}

/// Move a job from `from` to `to` with what the step produced, and publish the progress, in the caller's transaction. Returns
/// whether it moved: false means the job was not in `from` any more (another runner moved it), and nothing was published.
pub(crate) async fn advance_in_tx(
    hub: &Inner,
    tx: &mut Transaction<'_, Postgres>,
    id: PrJobId,
    from: PrJobState,
    to: PrJobState,
    step: &StepData,
    failure: Option<&str>,
) -> Res<bool> {
    let data = serde_json::to_value(step).map_err(|_| Internal::new("pr_job_set", "encode"))?;
    let done = sqlx::query(SET_SQL)
        .bind(id.get())
        .bind(from.as_str())
        .bind(to.as_str())
        .bind(data)
        .bind(failure)
        .bind(hub.now_ms())
        .execute(&mut **tx)
        .await
        .map_err(|e| Internal::db("pr_job_set", &e))?;
    if done.rows_affected() != 1 {
        return Ok(false);
    }
    if from != to {
        publish_progress(hub, tx, id, to).await?;
    }
    Ok(true)
}

/// Count one more try of the step the job is in. Returns how many tries it has had, or `None` if it moved on.
pub(crate) async fn bump(hub: &Inner, id: PrJobId, state: PrJobState) -> Res<Option<i32>> {
    let mut conn = hub
        .pool
        .acquire()
        .await
        .map_err(|e| Internal::db("pr_job_bump", &e))?;
    sqlx::query_scalar(BUMP_SQL)
        .bind(id.get())
        .bind(state.as_str())
        .bind(hub.now_ms())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_job_bump", &e))
}

pub(crate) async fn publish_progress(
    hub: &Inner,
    tx: &mut Transaction<'_, Postgres>,
    id: PrJobId,
    state: PrJobState,
) -> Res<()> {
    hub.bus
        .publish_in_tx(
            &mut Tx::from_pg(&mut *tx),
            DomainEvent::PrJobProgress { job_id: id, state },
        )
        .await
        .map_err(|_| Internal::new("pr_job", "event outbox"))
}

/// Keep the plan's blobs as long as the job's pull request is kept.
pub(crate) async fn keep_blobs(
    tx: &mut Transaction<'_, Postgres>,
    id: PrJobId,
    hashes: &[ContentHash],
) -> Res<()> {
    let hashes: Vec<Vec<u8>> = hashes.iter().map(|h| h.as_bytes().to_vec()).collect();
    sqlx::query(BLOB_REFS_SQL)
        .bind(&hashes)
        .bind(format!("job:{}", id.get()))
        .execute(&mut **tx)
        .await
        .map_err(|e| Internal::db("pr_job_blob_refs", &e))?;
    Ok(())
}

/// The statements by name, for the tests that run `EXPLAIN` on them (P12).
pub(crate) fn queries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("pr_job_get", GET_SQL),
        ("pr_job_lock", LOCK_SQL),
        ("pr_job_insert", INSERT_SQL),
        ("pr_job_active", ACTIVE_SQL),
        ("pr_job_resume", RESUME_SQL),
    ]
}

#[cfg(test)]
mod tests {
    use domain::{NfsPath, RepoPath};

    use super::*;
    use crate::prs::target::Side;

    fn sha(c: char) -> GitSha {
        GitSha::parse(&c.to_string().repeat(40)).unwrap()
    }

    fn row(step: StepData) -> JobRow {
        JobRow {
            id: PrJobId::from(7),
            author: ports::conformance::sample::user_id(1),
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            paths: vec!["a.yml".to_owned(), "b.yml".to_owned()],
            title: "Sync".to_owned(),
            body: Some("Please review.".to_owned()),
            state: PrJobState::Opening,
            failure: None,
            attempts: 0,
            step,
            request_id: Some("w-0123".to_owned()),
            created_ms: 1_000,
            updated_ms: 2_000,
        }
    }

    fn plan() -> PrPlan {
        PrPlan {
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            dest: Destination {
                side: Side::Tenant,
                branch: BranchName::parse("sit1").unwrap(),
            },
            repo: RepoRef::parse("acme/csp-tenant-data").unwrap(),
            head: sha('a'),
            tree: sha('b'),
            branch: BranchName::parse("lanekeeper/sit1/0123456789").unwrap(),
            title: ShortText::parse("Sync").unwrap(),
            text: Some("Please review.".to_owned()),
            files: vec![
                PlannedFile {
                    nfs: NfsPath::parse("a.yml").unwrap(),
                    nfs_hash: ports::content_hash(b"nfs a"),
                    repo_path: RepoPath::parse("data/config/a.yml").unwrap(),
                    content: Bytes::from_static(b"a: 1\n"),
                    exists: true,
                },
                PlannedFile {
                    nfs: NfsPath::parse("b.yml").unwrap(),
                    nfs_hash: ports::content_hash(b"nfs b"),
                    repo_path: RepoPath::parse("data/config/b.yml").unwrap(),
                    content: Bytes::from_static(b"b: 1\n"),
                    exists: false,
                },
            ],
            unchanged: vec![NfsPath::parse("c.yml").unwrap()],
            warnings: vec![ShortText::parse("b.yml is new on branch sit1").unwrap()],
            user: ports::conformance::sample::user_id(1),
            request_id: RequestId::parse("w-0123").unwrap(),
            job: Some(PrJobId::from(7)),
        }
    }

    #[test]
    fn a_plan_survives_the_row_and_keeps_its_content_in_blobs_only() {
        let original = plan();
        let blobs: Vec<ContentHash> = original
            .files
            .iter()
            .map(|f| ports::content_hash(&f.content))
            .collect();
        let stored = StoredPlan::of(&original, &blobs);
        assert_eq!(stored.blob_hashes(), blobs);
        let json = serde_json::to_string(&stored).unwrap();
        assert!(
            !json.contains("a: 1") && !json.contains("Please review"),
            "no content in the row: {json}"
        );

        let step = StepData {
            plan: Some(serde_json::from_str(&json).unwrap()),
            ..StepData::new(Via::Mcp, Some(PrTarget::Base), Some(9))
        };
        let job = row(step);
        let again = job
            .step
            .plan
            .clone()
            .unwrap()
            .into_plan(
                &job,
                Some(original.files.iter().map(|f| f.content.clone()).collect()),
            )
            .unwrap();
        assert_eq!(again.files, original.files);
        assert_eq!(again.branch, original.branch);
        assert_eq!(
            (again.head, again.tree),
            (original.head.clone(), original.tree.clone())
        );
        assert_eq!(again.dest, original.dest);
        assert_eq!(again.repo, original.repo);
        assert_eq!(again.unchanged, original.unchanged);
        assert_eq!(again.warnings, original.warnings);
        assert_eq!(again.text, original.text);
        assert_eq!(again.title, original.title);
        assert_eq!(again.job, Some(PrJobId::from(7)));
        assert_eq!(again.request_id, original.request_id);

        // A step that only names files gets empty content; content of the wrong number of files is refused.
        let bare = job.step.plan.clone().unwrap().into_plan(&job, None).unwrap();
        assert!(bare.files.iter().all(|f| f.content.is_empty()));
        assert!(
            job.step
                .plan
                .clone()
                .unwrap()
                .into_plan(&job, Some(vec![Bytes::new()]))
                .is_none()
        );
    }

    #[test]
    fn the_view_of_a_job_shows_the_address_the_warnings_and_the_sentence_and_nothing_else() {
        let mut step = StepData::new(Via::Ui, None, None);
        let original = plan();
        step.plan = Some(StoredPlan::of(
            &original,
            &[ports::content_hash(b"1"), ports::content_hash(b"2")],
        ));
        let mut job = row(step);
        let view = job.view().unwrap();
        assert_eq!((view.id, view.state), (7, PrJobState::Opening));
        assert_eq!(view.pr_url, None);
        assert_eq!(view.warnings.len(), 1);
        job.step.opened = Some(Opened {
            number: 3,
            url: "https://github.com/acme/csp-tenant-data/pull/3".to_owned(),
        });
        job.state = PrJobState::Failed;
        job.failure = Some("GitHub refused the change".to_owned());
        let view = job.view().unwrap();
        assert_eq!(
            view.pr_url.as_deref(),
            Some("https://github.com/acme/csp-tenant-data/pull/3")
        );
        assert_eq!(
            view.error.as_ref().map(ShortText::as_str),
            Some("GitHub refused the change")
        );
        let json = serde_json::to_value(&view).unwrap();
        let keys: std::collections::BTreeSet<&str> =
            json.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "id",
                "swimlane",
                "state",
                "paths",
                "pr_url",
                "error",
                "warnings",
                "created_at",
                "updated_at"
            ])
        );
        // No warnings: the key is absent (the contract: "absent when there is nothing to say").
        job.step.plan = None;
        let json = serde_json::to_value(job.view().unwrap()).unwrap();
        assert!(json.get("warnings").is_none());
        assert!(job.is_finished());
    }

    #[test]
    fn a_step_record_of_another_version_or_with_unknown_parts_does_not_read_as_current() {
        let step = StepData::new(Via::Ui, None, None);
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["v"], STEP_VERSION);
        // Everything but the version and the door is absent until a step produces it.
        let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 2, "{keys:?}");
        // A record from before the fields existed reads with them empty.
        let old: StepData = serde_json::from_str(r#"{"v":1}"#).unwrap();
        assert!(old.plan.is_none() && old.committed.is_none() && old.opened.is_none() && old.audit.is_none());
        // `read` refuses any version but this build's; the row type is checked in the database tests.
        assert_ne!(StepData::default().v, STEP_VERSION);
    }
}
