//! Pull requests for NFS-only changes (T6, D41, D43, D77): reverse-map the NFS paths, plan what goes where, commit it with the Git
//! Data API and open the pull request, all with the raising user's own GitHub token.
//!
//! The work is four steps, each its own method so that the job that runs them (T9) can persist its progress between them and
//! resume after a crash:
//!
//! 1. [`PrService::plan`]: authorise, route the paths ([`target`]), read the NFS files, read the destination branch and the
//!    repository files at its head, and put each file into the form of the repository file ([`plan`]). Nothing is written.
//! 2. [`PrService::commit`]: blobs, a tree and a commit on top of the head the plan was made on, and the new branch
//!    `lanekeeper/<swimlane>/<short id>`. The destination branch is not touched.
//! 3. [`PrService::open`]: the pull request, with the swimlane, the hashes and the user in its body.
//! 4. [`PrService::note_audit`]: the audit event of the pull request records its address, so it can only be written after step 3;
//!    the body is then replaced with the audit id added.
//!
//! [`PrService::create`] runs steps 1 to 3 in one request. A user who `requires_approval`, the link records, the job and the audit
//! event belong to the job (T9, [`runner`]): this service does what a pull request needs from GitHub and decides nothing about who
//! may raise one beyond the Editor floor of the token vault.
//!
//! **The job** ([`job`], [`runner`], [`submit`]): a request for a pull request is accepted as a persisted job, answered with its id,
//! and run one durable step at a time (`pending`, `preparing`, `committing`, `opening`, `completed` or `failed`), so that a crash
//! at any point resumes where the job stands. **The links** ([`links`]): each file of a pull request is linked to the swimlane,
//! the path and the NFS hash it came from; a second pull request for the same change is refused with the first one's link.
//!
//! Every GitHub call is made inside [`TokenVault::with_token`], so the token is decrypted for this user's request, handed to the
//! call and zeroized when the call ends (S8). Nothing in a plan, an error or a log holds a token or the content of a file.

pub mod job;
pub mod links;
pub mod plan;
pub mod runner;
pub mod submit;
pub mod target;
pub mod text;

use std::sync::Arc;

use tracing::Instrument;

use domain::{
    AuditId, ContentHash, ContentSource, MergeMode, NfsPath, PrRequest, RejectReason, Role, SwimlaneId, User,
    ValidationIssue, WriteCtx,
};
use engine::{file_role, merge_mode};
use ports::{ReadError, content_hash};

pub use job::PrJobView;
pub use links::{LinkQuery, PrLinkView, PrStateWatcher, Watched};
pub use plan::{MAX_WARNINGS, PlannedFile, PrPlan};
pub use runner::{Advanced, Driven, PrJobRunner, RunReport};
pub use submit::{PrDesk, Raised};
pub use target::{Destination, Routed, Side, TargetError, route_all};
pub use text::{commit_message, pull_body, short_id};

use crate::github::{BranchName, GitDataApi, GitSha, GithubError, NewPull, RepoFile, RepoRef, TreeEntry};
use crate::pipeline::authorize::authorize;
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::preview::effect::facts;
use crate::service::{Inner, WriteHub};
use crate::vault::{PolicyRepos, TokenError, TokenFuture, TokenVault};

/// Why a pull request step did not happen. A fixed word each: nothing of a path's content, of GitHub's answer or of the token. The
/// files of a request are named by their position in it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrError {
    /// The user may not raise a pull request (not active, or below Editor).
    #[error("not allowed")]
    Forbidden,
    #[error("the swimlane does not exist")]
    UnknownSwimlane,
    /// The paths cannot become one pull request.
    #[error("the request cannot be turned into one pull request")]
    Target(#[from] TargetError),
    /// A file cannot go into a pull request: denied, too large, not valid text, or it holds a secret (`issues` has the lines).
    #[error("a file cannot go into a pull request")]
    Rejected {
        index: usize,
        reason: RejectReason,
        issues: Vec<ValidationIssue>,
    },
    /// The file is not on NFS (a deleted file is not carried into a pull request).
    #[error("a file is not on NFS")]
    NotOnNfs { index: usize },
    /// The repository has something at the path that is not a regular file.
    #[error("the repository has no regular file there")]
    RepoFileUnreadable { index: usize },
    /// Every file already holds this content on the destination branch.
    #[error("there is nothing to change")]
    NothingToChange,
    /// The destination branch does not exist.
    #[error("the destination branch does not exist")]
    TargetBranchMissing,
    /// The pull request's branch exists and holds something else.
    #[error("the pull request branch is taken")]
    BranchTaken,
    /// The user has no usable GitHub token (none stored, expired, or it does not open).
    #[error("no usable github token")]
    Token(TokenError),
    /// GitHub did not do what was asked, after the bounded retries.
    #[error("github did not do what was asked")]
    Github(GithubError),
    /// The store, the registry or the user directory did not answer.
    #[error("the pull request service is unavailable")]
    Unavailable,
}

fn token_error(e: TokenError) -> PrError {
    match e {
        TokenError::Forbidden => PrError::Forbidden,
        TokenError::Unavailable => PrError::Unavailable,
        other => PrError::Token(other),
    }
}

/// What the pull-request path needs to know besides the write hub.
#[derive(Debug, Clone)]
pub struct PrSettings {
    /// The two repositories (`owner/name`).
    pub repos: PolicyRepos,
    /// The default branch of the base repository: where a base-targeted pull request goes (D43).
    pub base_branch: BranchName,
    /// The most bytes of NFS content one pull request takes in all, so that 100 large files cannot fill the hub's memory (rule 5).
    pub max_total_bytes: usize,
}

impl PrSettings {
    /// 32 MiB: sixteen files at the size limit, or far more files than a pull request of configuration has.
    pub const DEFAULT_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
}

/// What a [`PrService`] is built from.
#[derive(Clone)]
pub struct PrDeps {
    pub vault: TokenVault,
    pub git: Arc<dyn GitDataApi>,
    pub settings: PrSettings,
}

impl std::fmt::Debug for PrDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrDeps").finish_non_exhaustive()
    }
}

/// The branch Lanekeeper made for a pull request, and the commit on it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Committed {
    pub branch: BranchName,
    pub commit: GitSha,
}

/// The pull request GitHub made.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Opened {
    pub number: u32,
    pub url: String,
}

/// A pull request made by [`PrService::create`].
#[derive(Debug, Clone)]
pub struct Created {
    pub plan: PrPlan,
    pub committed: Committed,
    pub opened: Opened,
}

/// The pull-request service.
#[derive(Clone)]
pub struct PrService {
    hub: Arc<Inner>,
    vault: TokenVault,
    git: Arc<dyn GitDataApi>,
    settings: Arc<PrSettings>,
}

impl std::fmt::Debug for PrService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrService").finish_non_exhaustive()
    }
}

impl WriteHub {
    /// The pull-request service over this hub's store, registry and clock.
    #[must_use]
    pub fn pull_requests(&self, deps: PrDeps) -> PrService {
        PrService {
            hub: Arc::clone(&self.inner),
            vault: deps.vault,
            git: deps.git,
            settings: Arc::new(deps.settings),
        }
    }
}

/// What a request is checked against before it becomes a job.
pub(crate) struct Preflight {
    /// The user, as the directory has them now.
    pub user: User,
    /// The hash of each requested path's NFS file, in request order.
    pub hashes: Vec<ContentHash>,
}

/// What the destination repository holds, as of one commit.
struct RepoState {
    head: GitSha,
    tree: GitSha,
    files: Vec<RepoFile>,
}

impl PrService {
    fn repo_of(&self, side: Side) -> &RepoRef {
        match side {
            Side::Base => &self.settings.repos.base,
            Side::Tenant => &self.settings.repos.tenant,
        }
    }

    /// The NFS bytes of a path as the registry has them.
    async fn read_nfs(
        &self,
        user: &User,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        index: usize,
    ) -> Result<(ContentHash, bytes::Bytes), PrError> {
        let hub = &*self.hub;
        let content = match hub
            .within(
                "pr_nfs",
                hub.registry.content(user, swimlane, path, ContentSource::Nfs),
            )
            .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(ReadError::NotFound)) => return Err(PrError::NotOnNfs { index }),
            Ok(Err(ReadError::Forbidden)) => return Err(PrError::Forbidden),
            Ok(Err(_)) | Err(_) => return Err(PrError::Unavailable),
        };
        let denied = |reason| PrError::Rejected {
            index,
            reason,
            issues: Vec::new(),
        };
        if content.withheld {
            return Err(denied(RejectReason::Denied));
        }
        let Some(bytes) = content.bytes else {
            return Err(PrError::Unavailable);
        };
        if bytes.len() > hub.cfg.max_content_bytes {
            return Err(denied(RejectReason::TooLarge));
        }
        // What the registry says it hashed is what it gave: the hash goes into the pull request's record.
        if content_hash(&bytes) != content.hash {
            tracing::warn!("the registry gave content that is not the content it hashed");
            return Err(PrError::Unavailable);
        }
        Ok((content.hash, bytes))
    }

    /// Who may ask, which swimlane, and where the paths go: everything that needs no outside call.
    async fn route(
        &self,
        ctx: &WriteCtx,
        r: &PrRequest,
    ) -> Result<(User, crate::preview::effect::Facts, Vec<Routed>), PrError> {
        let hub = &*self.hub;
        let user = authorize(hub, ctx, Role::Editor).await.map_err(|e| match e {
            ports::WriteError::Forbidden => PrError::Forbidden,
            _ => PrError::Unavailable,
        })?;
        // The editing lock does not apply: a pull request changes nothing on NFS. A swimlane that does not exist is refused.
        match check_lock(hub, &r.swimlane).await {
            Ok(LockState::UnknownSwimlane) => return Err(PrError::UnknownSwimlane),
            Ok(_) => {}
            Err(_) => return Err(PrError::Unavailable),
        }
        let facts = facts(hub, &r.swimlane).await.map_err(|_| PrError::Unavailable)?;
        let routed = route_all(
            &facts.rule,
            &facts.tenants,
            &self.settings.base_branch,
            &r.paths,
            r.target.as_ref(),
        )?;
        for (index, row) in routed.iter().enumerate() {
            if hub.deny.is_denied(&row.nfs) {
                return Err(PrError::Rejected {
                    index,
                    reason: RejectReason::Denied,
                    issues: Vec::new(),
                });
            }
        }
        Ok((user, facts, routed))
    }

    /// The destination branch's head and tree, and each repository file as of that head, read with the user's own token. The
    /// closure of `with_token` owns what it uses: the future it returns may borrow only the token.
    async fn read_repo(
        &self,
        ctx: &WriteCtx,
        repo: RepoRef,
        branch: BranchName,
        repo_paths: Vec<domain::RepoPath>,
    ) -> Result<RepoState, PrError> {
        let git = Arc::clone(&self.git);
        self.vault
            .with_token(ctx, move |token| -> TokenFuture<'_, Result<RepoState, PrError>> {
                Box::pin(async move {
                    let head = git
                        .branch_head(token, &repo, &branch)
                        .await
                        .map_err(PrError::Github)?
                        .ok_or(PrError::TargetBranchMissing)?;
                    let tree = git
                        .commit_tree(token, &repo, &head)
                        .await
                        .map_err(PrError::Github)?;
                    let mut files = Vec::with_capacity(repo_paths.len());
                    for (index, path) in repo_paths.iter().enumerate() {
                        files.push(
                            git.file_at(token, &repo, &head, path)
                                .await
                                .map_err(|e| match e {
                                    GithubError::Unexpected => PrError::RepoFileUnreadable { index },
                                    other => PrError::Github(other),
                                })?,
                        );
                    }
                    Ok(RepoState { head, tree, files })
                })
            })
            .await
            .map_err(token_error)?
    }

    /// Each file in the form of its repository file, checked, on the blocking pool (rule 4). The token is gone by now: this is CPU
    /// work on bytes already in hand.
    async fn prepare(
        &self,
        routed: &[Routed],
        nfs: &[(ContentHash, bytes::Bytes)],
        files: &[RepoFile],
    ) -> Result<Vec<plan::Prepared>, PrError> {
        let hub = &*self.hub;
        let work: Vec<_> = routed
            .iter()
            .zip(nfs)
            .zip(files)
            .map(|((row, (_, bytes)), file)| (row.nfs.clone(), bytes.clone(), file.clone()))
            .collect();
        let (deny, limits, max_bytes) = (hub.deny.clone(), hub.limits.clone(), hub.cfg.max_content_bytes);
        hub.blocking("pr_prepare", move || {
            work.iter()
                .enumerate()
                .map(|(index, (path, bytes, file))| {
                    plan::prepare_file(path, bytes, file, &deny, &limits, max_bytes).map_err(|e| {
                        PrError::Rejected {
                            index,
                            reason: e.reason,
                            issues: e.issues,
                        }
                    })
                })
                .collect::<Result<Vec<_>, PrError>>()
        })
        .await
        .map_err(|_| PrError::Unavailable)?
    }

    /// What a request needs before a job can be made for it: who may ask, where the paths go, and the hash of each NFS file (the
    /// change the pull request is linked to, D77). No outside call is made.
    ///
    /// # Errors
    /// [`PrError`]; nothing was written anywhere.
    pub(crate) async fn preflight(&self, ctx: &WriteCtx, r: &PrRequest) -> Result<Preflight, PrError> {
        let (user, _facts, routed) = self.route(ctx, r).await?;
        let mut hashes = Vec::with_capacity(routed.len());
        let mut total = 0_usize;
        for (index, row) in routed.iter().enumerate() {
            let (hash, bytes) = self.read_nfs(&user, &r.swimlane, &row.nfs, index).await?;
            total = total.saturating_add(bytes.len());
            if total > self.settings.max_total_bytes {
                return Err(PrError::Rejected {
                    index,
                    reason: RejectReason::TooLarge,
                    issues: Vec::new(),
                });
            }
            hashes.push(hash);
        }
        Ok(Preflight { user, hashes })
    }

    /// Step 1: what the pull request will contain.
    ///
    /// # Errors
    /// [`PrError`]; nothing was written anywhere.
    pub async fn plan(&self, ctx: &WriteCtx, r: &PrRequest) -> Result<PrPlan, PrError> {
        let span = tracing::info_span!(
            "pr.plan",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
        );
        self.plan_inner(ctx, r).instrument(span).await
    }

    async fn plan_inner(&self, ctx: &WriteCtx, r: &PrRequest) -> Result<PrPlan, PrError> {
        let (user, facts, routed) = self.route(ctx, r).await?;
        let mut nfs: Vec<(ContentHash, bytes::Bytes)> = Vec::with_capacity(routed.len());
        let mut total = 0_usize;
        for (index, row) in routed.iter().enumerate() {
            let read = self.read_nfs(&user, &r.swimlane, &row.nfs, index).await?;
            total = total.saturating_add(read.1.len());
            if total > self.settings.max_total_bytes {
                return Err(PrError::Rejected {
                    index,
                    reason: RejectReason::TooLarge,
                    issues: Vec::new(),
                });
            }
            nfs.push(read);
        }
        let dest = routed[0].dest.clone();
        let repo = self.repo_of(dest.side).clone();
        let repo_paths = routed.iter().map(|x| x.repo_path.clone()).collect();
        let state = self
            .read_repo(ctx, repo.clone(), dest.branch.clone(), repo_paths)
            .await?;
        let prepared = self.prepare(&routed, &nfs, &state.files).await?;

        let mut files = Vec::new();
        let mut unchanged = Vec::new();
        let mut unchanged_repo = Vec::new();
        let mut rows = Vec::new();
        for (((row, (nfs_hash, _)), repo_file), prep) in
            routed.iter().zip(&nfs).zip(&state.files).zip(prepared)
        {
            let exists = matches!(repo_file, RepoFile::File { .. });
            if prep.unchanged {
                unchanged.push(row.nfs.clone());
                unchanged_repo.push(row.repo_path.clone());
                continue;
            }
            let whole_file =
                merge_mode(file_role(&row.nfs, &facts.tenants, &facts.apps)) == MergeMode::WholeFile;
            rows.push(plan::Row {
                routed: row,
                exists,
                whole_file,
            });
            files.push(PlannedFile {
                nfs: row.nfs.clone(),
                nfs_hash: *nfs_hash,
                repo_path: row.repo_path.clone(),
                content: prep.content,
                exists,
            });
        }
        if files.is_empty() {
            return Err(PrError::NothingToChange);
        }
        let warnings = plan::warnings(&dest, &repo, &rows, &unchanged_repo);

        let mut made = PrPlan {
            swimlane: r.swimlane.clone(),
            dest,
            repo,
            head: state.head,
            tree: state.tree,
            // Placeholder until the files are known; the short id follows the request and the files.
            branch: self.settings.base_branch.clone(),
            title: r.title.clone(),
            text: r.body.clone(),
            files,
            unchanged,
            warnings,
            user: ctx.user.id.clone(),
            request_id: ctx.request_id.clone(),
            job: None,
        };
        made.branch =
            BranchName::for_pull(&made.swimlane, &short_id(&made)).map_err(|_| PrError::Unavailable)?;
        Ok(made)
    }

    /// Step 2: the commit and the branch.
    ///
    /// A branch that exists already counts as this step's own earlier work when its commit holds the tree this step builds (a
    /// lost answer to the first try); anything else makes it [`PrError::BranchTaken`].
    ///
    /// # Errors
    /// [`PrError`]; blobs, trees and commits that were made stay in the repository, unreachable, and GitHub collects them.
    pub async fn commit(&self, ctx: &WriteCtx, plan: &PrPlan) -> Result<Committed, PrError> {
        let span = tracing::info_span!(
            "pr.commit",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %plan.swimlane,
        );
        self.commit_inner(ctx, plan).instrument(span).await
    }

    async fn commit_inner(&self, ctx: &WriteCtx, plan: &PrPlan) -> Result<Committed, PrError> {
        let (git, plan) = (Arc::clone(&self.git), plan.clone());
        self.vault
            .with_token(ctx, move |token| -> TokenFuture<'_, Result<Committed, PrError>> {
                Box::pin(async move {
                    let repo = &plan.repo;
                    let mut entries = Vec::with_capacity(plan.files.len());
                    for f in &plan.files {
                        let blob = git
                            .create_blob(token, repo, &f.content)
                            .await
                            .map_err(PrError::Github)?;
                        entries.push(TreeEntry {
                            path: f.repo_path.clone(),
                            blob,
                        });
                    }
                    let tree = git
                        .create_tree(token, repo, &plan.tree, &entries)
                        .await
                        .map_err(PrError::Github)?;
                    let commit = git
                        .create_commit(token, repo, &commit_message(&plan), &tree, &plan.head)
                        .await
                        .map_err(PrError::Github)?;
                    let committed = |commit| Committed {
                        branch: plan.branch.clone(),
                        commit,
                    };
                    match git.create_branch(token, repo, &plan.branch, &commit).await {
                        Ok(()) => Ok(committed(commit)),
                        Err(GithubError::Unprocessable) => {
                            let existing = git
                                .branch_head(token, repo, &plan.branch)
                                .await
                                .map_err(PrError::Github)?
                                .ok_or(PrError::BranchTaken)?;
                            let theirs = git
                                .commit_tree(token, repo, &existing)
                                .await
                                .map_err(PrError::Github)?;
                            if theirs == tree {
                                Ok(committed(existing))
                            } else {
                                Err(PrError::BranchTaken)
                            }
                        }
                        Err(e) => Err(PrError::Github(e)),
                    }
                })
            })
            .await
            .map_err(token_error)?
    }

    /// Step 3: the pull request. One that is open for the branch already is the answer of an earlier try.
    ///
    /// # Errors
    /// [`PrError`].
    pub async fn open(
        &self,
        ctx: &WriteCtx,
        plan: &PrPlan,
        committed: &Committed,
    ) -> Result<Opened, PrError> {
        let body = pull_body(plan, None);
        let git = Arc::clone(&self.git);
        let (repo, title, base, head) = (
            plan.repo.clone(),
            plan.title.clone(),
            plan.dest.branch.clone(),
            committed.branch.clone(),
        );
        self.vault
            .with_token(ctx, move |token| -> TokenFuture<'_, Result<Opened, PrError>> {
                Box::pin(async move {
                    let opened = |p: crate::github::PullInfo| Opened {
                        number: p.number,
                        url: p.url,
                    };
                    let pull = NewPull {
                        title: title.as_str(),
                        body: &body,
                        head: &head,
                        base: &base,
                    };
                    match git.open_pull(token, &repo, &pull).await {
                        Ok(p) => Ok(opened(p)),
                        Err(GithubError::Unprocessable) => git
                            .find_pull(token, &repo, &head)
                            .await
                            .map_err(PrError::Github)?
                            .map(opened)
                            .ok_or(PrError::Github(GithubError::Unprocessable)),
                        Err(e) => Err(PrError::Github(e)),
                    }
                })
            })
            .await
            .map_err(token_error)?
    }

    /// Step 4: put the audit event's id in the pull request's body.
    ///
    /// # Errors
    /// [`PrError`]. The pull request exists whatever this returns; a caller records the failure and goes on.
    pub async fn note_audit(
        &self,
        ctx: &WriteCtx,
        plan: &PrPlan,
        opened: &Opened,
        audit: AuditId,
    ) -> Result<(), PrError> {
        let body = pull_body(plan, Some(audit));
        let (git, repo, number) = (Arc::clone(&self.git), plan.repo.clone(), opened.number);
        self.vault
            .with_token(ctx, move |token| -> TokenFuture<'_, Result<(), PrError>> {
                Box::pin(async move {
                    git.update_pull_body(token, &repo, number, &body)
                        .await
                        .map_err(PrError::Github)
                })
            })
            .await
            .map_err(token_error)?
    }

    /// Steps 1 to 3 in this request.
    ///
    /// # Errors
    /// [`PrError`] from the step that failed.
    pub async fn create(&self, ctx: &WriteCtx, r: &PrRequest) -> Result<Created, PrError> {
        let plan = self.plan(ctx, r).await?;
        let committed = self.commit(ctx, &plan).await?;
        let opened = self.open(ctx, &plan, &committed).await?;
        Ok(Created {
            plan,
            committed,
            opened,
        })
    }
}
