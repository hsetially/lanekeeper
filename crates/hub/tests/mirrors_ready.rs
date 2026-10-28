//! T8 (P11): `/readyz` waits for the Git mirrors. A replica whose mirrors are empty (a first start, or a deleted volume) is not
//! ready to serve a read that needs a commit; one whose two mirrors each hold a branch is.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use hub::git::MirrorsReady;
use hub::ops::ReadyCheck;
use hub::{Deps, Hub, HubSettings, SessionEdge};
use hub_platform::FakeSessionAuth;
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
use support::{header_of, send, text};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@lanekeeper.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@lanekeeper.invalid")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A bare repository with one branch holding one empty commit: the smallest remote a mirror can be filled from.
fn remote(root: &Path, name: &str) -> String {
    let path = root.join(name);
    std::fs::create_dir_all(&path).unwrap();
    git(
        &path,
        &[
            "init",
            "--bare",
            "--quiet",
            "--template=",
            "--initial-branch=main",
        ],
    );
    let tree = {
        let out = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["hash-object", "-t", "tree", "-w", "--stdin"])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    let commit = git(&path, &["commit-tree", &tree, "-m", "empty"]);
    git(&path, &["update-ref", "refs/heads/main", &commit]);
    format!("file://{}", path.display())
}

#[tokio::test(flavor = "multi_thread")]
async fn readiness_waits_for_both_mirrors_to_hold_a_branch() {
    let remotes = tempfile::tempdir().unwrap();
    let (base, tenant) = (
        remote(remotes.path(), "base.git"),
        remote(remotes.path(), "tenant.git"),
    );
    let volume = tempfile::tempdir().unwrap();
    let mirror = Mirror::open(
        MirrorSettings::new(volume.path(), RemoteUrl::local(&base), RemoteUrl::local(&tenant)),
        Arc::new(NoCredentials),
        GitMetrics::default(),
    )
    .unwrap();
    let check = MirrorsReady::new(Arc::clone(&mirror));
    assert!(!check.ready().await, "an empty mirror is not ready");

    mirror.initialize().await.unwrap();
    assert!(check.ready().await, "both mirrors hold a branch");

    // And a replica that restarts on its populated volume is ready without fetching anything.
    drop(check);
    drop(mirror);
    let reopened = Mirror::open(
        MirrorSettings::new(volume.path(), RemoteUrl::local(&base), RemoteUrl::local(&tenant)),
        Arc::new(NoCredentials),
        GitMetrics::default(),
    )
    .unwrap();
    assert!(MirrorsReady::new(reopened).ready().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn readyz_names_the_mirrors_while_they_are_empty() {
    let remotes = tempfile::tempdir().unwrap();
    let (base, tenant) = (
        remote(remotes.path(), "base.git"),
        remote(remotes.path(), "tenant.git"),
    );
    let volume = tempfile::tempdir().unwrap();
    let mirror = Mirror::open(
        MirrorSettings::new(volume.path(), RemoteUrl::local(&base), RemoteUrl::local(&tenant)),
        Arc::new(NoCredentials),
        GitMetrics::default(),
    )
    .unwrap();
    let hub = Hub::build(Deps {
        settings: HubSettings::default(),
        session: SessionEdge::without_sessions(Arc::new(FakeSessionAuth::new())),
        mounted: Vec::new(),
        modules: Vec::new(),
        static_files: None,
        checks: vec![("mirrors", Arc::new(MirrorsReady::new(Arc::clone(&mirror))))],
    })
    .unwrap();
    let ask = || {
        let mut request = http::Request::builder()
            .uri("/readyz")
            .body(axum::body::Body::empty())
            .unwrap();
        request.extensions_mut().insert(support::peer([127, 0, 0, 1]));
        request
    };
    let response = send(&hub.ops, ask()).await;
    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(header_of(&response, "cache-control"), Some("no-store"));
    assert_eq!(text(response).await, "not ready: mirrors\n");

    mirror.initialize().await.unwrap();
    let response = send(&hub.ops, ask()).await;
    assert_eq!(response.status(), http::StatusCode::OK);
}
