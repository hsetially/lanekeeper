//! The adoption pass (T5, D2, D19, D21, D22, S4, S10, S13): an admin confirms which tenant branches a swimlane was deployed from, which
//! base version, and what to make of every file that differs, and only then does the swimlane have a baseline and editing unlock.
//!
//! [`AdoptionService`] runs the wizard of `design/10-findings-adoption.md` as typed calls. Every call takes the calling [`User`] and
//! requires an **active Admin** first, before it reads or writes anything, and answers [`ReadError::Forbidden`] otherwise (inside
//! [`AdoptionError::Read`]): the check is on the service, so a route, an MCP tool or a test that calls it without a router gets it too
//! (S4). The marks of an intentional divergence ([`AdoptionService::mark_intentional`] and `clear_intentional`) need an **Operator**.
//!
//! | Step | Call | What it does |
//! |---|---|---|
//! | 1. branch | [`start`](AdoptionService::start) | Opens the pass (or resumes the open one) and ranks every deployable branch against the tenant files on NFS (`engine::adoption_rank` on rayon over the tree indexes, no Git read), with the hints from the Deployments' `SPRING_PROFILES_ACTIVE` and the Helm release names. |
//! | | [`choose_branches`](AdoptionService::choose_branches) | The branches the swimlane was deployed from (a set, Q34); their heads are fixed here. Proposes the best-matching base commit. |
//! | 2. base version | [`choose_base_commit`](AdoptionService::choose_base_commit) | Accepts the proposed base commit or names another. |
//! | 3. files | [`differing_files`](AdoptionService::differing_files), [`resolve`](AdoptionService::resolve), [`resolve_all`](AdoptionService::resolve_all) | The files whose NFS content differs from Git at the chosen commits, each to be resolved: "NFS is right", "Git is right" or "Intentional (reason)", one by one or in bulk. |
//! | 4. secrets | [`scan_secrets`](AdoptionService::scan_secrets) | The value-based secret scan of the base head, the chosen tenant heads and the NFS content. Only file, line, length and rule are stored (S13). |
//! | 5. confirm | [`confirm`](AdoptionService::confirm) | Baselines, marks, the tenant set, editing unlocked: one transaction. |
//!
//! # The same rows as the tenant-suffix review
//!
//! `pipeline::tenant_rematch` (T4) writes an `adoptions` row with the reason `tenant_suffix_changed` and the status `review` when the
//! suffixes on NFS stop agreeing with the tenant set. This service works exactly such rows, with the same steps and the same calls:
//! `start` with that reason takes the open review (`review` becomes `in_progress`, and the registry no longer refreshes it under the
//! admin) and a confirmation sets the baselines and the tenant set of the chosen branches. What differs is what a confirmation
//! leaves alone: the swimlane is adopted already, so `editing_unlocked` and `baseline_confirmed_*` stay as they are. At most one pass
//! of each reason is open for a swimlane (the table's unique index), and `start` with an open one resumes it.
//!
//! # Persistence
//!
//! Progress lives in `adoptions` and `adoption_resolutions` (D22): the step, the ranking, the hints, the chosen branches and commits,
//! the base commit, the secret scan and every resolution. A new service over the same database (a restart, another replica) resumes at
//! the stored step with everything the admin had done. A resolution is bound to the hashes the admin looked at: a file that has changed
//! since is *stale* and has to be resolved again, and a confirmation refuses to go ahead on a stale one.
//!
//! # What a resolution means
//!
//! The baseline is what Git had at the last sync (D19). Where NFS differs from Git at the chosen commits, the admin says which side the
//! difference came from, and that decides the baseline B:
//!
//! | Resolution | B | State after the pipeline recomputes |
//! |---|---|---|
//! | NFS is right | Git's content (G) | `nfs_ahead`: NFS carries a change that Git does not have |
//! | Git is right | NFS's content (N) | `git_ahead`: Git has a change that NFS will get at the next sync |
//! | Intentional (reason) | G, and a mark with N and G | `intentional_divergence`, while both stay as they are (D20) |
//!
//! A file that equals Git (after line endings are normalised) gets B = N = G and is `in_sync`. A file that has no Git side at the
//! chosen commits (untracked, or of a tenant the swimlane does not deploy) and a file Git has that NFS lacks are not differing files
//! with a resolution: no baseline can be stored for "absent" (A24), so they stay as they are (`untracked`, `unknown`), and the count of
//! the second kind is part of the files' summary.
//!
//! # Audit (rule 8, D93)
//!
//! One event for each state change, in its transaction: opening a pass and cancelling one (`SettingsChanged`), and a confirmation, which
//! writes exactly **one `BaselineAdopted`** for the confirmation (counts and the paths of each kind in `diff`), one
//! `DriftMarkedIntentional` for each intentional resolution, and one `SettingsChanged` when the tenant set changes. The steps between
//! (a branch chosen, a resolution recorded, a scan stored) are the wizard's working data and are stored with who and when, not audited.
//! `DriftChanged` for the baselined paths and `FindingsChanged` go to the outbox in the confirmation's transaction (D80), and the
//! pipeline is told to recompute the swimlane after it commits.

mod base_commit;
mod confirm;
mod hints;
mod marks;
pub(crate) mod rank;
mod resolutions;
mod secrets;
mod steps;
mod store;

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::{CommitId, ContentHash, GitRef, NfsPath, Role, SwimlaneId, TenantId, User, VersionLabel};
use engine::{DenyMatcher, Limits, ScanConfig};
use ports::{AuditLog, BlobStore, EventBus, GitReader, ReadError};
use sqlx::PgPool;

pub use self::confirm::{ADOPT_BASELINES_SQL, ADOPT_MARKS_SQL, CURRENT_NFS_SQL, ConfirmOutcome};
pub use self::hints::{Hint, HintSource};
pub use self::resolutions::{
    Bulk, DifferingFile, DifferingFiles, NFS_FILES_SQL, Reason, Resolution, ResolutionKind, ResolutionView,
};
pub use self::secrets::{SecretHit, TRUNCATED_RULE, UNSCANNED_RULE};
pub use self::store::{AdoptionReason, AdoptionStatus, AdoptionStep};
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::error::RegistryError;
use crate::index::{GitIndex, GitIndexDeps};
use crate::metrics::{AdoptionAction, AdoptionOutcome, RegistryMetrics};
use crate::pipeline::PipelineHandle;

/// Why a call of the adoption service was refused or failed. Carries no input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdoptionError {
    /// The closed set of the read port: `Forbidden` for a caller that is not an active user with the role the call needs (S4),
    /// `NotFound` for a swimlane or adoption that does not exist, `BadRequest` for a parameter out of range, `Unavailable` for a
    /// capacity limit.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The swimlane has not had its first full scan yet, so there are no files to rank against.
    #[error("the swimlane has no inventory yet")]
    NoInventory,
    /// The first pass of a swimlane that an admin has confirmed already.
    #[error("the swimlane is already adopted")]
    AlreadyAdopted,
    /// A tenant-suffix review of a swimlane that no admin has confirmed yet.
    #[error("the swimlane is not adopted yet")]
    NotAdopted,
    /// The adoption does not exist or is closed (confirmed or cancelled), or there is no open one to work on.
    #[error("no such adoption is open")]
    NotOpen,
    /// The call belongs to a step that the pass has not reached.
    #[error("the pass has not reached that step")]
    WrongStep,
    /// A branch the mirror does not have, or one that is a template, the docs branch, or one no rule makes deployable.
    #[error("the branch is not deployable")]
    NotDeployable,
    /// A commit the mirror does not have.
    #[error("the mirror has no such commit")]
    UnknownCommit,
    /// A path that is not one of the files that differ.
    #[error("the path is not a file that differs")]
    NotDiffering,
    /// Files that differ and have no current resolution (none, or one made for another version of the file).
    #[error("{0} files have no current resolution")]
    Unresolved(usize),
    /// A file changed between the plan the admin reviewed and the moment of writing it.
    #[error("a file changed since it was reviewed")]
    Changed,
    /// An empty reason, a reason over 2,000 characters, or one with a NUL.
    #[error("the reason is not valid")]
    InvalidReason,
    /// The file is not in a state a divergence can be marked on (`in_sync`, `untracked`, or an intentional one already).
    #[error("the file is not diverged")]
    NotDivergent,
    #[error("the registry failed")]
    Registry(RegistryError),
}

impl From<RegistryError> for AdoptionError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::NotFound => Self::Read(ReadError::NotFound),
            RegistryError::Invalid | RegistryError::TooLarge => Self::Read(ReadError::BadRequest),
            RegistryError::Capacity => Self::Read(ReadError::Unavailable),
            RegistryError::Git(ports::GitError::NotFound | ports::GitError::InvalidRef) => {
                Self::UnknownCommit
            }
            other => Self::Registry(other),
        }
    }
}

impl AdoptionError {
    /// Whether the call was refused for who made it, for what it asked, or failed.
    fn outcome(&self) -> AdoptionOutcome {
        match self {
            Self::Read(ReadError::Forbidden) => AdoptionOutcome::Forbidden,
            Self::Read(ReadError::Unavailable) | Self::Registry(_) => AdoptionOutcome::Failed,
            _ => AdoptionOutcome::Rejected,
        }
    }
}

/// One ranked branch: how many (path, content) pairs it shares with the tenant files on NFS, over how many either side has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedBranch {
    pub branch: GitRef,
    pub tenant: TenantId,
    pub shared: u64,
    pub union: u64,
}

impl RankedBranch {
    /// For display only: the ranking never uses it.
    #[allow(clippy::cast_precision_loss)]
    pub fn ratio(&self) -> f64 {
        if self.union == 0 {
            0.0
        } else {
            self.shared as f64 / self.union as f64
        }
    }
}

/// A pass as the wizard shows it: what is stored, and nothing computed from Git.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptionView {
    pub id: i64,
    pub swimlane: SwimlaneId,
    pub reason: AdoptionReason,
    pub status: AdoptionStatus,
    /// The step to resume at.
    pub step: AdoptionStep,
    /// The deployable branches, best first (at most `rematch_ranking`).
    pub ranking: Vec<RankedBranch>,
    /// Environment and release hints that name a deployable branch.
    pub hints: Vec<Hint>,
    /// For a tenant-suffix review: the suffixes on NFS that the swimlane does not deploy.
    pub foreign_suffixes: Vec<TenantId>,
    /// The branches the admin chose, and the commit each was at when it was chosen.
    pub tenants: Vec<TenantId>,
    pub tenant_commits: BTreeMap<TenantId, CommitId>,
    /// The base commit that matches NFS best (or the one the admin named), and what to call it ("v2.3.1 + 4 commits").
    pub base_commit: Option<CommitId>,
    pub base_label: Option<VersionLabel>,
    /// Where a secret may be: file, line, length and rule. Never a value (S13).
    pub secret_scan: Vec<SecretHit>,
    /// Resolutions recorded so far (some may be stale, see [`DifferingFiles`]).
    pub resolutions: usize,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
}

/// What the adoption service needs from the rest of the hub.
pub struct AdoptionDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub git: Arc<dyn GitReader>,
    /// The content of the NFS files, for the secret scan.
    pub blobs: Arc<dyn BlobStore>,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    /// Told to recompute a swimlane after a confirmation and the paths of a mark. Without it the reconcile finds the change within
    /// `reconcile_interval`.
    pub pipeline: Option<PipelineHandle>,
}

impl std::fmt::Debug for AdoptionDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdoptionDeps").finish_non_exhaustive()
    }
}

pub(crate) struct Inner {
    pub db: Db,
    pub git: Arc<dyn GitReader>,
    pub git_index: GitIndex,
    pub blobs: Arc<dyn BlobStore>,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    pub pool: Arc<rayon::ThreadPool>,
    pub limits: Limits,
    pub scan: ScanConfig,
    pub deny: Arc<DenyMatcher>,
    pub pipeline: Option<PipelineHandle>,
}

/// The adoption service. Cloning shares it.
#[derive(Clone)]
pub struct AdoptionService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AdoptionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdoptionService").finish_non_exhaustive()
    }
}

/// An active Admin, or `Forbidden` (S4). Called first by every method of the pass, before any I/O.
fn require_admin(u: &User) -> Result<(), AdoptionError> {
    if u.allows(Role::Admin) {
        Ok(())
    } else {
        Err(AdoptionError::Read(ReadError::Forbidden))
    }
}

/// An active Operator or above, or `Forbidden` (S4): the marks of an intentional divergence.
fn require_operator(u: &User) -> Result<(), AdoptionError> {
    if u.allows(Role::Operator) {
        Ok(())
    } else {
        Err(AdoptionError::Read(ReadError::Forbidden))
    }
}

impl AdoptionService {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range.
    pub fn new(deps: AdoptionDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let git_index = GitIndex::new(GitIndexDeps {
            pool: deps.pool.clone(),
            git: Arc::clone(&deps.git),
            config: deps.config.clone(),
            metrics: deps.metrics.clone(),
        })?;
        let deny = Arc::new(
            DenyMatcher::with_globs(deps.config.extra_deny_globs.iter().map(String::as_str))
                .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?,
        );
        let threads = rayon::ThreadPoolBuilder::new()
            .num_threads(deps.config.pool_threads)
            .thread_name(|i| format!("registry-adoption-{i}"))
            .build()
            .map_err(|_| ConfigError::Invalid("pool_threads"))?;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                git: deps.git,
                git_index,
                blobs: deps.blobs,
                bus: deps.bus,
                audit: deps.audit,
                clock: deps.clock,
                config: deps.config,
                metrics: deps.metrics,
                pool: Arc::new(threads),
                limits: Limits::default(),
                scan: ScanConfig::default(),
                deny,
                pipeline: deps.pipeline,
            }),
        })
    }

    /// Open the pass of `swimlane` for `reason`, or resume the open one.
    ///
    /// - `Initial`: the swimlane must have its inventory and no confirmed baseline. A new pass ranks the deployable branches and
    ///   collects the hints; an open one is returned as it is.
    /// - `TenantSuffixChanged`: the swimlane must be adopted, and a review must be open (the pipeline raises it, T4). An open review
    ///   becomes `in_progress`: from then on the registry leaves it alone.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `Read(Forbidden)` for anyone but an active Admin, before anything is read.
    pub async fn start(
        &self,
        u: &User,
        swimlane: &SwimlaneId,
        reason: AdoptionReason,
    ) -> Result<AdoptionView, AdoptionError> {
        self.call(AdoptionAction::Start, require_admin(u), async {
            self.inner.start(u, swimlane, reason).await
        })
        .await
    }

    /// The open pass of `swimlane` for `reason`, as it is stored: how a wizard that was closed finds where it was. `None` when there
    /// is none.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `Read(Forbidden)` for anyone but an active Admin.
    pub async fn open(
        &self,
        u: &User,
        swimlane: &SwimlaneId,
        reason: AdoptionReason,
    ) -> Result<Option<AdoptionView>, AdoptionError> {
        require_admin(u)?;
        Ok(self.inner.open_view(swimlane, reason).await?)
    }

    /// A pass by its id, as it is stored.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `Read(Forbidden)` for anyone but an active Admin, `Read(NotFound)` for an id there is none of.
    pub async fn get(&self, u: &User, id: i64) -> Result<AdoptionView, AdoptionError> {
        require_admin(u)?;
        self.inner.view(id).await
    }

    /// Step 1: the branches the swimlane was deployed from. Each must be a deployable branch of the mirror; its head is fixed here as
    /// the commit the baselines are taken from. Proposes the base commit that matches NFS best, and moves on to the base version.
    /// Choosing again resets everything after it (the base commit, the resolutions, the scan).
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `NotOpen` for a pass that is not open, `NotDeployable` for a branch that is not a deployable one,
    /// `Read(BadRequest)` for none or more than 64.
    pub async fn choose_branches(
        &self,
        u: &User,
        id: i64,
        tenants: &[TenantId],
    ) -> Result<AdoptionView, AdoptionError> {
        self.call(AdoptionAction::ChooseBranches, require_admin(u), async {
            self.inner.choose_branches(id, tenants).await
        })
        .await
    }

    /// Step 2: accept the proposed base commit (`None`) or name another one. Moves on to the files: when none differs, to the secret scan.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `WrongStep` before the branches are chosen, `UnknownCommit` for a commit the mirror does not have.
    pub async fn choose_base_commit(
        &self,
        u: &User,
        id: i64,
        commit: Option<&CommitId>,
    ) -> Result<AdoptionView, AdoptionError> {
        self.call(AdoptionAction::ChooseBase, require_admin(u), async {
            self.inner.choose_base_commit(id, commit).await
        })
        .await
    }

    /// Step 3: the files whose NFS content differs from Git at the chosen commits, in path order, after `after`, at most `limit`
    /// (1,000 at most), each with its resolution if it has a current one, and the totals.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `WrongStep` before the base commit is chosen.
    pub async fn differing_files(
        &self,
        u: &User,
        id: i64,
        after: Option<&NfsPath>,
        limit: usize,
    ) -> Result<DifferingFiles, AdoptionError> {
        require_admin(u)?;
        self.inner.differing_files(id, after, limit).await
    }

    /// Step 3: record the admin's resolution of some files. A path must be one of the files that differ. Moves on to the secret scan
    /// when the last one is resolved (and back to the files when a changed file needs a new resolution).
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `WrongStep`, `NotDiffering` for a path that is not a differing file (nothing is recorded), `Read(BadRequest)`
    /// for more than `adoption_resolutions_per_call`.
    pub async fn resolve(
        &self,
        u: &User,
        id: i64,
        items: Vec<(NfsPath, Resolution)>,
    ) -> Result<AdoptionView, AdoptionError> {
        self.call(AdoptionAction::Resolve, require_admin(u), async {
            self.inner.resolve(u, id, items).await
        })
        .await
    }

    /// Step 3, the bulk action: the same resolution for every differing file in `scope` (all of them, or those under a folder, and only
    /// the ones without a current resolution unless `scope` says otherwise).
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `WrongStep`.
    pub async fn resolve_all(
        &self,
        u: &User,
        id: i64,
        scope: Bulk,
        resolution: Resolution,
    ) -> Result<AdoptionView, AdoptionError> {
        self.call(AdoptionAction::Resolve, require_admin(u), async {
            self.inner.resolve_all(u, id, scope, resolution).await
        })
        .await
    }

    /// Step 4: scan the base head, the chosen tenant heads and the NFS content for secrets, and store where they may be: file, line,
    /// length and rule, never a value (S13). The scan can be run again.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `WrongStep` before the files are resolved.
    pub async fn scan_secrets(&self, u: &User, id: i64) -> Result<AdoptionView, AdoptionError> {
        self.call(AdoptionAction::ScanSecrets, require_admin(u), async {
            self.inner.scan_secrets(id).await
        })
        .await
    }

    /// Step 5: confirm. One transaction writes the baselines (and the reference to the raw version behind each), the marks of the
    /// intentional resolutions, the tenant set, and (for the first pass) unlocks editing; with one `BaselineAdopted` audit event, one
    /// `DriftMarkedIntentional` for each mark, one `SettingsChanged` when the tenant set changed, and `DriftChanged` and
    /// `FindingsChanged` in the outbox. A second confirmation of the same pass finds it closed.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `WrongStep` before the secret scan, `Unresolved(n)` for files without a current resolution, `Changed` for a
    /// file that changed while the confirmation was prepared (nothing is written; confirm again), `NotOpen` for a closed pass.
    pub async fn confirm(&self, u: &User, id: i64) -> Result<ConfirmOutcome, AdoptionError> {
        self.call(AdoptionAction::Confirm, require_admin(u), async {
            self.inner.confirm(u, id).await
        })
        .await
    }

    /// Close an open pass without confirming it. A review that is cancelled is not raised again for the same suffixes (T4).
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `NotOpen` for a pass that is not open.
    pub async fn cancel(&self, u: &User, id: i64) -> Result<(), AdoptionError> {
        self.call(AdoptionAction::Cancel, require_admin(u), async {
            self.inner.cancel(u, id).await
        })
        .await
    }

    /// Mark the divergence of a file as intended: while NFS and Git both keep the content they have now, the file is
    /// `intentional_divergence` and not a finding (D20). One audit event (`DriftMarkedIntentional`) and the pipeline is told.
    /// `via` is how the caller reached the service (the UI, an MCP tool).
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `Read(Forbidden)` below Operator, `Read(NotFound)` for a file the swimlane does not have, `NotDivergent`
    /// for a file that is in sync, untracked or marked already, `InvalidReason`.
    pub async fn mark_intentional(
        &self,
        u: &User,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        reason: &Reason,
        via: ports::AuditVia,
    ) -> Result<(), AdoptionError> {
        self.call(AdoptionAction::Mark, require_operator(u), async {
            self.inner.mark_intentional(u, swimlane, path, reason, via).await
        })
        .await
    }

    /// Remove the mark of an intentional divergence. One audit event (`DriftMarkCleared`) and the pipeline is told.
    ///
    /// # Errors
    ///
    /// [`AdoptionError`]; `Read(Forbidden)` below Operator, `Read(NotFound)` for a file with no mark.
    pub async fn clear_intentional(
        &self,
        u: &User,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        via: ports::AuditVia,
    ) -> Result<(), AdoptionError> {
        self.call(AdoptionAction::Clear, require_operator(u), async {
            self.inner.clear_intentional(u, swimlane, path, via).await
        })
        .await
    }

    /// Run a call after its role check: a refused caller costs nothing, and every outcome is counted (labels: action and outcome, never a
    /// swimlane or a user).
    async fn call<T>(
        &self,
        action: AdoptionAction,
        allowed: Result<(), AdoptionError>,
        run: impl std::future::Future<Output = Result<T, AdoptionError>>,
    ) -> Result<T, AdoptionError> {
        let result = match allowed {
            Ok(()) => run.await,
            Err(e) => Err(e),
        };
        let outcome = match &result {
            Ok(_) => AdoptionOutcome::Ok,
            Err(e) => e.outcome(),
        };
        self.inner.metrics.adoption(action, outcome);
        result
    }
}

/// The hash of a file's raw content, as the 32 bytes the database holds, or `Corrupt`.
pub(crate) fn hash_of(bytes: Vec<u8>) -> Result<ContentHash, RegistryError> {
    <[u8; 32]>::try_from(bytes)
        .map(ContentHash::from_bytes)
        .map_err(|_| RegistryError::Corrupt)
}

#[cfg(test)]
mod tests {
    use domain::{Role, ShortText, UserId, UserStatus};

    use super::*;

    fn user(role: Option<Role>, status: UserStatus) -> User {
        let guid =
            |n: u8| domain::Guid::parse(&format!("00000000-0000-0000-0000-0000000000{n:02x}")).unwrap();
        User {
            id: UserId::new(guid(1), guid(2)),
            email: ShortText::parse("a@example.com").unwrap(),
            display_name: ShortText::parse("A").unwrap(),
            role,
            status,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        }
    }

    #[test]
    fn only_an_active_admin_passes_the_admin_check() {
        for role in [None, Some(Role::Viewer), Some(Role::Editor), Some(Role::Operator)] {
            assert_eq!(
                require_admin(&user(role, UserStatus::Active)),
                Err(AdoptionError::Read(ReadError::Forbidden)),
                "{role:?}"
            );
        }
        for status in [UserStatus::Pending, UserStatus::Disabled] {
            assert_eq!(
                require_admin(&user(Some(Role::Admin), status)),
                Err(AdoptionError::Read(ReadError::Forbidden)),
                "{status:?}"
            );
        }
        assert_eq!(
            require_admin(&user(Some(Role::Admin), UserStatus::Active)),
            Ok(())
        );
    }

    #[test]
    fn the_marks_need_an_active_operator_or_above() {
        for role in [None, Some(Role::Viewer), Some(Role::Editor)] {
            assert_eq!(
                require_operator(&user(role, UserStatus::Active)),
                Err(AdoptionError::Read(ReadError::Forbidden)),
                "{role:?}"
            );
        }
        for status in [UserStatus::Pending, UserStatus::Disabled] {
            assert!(
                require_operator(&user(Some(Role::Admin), status)).is_err(),
                "{status:?}"
            );
        }
        for role in [Role::Operator, Role::Admin] {
            assert_eq!(
                require_operator(&user(Some(role), UserStatus::Active)),
                Ok(()),
                "{role:?}"
            );
        }
    }

    #[test]
    fn registry_errors_map_to_what_a_caller_can_act_on() {
        assert_eq!(
            AdoptionError::from(RegistryError::NotFound),
            AdoptionError::Read(ReadError::NotFound)
        );
        assert_eq!(
            AdoptionError::from(RegistryError::Git(ports::GitError::NotFound)),
            AdoptionError::UnknownCommit
        );
        assert_eq!(
            AdoptionError::from(RegistryError::Capacity),
            AdoptionError::Read(ReadError::Unavailable)
        );
        assert_eq!(
            AdoptionError::from(RegistryError::Database),
            AdoptionError::Registry(RegistryError::Database)
        );
        assert_eq!(
            AdoptionError::Read(ReadError::Forbidden).outcome(),
            AdoptionOutcome::Forbidden
        );
        assert_eq!(AdoptionError::Changed.outcome(), AdoptionOutcome::Rejected);
        assert_eq!(
            AdoptionError::Registry(RegistryError::Timeout).outcome(),
            AdoptionOutcome::Failed
        );
    }

    #[test]
    fn no_error_text_carries_input() {
        for e in [
            AdoptionError::Read(ReadError::Forbidden),
            AdoptionError::NoInventory,
            AdoptionError::AlreadyAdopted,
            AdoptionError::NotAdopted,
            AdoptionError::NotOpen,
            AdoptionError::WrongStep,
            AdoptionError::NotDeployable,
            AdoptionError::UnknownCommit,
            AdoptionError::NotDiffering,
            AdoptionError::Unresolved(3),
            AdoptionError::Changed,
            AdoptionError::InvalidReason,
            AdoptionError::NotDivergent,
            AdoptionError::Registry(RegistryError::Database),
        ] {
            let text = e.to_string();
            assert!(text.len() < 80 && !text.contains("SELECT"), "{text}");
        }
    }

    #[test]
    fn a_ratio_of_nothing_is_zero() {
        let branch = RankedBranch {
            branch: GitRef::parse("sit1").unwrap(),
            tenant: TenantId::parse("sit1").unwrap(),
            shared: 0,
            union: 0,
        };
        assert!((branch.ratio() - 0.0).abs() < f64::EPSILON);
        let half = RankedBranch {
            shared: 1,
            union: 2,
            ..branch
        };
        assert!((half.ratio() - 0.5).abs() < f64::EPSILON);
    }
}
