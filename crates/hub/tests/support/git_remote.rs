//! A Git remote the hub fetches from over HTTPS (plan 18, T3): the mirrors are a required part of the hub, so a process test needs
//! something to mirror. A bare repository made with `git fast-import` (the platform's `GitRemote`) is served by the platform's
//! smart-HTTP server over TLS with a leaf certificate of the test CA. The child process trusts that CA through `SSL_CERT_FILE`
//! (`gix` reaches the remote through reqwest, which reads the system roots; the workspace forbids `unsafe`, so the test sets the
//! variable for the child it spawns and not for itself).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use rustls::ServerConfig;

use crate::support::pki::Leaf;

#[path = "../../../hub-platform/tests/support/fake_github.rs"]
pub mod fake_github;
#[path = "../../../hub-platform/tests/support/git_http.rs"]
pub mod git_http;

pub use fake_github::GitRemote;
pub use git_http::GitHttpServer;

/// The two branches the mirrors find: `main` as the base repository's, `sit1` as the tenant repository's. Both repositories are
/// mirrored from the same remote, so each mirror holds both.
///
/// The server runs on a thread and a runtime of its own. A process test blocks its own thread while it waits for the child (the
/// output reader and `wait` are not async), and a server that shared that runtime would stop answering exactly then, which is
/// when the child's first fetch is running.
pub struct HttpsRemote {
    pub remote: GitRemote,
    url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HttpsRemote {
    pub async fn start(leaf: &Leaf) -> Self {
        let remote = GitRemote::new();
        remote.commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "base");
        remote.commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "tenant");
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = Arc::new(
            ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![leaf.cert.clone()], leaf.key.clone_key())
                .unwrap(),
        );
        let path = remote.path.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (told, hears) = std::sync::mpsc::channel::<String>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let server = GitHttpServer::start_tls(path, None, None, config).await;
                told.send(server.url_for_host("localhost")).unwrap();
                let _ = stopped.await;
                drop(server);
            });
        });
        let url = tokio::task::spawn_blocking(move || hears.recv().unwrap())
            .await
            .unwrap();
        Self {
            remote,
            url,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    /// `https://localhost:<port>/repo.git`
    pub fn url(&self) -> String {
        self.url.clone()
    }
}

impl Drop for HttpsRemote {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
