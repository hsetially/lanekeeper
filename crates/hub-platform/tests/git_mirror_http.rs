//! T7 (A16, S6, S7, S18): the HTTPS leg of the mirror. `gix` reaches the remote through its `reqwest` transport on rustls with
//! aws-lc-rs (the one the human accepted at the A16 gate); here the remote is a smart-HTTP server on the loopback address.
//!
//! What is proved, over real TLS: the service credential is offered only after the server asks for one, it reaches the server as
//! HTTP basic authentication and never as part of a URL, a wrong or missing credential fails without leaking, and a redirect to
//! another host gets no credential. Over plain `http`, `gix` itself refuses to send a credential, and the mirror never gets
//! the chance to.
//!
//! The TLS scenarios need a client that trusts a throwaway CA. The reqwest transport reads the system roots (it ignores
//! `http.sslCAInfo`), and the way to point it at a test CA is `SSL_CERT_FILE`, which this crate cannot set in-process (the
//! workspace forbids `unsafe`, and `set_var` is `unsafe`). So the parent test starts this same test binary as a child process
//! with the variable set, and the child runs the scenarios (`tls_scenarios_child`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::print_stderr
)]

mod support;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use domain::{GitRef, RepoKind};
use hub_platform::git::credentials::{
    GitCredentials, MountedFileCredentials, NoCredentials, SecretManagerCredentials,
};
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorError, MirrorSettings, RemoteUrl};
use ports::fakes::FakeSecretSource;
use support::fake_github::GitRemote;
use support::git_http::GitHttpServer;
use support::tls::TestPki;
use tempfile::TempDir;

const TOKEN: &str = "ghp_PlantedTokenForTheMirrorTest0123456789";
const USER: &str = "x-access-token";
const CHILD_DIR: &str = "LK_GIT_TLS_DIR";

fn basic(user: &str, token: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{token}")))
}

fn mirror_of(url: RemoteUrl, credentials: Arc<dyn GitCredentials>) -> (TempDir, Arc<Mirror>) {
    let dir = TempDir::new().unwrap();
    // Both repositories point at the same server: this is about the transport, not about the two repositories.
    let settings = MirrorSettings::new(dir.path(), url.clone(), url);
    let mirror = Mirror::open(settings, credentials, GitMetrics::default()).unwrap();
    (dir, mirror)
}

fn remote_with_two_branches() -> GitRemote {
    let remote = GitRemote::new();
    remote.commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    remote.commit(
        "templates/core",
        &[("data/config/c.yml", Some(b"c: 1\n"))],
        "core",
    );
    remote
}

// ------------------------------------------------------------------------------------------------ plain http

#[tokio::test]
async fn mirror_fetches_a_public_repository_over_plain_http_without_a_credential() {
    let remote = remote_with_two_branches();
    let server = GitHttpServer::start(remote.path.clone(), None, None).await;
    let (_dir, mirror) = mirror_of(RemoteUrl::local(&server.url()), Arc::new(NoCredentials));
    mirror.fetch(RepoKind::Base).await.unwrap();
    let refs = mirror.refs(RepoKind::Base).unwrap();
    assert_eq!(refs.branches.len(), 2);
    assert_eq!(
        refs.branches[&GitRef::parse("main").unwrap()].as_str(),
        remote.head("main").unwrap()
    );
}

#[tokio::test]
async fn a_credential_is_never_sent_over_plain_http() {
    let remote = remote_with_two_branches();
    let server = GitHttpServer::start(remote.path.clone(), Some((USER, TOKEN)), None).await;
    let secrets = Arc::new(FakeSecretSource::new().with("git-token", TOKEN));
    let credentials = Arc::new(SecretManagerCredentials::new(secrets, "git-token"));
    let (_dir, mirror) = mirror_of(RemoteUrl::local(&server.url()), credentials);
    assert_eq!(
        mirror.fetch(RepoKind::Base).await.unwrap_err(),
        MirrorError::Unavailable
    );
    assert!(
        server.seen().iter().all(|r| r.authorization.is_none()),
        "no request carried the credential"
    );
}

// ----------------------------------------------------------------------------------------------------- https

/// Starts this test binary with a CA the HTTPS client trusts, and has it run the TLS scenarios.
#[test]
fn https_scenarios_run_in_a_child_process_that_trusts_a_throwaway_ca() {
    if std::env::var_os(CHILD_DIR).is_some() {
        return;
    }
    let pki = TestPki::new("lanekeeper git test CA");
    let dir = TempDir::new().unwrap();
    let (cert, key) = pki.pem();
    std::fs::write(dir.path().join("ca.pem"), cert).unwrap();
    std::fs::write(dir.path().join("ca.key.pem"), key).unwrap();
    let empty = dir.path().join("no-certs");
    std::fs::create_dir(&empty).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tls_scenarios_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SSL_CERT_FILE", dir.path().join("ca.pem"))
        .env("SSL_CERT_DIR", &empty)
        .env(CHILD_DIR, dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the HTTPS scenarios failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("test tls_scenarios_child ... ok"),
        "the child did run the scenarios"
    );
}

#[test]
#[ignore = "run by https_scenarios_run_in_a_child_process_that_trusts_a_throwaway_ca, with SSL_CERT_FILE set"]
fn tls_scenarios_child() {
    let Some(dir) = std::env::var_os(CHILD_DIR) else {
        eprintln!("tls_scenarios_child is run by its parent test, which sets {CHILD_DIR}");
        return;
    };
    let logs = support::log_capture::LogCapture::install_everywhere();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(tls_scenarios(Path::new(&dir)));
    // S7, S10: the fetches ran on blocking threads and logged their failures; none of it holds the token, in any form.
    let text = logs.text();
    assert!(
        text.contains("git_fetch_failed"),
        "the failed fetches were logged, so the capture means something: {text}"
    );
    for secret in [
        TOKEN,
        "ghp_WrongTokenEntirely",
        &STANDARD.encode(format!("{USER}:{TOKEN}")),
    ] {
        assert!(!text.contains(secret), "a secret is in the logs: {text}");
    }
}

async fn tls_scenarios(dir: &Path) {
    let pki = TestPki::from_pem(
        &std::fs::read_to_string(dir.join("ca.pem")).unwrap(),
        &std::fs::read_to_string(dir.join("ca.key.pem")).unwrap(),
    );
    let tls = pki.loopback_server_config();
    let remote = remote_with_two_branches();

    // 1. The credential follows a challenge, as basic authentication, and is in no URL.
    {
        let server =
            GitHttpServer::start_tls(remote.path.clone(), Some((USER, TOKEN)), None, Arc::clone(&tls)).await;
        let secrets = Arc::new(FakeSecretSource::new().with("git-token", &format!("{TOKEN}\n")));
        let credentials = Arc::new(SecretManagerCredentials::new(secrets, "git-token"));
        let url = RemoteUrl::https(&server.url_for_host("localhost")).unwrap();
        let (_dir, mirror) = mirror_of(url, credentials);
        mirror.fetch(RepoKind::Base).await.unwrap();
        let refs = mirror.refs(RepoKind::Base).unwrap();
        assert_eq!(refs.branches.len(), 2);
        assert_eq!(
            refs.branches[&GitRef::parse("main").unwrap()].as_str(),
            remote.head("main").unwrap()
        );
        let seen = server.seen();
        assert!(
            seen[0].authorization.is_none(),
            "the first request carries no credential"
        );
        let want = basic(USER, TOKEN);
        assert!(
            seen.iter()
                .any(|r| r.authorization.as_deref() == Some(want.as_str())),
            "a later request carries it as basic authentication: {seen:?}"
        );
        for request in &seen {
            assert!(!request.path_and_query.contains(TOKEN), "the token is in no URL");
            assert!(!request.host.contains(TOKEN));
        }
    }

    // 2. The same with the token in a mounted file.
    {
        let server =
            GitHttpServer::start_tls(remote.path.clone(), Some((USER, TOKEN)), None, Arc::clone(&tls)).await;
        let file = dir.join("token");
        std::fs::write(&file, format!("{TOKEN}\n")).unwrap();
        let url = RemoteUrl::https(&server.url_for_host("localhost")).unwrap();
        let (_dir, mirror) = mirror_of(url, Arc::new(MountedFileCredentials::new(&file)));
        mirror.fetch(RepoKind::Tenant).await.unwrap();
        assert_eq!(mirror.refs(RepoKind::Tenant).unwrap().branches.len(), 2);
    }

    // 3. A wrong token fails, and the error names no secret.
    {
        let server =
            GitHttpServer::start_tls(remote.path.clone(), Some((USER, TOKEN)), None, Arc::clone(&tls)).await;
        let secrets = Arc::new(FakeSecretSource::new().with("git-token", "ghp_WrongTokenEntirely"));
        let credentials = Arc::new(SecretManagerCredentials::new(secrets, "git-token"));
        let url = RemoteUrl::https(&server.url_for_host("localhost")).unwrap();
        let (_dir, mirror) = mirror_of(url, credentials);
        let error = mirror.fetch(RepoKind::Base).await.unwrap_err();
        assert_eq!(error, MirrorError::Unavailable);
        let text = format!("{error} {error:?}");
        assert!(!text.contains("ghp_") && !text.contains(TOKEN));
        assert!(mirror.refs(RepoKind::Base).unwrap().branches.is_empty());
    }

    // 4. No credential at all against a server that asks for one.
    {
        let server =
            GitHttpServer::start_tls(remote.path.clone(), Some((USER, TOKEN)), None, Arc::clone(&tls)).await;
        let url = RemoteUrl::https(&server.url_for_host("localhost")).unwrap();
        let (_dir, mirror) = mirror_of(url, Arc::new(NoCredentials));
        assert_eq!(
            mirror.fetch(RepoKind::Base).await.unwrap_err(),
            MirrorError::Unavailable
        );
    }

    // 5. A redirect to another host name gets no credential, on either host.
    {
        let server = GitHttpServer::start_tls(
            remote.path.clone(),
            Some((USER, TOKEN)),
            Some("localhost"),
            Arc::clone(&tls),
        )
        .await;
        let secrets = Arc::new(FakeSecretSource::new().with("git-token", TOKEN));
        let credentials = Arc::new(SecretManagerCredentials::new(secrets, "git-token"));
        let url = RemoteUrl::https(&server.url_for_host("127.0.0.1")).unwrap();
        let (_dir, mirror) = mirror_of(url, credentials);
        let result = mirror.fetch(RepoKind::Base).await;
        let seen = server.seen();
        assert!(
            seen.iter().any(|r| r.host.starts_with("localhost")),
            "the redirect was followed, so this scenario means something: {seen:?}"
        );
        assert!(
            seen.iter().all(|r| r.authorization.is_none()),
            "no request carried the credential: {seen:?}"
        );
        assert_eq!(result.unwrap_err(), MirrorError::Unavailable);
    }
}
