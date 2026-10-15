//! Test doubles of GitHub (T5, and T6 later).
//!
//! - [`FakeGithub`] is the [`GithubApi`] the vault tests use: a script of which token is whose, which repositories it can push to,
//!   and what fails, with a count of the calls made.
//! - [`LocalGithub`] is a small HTTP/1.1 server on the loopback, plain or over TLS, that the real [`hub_writes::github::GithubClient`]
//!   talks to in the client tests. It records each request it sees (method, path and headers) and answers from a handler, so a
//!   test can say what GitHub sends back and check what the client sent.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use domain::{Secret, Timestamp};
use hub_writes::github::{GithubApi, GithubError, GithubLogin, GithubUser, RepoRef};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

// ----------------------------------------------------------------------------------------------------------- the trait

/// What the fake knows about one token.
#[derive(Clone)]
pub struct Account {
    pub login: String,
    pub expires_at: Option<Timestamp>,
    /// The repositories (`owner/name`) the token can push to.
    pub pushes: Vec<String>,
}

#[derive(Default)]
struct Script {
    accounts: BTreeMap<String, Account>,
    /// An error the next call to `user` returns instead of an answer, then the script goes on.
    fail_user: VecDeque<GithubError>,
    fail_push: VecDeque<GithubError>,
    user_calls: usize,
    push_calls: usize,
    /// The tokens presented, in order, for the tests that check which token a call used.
    presented: Vec<String>,
}

/// A scripted [`GithubApi`].
#[derive(Clone, Default)]
pub struct FakeGithub(Arc<Mutex<Script>>);

impl std::fmt::Debug for FakeGithub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeGithub").finish_non_exhaustive()
    }
}

impl FakeGithub {
    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `token` belongs to `login` and can push to `pushes`.
    pub fn token(&self, token: &str, login: &str, pushes: &[&str]) -> &Self {
        self.script().accounts.insert(
            token.to_owned(),
            Account {
                login: login.to_owned(),
                expires_at: None,
                pushes: pushes.iter().map(|p| (*p).to_owned()).collect(),
            },
        );
        self
    }

    pub fn expires(&self, token: &str, at: Timestamp) -> &Self {
        if let Some(a) = self.script().accounts.get_mut(token) {
            a.expires_at = Some(at);
        }
        self
    }

    pub fn fail_next_user(&self, e: GithubError) {
        self.script().fail_user.push_back(e);
    }

    pub fn fail_next_push_check(&self, e: GithubError) {
        self.script().fail_push.push_back(e);
    }

    pub fn user_calls(&self) -> usize {
        self.script().user_calls
    }

    pub fn push_calls(&self) -> usize {
        self.script().push_calls
    }

    pub fn presented(&self) -> Vec<String> {
        self.script().presented.clone()
    }
}

#[async_trait]
impl GithubApi for FakeGithub {
    async fn user(&self, token: &Secret<String>) -> Result<GithubUser, GithubError> {
        let mut s = self.script();
        s.user_calls += 1;
        s.presented.push(token.expose().clone());
        if let Some(e) = s.fail_user.pop_front() {
            return Err(e);
        }
        let account = s.accounts.get(token.expose()).ok_or(GithubError::Unauthorized)?;
        Ok(GithubUser {
            login: GithubLogin::parse(&account.login).map_err(|_| GithubError::Unexpected)?,
            expires_at: account.expires_at,
        })
    }

    async fn can_push(&self, token: &Secret<String>, repo: &RepoRef) -> Result<bool, GithubError> {
        let mut s = self.script();
        s.push_calls += 1;
        s.presented.push(token.expose().clone());
        if let Some(e) = s.fail_push.pop_front() {
            return Err(e);
        }
        let account = s.accounts.get(token.expose()).ok_or(GithubError::Unauthorized)?;
        Ok(account.pushes.iter().any(|p| *p == repo.to_string()))
    }
}

// ------------------------------------------------------------------------------------------------------ a local server

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    /// The body, whole (a content length or chunks).
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Everything the request sent, as one text, for "the token is nowhere else" checks.
    pub fn everything(&self) -> String {
        let mut text = format!("{} {}\n", self.method, self.path);
        for (n, v) in &self.headers {
            let _ = writeln!(text, "{n}: {v}");
        }
        text.push_str(&String::from_utf8_lossy(&self.body));
        text
    }

    /// The body as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// What the server answers.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering (a slow GitHub).
    pub delay: Duration,
}

impl Reply {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// Decides the answer to a request, given the request and how many requests the server has seen before it.
pub type Handler = Arc<dyn Fn(&Seen, usize) -> Reply + Send + Sync>;

/// A handler that answers every request the same way.
pub fn always(reply: Reply) -> Handler {
    Arc::new(move |_, _| reply.clone())
}

/// A handler that answers the requests in order, and the last answer for every later one.
pub fn sequence(replies: Vec<Reply>) -> Handler {
    Arc::new(move |_, n| replies[n.min(replies.len() - 1)].clone())
}

/// What the TLS server speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tls {
    Plain,
    /// TLS 1.3 and 1.2.
    Both,
    /// TLS 1.3 only.
    V13,
    /// TLS 1.2 only: the client must refuse it (S6).
    V12,
}

pub struct LocalGithub {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// The certificate of a TLS server, to give the client as its only root.
    pub cert: Option<CertificateDer<'static>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LocalGithub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LocalGithub {
    pub async fn start(handler: Handler) -> Self {
        Self::start_with(handler, Tls::Plain).await
    }

    pub async fn start_with(handler: Handler, tls: Tls) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();

        let (acceptor, cert) = if tls == Tls::Plain {
            (None, None)
        } else {
            let made = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
            let cert = made.cert.der().clone();
            let key = PrivatePkcs8KeyDer::from(made.signing_key.serialize_der());
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let versions: &[&'static rustls::SupportedProtocolVersion] = match tls {
                Tls::V13 => &[&rustls::version::TLS13],
                Tls::V12 => &[&rustls::version::TLS12],
                _ => &[&rustls::version::TLS13, &rustls::version::TLS12],
            };
            let config = rustls::ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(versions)
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.clone()], key.into())
                .unwrap();
            (Some(TlsAcceptor::from(Arc::new(config))), Some(cert))
        };

        let task = tokio::spawn({
            let seen = Arc::clone(&seen);
            async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let (seen, handler) = (Arc::clone(&seen), Arc::clone(&handler));
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        match acceptor {
                            None => serve(stream, seen, handler).await,
                            Some(acceptor) => {
                                if let Ok(stream) = acceptor.accept(stream).await {
                                    serve(stream, seen, handler).await;
                                }
                            }
                        }
                    });
                }
            }
        });
        Self {
            addr,
            seen,
            cert,
            task,
        }
    }

    pub fn base(&self) -> String {
        let scheme = if self.cert.is_some() { "https" } else { "http" };
        format!("{scheme}://{}", self.addr)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn requests(&self) -> usize {
        self.seen().len()
    }
}

/// Serves requests on one connection until the peer closes it.
async fn serve(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    seen: Arc<Mutex<Vec<Seen>>>,
    handler: Handler,
) {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        // Read one request head.
        let head_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            if buf.len() > 32 * 1024 {
                return;
            }
            let mut chunk = [0_u8; 4096];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        buf.drain(..head_end + 4);
        let mut lines = head.split("\r\n");
        let first = lines.next().unwrap_or_default();
        let mut parts = first.split(' ');
        let method = parts.next().unwrap_or_default().to_owned();
        let path = parts.next().unwrap_or_default().to_owned();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        let length = headers
            .iter()
            .find(|(n, _)| n == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok());
        let chunked = headers
            .iter()
            .any(|(n, v)| n == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
        let Some(body) = read_body(&mut stream, &mut buf, length, chunked).await else {
            return;
        };
        let request = Seen {
            method,
            path,
            headers,
            body,
        };

        let before = {
            let mut all = seen.lock().unwrap_or_else(PoisonError::into_inner);
            all.push(request.clone());
            all.len() - 1
        };
        let reply = handler(&request, before);
        if !reply.delay.is_zero() {
            tokio::time::sleep(reply.delay).await;
        }
        let mut out = format!(
            "HTTP/1.1 {} X\r\ncontent-length: {}\r\n",
            reply.status,
            reply.body.len()
        );
        for (n, v) in &reply.headers {
            let _ = write!(out, "{n}: {v}\r\n");
        }
        out.push_str("\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&reply.body);
        if stream.write_all(&bytes).await.is_err() || stream.flush().await.is_err() {
            return;
        }
    }
}

/// Reads more of the stream into `buf`; false when the peer is gone.
async fn fill(stream: &mut (impl AsyncRead + Unpin), buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0_u8; 16 * 1024];
    match stream.read(&mut chunk).await {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            true
        }
    }
}

/// The body of a request: `length` bytes, or chunks, or nothing.
async fn read_body(
    stream: &mut (impl AsyncRead + Unpin),
    buf: &mut Vec<u8>,
    length: Option<usize>,
    chunked: bool,
) -> Option<Vec<u8>> {
    if let Some(len) = length {
        while buf.len() < len {
            if !fill(stream, buf).await {
                return None;
            }
        }
        return Some(buf.drain(..len).collect());
    }
    if !chunked {
        return Some(Vec::new());
    }
    let mut body = Vec::new();
    loop {
        let line_end = loop {
            if let Some(i) = buf.windows(2).position(|w| w == b"\r\n") {
                break i;
            }
            if !fill(stream, buf).await {
                return None;
            }
        };
        let size = usize::from_str_radix(std::str::from_utf8(&buf[..line_end]).ok()?.trim(), 16).ok()?;
        buf.drain(..line_end + 2);
        while buf.len() < size + 2 {
            if !fill(stream, buf).await {
                return None;
            }
        }
        body.extend(buf.drain(..size));
        buf.drain(..2);
        if size == 0 {
            return Some(body);
        }
    }
}
