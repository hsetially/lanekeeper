//! A smart-HTTP Git server over a real loopback socket, for the HTTPS leg of the mirror (T7, A16): it answers the two requests of a
//! fetch (`GET info/refs?service=git-upload-pack` and `POST git-upload-pack`) by running `git upload-pack --stateless-rpc` on a bare
//! repository, as `git http-backend` would. It can demand HTTP basic authentication (a 401 with a challenge first, as GitHub does),
//! redirect to another host name, and it records every request, so a test can see which credential went where.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::{Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path_and_query: String,
    pub host: String,
    pub authorization: Option<String>,
}

#[derive(Clone)]
struct Config {
    repo: PathBuf,
    /// `(user, token)` that basic authentication must carry; `None` serves anonymously.
    basic: Option<(String, String)>,
    /// Redirect every request whose `Host` is the loopback address to this host name (same port, same path).
    redirect_host: Option<String>,
}

pub struct GitHttpServer {
    scheme: &'static str,
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: JoinHandle<()>,
}

impl Drop for GitHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for GitHttpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GitHttpServer")
    }
}

impl GitHttpServer {
    pub async fn start(repo: PathBuf, basic: Option<(&str, &str)>, redirect_host: Option<&str>) -> Self {
        Self::serve(repo, basic, redirect_host, None).await
    }

    /// The same over TLS (the tests that need `https`: gix will not send a credential over plain `http`).
    pub async fn start_tls(
        repo: PathBuf,
        basic: Option<(&str, &str)>,
        redirect_host: Option<&str>,
        tls: Arc<rustls::ServerConfig>,
    ) -> Self {
        Self::serve(repo, basic, redirect_host, Some(tls)).await
    }

    async fn serve(
        repo: PathBuf,
        basic: Option<(&str, &str)>,
        redirect_host: Option<&str>,
        tls: Option<Arc<rustls::ServerConfig>>,
    ) -> Self {
        let scheme = if tls.is_some() { "https" } else { "http" };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let config = Config {
            repo,
            basic: basic.map(|(u, t)| (u.to_owned(), t.to_owned())),
            redirect_host: redirect_host.map(str::to_owned),
        };
        let log = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let config = config.clone();
                let log = Arc::clone(&log);
                let tls = tls.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                        let config = config.clone();
                        let log = Arc::clone(&log);
                        async move {
                            Ok::<_, std::convert::Infallible>(handle(&config, &log, request, scheme).await)
                        }
                    });
                    let builder = hyper::server::conn::http1::Builder::new();
                    if let Some(tls) = tls {
                        let Ok(stream) = TlsAcceptor::from(tls).accept(stream).await else {
                            return;
                        };
                        let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                    } else {
                        let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                    }
                });
            }
        });
        Self {
            scheme,
            addr,
            seen,
            task,
        }
    }

    /// `<scheme>://127.0.0.1:<port>/repo.git`
    pub fn url(&self) -> String {
        format!("{}://{}/repo.git", self.scheme, self.addr)
    }

    /// The same server by another name (`localhost`), for the redirect and the TLS tests.
    pub fn url_for_host(&self, host: &str) -> String {
        format!("{}://{host}:{}/repo.git", self.scheme, self.addr.port())
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn plain(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

async fn handle(
    config: &Config,
    log: &Mutex<Vec<Seen>>,
    request: Request<Incoming>,
    scheme: &str,
) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let authorization = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(ToString::to_string)
        .unwrap_or_default();
    log.lock().unwrap().push(Seen {
        method: parts.method.to_string(),
        path_and_query: path_and_query.clone(),
        host: host.clone(),
        authorization: authorization.clone(),
    });

    if let Some(other) = &config.redirect_host {
        if host.starts_with("127.0.0.1") {
            let port = host.rsplit(':').next().unwrap_or_default();
            return Response::builder()
                .status(StatusCode::MOVED_PERMANENTLY)
                .header(
                    header::LOCATION,
                    format!("{scheme}://{other}:{port}{path_and_query}"),
                )
                .body(Full::new(Bytes::new()))
                .unwrap();
        }
    }
    if let Some((user, token)) = &config.basic {
        let want = format!("Basic {}", STANDARD.encode(format!("{user}:{token}")));
        if authorization.as_deref() != Some(want.as_str()) {
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"git\"")
                .body(Full::new(Bytes::new()))
                .unwrap();
        }
    }

    let protocol = parts
        .headers
        .get("git-protocol")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let repo = config.repo.clone();
    match (parts.method.as_str(), parts.uri.path()) {
        ("GET", "/repo.git/info/refs") if parts.uri.query() == Some("service=git-upload-pack") => {
            let out = upload_pack(&repo, &protocol, true, Vec::new()).await;
            let mut body = Vec::new();
            if !protocol.contains("version=2") {
                body.extend_from_slice(b"001e# service=git-upload-pack\n0000");
            }
            body.extend_from_slice(&out);
            Response::builder()
                .header(
                    header::CONTENT_TYPE,
                    "application/x-git-upload-pack-advertisement",
                )
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Full::new(Bytes::from(body)))
                .unwrap()
        }
        ("POST", "/repo.git/git-upload-pack") => {
            let input = body
                .collect()
                .await
                .map(|b| b.to_bytes().to_vec())
                .unwrap_or_default();
            let out = upload_pack(&repo, &protocol, false, input).await;
            Response::builder()
                .header(header::CONTENT_TYPE, "application/x-git-upload-pack-result")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Full::new(Bytes::from(out)))
                .unwrap()
        }
        _ => plain(StatusCode::NOT_FOUND),
    }
}

async fn upload_pack(repo: &std::path::Path, protocol: &str, advertise: bool, input: Vec<u8>) -> Vec<u8> {
    let repo = repo.to_owned();
    let protocol = protocol.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new("git");
        cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_PROTOCOL", protocol)
            .arg("upload-pack")
            .arg("--stateless-rpc");
        if advertise {
            cmd.arg("--advertise-refs");
        }
        cmd.arg(&repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let mut out = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
        let _ = writer.join();
        let _ = child.wait();
        out
    })
    .await
    .unwrap()
}
