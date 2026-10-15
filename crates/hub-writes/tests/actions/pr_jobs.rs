//! T9 (D77; S4, S8, S9, S10, P12): pull-request links and jobs. A request for a pull request is accepted as a persisted job and
//! answered with its id; the job runs one durable step at a time and resumes after a crash in any state; a second pull request for
//! a change that has an open one is refused with the first one's link; and a change of state learned from GitHub reaches the
//! screens that show the drift. All of it over a real Postgres and the simulated GitHub of the T6 tests.
#![allow(clippy::too_many_lines)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use domain::{
    DomainEvent, Page, PrJobId, PrJobState, PrState, ProposalStatus, RejectReason, Role, Secret, User,
    UserStatus, WriteOutcome,
};
use futures::StreamExt;
use hub_writes::github::GithubClient;
use hub_writes::prs::{
    Advanced, Driven, LinkQuery, PrDeps, PrDesk, PrJobRunner, PrJobView, PrStateWatcher, Raised, Watched,
};
use hub_writes::vault::VaultDeps;
use hub_writes::{ProposalDesk, WriteHub, WriteHubDeps};
use ports::conformance::sample::nfs;
use ports::{
    AuditAction, AuditActor, BlobStore, EventBus, EventFilter, EventKind, WriteError, WriteService,
    verify_chain,
};
use sqlx::Row;

use super::pull_requests::{BASE, Prs, TENANT, fast, fixture, tok};
use crate::support::fake_github::Reply;
use crate::support::log_capture;
use crate::support::pg_bus::PgTestBus;
use crate::support::rig::{ctx, h};

const ENTRY: &str = "tx-api/entry-sit1.yml";
const OTHER: &str = "tx-api/other-sit1.yml";
const THIRD: &str = "tx-api/third-sit1.yml";
const ENTRY_NFS: &str = "a: 1\nb: 20\n";

/// The simulated GitHub and a hub with its pull-request jobs wired.
struct Jobs {
    f: Prs,
    runner: PrJobRunner,
    /// Someone who only looks, so that a test can look at a job whose author was disabled.
    observer: User,
    /// What the keys table holds before the test acts (the key the fixture stored a GitHub token with).
    keys: i64,
}

async fn jobs() -> Option<Jobs> {
    let f = fixture().await?;
    f.sim.branch(
        TENANT,
        "sit1",
        &[
            ("data/config/tx-api/entry.yml", b"a: 1\nb: 2\n"),
            ("data/config/tx-api/other.yml", b"o: 1\n"),
        ],
    );
    f.sim
        .branch(BASE, "master", &[("config/tx-api/entry.yml", b"a: 0\n")]);
    f.rig
        .seed(&f.s, &[(ENTRY, ENTRY_NFS), (OTHER, "o: 2\n"), (THIRD, "t: 1\n")]);
    let runner = f.rig.hub.pr_jobs(f.deps.clone());
    let observer = f.rig.user(99, Role::Viewer).await;
    let keys = f.rig.count("idempotency_keys").await;
    Some(Jobs {
        f,
        runner,
        observer,
        keys,
    })
}

impl Jobs {
    /// Another replica: its own hub (and so its own runner and vault) over the same database, agent and GitHub.
    fn replica(&self) -> PrJobRunner {
        let hub = self.f.rig.replica();
        let vault = hub.token_vault(VaultDeps {
            kms: self.f.kms.clone(),
            github: Arc::new(self.f.gh.clone()),
            repos: self.f.repos.clone(),
        });
        let client = GithubClient::for_tests(&self.f.server.base(), fast()).unwrap();
        hub.pr_jobs(PrDeps {
            vault,
            git: Arc::new(client),
            settings: self.f.deps.settings.clone(),
        })
    }

    async fn raise(&self, key: &str, paths: &[&str]) -> Raised {
        self.runner
            .raise(&self.f.ctx(key), self.f.request(paths, None))
            .await
            .unwrap()
    }

    async fn job(&self, key: &str, paths: &[&str]) -> PrJobView {
        match self.raise(key, paths).await {
            Raised::Job(view) => view,
            other => panic!("expected a job, got {other:?}"),
        }
    }

    async fn state_of(&self, id: PrJobId) -> PrJobView {
        self.runner.job(&self.observer, id).await.unwrap().unwrap()
    }

    async fn progress(&self, id: PrJobId) -> Vec<PrJobState> {
        self.f
            .rig
            .bus
            .published()
            .await
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::PrJobProgress { job_id, state } if job_id == id => Some(state),
                _ => None,
            })
            .collect()
    }

    async fn raised_events(&self) -> Vec<ports::AuditEntry> {
        self.f
            .rig
            .chain()
            .await
            .into_iter()
            .filter(|e| e.event.action == AuditAction::PrRaised)
            .collect()
    }

    /// The pull requests of the tenant repository.
    fn pulls(&self) -> Vec<crate::support::fake_git::SimPull> {
        self.f.sim.pulls(TENANT)
    }

    /// The branches Lanekeeper made.
    fn made_branches(&self) -> Vec<String> {
        self.f
            .sim
            .branches(TENANT)
            .into_iter()
            .filter(|b| b.starts_with("lanekeeper/"))
            .collect()
    }
}

/// The directory disables the user (the next time anyone asks it).
fn disable(t: &Jobs, user: &User) {
    let mut u = user.clone();
    u.status = UserStatus::Disabled;
    assert!(t.f.rig.users.insert(u));
}

fn id_of(view: &PrJobView) -> PrJobId {
    PrJobId::from(view.id)
}

/// A hub whose bus also listens, so that what is published reaches the subscribers of the other hubs of the test (replicas).
fn hub_listening(t: &Jobs, bus: PgTestBus) -> WriteHub {
    let rig = &t.f.rig;
    WriteHub::new(WriteHubDeps {
        pool: rig.pool.clone(),
        users: rig.users.clone(),
        audit: rig.audit.clone(),
        bus: Arc::new(bus),
        gateway: Arc::new(rig.agent.clone()),
        blobs: rig.blobs.clone(),
        registry: rig.registry.clone(),
        tool_writes: Arc::new(crate::support::pg_tool_writes::PgToolWrites),
        notifier: rig.notifier.clone(),
        secrets: rig.secrets.clone(),
        leases: rig.leases.clone(),
        clock: rig.clock.clone(),
        config: hub_writes::WriteConfig::default(),
    })
    .unwrap()
}

/// What a finished pull request leaves, however it got there: one pull request on one branch of ours, a link for each file, one
/// audit event that records the pull request's address, and a body that names that event.
async fn assert_one_pull_request(t: &Jobs, id: PrJobId, files: usize) {
    let pulls = t.pulls();
    assert_eq!(pulls.len(), 1, "exactly one pull request: {pulls:?}");
    assert_eq!(t.made_branches().len(), 1, "exactly one branch of ours");
    assert_eq!(t.f.rig.count("pr_links").await, i64::try_from(files).unwrap());
    let events = t.raised_events().await;
    assert_eq!(events.len(), 1, "exactly one audit event for the pull request");
    let diff = events[0].event.diff.as_deref().unwrap();
    assert!(
        diff.contains(&pulls[0].url),
        "the event records the address: {diff}"
    );
    assert!(
        pulls[0].body.contains(&format!("Audit event: {}", events[0].id)),
        "the body names the event: {}",
        pulls[0].body
    );
    assert!(verify_chain(&t.f.rig.chain().await).is_ok());
    let job = t.state_of(id).await;
    assert_eq!(job.state, PrJobState::Completed);
    assert_eq!(job.pr_url.as_deref(), Some(pulls[0].url.as_str()));
    assert_eq!(job.error, None);
}

// ------------------------------------------------------------------------------------------------------------ links

#[tokio::test]
async fn duplicate_pr_is_409_with_link() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    assert_eq!(job.state, PrJobState::Pending);
    assert_eq!(
        t.runner.drive(id_of(&job)).await,
        Driven::Finished(PrJobState::Completed)
    );
    let first = t.pulls();
    assert_eq!(first.len(), 1);

    // The same change, asked for again with another key: nothing is made, and the answer is the link of the pull request there.
    let Raised::Duplicate(link) = t.raise("k-2", &[ENTRY]).await else {
        panic!("expected the duplicate");
    };
    assert_eq!(link.pr_number, 1);
    assert_eq!(link.pr_url, first[0].url);
    assert_eq!(link.path, nfs(ENTRY));
    assert_eq!(link.nfs_hash, h(ENTRY_NFS));
    assert_eq!(link.state, PrState::Open);
    assert_eq!(link.author, t.f.editor.id);
    assert_eq!(link.closed_at, None);
    assert_eq!(t.f.rig.count("pr_jobs").await, 1, "no second job");
    assert_eq!(t.pulls().len(), 1, "no second pull request");
    assert_eq!(
        t.f.rig.count("idempotency_keys").await,
        t.keys + 1,
        "the refused request keeps no key"
    );

    // Another person, more files, one of them the same change: still the same pull request's link.
    let other = t.f.rig.user(2, Role::Editor).await;
    let again = t
        .runner
        .raise(&ctx(&other, "k-3"), t.f.request(&[ENTRY, OTHER], None))
        .await
        .unwrap();
    assert!(
        matches!(&again, Raised::Duplicate(l) if l.pr_number == 1),
        "{again:?}"
    );

    // It goes through the trait too: a conflict, and nothing else.
    t.f.rig.hub.pr_jobs(t.f.deps.clone());
    assert_eq!(
        t.f.rig
            .hub
            .raise_pr(&t.f.ctx("k-4"), t.f.request(&[ENTRY], None))
            .await,
        Ok(WriteOutcome::Conflict { current: None })
    );

    // A change is the file and its hash: once the file has other content, the old pull request does not cover it.
    t.f.rig.agent.put(&t.f.s, &nfs(ENTRY), b"a: 1\nb: 30\n");
    t.f.rig.catch_up_registry(&t.f.s);
    assert!(matches!(t.raise("k-5", &[ENTRY]).await, Raised::Job(_)));

    // And a pull request that was closed or merged no longer blocks the same change.
    sqlx::query("UPDATE pr_links SET state = 'closed' WHERE pr_number = 1")
        .execute(&t.f.rig.pool)
        .await
        .unwrap();
    t.f.rig.agent.put(&t.f.s, &nfs(ENTRY), ENTRY_NFS.as_bytes());
    t.f.rig.catch_up_registry(&t.f.s);
    assert!(matches!(t.raise("k-6", &[ENTRY]).await, Raised::Job(_)));
}

#[tokio::test]
async fn a_second_job_for_a_change_that_got_a_pull_request_meanwhile_fails_instead_of_opening_another() {
    let Some(t) = jobs().await else { return };
    // Two jobs for the same change are accepted before either has a link (two people, two keys, at the same moment).
    let other = t.f.rig.user(2, Role::Editor).await;
    t.f.gh.token(&tok(2), "hubot", &[BASE, TENANT]);
    t.f.sim.expect_token(&tok(2));
    t.f.vault
        .put(&ctx(&other, "k-token"), Secret::new(tok(2)))
        .await
        .unwrap();
    let first = t.job("k-1", &[ENTRY]).await;
    let Raised::Job(second) = t
        .runner
        .raise(&ctx(&other, "k-2"), t.f.request(&[ENTRY], None))
        .await
        .unwrap()
    else {
        panic!("the second request is accepted: it is another person's");
    };
    assert_ne!(first.id, second.id);

    assert_eq!(
        t.runner.drive(id_of(&first)).await,
        Driven::Finished(PrJobState::Completed)
    );
    assert_eq!(
        t.runner.drive(id_of(&second)).await,
        Driven::Finished(PrJobState::Failed)
    );
    let failed = t.state_of(id_of(&second)).await;
    assert_eq!(failed.state, PrJobState::Failed);
    assert_eq!(
        failed.error.as_ref().map(domain::ShortText::as_str),
        Some("an open pull request (#1) already covers this change")
    );
    assert_eq!(t.pulls().len(), 1, "the second job never reached GitHub");
    assert_eq!(t.made_branches().len(), 1);
    assert_eq!(t.raised_events().await.len(), 1);
}

// --------------------------------------------------------------------------------------------------------- resuming

/// Run `steps` steps of the job on one replica, then let that replica die (its hub, vault and runner are dropped), and finish the
/// job on another.
async fn crash_after(steps: usize) {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY, OTHER]).await;
    let id = id_of(&job);
    {
        let first = t.replica();
        for _ in 0..steps {
            assert!(matches!(first.advance(id).await, Advanced::Moved(_)));
        }
    }
    let before = t.state_of(id).await;
    assert!(
        !matches!(before.state, PrJobState::Completed | PrJobState::Failed),
        "{before:?}"
    );

    let second = t.replica();
    assert_eq!(
        second.drive(id).await,
        Driven::Finished(PrJobState::Completed),
        "after {steps} steps"
    );
    assert_one_pull_request(&t, id, 2).await;
    let states = t.progress(id).await;
    assert_eq!(
        states,
        vec![
            PrJobState::Pending,
            PrJobState::Preparing,
            PrJobState::Committing,
            PrJobState::Opening,
            PrJobState::Completed,
        ],
        "each state is entered once, whoever runs it"
    );
}

/// Run until the simulated GitHub has done what `matches` selects and its answer is lost, then kill the step in flight (the
/// process dies after GitHub did the work and before the job recorded it), and finish the job on another replica.
async fn crash_inside(steps_before: usize, matches: fn(&crate::support::fake_github::Seen) -> bool) {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    let id = id_of(&job);
    let first = t.replica();
    for _ in 0..steps_before {
        assert!(matches!(first.advance(id).await, Advanced::Moved(_)));
    }
    let fired = Arc::new(tokio::sync::Notify::new());
    let armed = Arc::new(AtomicBool::new(true));
    {
        let (fired, armed) = (fired.clone(), armed.clone());
        t.f.sim.inject_after(move |seen| {
            if matches(seen) && armed.swap(false, Ordering::SeqCst) {
                fired.notify_one();
                // The answer never comes in time: the work was done and the caller is gone.
                Some(Reply::json(200, "{}").after(Duration::from_secs(120)))
            } else {
                None
            }
        });
    }
    let running = tokio::spawn({
        let first = first.clone();
        async move { first.advance(id).await }
    });
    fired.notified().await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    drop(first);
    assert!(!armed.load(Ordering::SeqCst), "GitHub did the work");

    let second = t.replica();
    assert_eq!(second.drive(id).await, Driven::Finished(PrJobState::Completed));
    assert_one_pull_request(&t, id, 1).await;
}

#[tokio::test]
async fn a_job_resumes_after_a_crash_in_every_state() {
    // Between steps: the job is `pending`, `preparing`, `committing`, `opening` with the pull request not yet opened, and
    // `opening` with the pull request recorded and its body still without the audit id.
    for steps in 0..=4 {
        crash_after(steps).await;
    }
    // Inside a step, after GitHub did the work: the branch exists and the job does not know; the pull request exists and the job
    // does not know.
    crash_inside(2, |seen| {
        seen.method == "POST" && seen.path.ends_with("/git/refs")
    })
    .await;
    crash_inside(3, |seen| seen.method == "POST" && seen.path.ends_with("/pulls")).await;
}

#[tokio::test]
async fn a_replica_that_took_over_does_not_repeat_what_took_effect() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    let id = id_of(&job);
    let first = t.replica();
    for _ in 0..3 {
        first.advance(id).await;
    }
    // `opening`, with the branch made: the objects of the repository are what they are.
    let objects = t.f.sim.object_count(TENANT);
    let branch = t.made_branches();
    assert_eq!(branch.len(), 1);
    let second = t.replica();
    assert_eq!(second.drive(id).await, Driven::Finished(PrJobState::Completed));
    assert_eq!(
        t.f.sim.object_count(TENANT),
        objects,
        "the commit was not made again"
    );
    assert_eq!(t.made_branches(), branch);
}

#[tokio::test]
async fn the_scan_takes_up_pending_jobs_at_once_and_the_others_when_nobody_touched_them() {
    let Some(t) = jobs().await else { return };
    let pending = t.job("k-1", &[ENTRY]).await;
    let other = t.job("k-2", &[OTHER]).await;
    // One job was being run when its runner died: it is not pending, and it was touched a moment ago.
    {
        let dead = t.replica();
        for _ in 0..2 {
            dead.advance(id_of(&other)).await;
        }
    }
    let report = t.runner.run_once(&|| true).await;
    assert_eq!(report.driven, 1, "{report:?}");
    assert_eq!(report.finished, 1);
    assert_eq!(t.state_of(id_of(&pending)).await.state, PrJobState::Completed);
    assert_eq!(t.state_of(id_of(&other)).await.state, PrJobState::Committing);

    // A runner that is alive moves or retries its job well inside the grace period; past it, the job is nobody's.
    t.f.rig.advance_secs(31);
    let report = t.runner.run_once(&|| true).await;
    assert_eq!(report.driven, 1, "{report:?}");
    assert_eq!(t.state_of(id_of(&other)).await.state, PrJobState::Completed);
    assert_eq!(t.pulls().len(), 2);
    // Nothing is left to do.
    assert_eq!(t.runner.run_once(&|| true).await.driven, 0);

    // A scan that lost its lease stops.
    t.f.rig.agent.put(&t.f.s, &nfs(ENTRY), b"a: 1\nb: 30\n");
    t.f.rig.catch_up_registry(&t.f.s);
    let third = t.job("k-3", &[ENTRY]).await;
    assert_eq!(t.runner.run_once(&|| false).await.driven, 0);
    assert_eq!(t.state_of(id_of(&third)).await.state, PrJobState::Pending);
}

#[tokio::test]
async fn a_job_whose_author_was_disabled_meanwhile_fails_without_touching_github() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    disable(&t, &t.f.editor);
    assert_eq!(
        t.runner.drive(id_of(&job)).await,
        Driven::Finished(PrJobState::Failed)
    );
    let failed = t.state_of(id_of(&job)).await;
    assert_eq!(
        failed.error.as_ref().map(domain::ShortText::as_str),
        Some("you may no longer raise pull requests")
    );
    assert_eq!(
        t.f.server.requests(),
        0,
        "the token of a disabled user is not used"
    );
    assert_eq!(t.progress(id_of(&job)).await.last(), Some(&PrJobState::Failed));
}

#[tokio::test]
async fn a_job_without_a_token_fails_with_a_sentence_that_names_nothing() {
    let Some(t) = jobs().await else { return };
    let user = t.f.rig.user(2, Role::Editor).await;
    let Raised::Job(job) = t
        .runner
        .raise(&ctx(&user, "k-1"), t.f.request(&[ENTRY], None))
        .await
        .unwrap()
    else {
        panic!("accepted");
    };
    assert_eq!(
        t.runner.drive(id_of(&job)).await,
        Driven::Finished(PrJobState::Failed)
    );
    let failed = t.state_of(id_of(&job)).await;
    assert_eq!(
        failed.error.as_ref().map(domain::ShortText::as_str),
        Some("store a GitHub token first")
    );
    assert_eq!(failed.pr_url, None);
    assert_eq!(t.pulls().len(), 0);
    assert_eq!(t.raised_events().await.len(), 0, "no pull request, no event");
    assert_eq!(t.f.rig.count("pr_links").await, 0);
    // Terminal: running it again changes nothing.
    assert_eq!(
        t.runner.advance(id_of(&job)).await,
        Advanced::Finished(PrJobState::Failed)
    );
}

#[tokio::test]
async fn a_file_with_a_secret_fails_the_job_with_its_place_and_not_its_text() {
    let Some(t) = jobs().await else { return };
    let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
    t.f.rig
        .agent
        .put(&t.f.s, &nfs(ENTRY), format!("a: 1\nk: {key}\n").as_bytes());
    t.f.rig.catch_up_registry(&t.f.s);
    let job = t.job("k-1", &[ENTRY]).await;
    assert_eq!(
        t.runner.drive(id_of(&job)).await,
        Driven::Finished(PrJobState::Failed)
    );
    let failed = t.state_of(id_of(&job)).await;
    let error = failed.error.unwrap();
    assert_eq!(
        error.as_str(),
        "file 1 holds a value that looks like a secret (line 2)"
    );
    assert!(!error.as_str().contains(&key));
    assert_eq!(
        t.f.sim.branches(TENANT),
        vec!["sit1".to_owned()],
        "nothing was written to GitHub"
    );
}

#[tokio::test]
async fn github_that_does_not_answer_is_retried_a_few_times_and_then_the_job_fails() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    let id = id_of(&job);
    assert_eq!(t.runner.advance(id).await, Advanced::Moved(PrJobState::Preparing));
    let down = Arc::new(AtomicBool::new(true));
    {
        let down = down.clone();
        t.f.sim.inject(move |_| {
            down.load(Ordering::SeqCst)
                .then(|| Reply::json(503, r#"{"message":"unavailable"}"#))
        });
    }
    // Each try is counted and leaves the job where it is.
    for tries in 1..hub_writes::prs::runner::MAX_ATTEMPTS {
        assert_eq!(t.runner.advance(id).await, Advanced::Retry);
        let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM pr_jobs WHERE id = $1")
            .bind(id.get())
            .fetch_one(&t.f.rig.pool)
            .await
            .unwrap();
        assert_eq!(attempts, tries);
        assert_eq!(t.state_of(id).await.state, PrJobState::Preparing);
    }
    // GitHub comes back: the job goes on from where it was.
    down.store(false, Ordering::SeqCst);
    assert_eq!(t.runner.drive(id).await, Driven::Finished(PrJobState::Completed));

    // A job that never sees GitHub again fails when its tries are used.
    let job = t.job("k-2", &[OTHER]).await;
    let id = id_of(&job);
    assert_eq!(t.runner.advance(id).await, Advanced::Moved(PrJobState::Preparing));
    down.store(true, Ordering::SeqCst);
    for _ in 1..hub_writes::prs::runner::MAX_ATTEMPTS {
        assert_eq!(t.runner.advance(id).await, Advanced::Retry);
    }
    assert_eq!(t.runner.advance(id).await, Advanced::Finished(PrJobState::Failed));
    let failed = t.state_of(id).await;
    assert!(failed.error.unwrap().as_str().contains("did not answer"));
}

// ---------------------------------------------------------------------------------------------------------- idempotency

#[tokio::test]
async fn job_is_idempotent_through_its_id() {
    let Some(t) = jobs().await else { return };
    // The same key and the same request: the same job, and nothing is made twice.
    let first = t.job("k-1", &[ENTRY]).await;
    let replay = t.job("k-1", &[ENTRY]).await;
    assert_eq!(first.id, replay.id);
    assert_eq!(t.f.rig.count("pr_jobs").await, 1);
    // The same key and another request is refused.
    let changed = t
        .runner
        .raise(&t.f.ctx("k-1"), {
            let mut r = t.f.request(&[ENTRY], None);
            r.title = domain::ShortText::parse("another title").unwrap();
            r
        })
        .await;
    assert_eq!(changed, Err(WriteError::IdempotencyConflict));
    // Another key, the same request, while the job is unfinished: the job again, not a second one.
    let again = t.job("k-2", &[ENTRY]).await;
    assert_eq!(first.id, again.id);
    // The order of the paths is not part of the request.
    let both = t.job("k-3", &[OTHER, THIRD]).await;
    let swapped = t.job("k-4", &[THIRD, OTHER]).await;
    assert_eq!(both.id, swapped.id);
    assert_eq!(t.f.rig.count("pr_jobs").await, 2);

    // Two runners at once: the lease keeps one out, and it does not matter which wins.
    let other = t.replica();
    let id = id_of(&first);
    let (a, b) = tokio::join!(t.runner.drive(id), other.drive(id));
    assert!(
        matches!(
            (a, b),
            (Driven::Finished(PrJobState::Completed), _) | (_, Driven::Finished(PrJobState::Completed))
        ),
        "{a:?} {b:?}"
    );
    assert_eq!(t.pulls().len(), 1);

    // Two steps at once, with no lease: the state guard lets one of them through.
    let id = id_of(&both);
    for _ in 0..12 {
        let (a, b) = tokio::join!(t.runner.advance(id), other.advance(id));
        if matches!(a, Advanced::Finished(_)) && matches!(b, Advanced::Finished(_)) {
            break;
        }
    }
    assert_eq!(t.state_of(id).await.state, PrJobState::Completed);
    assert_eq!(t.pulls().len(), 2, "one pull request for each job");
    assert_eq!(t.made_branches().len(), 2);
    assert_eq!(t.raised_events().await.len(), 2);
    assert_eq!(t.f.rig.count("pr_links").await, 3, "a link for each file of each");

    // A finished job does nothing, however often it is run.
    let events = t.f.rig.chain().await.len();
    let objects = t.f.sim.object_count(TENANT);
    for _ in 0..3 {
        assert_eq!(
            t.runner.advance(id).await,
            Advanced::Finished(PrJobState::Completed)
        );
        assert_eq!(other.drive(id).await, Driven::Finished(PrJobState::Completed));
    }
    assert_eq!(t.f.rig.chain().await.len(), events);
    assert_eq!(t.f.sim.object_count(TENANT), objects);
    assert_eq!(t.runner.advance(PrJobId::from(9_999)).await, Advanced::Gone);

    // Progress was published once for each state a job entered.
    assert_eq!(
        t.progress(id).await,
        vec![
            PrJobState::Pending,
            PrJobState::Preparing,
            PrJobState::Committing,
            PrJobState::Opening,
            PrJobState::Completed
        ]
    );
}

// ------------------------------------------------------------------------------------------------------------ progress

#[tokio::test]
async fn pr_job_progress_is_published() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    let id = id_of(&job);
    assert_eq!(
        t.progress(id).await,
        vec![PrJobState::Pending],
        "accepting the job is the first move"
    );
    // One event for each durable move, in the transaction that makes it: after each step the outbox holds one more.
    let mut seen = 1;
    let mut last = Advanced::Moved(PrJobState::Pending);
    while !matches!(last, Advanced::Finished(_)) {
        last = t.runner.advance(id).await;
        let now = t.progress(id).await.len();
        match last {
            // The recording of the pull request and the putting of its audit id in the body stay inside `opening`.
            Advanced::Moved(PrJobState::Opening) => assert!(now == seen || now == seen + 1),
            Advanced::Moved(_) => assert_eq!(now, seen + 1),
            _ => {}
        }
        seen = now;
    }
    assert_eq!(
        t.progress(id).await,
        vec![
            PrJobState::Pending,
            PrJobState::Preparing,
            PrJobState::Committing,
            PrJobState::Opening,
            PrJobState::Completed
        ]
    );
    // A job that fails publishes the failure.
    let doomed = t.doomed_job(3, "k-2").await;
    t.runner.drive(doomed).await;
    assert_eq!(
        t.progress(doomed).await,
        vec![PrJobState::Pending, PrJobState::Preparing, PrJobState::Failed]
    );
    // The events carry the id and the state and nothing else.
    for e in t.f.rig.bus.published().await {
        if let DomainEvent::PrJobProgress { .. } = &e {
            let json = serde_json::to_value(&e).unwrap();
            assert_eq!(json.as_object().unwrap().len(), 3, "{json}");
        }
    }
}

impl Jobs {
    /// A job of a user who is disabled after it was accepted, so that it will fail.
    async fn doomed_job(&self, n: u32, key: &str) -> PrJobId {
        let user = self.f.rig.user(n, Role::Editor).await;
        let Raised::Job(view) = self
            .runner
            .raise(&ctx(&user, key), self.f.request(&[OTHER], None))
            .await
            .unwrap()
        else {
            panic!("accepted");
        };
        disable(self, &user);
        id_of(&view)
    }
}

// ------------------------------------------------------------------------------------------------------- state changes

/// What the pull-request webhook of prompt 03b does to a link, in one transaction: the state, and `PrStateChanged`.
async fn webhook_moves(t: &Jobs, bus: &PgTestBus, number: i32, to: PrState) -> Vec<DomainEvent> {
    let mut tx = t.f.rig.pool.begin().await.unwrap();
    let rows = sqlx::query("UPDATE pr_links SET state = $2 WHERE pr_number = $1 RETURNING id")
        .bind(number)
        .bind(to.as_str())
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let mut out = Vec::new();
    for row in rows {
        let link_id = domain::PrLinkId::from(row.get::<i64, _>("id"));
        let event = DomainEvent::PrStateChanged { link_id, state: to };
        bus.publish_in_tx(&mut ports::Tx::from_pg(&mut tx), event.clone())
            .await
            .unwrap();
        out.push(event);
    }
    tx.commit().await.unwrap();
    out
}

#[tokio::test]
async fn merged_pr_marks_fix_merged_awaiting_sync() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY, OTHER]).await;
    assert_eq!(
        t.runner.drive(id_of(&job)).await,
        Driven::Finished(PrJobState::Completed)
    );

    // Two replicas, each with the bus it listens on; the webhook lands on one of them.
    let bus_a = PgTestBus::listening(t.f.rig.pool.clone()).await;
    let bus_b = PgTestBus::listening(t.f.rig.pool.clone()).await;
    let a = hub_listening(&t, bus_a.clone()).pr_state_watcher();
    let b = hub_listening(&t, bus_b.clone()).pr_state_watcher();
    let mut drift = bus_b.subscribe(EventFilter {
        swimlanes: None,
        kinds: std::collections::BTreeSet::from([EventKind::DriftChanged]),
    });
    // Each follows its own bus; the subscriptions are made before the webhook commits, so that nothing is missed.
    let (events_a, events_b) = (a.subscribe(), b.subscribe());
    let tasks = [
        tokio::spawn(async move { a.follow(events_a).await }),
        tokio::spawn(async move { b.follow(events_b).await }),
    ];

    let moved = webhook_moves(&t, &bus_a, 1, PrState::Merged).await;
    assert_eq!(moved.len(), 2, "a link for each file");
    // The screens are told which paths to read again, for each link.
    let mut paths = Vec::new();
    for _ in 0..2 {
        let event = tokio::time::timeout(Duration::from_secs(10), drift.next())
            .await
            .expect("DriftChanged arrives")
            .unwrap();
        let DomainEvent::DriftChanged { swimlane, paths: p } = event else {
            panic!("{event:?}")
        };
        assert_eq!(swimlane, t.f.s);
        assert_eq!(p.len(), 1);
        paths.push(p[0].as_str().to_owned());
    }
    paths.sort();
    assert_eq!(paths, vec![ENTRY.to_owned(), OTHER.to_owned()]);
    for task in tasks {
        task.abort();
    }

    // What they read: the link of each file is merged, with the time of the merge. "Fix merged; awaiting sync" is that, for a file
    // whose NFS content is still not what Git has (plan A17).
    let page = t
        .runner
        .links(
            &t.f.editor,
            LinkQuery {
                swimlane: Some(t.f.s.clone()),
                state: Some(PrState::Merged),
                page: Page::new(10, None).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(
        page.items
            .iter()
            .all(|l| l.state == PrState::Merged && l.closed_at.is_some())
    );
    assert_eq!(page.next, None);
    let open = t
        .runner
        .links(
            &t.f.editor,
            LinkQuery {
                swimlane: None,
                state: Some(PrState::Open),
                page: Page::new(10, None).unwrap(),
            },
        )
        .await
        .unwrap();
    assert!(open.items.is_empty());
}

#[tokio::test]
async fn the_watcher_says_a_change_once_however_many_replicas_hear_it() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    t.runner.drive(id_of(&job)).await;
    let link: i64 = sqlx::query_scalar("SELECT id FROM pr_links")
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    let a: PrStateWatcher = t.f.rig.hub.pr_state_watcher();
    let b: PrStateWatcher = t.f.rig.replica().pr_state_watcher();
    let event = DomainEvent::PrStateChanged {
        link_id: domain::PrLinkId::from(link),
        state: PrState::Merged,
    };
    assert_eq!(a.on_event(&event).await, Watched::Announced);
    assert_eq!(b.on_event(&event).await, Watched::Duplicate);
    let drift =
        t.f.rig
            .bus
            .published()
            .await
            .into_iter()
            .filter(|e| matches!(e, DomainEvent::DriftChanged { .. }))
            .count();
    // One from the recording of the pull request, one from the merge.
    assert_eq!(drift, 2);
    // Another state of the same link is another change.
    let closed = DomainEvent::PrStateChanged {
        link_id: domain::PrLinkId::from(link),
        state: PrState::Closed,
    };
    assert_eq!(b.on_event(&closed).await, Watched::Announced);
    // Other events are not its business, and a link that is not there is not announced.
    assert_eq!(a.on_event(&DomainEvent::Resync).await, Watched::Ignored);
    assert_eq!(
        a.on_event(&DomainEvent::PrStateChanged {
            link_id: domain::PrLinkId::from(9_999),
            state: PrState::Merged
        })
        .await,
        Watched::Unknown
    );
}

// ------------------------------------------------------------------------------------------------------------ approval

#[tokio::test]
async fn a_user_who_requires_approval_gets_a_proposal_and_the_approval_makes_the_job() {
    let Some(t) = jobs().await else { return };
    let careful = t.f.rig.approval_user(5, Role::Editor).await;
    let admin = t.f.rig.user(6, Role::Admin).await;
    t.f.gh.token(&tok(5), "careful", &[BASE, TENANT]);
    t.f.sim.expect_token(&tok(5));
    t.f.vault
        .put(&ctx(&careful, "k-token"), Secret::new(tok(5)))
        .await
        .unwrap();

    let Raised::Proposal(proposal) = t
        .runner
        .raise(&ctx(&careful, "k-1"), t.f.request(&[ENTRY, OTHER], None))
        .await
        .unwrap()
    else {
        panic!("a proposal");
    };
    assert_eq!(
        t.f.rig.count("pr_jobs").await,
        0,
        "nothing is made before an admin approves"
    );
    assert_eq!(t.f.server.requests(), 0);
    assert_eq!(
        t.f.rig
            .hub
            .raise_pr(&ctx(&careful, "k-1"), t.f.request(&[ENTRY, OTHER], None))
            .await,
        Ok(WriteOutcome::ProposalCreated { id: proposal }),
        "the same key, the same proposal"
    );

    // An editor does not decide.
    assert_eq!(
        t.f.rig
            .hub
            .approve_proposal(&ctx(&careful, "k-self"), proposal, None)
            .await,
        Err(WriteError::Forbidden)
    );
    let done =
        t.f.rig
            .hub
            .approve_proposal(&ctx(&admin, "k-ok"), proposal, None)
            .await
            .unwrap();
    assert!(
        matches!(done, WriteOutcome::Applied { hash: None, .. }),
        "{done:?}"
    );
    assert_eq!(t.f.rig.count("pr_jobs").await, 1);
    let author: String = sqlx::query_scalar("SELECT author_oid FROM pr_jobs")
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    assert_eq!(
        author,
        careful.id.oid().as_str(),
        "the pull request is the proposer's own"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM proposals WHERE id = $1")
        .bind(proposal.get())
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    assert_eq!(status, "approved", "applied when the pull request is open");

    // The scan runs the job, with the proposer's token.
    let report = t.runner.run_once(&|| true).await;
    assert_eq!((report.driven, report.finished), (1, 1), "{report:?}");
    let status: String = sqlx::query_scalar("SELECT status FROM proposals WHERE id = $1")
        .bind(proposal.get())
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    assert_eq!(status, "applied");
    assert_eq!(t.pulls().len(), 1);
    assert!(t.pulls()[0].body.contains(&careful.id.to_string()));
    let page =
        t.f.rig
            .hub
            .list_proposals(
                &admin,
                hub_writes::ProposalQuery {
                    status: None,
                    swimlane: None,
                    page: Page::new(10, None).unwrap(),
                },
            )
            .await
            .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].status, ProposalStatus::Applied);

    // The audit trail: the proposal, the approval (the admin acting, the proposer as author) and the pull request.
    let chain = t.f.rig.chain().await;
    let actions: Vec<_> = chain.iter().map(|e| e.event.action).collect();
    assert!(
        actions.ends_with(&[
            AuditAction::GithubTokenStored,
            AuditAction::ProposalCreated,
            AuditAction::ProposalApproved,
            AuditAction::PrRaised
        ]),
        "{actions:?}"
    );
    let approved = &chain[chain.len() - 2].event;
    assert_eq!(
        approved.actor,
        AuditActor::User {
            user: admin.id.clone()
        }
    );
    assert_eq!(approved.author.as_ref(), Some(&careful.id));
    assert_eq!(approved.approval, Some(proposal));
    let raised = &chain[chain.len() - 1].event;
    assert_eq!(
        raised.actor,
        AuditActor::User {
            user: careful.id.clone()
        }
    );
    assert_eq!(raised.approval, Some(proposal));
    assert_eq!(
        raised.github_login.as_ref().map(domain::ShortText::as_str),
        Some("careful")
    );
    assert!(verify_chain(&chain).is_ok());
}

#[tokio::test]
async fn approval_of_a_pull_request_rechecks_the_files_and_the_one_open_rule() {
    let Some(t) = jobs().await else { return };
    let careful = t.f.rig.approval_user(5, Role::Editor).await;
    let admin = t.f.rig.user(6, Role::Admin).await;
    let Raised::Proposal(stale) = t
        .runner
        .raise(&ctx(&careful, "k-1"), t.f.request(&[ENTRY], None))
        .await
        .unwrap()
    else {
        panic!("a proposal");
    };
    // The file changes before an admin looks: the pull request would carry something nobody approved.
    t.f.rig.agent.put(&t.f.s, &nfs(ENTRY), b"a: 1\nb: 99\n");
    t.f.rig.catch_up_registry(&t.f.s);
    let out =
        t.f.rig
            .hub
            .approve_proposal(&ctx(&admin, "k-a"), stale, None)
            .await
            .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Conflict {
            current: Some(h("a: 1\nb: 99\n"))
        }
    );
    assert_eq!(t.f.rig.count("pr_jobs").await, 0);
    let status: String = sqlx::query_scalar("SELECT status FROM proposals WHERE id = $1")
        .bind(stale.get())
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    assert_eq!(status, "stale");

    // A proposal for a change that has a pull request by now stays pending, and nothing is made.
    t.f.rig.agent.put(&t.f.s, &nfs(ENTRY), ENTRY_NFS.as_bytes());
    t.f.rig.catch_up_registry(&t.f.s);
    let Raised::Proposal(second) = t
        .runner
        .raise(&ctx(&careful, "k-2"), t.f.request(&[ENTRY], None))
        .await
        .unwrap()
    else {
        panic!("a proposal");
    };
    let job = t.job("k-3", &[ENTRY]).await;
    t.runner.drive(id_of(&job)).await;
    let out =
        t.f.rig
            .hub
            .approve_proposal(&ctx(&admin, "k-b"), second, None)
            .await
            .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Rejected {
            reason: RejectReason::NotAllowed,
            issues: Vec::new()
        }
    );
    assert_eq!(t.f.rig.count("pr_jobs").await, 1, "only the editor's job");
    // The same change is also refused when it is asked for.
    assert!(matches!(
        t.runner
            .raise(&ctx(&careful, "k-4"), t.f.request(&[ENTRY], None))
            .await
            .unwrap(),
        Raised::Duplicate(_)
    ));
}

/// Two proposals for the same files, both approved before the first job has opened its pull request: no link exists yet, so the
/// open-pull-request check alone lets the second approval make a second job and a second pull request.
#[tokio::test]
async fn an_approval_for_files_an_unfinished_job_already_covers_makes_no_second_job() {
    let Some(t) = jobs().await else { return };
    let careful = t.f.rig.approval_user(5, Role::Editor).await;
    let admin = t.f.rig.user(6, Role::Admin).await;
    t.f.gh.token(&tok(5), "careful", &[BASE, TENANT]);
    t.f.sim.expect_token(&tok(5));
    t.f.vault
        .put(&ctx(&careful, "k-token"), Secret::new(tok(5)))
        .await
        .unwrap();
    let mut proposals = Vec::new();
    for key in ["k-1", "k-2"] {
        let Raised::Proposal(id) = t
            .runner
            .raise(&ctx(&careful, key), t.f.request(&[ENTRY, OTHER], None))
            .await
            .unwrap()
        else {
            panic!("a proposal");
        };
        proposals.push(id);
    }
    assert_ne!(proposals[0], proposals[1]);

    let first =
        t.f.rig
            .hub
            .approve_proposal(&ctx(&admin, "k-a1"), proposals[0], None)
            .await
            .unwrap();
    assert!(matches!(first, WriteOutcome::Applied { .. }), "{first:?}");
    // The first job has not run: there is no pull request and no link.
    assert_eq!(t.f.rig.count("pr_jobs").await, 1);
    assert_eq!(t.f.rig.count("pr_links").await, 0);

    let second =
        t.f.rig
            .hub
            .approve_proposal(&ctx(&admin, "k-a2"), proposals[1], None)
            .await
            .unwrap();
    assert!(
        matches!(second, WriteOutcome::Locked { .. }),
        "the second approval waits for the first job: {second:?}"
    );
    assert_eq!(t.f.rig.count("pr_jobs").await, 1, "no second job");
    let status: String = sqlx::query_scalar("SELECT status FROM proposals WHERE id = $1")
        .bind(proposals[1].get())
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    assert_eq!(status, "pending", "the proposal is still there to decide");

    // The first job runs and opens its pull request; the second proposal, decided now, is a duplicate of an open pull request.
    let report = t.runner.run_once(&|| true).await;
    assert_eq!((report.driven, report.finished), (1, 1), "{report:?}");
    assert_eq!(t.pulls().len(), 1);
    let again =
        t.f.rig
            .hub
            .approve_proposal(&ctx(&admin, "k-a2"), proposals[1], None)
            .await
            .unwrap();
    assert_eq!(
        again,
        WriteOutcome::Rejected {
            reason: RejectReason::NotAllowed,
            issues: Vec::new()
        }
    );
    assert_eq!(t.pulls().len(), 1, "one pull request for one change");
}

/// The admin approved the files at the hashes the author saw. The job reads them again when it runs; a file that has another hash
/// by then is not what was approved, and the job must not put it in a pull request.
#[tokio::test]
async fn a_job_does_not_carry_a_file_that_changed_after_it_was_approved() {
    let Some(t) = jobs().await else { return };
    let careful = t.f.rig.approval_user(5, Role::Editor).await;
    let admin = t.f.rig.user(6, Role::Admin).await;
    t.f.gh.token(&tok(5), "careful", &[BASE, TENANT]);
    t.f.sim.expect_token(&tok(5));
    t.f.vault
        .put(&ctx(&careful, "k-token"), Secret::new(tok(5)))
        .await
        .unwrap();
    let Raised::Proposal(proposal) = t
        .runner
        .raise(&ctx(&careful, "k-1"), t.f.request(&[ENTRY, OTHER], None))
        .await
        .unwrap()
    else {
        panic!("a proposal");
    };
    let done =
        t.f.rig
            .hub
            .approve_proposal(&ctx(&admin, "k-ok"), proposal, None)
            .await
            .unwrap();
    assert!(matches!(done, WriteOutcome::Applied { .. }), "{done:?}");

    // Somebody changes a file between the approval and the job's first look at it.
    t.f.rig.agent.put(&t.f.s, &nfs(ENTRY), b"a: 1\nb: 99\n");
    t.f.rig.catch_up_registry(&t.f.s);
    let report = t.runner.run_once(&|| true).await;
    assert_eq!(report.driven, 1, "{report:?}");

    let id: i64 = sqlx::query_scalar("SELECT id FROM pr_jobs")
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    let view = t.state_of(PrJobId::from(id)).await;
    assert_eq!(view.state, PrJobState::Failed, "{view:?}");
    let error = view
        .error
        .as_ref()
        .map(domain::ShortText::as_str)
        .unwrap_or_default();
    assert!(error.contains("changed after"), "{error}");
    assert_eq!(
        t.pulls().len(),
        0,
        "nothing the admin did not approve reaches GitHub"
    );
    assert_eq!(t.made_branches().len(), 0);
    let status: String = sqlx::query_scalar("SELECT status FROM proposals WHERE id = $1")
        .bind(proposal.get())
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    assert_eq!(status, "approved", "not applied: no pull request was made");
}

// -------------------------------------------------------------------------------------------------------- the trait

#[tokio::test]
async fn raise_pr_through_the_write_service_waits_for_the_pull_request() {
    let Some(t) = jobs().await else { return };
    let out =
        t.f.rig
            .hub
            .raise_pr(&t.f.ctx("k-1"), t.f.request(&[ENTRY], None))
            .await
            .unwrap();
    let WriteOutcome::Applied {
        hash: None, audit_id, ..
    } = out
    else {
        panic!("{out:?}");
    };
    let events = t.raised_events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, audit_id);
    assert_eq!(t.pulls().len(), 1);
    // The same key again: the same answer, nothing new.
    assert_eq!(
        t.f.rig
            .hub
            .raise_pr(&t.f.ctx("k-1"), t.f.request(&[ENTRY], None))
            .await,
        Ok(out)
    );
    assert_eq!(t.pulls().len(), 1);
    // A job that fails is a rejection: this editor has no token.
    let tokenless = t.f.rig.user(2, Role::Editor).await;
    assert_eq!(
        t.f.rig
            .hub
            .raise_pr(&ctx(&tokenless, "k-2"), t.f.request(&[OTHER], None))
            .await,
        Ok(WriteOutcome::Rejected {
            reason: RejectReason::NotAllowed,
            issues: Vec::new()
        })
    );
}

#[tokio::test]
async fn nobody_below_editor_raises_a_pull_request_and_nothing_is_claimed() {
    let Some(t) = jobs().await else { return };
    let viewer = t.f.rig.user(2, Role::Viewer).await;
    let pending =
        t.f.rig
            .user_with_status(3, Some(Role::Editor), UserStatus::Pending)
            .await;
    let disabled =
        t.f.rig
            .user_with_status(4, Some(Role::Editor), UserStatus::Disabled)
            .await;
    let none = t.f.rig.user_with_status(5, None, UserStatus::Active).await;
    for who in [&viewer, &pending, &disabled, &none] {
        assert_eq!(
            t.runner
                .raise(&ctx(who, "k-x"), t.f.request(&[ENTRY], None))
                .await,
            Err(WriteError::Forbidden)
        );
        assert_eq!(
            t.f.rig
                .hub
                .raise_pr(&ctx(who, "k-y"), t.f.request(&[ENTRY], None))
                .await,
            Err(WriteError::Forbidden)
        );
    }
    assert_eq!(t.f.rig.count("pr_jobs").await, 0);
    assert_eq!(t.f.rig.count("idempotency_keys").await, t.keys);
    assert_eq!(t.f.rig.count("proposals").await, 0);
    assert_eq!(t.f.server.requests(), 0);
}

#[tokio::test]
async fn requests_that_cannot_become_a_pull_request_are_refused_and_keep_no_key() {
    let Some(t) = jobs().await else { return };
    let ctx_1 = t.f.ctx("k-1");
    // A swimlane that does not exist.
    let mut r = t.f.request(&[ENTRY], None);
    r.swimlane = domain::SwimlaneId::parse("nowhere").unwrap();
    assert_eq!(
        t.runner.raise(&ctx_1, r).await,
        Ok(Raised::Refused(WriteOutcome::Rejected {
            reason: RejectReason::UnknownSwimlane,
            issues: Vec::new()
        }))
    );
    // A path that is not there.
    assert_eq!(
        t.runner
            .raise(&ctx_1, t.f.request(&["tx-api/missing-sit1.yml"], None))
            .await,
        Ok(Raised::Refused(WriteOutcome::Rejected {
            reason: RejectReason::InvalidContent,
            issues: Vec::new()
        }))
    );
    // A tenant branch the swimlane does not deploy.
    let elsewhere = domain::PrTarget::Tenant {
        branch: domain::TenantId::parse("sit9").unwrap(),
    };
    assert_eq!(
        t.runner
            .raise(&ctx_1, t.f.request(&[ENTRY], Some(elsewhere)))
            .await,
        Ok(Raised::Refused(WriteOutcome::Rejected {
            reason: RejectReason::InvalidContent,
            issues: Vec::new()
        }))
    );
    assert_eq!(t.f.rig.count("pr_jobs").await, 0);
    assert_eq!(
        t.f.rig.count("idempotency_keys").await,
        t.keys,
        "a refusal is not an outcome of the key"
    );
    // The key is free for a request that can be made.
    assert!(matches!(t.raise("k-1", &[ENTRY]).await, Raised::Job(_)));
}

#[tokio::test]
async fn the_api_shows_jobs_and_links_to_viewers_and_nothing_to_nobody_else() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY, OTHER]).await;
    t.runner.drive(id_of(&job)).await;
    let viewer = t.f.rig.user(2, Role::Viewer).await;
    let seen = t.runner.job(&viewer, id_of(&job)).await.unwrap().unwrap();
    assert_eq!(seen.state, PrJobState::Completed);
    assert_eq!(seen.paths, vec![nfs(ENTRY), nfs(OTHER)], "sorted");
    assert_eq!(seen.warnings.len(), 0);
    assert_eq!(t.runner.job(&viewer, PrJobId::from(9_999)).await, Ok(None));
    let disabled =
        t.f.rig
            .user_with_status(3, Some(Role::Viewer), UserStatus::Disabled)
            .await;
    assert_eq!(
        t.runner.job(&disabled, id_of(&job)).await,
        Err(WriteError::Forbidden)
    );
    assert_eq!(
        t.runner
            .links(
                &disabled,
                LinkQuery {
                    swimlane: None,
                    state: None,
                    page: Page::new(5, None).unwrap()
                }
            )
            .await,
        Err(WriteError::Forbidden)
    );

    // Pages are keyset pages, newest first.
    let first = t
        .runner
        .links(
            &viewer,
            LinkQuery {
                swimlane: None,
                state: None,
                page: Page::new(1, None).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let next = first.next.clone().unwrap();
    let second = t
        .runner
        .links(
            &viewer,
            LinkQuery {
                swimlane: None,
                state: None,
                page: Page::new(1, Some(next)).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.next, None);
    assert!(first.items[0].id > second.items[0].id);
}

// ---------------------------------------------------------------------------------------------------------------- audit

#[tokio::test]
async fn the_pull_request_has_exactly_one_chained_audit_event_with_its_address() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY, OTHER]).await;
    t.runner.drive(id_of(&job)).await;
    let chain = t.f.rig.chain().await;
    let events: Vec<_> = chain
        .iter()
        .filter(|e| e.event.action == AuditAction::PrRaised)
        .collect();
    assert_eq!(events.len(), 1);
    let e = &events[0].event;
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: t.f.editor.id.clone()
        }
    );
    assert_eq!(e.swimlane.as_ref(), Some(&t.f.s));
    assert_eq!(
        e.github_login.as_ref().map(domain::ShortText::as_str),
        Some("octocat")
    );
    let diff = e.diff.as_deref().unwrap();
    assert!(diff.contains("pull request #1 https://"), "{diff}");
    assert!(diff.contains(&h(ENTRY_NFS).to_string()), "the hashes: {diff}");
    assert!(!diff.contains("b: 20"), "never the content: {diff}");
    assert!(verify_chain(&chain).is_ok());
    // Accepting the job wrote nothing.
    assert_eq!(
        chain
            .iter()
            .filter(|e| e.event.action != AuditAction::GithubTokenStored)
            .count(),
        1
    );
}

#[tokio::test]
async fn an_audit_failure_rolls_back_the_links_and_the_job_does_not_record() {
    let Some(t) = jobs().await else { return };
    let job = t.job("k-1", &[ENTRY]).await;
    let id = id_of(&job);
    for _ in 0..3 {
        t.runner.advance(id).await;
    }
    t.f.rig.audit.fail_next();
    // The pull request is opened, and recording it fails: no link, no event, and the job stays where it is for the next try.
    assert_eq!(t.runner.advance(id).await, Advanced::Unavailable);
    assert_eq!(t.f.rig.count("pr_links").await, 0);
    assert_eq!(t.raised_events().await.len(), 0);
    assert_eq!(t.state_of(id).await.state, PrJobState::Opening);
    assert_eq!(t.pulls().len(), 1);
    // The next try finds the pull request and records it.
    assert_eq!(t.runner.drive(id).await, Driven::Finished(PrJobState::Completed));
    assert_one_pull_request(&t, id, 1).await;
}

#[tokio::test]
async fn nothing_secret_or_from_a_file_is_logged_by_a_job() {
    let Some(t) = jobs().await else { return };
    let (capture, _guard) = log_capture::install();
    let job = t.job("k-1", &[ENTRY]).await;
    t.runner.drive(id_of(&job)).await;
    let doomed = t.doomed_job(3, "k-2").await;
    t.runner.drive(doomed).await;
    let logged = capture.text();
    for needle in [tok(1).as_str(), "b: 20", "o: 2", "Please review"] {
        assert!(!logged.contains(needle), "the log holds {needle}");
    }
}

#[tokio::test]
async fn a_pull_request_keeps_the_versions_it_links_and_the_files_it_committed() {
    let Some(t) = jobs().await else { return };
    // The registry stores the versions it observes (05); here the blob store is told about one of the two files.
    let kept =
        t.f.rig
            .blobs
            .put(Bytes::from_static(ENTRY_NFS.as_bytes()))
            .await
            .unwrap();
    assert_eq!(kept, h(ENTRY_NFS));
    let job = t.job("k-1", &[ENTRY, OTHER]).await;
    assert_eq!(
        t.runner.drive(id_of(&job)).await,
        Driven::Finished(PrJobState::Completed)
    );

    // The version a link came from is kept for as long as the link lives (D78): the one the store has, referenced by that link.
    let mut refs: Vec<String> =
        sqlx::query_scalar("SELECT ref_id FROM blob_refs WHERE ref_kind = 'pr_link' AND hash = $1")
            .bind(kept.as_bytes().to_vec())
            .fetch_all(&t.f.rig.pool)
            .await
            .unwrap();
    let link: i64 = sqlx::query_scalar("SELECT id FROM pr_links WHERE path = $1")
        .bind(ENTRY)
        .fetch_one(&t.f.rig.pool)
        .await
        .unwrap();
    // (This file is LF on NFS and in the repository, so the bytes the job committed are the same blob, kept under the job as well.)
    refs.sort();
    assert_eq!(refs, vec![link.to_string(), format!("job:{}", job.id)]);
    // And what the job committed is kept under the job, one blob for each file.
    let committed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blob_refs WHERE ref_kind = 'pr_link' AND ref_id = $1")
            .bind(format!("job:{}", job.id))
            .fetch_one(&t.f.rig.pool)
            .await
            .unwrap();
    assert_eq!(committed, 2);
}
