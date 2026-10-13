//! T7 (P2, S18): the bare mirrors. A repository on the local disk plays GitHub, reached over the file transport, so no network is
//! needed. The HTTPS leg (credentials, redirects) is in `git_mirror_http.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use domain::{GitRef, RepoKind};
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::metrics::{FetchOutcome, GitMetrics};
use hub_platform::git::mirror::{Mirror, MirrorError, MirrorSettings, RemoteUrl};
use support::fake_github::GitRemote;
use tempfile::TempDir;

struct Rig {
    _dir: TempDir,
    base: GitRemote,
    tenant: GitRemote,
    mirror: Arc<Mirror>,
    metrics: GitMetrics,
}

fn rig() -> Rig {
    let dir = TempDir::new().unwrap();
    let base = GitRemote::new();
    let tenant = GitRemote::new();
    let metrics = GitMetrics::default();
    let settings = MirrorSettings::new(
        dir.path(),
        RemoteUrl::local(&base.url()),
        RemoteUrl::local(&tenant.url()),
    );
    let mirror = Mirror::open(settings, Arc::new(NoCredentials), metrics.clone()).unwrap();
    Rig {
        _dir: dir,
        base,
        tenant,
        mirror,
        metrics,
    }
}

fn name(s: &str) -> GitRef {
    GitRef::parse(s).unwrap()
}

#[tokio::test]
async fn mirror_open_creates_empty_bare_repositories_and_is_not_ready_until_initialized() {
    let rig = rig();
    assert!(!rig.mirror.ready());
    assert!(rig.mirror.refs(RepoKind::Base).unwrap().branches.is_empty());
    rig.base
        .commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    rig.tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "first");
    rig.mirror.initialize().await.unwrap();
    assert!(rig.mirror.ready());
}

#[tokio::test]
async fn mirror_initialize_clones_every_branch_and_tag_including_templates() {
    let rig = rig();
    let c1 = rig
        .base
        .commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    rig.base.tag("v1.0.0", &c1);
    let t1 = rig
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "sit1");
    let t2 = rig
        .tenant
        .commit("sit2", &[("data/config/a-sit2.yml", Some(b"a: 2\n"))], "sit2");
    let t3 = rig.tenant.commit(
        "templates/common-docs",
        &[("docs/readme.md", Some(b"# docs\n"))],
        "docs",
    );
    let t4 = rig.tenant.commit(
        "templates/core",
        &[("data/config/core.yml", Some(b"x: 1\n"))],
        "core",
    );
    rig.tenant.tag("release-1", &t1);
    rig.tenant.annotated_tag("release-2", &t2);

    rig.mirror.initialize().await.unwrap();

    let base = rig.mirror.refs(RepoKind::Base).unwrap();
    assert_eq!(base.branches.len(), 1);
    assert_eq!(base.branches[&name("main")].as_str(), c1);
    assert_eq!(base.tags[&name("v1.0.0")].as_str(), c1);

    let tenant = rig.mirror.refs(RepoKind::Tenant).unwrap();
    assert_eq!(
        tenant.branches.len(),
        4,
        "{:?}",
        tenant.branches.keys().collect::<Vec<_>>()
    );
    assert_eq!(tenant.branches[&name("sit1")].as_str(), t1);
    assert_eq!(tenant.branches[&name("sit2")].as_str(), t2);
    assert_eq!(tenant.branches[&name("templates/common-docs")].as_str(), t3);
    assert_eq!(tenant.branches[&name("templates/core")].as_str(), t4);
    assert_eq!(tenant.tags[&name("release-1")].as_str(), t1);
    assert_eq!(
        tenant.tags[&name("release-2")].as_str(),
        t2,
        "an annotated tag is listed at the commit it points at"
    );
}

#[tokio::test]
async fn mirror_initialize_twice_fetches_nothing_the_second_time() {
    let rig = rig();
    rig.base
        .commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    rig.tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "first");
    rig.mirror.initialize().await.unwrap();
    let after_first = rig.metrics.fetches_total(RepoKind::Tenant, FetchOutcome::Ok);
    rig.mirror.initialize().await.unwrap();
    assert_eq!(
        rig.metrics.fetches_total(RepoKind::Tenant, FetchOutcome::Ok),
        after_first
    );
}

#[tokio::test]
async fn mirror_fetch_brings_new_commits_branches_tags_and_forced_updates() {
    let rig = rig();
    rig.base
        .commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    let first = rig
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "first");
    rig.mirror.initialize().await.unwrap();

    let second = rig
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 2\n"))], "second");
    let new_branch = rig.tenant.commit(
        "sit9",
        &[("data/config/b-sit9.yml", Some(b"b: 1\n"))],
        "new branch",
    );
    rig.tenant.tag("release-3", &second);
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    let refs = rig.mirror.refs(RepoKind::Tenant).unwrap();
    assert_eq!(refs.branches[&name("sit1")].as_str(), second);
    assert_eq!(refs.branches[&name("sit9")].as_str(), new_branch);
    assert_eq!(refs.tags[&name("release-3")].as_str(), second);

    // A forced push (history rewritten): the mirror follows the remote, it is not a merge.
    rig.tenant.force_branch("sit1", &first);
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    assert_eq!(
        rig.mirror.refs(RepoKind::Tenant).unwrap().branches[&name("sit1")].as_str(),
        first
    );
}

#[tokio::test]
async fn mirror_fetch_prunes_branches_and_tags_deleted_upstream() {
    let rig = rig();
    rig.base
        .commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    let c = rig
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "first");
    rig.tenant
        .commit("sit2", &[("data/config/a-sit2.yml", Some(b"a: 1\n"))], "first");
    rig.tenant.tag("old-tag", &c);
    rig.mirror.initialize().await.unwrap();
    assert_eq!(rig.mirror.refs(RepoKind::Tenant).unwrap().branches.len(), 2);

    rig.tenant.delete_branch("sit2");
    support::fake_github::run({
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(&rig.tenant.path).args(["tag", "-d", "old-tag"]);
        cmd
    });
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    let refs = rig.mirror.refs(RepoKind::Tenant).unwrap();
    assert_eq!(
        refs.branches.keys().map(GitRef::as_str).collect::<Vec<_>>(),
        ["sit1"]
    );
    assert!(refs.tags.is_empty());
}

#[tokio::test]
async fn mirror_fetch_is_single_flight_and_waiters_share_the_next_fetch() {
    let rig = rig();
    rig.base
        .commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    rig.tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "first");
    rig.mirror.initialize().await.unwrap();
    let before = rig.metrics.fetches_total(RepoKind::Tenant, FetchOutcome::Ok);
    let commit = rig
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 2\n"))], "second");

    let mut calls = Vec::new();
    for _ in 0..8 {
        let mirror = Arc::clone(&rig.mirror);
        calls.push(tokio::spawn(async move { mirror.fetch(RepoKind::Tenant).await }));
    }
    for call in calls {
        call.await.unwrap().unwrap();
    }
    let ran = rig.metrics.fetches_total(RepoKind::Tenant, FetchOutcome::Ok) - before;
    assert!(
        (1..=2).contains(&ran),
        "eight callers need at most two fetches, {ran} ran"
    );
    assert!(
        rig.metrics
            .fetches_total(RepoKind::Tenant, FetchOutcome::Coalesced)
            >= 6,
        "the others share them"
    );
    assert_eq!(
        rig.mirror.refs(RepoKind::Tenant).unwrap().branches[&name("sit1")].as_str(),
        commit
    );
}

#[tokio::test]
async fn mirror_fetch_of_a_remote_that_does_not_exist_is_a_typed_error_and_the_mirror_stays_usable() {
    let dir = TempDir::new().unwrap();
    let good = GitRemote::new();
    good.commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    let missing = RemoteUrl::local("file:///nonexistent/lanekeeper/remote.git");
    let settings = MirrorSettings::new(dir.path(), RemoteUrl::local(&good.url()), missing);
    let mirror = Mirror::open(settings, Arc::new(NoCredentials), GitMetrics::default()).unwrap();
    let error = mirror.fetch(RepoKind::Tenant).await.unwrap_err();
    assert_eq!(error, MirrorError::Unavailable);
    assert!(
        !format!("{error} {error:?}").contains("nonexistent"),
        "the error carries no URL or path"
    );
    mirror.fetch(RepoKind::Base).await.unwrap();
    assert_eq!(mirror.refs(RepoKind::Base).unwrap().branches.len(), 1);
    assert!(
        !mirror.ready(),
        "the tenant mirror has nothing, so the hub is not ready"
    );
}

#[tokio::test]
async fn mirror_fetch_gives_up_at_its_deadline() {
    let dir = TempDir::new().unwrap();
    let remote = GitRemote::new();
    // A repository with enough content that a one-nanosecond budget is certainly spent before the fetch can finish.
    let big = vec![b'x'; 2 * 1024 * 1024];
    remote.commit("main", &[("config/big.bin", Some(&big))], "big");
    let mut settings = MirrorSettings::new(
        dir.path(),
        RemoteUrl::local(&remote.url()),
        RemoteUrl::local(&remote.url()),
    );
    settings.fetch_timeout = Duration::from_nanos(1);
    let metrics = GitMetrics::default();
    let mirror = Mirror::open(settings, Arc::new(NoCredentials), metrics.clone()).unwrap();
    let error = mirror.fetch(RepoKind::Base).await.unwrap_err();
    assert_eq!(error, MirrorError::Timeout);
    assert_eq!(metrics.fetches_total(RepoKind::Base, FetchOutcome::TimedOut), 1);
}

#[test]
fn remote_urls_must_be_https_without_credentials() {
    for good in [
        "https://github.com/acme/csp-tenant-data.git",
        "https://ghe.example.org:8443/a/b.git",
    ] {
        assert!(RemoteUrl::https(good).is_ok(), "{good}");
    }
    for bad in [
        "http://github.com/a/b.git",
        "git://github.com/a/b.git",
        "ssh://git@github.com/a/b.git",
        "file:///srv/repo.git",
        "https://user:token@github.com/a/b.git",
        "https://token@github.com/a/b.git",
        "https://github.com/a/b.git\n",
        "https:///a/b.git",
        "github.com/a/b.git",
        "",
    ] {
        assert!(RemoteUrl::https(bad).is_err(), "{bad:?} must be refused");
    }
}
