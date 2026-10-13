//! T7 (P2, D64): a push reaches `GitHeadMoved` on every replica, and every replica's mirror has the commit. Three hub replicas
//! share one Postgres; each has its own mirror on its own disk, its own bus and its own webhook endpoint. The poll that backs the
//! webhook up is tested on a paused clock against the `ports` fake leases (its schedule) and on a real remote (its effect).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use domain::{CommitId, DomainEvent, RepoKind};
use futures::stream::BoxStream;
use hub_platform::git::fetch_queue::FetchQueue;
use hub_platform::git::poll::{GitPoll, POLL_INTERVAL, worst_case_latency};
use hub_platform::git::sync::{FetchWorker, RepoSync, RetrySettings, SyncError};
use hub_platform::jobs::{JobMetrics, JobRunner};
use ports::fakes::FakeLeases;
use ports::{BusError, EventBus, EventFilter, Tx};
use support::git_cluster::{GitCluster, eventually, next_head_moved};

fn commit(s: &str) -> CommitId {
    CommitId::parse(s).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn push_reaches_git_head_moved_within_10s_on_every_replica() {
    let Some(mut cluster) = GitCluster::start(3).await else {
        return;
    };
    let before = cluster.tenant.head("sit1").unwrap();
    let after = cluster
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 2\n"))], "push");

    let started = std::time::Instant::now();
    let status = cluster
        .deliver_push(0, RepoKind::Tenant, "sit1", &before, &after, "delivery-1")
        .await;
    assert_eq!(status, http::StatusCode::ACCEPTED);

    for replica in &mut cluster.replicas {
        let (repo, branch, from, to) = next_head_moved(&mut replica.events, Duration::from_secs(10))
            .await
            .unwrap_or_else(|| panic!("{} saw no GitHeadMoved", replica.name));
        assert_eq!(repo, RepoKind::Tenant);
        assert_eq!(branch.as_str(), "sit1");
        assert_eq!(from, Some(commit(&before)), "`from` is the head before the fetch");
        assert_eq!(to, commit(&after));
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );

    // Every replica ends up with the commit in its own mirror: the receiver fetched, and the others fetched on the event.
    let target = commit(&after);
    for replica in &cluster.replicas {
        let mirror = Arc::clone(&replica.mirror);
        let target = target.clone();
        eventually(
            &format!("{} to have the commit", replica.name),
            Duration::from_secs(10),
            || mirror.has_commit(RepoKind::Tenant, &target),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_replica_that_took_the_webhook_announces_and_the_others_do_not_repeat_it() {
    let Some(mut cluster) = GitCluster::start(3).await else {
        return;
    };
    let before = cluster.tenant.head("sit1").unwrap();
    let after = cluster
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 2\n"))], "push");
    cluster
        .deliver_push(1, RepoKind::Tenant, "sit1", &before, &after, "delivery-1")
        .await;

    for replica in &mut cluster.replicas {
        assert!(
            next_head_moved(&mut replica.events, Duration::from_secs(10))
                .await
                .is_some()
        );
    }
    // The others fetch because of the announcement. If they announced too, a second event would arrive.
    for replica in &cluster.replicas {
        let mirror = Arc::clone(&replica.mirror);
        let target = commit(&after);
        eventually("the commit in every mirror", Duration::from_secs(10), || {
            mirror.has_commit(RepoKind::Tenant, &target)
        })
        .await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    for replica in &mut cluster.replicas {
        assert!(
            next_head_moved(&mut replica.events, Duration::from_millis(300))
                .await
                .is_none(),
            "{} saw a second announcement",
            replica.name
        );
    }
    let announced: u64 = cluster.replicas.iter().map(|r| r.metrics.announced_total()).sum();
    assert_eq!(announced, 1, "one announcement in the whole cluster");
    assert_eq!(cluster.replicas[1].metrics.announced_total(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_pushes_to_several_branches_announces_each_moved_branch_once() {
    let Some(mut cluster) = GitCluster::start(2).await else {
        return;
    };
    let mut expected = Vec::new();
    for n in 0..5 {
        let branch = format!("sit{}", n + 2);
        let commit_id = cluster.tenant.commit(
            &branch,
            &[("data/config/x.yml", Some(format!("n: {n}\n").as_bytes()))],
            "new",
        );
        expected.push((branch, commit_id));
    }
    for (n, (branch, commit_id)) in expected.iter().enumerate() {
        let status = cluster
            .deliver_push(
                0,
                RepoKind::Tenant,
                branch,
                &"0".repeat(40),
                commit_id,
                &format!("burst-{n}"),
            )
            .await;
        assert_eq!(status, http::StatusCode::ACCEPTED);
    }
    let mut seen = std::collections::BTreeMap::new();
    for replica in &mut cluster.replicas {
        let mut here = std::collections::BTreeMap::new();
        while here.len() < expected.len() {
            let (_, branch, from, to) = next_head_moved(&mut replica.events, Duration::from_secs(10))
                .await
                .unwrap_or_else(|| panic!("{} missed a branch: {here:?}", replica.name));
            assert_eq!(from, None, "a new branch has no previous head");
            assert!(
                here.insert(branch.as_str().to_owned(), to.as_str().to_owned())
                    .is_none(),
                "announced twice"
            );
        }
        seen = here;
    }
    for (branch, commit_id) in expected {
        assert_eq!(seen[&branch], commit_id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn syncing_when_nothing_moved_announces_nothing() {
    let Some(mut cluster) = GitCluster::start(1).await else {
        return;
    };
    let replica = &mut cluster.replicas[0];
    replica.syncer.sync(RepoKind::Tenant).await.unwrap();
    replica.syncer.sync(RepoKind::Base).await.unwrap();
    assert!(
        next_head_moved(&mut replica.events, Duration::from_millis(500))
            .await
            .is_none()
    );
    assert_eq!(replica.metrics.announced_total(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_branch_is_not_announced_and_a_recreated_one_has_no_previous_head() {
    let Some(mut cluster) = GitCluster::start(1).await else {
        return;
    };
    cluster
        .tenant
        .commit("sit2", &[("data/config/x.yml", Some(b"x\n"))], "sit2");
    cluster.replicas[0].syncer.sync(RepoKind::Tenant).await.unwrap();
    assert!(
        next_head_moved(&mut cluster.replicas[0].events, Duration::from_secs(5))
            .await
            .is_some()
    );

    cluster.tenant.delete_branch("sit2");
    cluster.replicas[0].syncer.sync(RepoKind::Tenant).await.unwrap();
    assert!(
        next_head_moved(&mut cluster.replicas[0].events, Duration::from_millis(400))
            .await
            .is_none(),
        "a deletion announces nothing"
    );

    let again = cluster
        .tenant
        .commit("sit2", &[("data/config/y.yml", Some(b"y\n"))], "sit2 again");
    cluster.replicas[0].syncer.sync(RepoKind::Tenant).await.unwrap();
    let (_, branch, from, to) = next_head_moved(&mut cluster.replicas[0].events, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(branch.as_str(), "sit2");
    assert_eq!(from, None, "the old head was forgotten with the branch");
    assert_eq!(to, commit(&again));
}

/// A bus whose `publish` fails while a flag is set.
struct FlakyBus {
    inner: Arc<dyn EventBus>,
    failing: AtomicBool,
}

#[async_trait]
impl EventBus for FlakyBus {
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError> {
        if self.failing.load(Ordering::SeqCst) {
            return Err(BusError::Unavailable);
        }
        self.inner.publish(e).await
    }

    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        self.inner.publish_in_tx(tx, e).await
    }

    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent> {
        self.inner.subscribe(f)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_announcement_that_could_not_be_published_is_made_by_the_next_sync() {
    use hub_platform::git::metrics::GitMetrics;
    use hub_platform::git::sync::Announcer;
    let Some(mut cluster) = GitCluster::start(1).await else {
        return;
    };
    let flaky = Arc::new(FlakyBus {
        inner: Arc::clone(&cluster.replicas[0].bus),
        failing: AtomicBool::new(false),
    });
    let announcer = Announcer::start(
        Arc::clone(&cluster.replicas[0].mirror),
        flaky.clone(),
        GitMetrics::default(),
    )
    .await;
    let moved = cluster
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 9\n"))], "moved");
    cluster.replicas[0].mirror.fetch(RepoKind::Tenant).await.unwrap();

    flaky.failing.store(true, Ordering::SeqCst);
    assert_eq!(
        announcer.announce(RepoKind::Tenant).await.unwrap(),
        0,
        "the publish failed, so nothing was announced"
    );
    assert!(
        next_head_moved(&mut cluster.replicas[0].events, Duration::from_millis(300))
            .await
            .is_none()
    );

    flaky.failing.store(false, Ordering::SeqCst);
    assert_eq!(
        announcer.announce(RepoKind::Tenant).await.unwrap(),
        1,
        "the next sync still finds the move"
    );
    let (_, _, _, to) = next_head_moved(&mut cluster.replicas[0].events, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(to, commit(&moved));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resync_makes_a_replica_fetch_everything_it_may_have_missed() {
    let Some(cluster) = GitCluster::start(2).await else {
        return;
    };
    // Replica 0 never hears about this push: it is made on the remote and nobody delivers a webhook.
    let moved = commit(&cluster.tenant.commit(
        "sit1",
        &[("data/config/a-sit1.yml", Some(b"a: 5\n"))],
        "silent",
    ));
    assert!(!cluster.replicas[0].mirror.has_commit(RepoKind::Tenant, &moved));
    // Any replica's lag becomes a Resync for its subscribers; publish one for the whole cluster.
    cluster.replicas[1]
        .bus
        .publish(DomainEvent::Resync)
        .await
        .unwrap();
    let mirror = Arc::clone(&cluster.replicas[0].mirror);
    eventually(
        "replica 0 to fetch after the resync",
        Duration::from_secs(10),
        || mirror.has_commit(RepoKind::Tenant, &moved),
    )
    .await;
}

// ------------------------------------------------------------------------------------------------ the worker

/// A syncer that fails a set number of times, then works, and counts calls.
struct Scripted {
    fail_first: u32,
    calls: AtomicU32,
}

#[async_trait]
impl RepoSync for Scripted {
    async fn sync(&self, _repo: RepoKind) -> Result<(), SyncError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_first {
            Err(SyncError::Read)
        } else {
            Ok(())
        }
    }
}

#[tokio::test(start_paused = true)]
async fn the_worker_retries_a_failed_fetch_a_bounded_number_of_times_with_jitter_and_then_leaves_it_to_the_poll()
 {
    let queue = FetchQueue::new(8);
    let retry = RetrySettings {
        attempts: 3,
        base: Duration::from_millis(500),
        cap: Duration::from_secs(5),
    };
    let (stop, closing) = tokio::sync::watch::channel(false);

    // Fails twice, works on the third attempt.
    let flaky = Arc::new(Scripted {
        fail_first: 2,
        calls: AtomicU32::new(0),
    });
    let worker = FetchWorker::new(Arc::clone(&queue), flaky.clone(), retry).spawn(closing.clone());
    queue.push(
        RepoKind::Tenant,
        hub_platform::git::payload::PushedRef::Branch(domain::GitRef::parse("sit1").unwrap()),
    );
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(flaky.calls.load(Ordering::SeqCst), 3);
    worker.abort();

    // Always fails: three attempts and no more, however long it waits.
    let broken = Arc::new(Scripted {
        fail_first: u32::MAX,
        calls: AtomicU32::new(0),
    });
    let worker = FetchWorker::new(Arc::clone(&queue), broken.clone(), retry).spawn(closing);
    queue.push(
        RepoKind::Tenant,
        hub_platform::git::payload::PushedRef::Branch(domain::GitRef::parse("sit1").unwrap()),
    );
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(
        broken.calls.load(Ordering::SeqCst),
        3,
        "bounded: the poll takes over"
    );
    drop(stop);
    worker.abort();
}

// -------------------------------------------------------------------------------------------------- the poll

/// A remote that has moved when `moved` is set; a sync announces it, once.
struct FakeRemoteSync {
    moved: AtomicBool,
    announced_at: Mutex<Option<tokio::time::Instant>>,
    polls: AtomicU32,
}

#[async_trait]
impl RepoSync for FakeRemoteSync {
    async fn sync(&self, repo: RepoKind) -> Result<(), SyncError> {
        if repo == RepoKind::Tenant {
            self.polls.fetch_add(1, Ordering::SeqCst);
            if self.moved.load(Ordering::SeqCst) {
                self.announced_at
                    .lock()
                    .unwrap()
                    .get_or_insert_with(tokio::time::Instant::now);
            }
        }
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn poll_fallback_reaches_git_head_moved_within_75s() {
    let leases = FakeLeases::new();
    let remote = Arc::new(FakeRemoteSync {
        moved: AtomicBool::new(false),
        announced_at: Mutex::new(None),
        polls: AtomicU32::new(0),
    });
    // Two replicas race for the lease; only the holder polls.
    let mut handles = Vec::new();
    for _ in 0..2 {
        let mut runner = JobRunner::new(Arc::new(leases.clone()), JobMetrics::default());
        runner
            .register(GitPoll::spec().unwrap(), Arc::new(GitPoll::new(remote.clone())))
            .unwrap();
        handles.push(runner.start());
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    let polls_so_far = remote.polls.load(Ordering::SeqCst);
    assert_eq!(
        polls_so_far, 1,
        "the new holder polls at once, and only one replica holds the lease"
    );

    // A push the webhook never delivered: the remote moves five seconds in.
    let pushed_at = tokio::time::Instant::now();
    remote.moved.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(80)).await;

    let announced = remote
        .announced_at
        .lock()
        .unwrap()
        .expect("the poll noticed the push");
    let delay = announced - pushed_at;
    assert!(delay <= Duration::from_secs(75), "{delay:?}");
    assert!(
        delay <= worst_case_latency(POLL_INTERVAL, Duration::ZERO),
        "{delay:?}"
    );
    assert!(
        remote.polls.load(Ordering::SeqCst) >= 2,
        "the poll ran again after the first run"
    );
    for handle in handles {
        handle.shutdown().await;
    }
}

#[test]
fn the_poll_interval_is_60_seconds_and_its_worst_case_is_inside_75_with_a_fast_fetch() {
    assert_eq!(POLL_INTERVAL, Duration::from_secs(60));
    assert!(worst_case_latency(POLL_INTERVAL, Duration::from_secs(5)) <= Duration::from_secs(75));
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_announces_a_push_the_webhook_never_delivered() {
    let Some(mut cluster) = GitCluster::start(2).await else {
        return;
    };
    let moved = cluster.tenant.commit(
        "sit1",
        &[("data/config/a-sit1.yml", Some(b"a: 7\n"))],
        "no webhook for this",
    );
    // The real job on a real clock, with the interval scaled down so that the test does not wait a minute.
    let leases = FakeLeases::new();
    let mut runner = JobRunner::new(Arc::new(leases), JobMetrics::default());
    runner
        .register(
            GitPoll::spec_with(Duration::from_millis(300), Duration::from_secs(3)).unwrap(),
            Arc::new(GitPoll::new(cluster.replicas[0].syncer.clone())),
        )
        .unwrap();
    let handle = runner.start();
    for replica in &mut cluster.replicas {
        let (_, branch, _, to) = next_head_moved(&mut replica.events, Duration::from_secs(10))
            .await
            .unwrap_or_else(|| panic!("{} was not told", replica.name));
        assert_eq!(branch.as_str(), "sit1");
        assert_eq!(to, commit(&moved));
    }
    handle.shutdown().await;
}

// ------------------------------------------------------------------------------------------------- start-up

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_mirror_is_filled_by_the_start_up_task_once_the_remote_answers() {
    use hub_platform::git::credentials::NoCredentials;
    use hub_platform::git::metrics::GitMetrics;
    use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
    use hub_platform::git::sync::{StartupRetry, spawn_initialize};
    use support::fake_github::GitRemote;

    let dir = tempfile::TempDir::new().unwrap();
    let base = GitRemote::new();
    let tenant = GitRemote::new();
    // The remotes exist but hold no branch yet, as a repository that GitHub cannot serve yet: fetching them brings nothing.
    let settings = MirrorSettings::new(
        dir.path(),
        RemoteUrl::local(&base.url()),
        RemoteUrl::local(&tenant.url()),
    );
    let mirror = Mirror::open(settings, Arc::new(NoCredentials), GitMetrics::default()).unwrap();
    let (stop, closing) = tokio::sync::watch::channel(false);
    let task = spawn_initialize(
        Arc::clone(&mirror),
        StartupRetry {
            base: Duration::from_millis(20),
            cap: Duration::from_millis(100),
        },
        closing,
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !mirror.ready(),
        "nothing to fetch yet, so not ready, and the task is still trying"
    );
    assert!(!task.is_finished());

    base.commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "base");
    tenant.commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "sit1");
    eventually("the mirrors to be filled", Duration::from_secs(10), || {
        mirror.ready()
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    drop(stop);
}
