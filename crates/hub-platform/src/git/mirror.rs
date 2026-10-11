//! The bare mirrors of the two repositories, on the replica's own volume (T7, D64, P2, S18).
//!
//! ```text
//!   <LK_MIRROR_DIR>/base.git     all branches, all tags of configuration-base-saas
//!   <LK_MIRROR_DIR>/tenant.git   all branches (sit*, templates/*, templates/common-docs), all tags of csp-tenant-data
//! ```
//!
//! A mirror is filled by `gix`: a fetch with `+refs/heads/*:refs/heads/*` and `+refs/tags/*:refs/tags/*`, so a forced push upstream
//! is followed, a new branch appears, and a ref that no longer exists upstream is deleted here too. Everything `gix` does is
//! blocking, so it runs on a blocking thread (rule 4), and each fetch has a deadline.
//!
//! **Single flight.** One fetch per repository at a time. A caller that arrives while a fetch runs needs a fetch that *starts after
//! it arrived* (the running one may have missed what it wants), so it waits for the next one; any number of callers share it. N
//! webhooks, a poll and a read-through for a missing commit therefore cost at most two fetches.
//!
//! **Credentials** come from [`GitCredentials`] and reach `gix` through its credential hook, in memory. The hook answers only for
//! the host and protocol of the remote's own URL, so a redirect to another host gets nothing.
//!
//! The mirror volume is the one writable path of the hub besides `/tmp` (S16). Nothing in it is trusted as a source of truth: it
//! can be deleted, and the next start fetches everything again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use domain::{CommitId, GitRef, RepoKind};
use http::Uri;
use tracing::{debug, warn};

use super::credentials::{GitCredential, GitCredentials};
use super::metrics::{FetchOutcome, GitMetrics};

/// How long one fetch may take, unless the settings say otherwise.
pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Extra time the async side waits for a fetch past its deadline: the HTTP client's own limits end a stalled socket (30 s).
const HTTP_GRACE: Duration = Duration::from_secs(45);
/// The longest remote URL accepted.
const MAX_URL_BYTES: usize = 2048;

/// The refspecs of a mirror: every branch and every tag, forced.
const REFSPECS: [&str; 2] = ["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"];

/// Why a mirror operation failed. A closed set that carries no URL, path or credential: the cause goes to the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MirrorError {
    #[error("the remote url is not valid")]
    InvalidUrl,
    #[error("the git remote could not be reached")]
    Unavailable,
    #[error("the fetch took too long")]
    Timeout,
    #[error("the git credential is not available")]
    Credentials,
    #[error("the mirror could not be opened")]
    Open,
}

/// The URL of a remote. Only `https` without credentials is accepted: a credential in a URL would end up in logs and in `gix`'s
/// error messages, and a plain-text transport would expose the token (S18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    text: String,
    /// `host` or `host:port` as the credential hook sees it, and the scheme.
    host: String,
    scheme: &'static str,
}

impl RemoteUrl {
    /// `https://host[:port]/path`, with no user name or password.
    pub fn https(url: &str) -> Result<Self, MirrorError> {
        if url.is_empty()
            || url.len() > MAX_URL_BYTES
            || url.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(MirrorError::InvalidUrl);
        }
        let parsed: Uri = url.parse().map_err(|_| MirrorError::InvalidUrl)?;
        let authority = parsed.authority().ok_or(MirrorError::InvalidUrl)?;
        if parsed.scheme_str() != Some("https")
            || authority.as_str().contains('@')
            || authority.host().is_empty()
        {
            return Err(MirrorError::InvalidUrl);
        }
        Ok(Self {
            text: url.to_owned(),
            host: authority.as_str().to_ascii_lowercase(),
            scheme: "https",
        })
    }

    /// A local URL for tests: `file://`, or `http://` to a test server on the loopback address. Never reachable from settings.
    #[cfg(any(test, feature = "fakes"))]
    #[must_use]
    pub fn local(url: &str) -> Self {
        let (scheme, host) = if let Some(rest) = url.strip_prefix("http://") {
            (
                "http",
                rest.split('/').next().unwrap_or_default().to_ascii_lowercase(),
            )
        } else {
            ("file", String::new())
        };
        Self {
            text: url.to_owned(),
            host,
            scheme,
        }
    }

    fn as_str(&self) -> &str {
        &self.text
    }
}

/// Where the mirrors are and what they mirror.
#[derive(Debug, Clone)]
pub struct MirrorSettings {
    pub dir: PathBuf,
    pub base: RemoteUrl,
    pub tenant: RemoteUrl,
    /// The longest one fetch may take.
    pub fetch_timeout: Duration,
}

impl MirrorSettings {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, base: RemoteUrl, tenant: RemoteUrl) -> Self {
        Self {
            dir: dir.into(),
            base,
            tenant,
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
        }
    }
}

/// Every branch and tag of a mirror, by name, at the commit it points at (an annotated tag at the commit it ends in).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefSnapshot {
    pub branches: BTreeMap<GitRef, CommitId>,
    pub tags: BTreeMap<GitRef, CommitId>,
}

/// Whether a call ran a fetch or shared one that ran for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStatus {
    Ran,
    Shared,
}

// ------------------------------------------------------------------------------------------------ single flight

#[derive(Default)]
struct FlightState {
    running: bool,
    /// Tickets handed out: the number of fetches started.
    started: u64,
    /// The newest ticket that finished, and how.
    finished: u64,
    last: Option<Result<(), MirrorError>>,
}

#[derive(Default)]
struct Flight {
    state: Mutex<FlightState>,
    done: Condvar,
}

impl Flight {
    fn lock(&self) -> MutexGuard<'_, FlightState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The deadline of one operation, and the flag `gix` polls while it receives a pack.
struct Deadline {
    at: Instant,
    flag: Arc<AtomicBool>,
    stop: Option<std::sync::mpsc::Sender<()>>,
}

impl Deadline {
    fn start(after: Duration) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let watch = Arc::clone(&flag);
        // One short-lived thread per fetch sets the flag when the time is up, or ends when the fetch does.
        let spawned = std::thread::Builder::new()
            .name("git-deadline".to_owned())
            .spawn(move || {
                if matches!(
                    stopped.recv_timeout(after),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    watch.store(true, Ordering::SeqCst);
                }
            });
        if spawned.is_err() {
            // No thread to watch the clock: the phase checks below still enforce the deadline.
            warn!(
                code = "git_deadline_thread",
                "could not start the deadline thread"
            );
        }
        Self {
            at: Instant::now() + after,
            flag,
            stop: Some(stop),
        }
    }

    fn expired(&self) -> bool {
        self.flag.load(Ordering::SeqCst) || Instant::now() >= self.at
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

// ------------------------------------------------------------------------------------------------------ mirror

struct RepoMirror {
    kind: RepoKind,
    url: RemoteUrl,
    path: PathBuf,
    repo: gix::ThreadSafeRepository,
    flight: Flight,
    /// True once the mirror holds at least one branch.
    populated: AtomicBool,
}

/// Both mirrors of one replica.
pub struct Mirror {
    repos: [RepoMirror; 2],
    credentials: Arc<dyn GitCredentials>,
    metrics: GitMetrics,
    fetch_timeout: Duration,
    handle: tokio::runtime::Handle,
}

impl std::fmt::Debug for Mirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mirror")
            .field("ready", &self.ready())
            .finish_non_exhaustive()
    }
}

fn dir_name(kind: RepoKind) -> &'static str {
    match kind {
        RepoKind::Base => "base.git",
        RepoKind::Tenant => "tenant.git",
    }
}

fn open_or_init(path: &Path) -> Result<gix::ThreadSafeRepository, MirrorError> {
    let log = |e: &dyn std::fmt::Display| {
        warn!(code = "git_mirror_open", error = %e, "could not open a mirror");
        MirrorError::Open
    };
    // `isolated`: no system, user or environment configuration. A mirror behaves the same on every machine.
    let options = gix::open::Options::isolated();
    if path.join("HEAD").is_file() {
        return gix::ThreadSafeRepository::open_opts(path, options).map_err(|e| log(&e));
    }
    std::fs::create_dir_all(path).map_err(|e| log(&e))?;
    gix::ThreadSafeRepository::init_opts(
        path,
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        options,
    )
    .map_err(|e| log(&e))
}

impl Mirror {
    /// Opens (or creates, empty) both mirrors under `settings.dir`. Does no network I/O. Must be called inside a Tokio runtime:
    /// the fetches that run on blocking threads reach back into it for the credential.
    pub fn open(
        settings: MirrorSettings,
        credentials: Arc<dyn GitCredentials>,
        metrics: GitMetrics,
    ) -> Result<Arc<Self>, MirrorError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| MirrorError::Open)?;
        let build = |kind: RepoKind, url: RemoteUrl| -> Result<RepoMirror, MirrorError> {
            let path = settings.dir.join(dir_name(kind));
            let repo = open_or_init(&path)?;
            let mirror = RepoMirror {
                kind,
                url,
                path,
                repo,
                flight: Flight::default(),
                populated: AtomicBool::new(false),
            };
            let has = snapshot(&mirror.repo.to_thread_local()).is_ok_and(|s| !s.branches.is_empty());
            mirror.populated.store(has, Ordering::SeqCst);
            Ok(mirror)
        };
        Ok(Arc::new(Self {
            repos: [
                build(RepoKind::Base, settings.base)?,
                build(RepoKind::Tenant, settings.tenant)?,
            ],
            credentials,
            metrics,
            fetch_timeout: settings.fetch_timeout,
            handle,
        }))
    }

    fn repo(&self, kind: RepoKind) -> &RepoMirror {
        match kind {
            RepoKind::Base => &self.repos[0],
            RepoKind::Tenant => &self.repos[1],
        }
    }

    /// A handle on the mirror for reading. Cheap: the object database and its pack indexes are shared.
    pub(crate) fn repository(&self, kind: RepoKind) -> gix::Repository {
        self.repo(kind).repo.to_thread_local()
    }

    /// Whether the mirror of `kind` holds the commit `id`. Blocking (reads the object store).
    #[must_use]
    pub fn has_commit(&self, kind: RepoKind, id: &CommitId) -> bool {
        gix::ObjectId::from_hex(id.as_str().as_bytes()).is_ok_and(|oid| {
            self.repository(kind)
                .find_header(oid)
                .is_ok_and(|h| h.kind() == gix::object::Kind::Commit)
        })
    }

    /// The shared handle, for code that makes its own thread-local handles (a pool of workers).
    pub(crate) fn thread_safe(&self, kind: RepoKind) -> &gix::ThreadSafeRepository {
        &self.repo(kind).repo
    }

    /// Where the mirror of `kind` is on disk.
    #[must_use]
    pub fn path(&self, kind: RepoKind) -> &Path {
        &self.repo(kind).path
    }

    /// True once both mirrors hold a branch: readiness for `/readyz`.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.repos.iter().all(|r| r.populated.load(Ordering::SeqCst))
    }

    /// Every branch and tag of a mirror. Blocking (reads the ref store): call it from a blocking thread.
    pub fn refs(&self, kind: RepoKind) -> Result<RefSnapshot, MirrorError> {
        snapshot(&self.repository(kind))
    }

    /// Fetches every mirror that is still empty (a first start, or a deleted volume). One that already holds a branch is left to
    /// the webhook and the poll.
    pub async fn initialize(self: &Arc<Self>) -> Result<(), MirrorError> {
        for kind in [RepoKind::Base, RepoKind::Tenant] {
            if !self.repo(kind).populated.load(Ordering::SeqCst) {
                self.fetch(kind).await?;
                // A remote with no branch is not an error of the fetch, but it leaves nothing to serve.
                if !self.repo(kind).populated.load(Ordering::SeqCst) {
                    return Err(MirrorError::Unavailable);
                }
            }
        }
        Ok(())
    }

    /// Fetch `kind` from its remote, or share a fetch that starts after this call. Bounded by the fetch timeout.
    pub async fn fetch(self: &Arc<Self>, kind: RepoKind) -> Result<FetchStatus, MirrorError> {
        let this = Arc::clone(self);
        let work = tokio::task::spawn_blocking(move || this.fetch_blocking(kind));
        match tokio::time::timeout(self.fetch_timeout + HTTP_GRACE, work).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(MirrorError::Unavailable),
            Err(_) => Err(MirrorError::Timeout),
        }
    }

    /// The blocking form of [`Mirror::fetch`], for the thread that is already blocking (the read-through of a missing commit).
    pub fn fetch_blocking(&self, kind: RepoKind) -> Result<FetchStatus, MirrorError> {
        let repo = self.repo(kind);
        let give_up = Instant::now() + self.fetch_timeout + HTTP_GRACE;
        let mut state = repo.flight.lock();
        let arrival = state.started;
        let ticket = loop {
            if state.finished > arrival {
                // A fetch that started after this call began has finished: it is the one this call needed.
                let result = state.last.unwrap_or(Err(MirrorError::Unavailable));
                drop(state);
                self.metrics.fetch(kind, FetchOutcome::Coalesced, None);
                return result.map(|()| FetchStatus::Shared);
            }
            if !state.running {
                state.running = true;
                state.started += 1;
                break state.started;
            }
            let Some(left) = give_up.checked_duration_since(Instant::now()) else {
                return Err(MirrorError::Timeout);
            };
            state = repo
                .flight
                .done
                .wait_timeout(state, left)
                .map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
        };
        drop(state);

        let started = Instant::now();
        // A panic inside `gix` (none is expected) must not leave the flight marked as running, or every later fetch of this
        // repository would wait for it until its own deadline.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_fetch(repo)))
            .unwrap_or_else(|_| {
                warn!(
                    code = "git_fetch_panicked",
                    repo = kind.as_str(),
                    "a fetch panicked"
                );
                Err(MirrorError::Unavailable)
            });
        let took = started.elapsed();
        let outcome = match &result {
            Ok(()) => FetchOutcome::Ok,
            Err(MirrorError::Timeout) => FetchOutcome::TimedOut,
            Err(_) => FetchOutcome::Failed,
        };
        self.metrics.fetch(kind, outcome, Some(took));
        debug!(
            code = "git_fetch",
            repo = kind.as_str(),
            ok = result.is_ok(),
            ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX)
        );

        let mut state = repo.flight.lock();
        state.running = false;
        state.finished = ticket;
        state.last = Some(result);
        drop(state);
        repo.flight.done.notify_all();
        result.map(|()| FetchStatus::Ran)
    }

    fn run_fetch(&self, repo: &RepoMirror) -> Result<(), MirrorError> {
        let deadline = Deadline::start(self.fetch_timeout);
        if deadline.expired() {
            return Err(MirrorError::Timeout);
        }
        let credential = self
            .handle
            .block_on(self.credentials.credential())
            .map_err(|_| MirrorError::Credentials)?;
        let result = fetch_into(repo, credential, &deadline);
        if result.is_ok() {
            // Ready means "holds a branch": a fetch from a repository that has none succeeds and leaves the mirror empty.
            let has = snapshot(&repo.repo.to_thread_local()).is_ok_and(|s| !s.branches.is_empty());
            repo.populated.store(has, Ordering::SeqCst);
        }
        result
    }
}

/// Fetch every branch and tag of `repo`'s remote into it, then delete the refs the remote no longer has.
fn fetch_into(
    repo: &RepoMirror,
    credential: Option<GitCredential>,
    deadline: &Deadline,
) -> Result<(), MirrorError> {
    let local = repo.repo.to_thread_local();
    let fail = |stage: &'static str, e: &dyn std::fmt::Display| {
        // The message names no secret: the credential never enters a URL or an error. It can name the remote and a path.
        warn!(code = "git_fetch_failed", repo = repo.kind.as_str(), stage, error = %e, "fetch failed");
        if deadline.expired() {
            MirrorError::Timeout
        } else {
            MirrorError::Unavailable
        }
    };
    let remote = local
        .remote_at(repo.url.as_str())
        .map_err(|e| fail("remote", &e))?
        .with_refspecs(REFSPECS, gix::remote::Direction::Fetch)
        .map_err(|e| fail("refspecs", &e))?
        .with_fetch_tags(gix::remote::fetch::Tags::None);
    if deadline.expired() {
        return Err(MirrorError::Timeout);
    }
    let mut connection = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(|e| fail("connect", &e))?;
    if let Some(credential) = credential {
        let expected_host = repo.url.host.clone();
        let expected_scheme = repo.url.scheme;
        connection = connection.with_credentials(move |action| {
            use gix::credentials::helper::{Action, NextAction};
            match action {
                Action::Get(mut ctx) => {
                    // The request carries the URL; the host and protocol are read out of it.
                    if ctx.destructure_url_in_place(false).is_err() {
                        return Ok(None);
                    }
                    // Only the host the remote's URL names, and only over its scheme: a redirect elsewhere gets nothing.
                    // The credential protocol's `host` is `host` or `host:port`, as the URL wrote it.
                    let host_ok = ctx.host.as_ref().is_some_and(|h| {
                        let given = h.to_string().to_ascii_lowercase();
                        given == expected_host
                            || expected_host
                                .strip_suffix(":443")
                                .is_some_and(|bare| bare == given)
                    });
                    let scheme_ok = ctx.protocol.as_deref().is_some_and(|p| p == expected_scheme);
                    if !(host_ok && scheme_ok) {
                        return Ok(None);
                    }
                    Ok(Some(gix::credentials::protocol::Outcome {
                        identity: gix::sec::identity::Account {
                            username: credential.username.clone(),
                            password: credential.token.expose().clone(),
                            oauth_refresh_token: None,
                        },
                        next: NextAction::from(ctx),
                    }))
                }
                // Nothing is stored and nothing is erased: the token is not ours to persist.
                Action::Store(_) | Action::Erase(_) => Ok(None),
            }
        });
    }
    let prepare = connection
        .prepare_fetch(gix::progress::Discard, gix::remote::ref_map::Options::default())
        .map_err(|e| fail("prepare", &e))?;
    if deadline.expired() {
        return Err(MirrorError::Timeout);
    }
    let outcome = prepare
        .receive(gix::progress::Discard, &deadline.flag)
        .map_err(|e| fail("receive", &e))?;
    prune(&local, &outcome).map_err(|e| fail("prune", &e))?;
    Ok(())
}

/// Delete the local branches and tags that the remote did not list. Not done when the remote listed no branch at all: an empty
/// advertisement of a repository that had branches is far more likely to be a fault than a repository emptied of every branch.
fn prune(
    local: &gix::Repository,
    outcome: &gix::remote::fetch::Outcome,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::collections::HashSet;
    let mut remote_names: HashSet<Vec<u8>> = HashSet::new();
    for r in &outcome.ref_map.remote_refs {
        let (name, _, _) = r.unpack();
        remote_names.insert(name.to_vec());
    }
    let any_branch = remote_names.iter().any(|n| n.starts_with(b"refs/heads/"));
    if !any_branch {
        return Ok(());
    }
    let mut stale = Vec::new();
    for prefix in ["refs/heads/", "refs/tags/"] {
        let platform = local.references()?;
        for reference in platform.prefixed(prefix)? {
            let reference = reference?;
            let full = reference.name().as_bstr().to_vec();
            if !remote_names.contains(&full) {
                stale.push(full);
            }
        }
    }
    if stale.is_empty() {
        return Ok(());
    }
    let edits = stale
        .into_iter()
        .map(|name| {
            Ok(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Delete {
                    expected: gix::refs::transaction::PreviousValue::Any,
                    log: gix::refs::transaction::RefLog::AndReference,
                },
                name: gix::refs::FullName::try_from(gix::bstr::BString::from(name))?,
                deref: false,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync>>>()?;
    local.edit_references(edits)?;
    Ok(())
}

fn to_commit_id(id: gix::ObjectId) -> Option<CommitId> {
    CommitId::parse(&id.to_hex().to_string()).ok()
}

/// The branches and tags of `repo`. A ref whose name or target is not usable is skipped and logged.
pub(crate) fn snapshot(repo: &gix::Repository) -> Result<RefSnapshot, MirrorError> {
    let log = |e: &dyn std::fmt::Display| {
        warn!(code = "git_refs_unreadable", error = %e, "could not read the refs of a mirror");
        MirrorError::Unavailable
    };
    let mut out = RefSnapshot::default();
    for (prefix, is_tag) in [("refs/heads/", false), ("refs/tags/", true)] {
        let platform = repo.references().map_err(|e| log(&e))?;
        let iter = platform.prefixed(prefix).map_err(|e| log(&e))?;
        for reference in iter {
            let mut reference = match reference {
                Ok(r) => r,
                Err(e) => {
                    warn!(code = "git_ref_skipped", error = %e, "a ref could not be read");
                    continue;
                }
            };
            let full = reference.name().as_bstr().to_string();
            let Some(short) = full.strip_prefix(prefix) else {
                continue;
            };
            let Ok(name) = GitRef::parse(short) else {
                warn!(
                    code = "git_ref_name_skipped",
                    "a ref with an unusable name was skipped"
                );
                continue;
            };
            let Ok(id) = reference.peel_to_id() else {
                warn!(
                    code = "git_ref_target_skipped",
                    "a ref that does not resolve was skipped"
                );
                continue;
            };
            let id = id.detach();
            if !repo
                .find_header(id)
                .is_ok_and(|h| h.kind() == gix::object::Kind::Commit)
            {
                continue;
            }
            let Some(commit) = to_commit_id(id) else { continue };
            if is_tag {
                out.tags.insert(name, commit);
            } else {
                out.branches.insert(name, commit);
            }
        }
    }
    Ok(out)
}
