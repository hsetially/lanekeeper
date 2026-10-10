//! A scripted HTTP/1.1 server on a loopback port, for the plain-HTTP clients (metadata server, config-server).
//!
//! It reads the request head and body by hand and answers with bytes the test chooses, so a test can send a huge
//! body, garbage, a hung connection or a reset, none of which a real HTTP library would produce. Every request is
//! recorded.
//!
//! It listens on a loopback port ([`RawServer::start`]) or lives in memory ([`RawServer::in_memory`]). A test that waits
//! on a clock must use the in-memory one: a paused clock does not work with real sockets, because the runtime jumps over
//! the wait for the socket and every request times out.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::sync::{Arc, Mutex};

use agent::transport::dial::{BoxedIo, Dialer};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// What the server saw.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    /// Path and query, as written on the request line.
    pub target: String,
    /// Header names are lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// What the server does with a request.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A well-formed response.
    Http {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Exactly these bytes, then close.
    Raw(Vec<u8>),
    /// Accept the request and never answer.
    Hang,
    /// Close the connection without answering.
    Close,
}

impl Reply {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::status(200, body)
    }

    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::Http {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    fn render(&self) -> Option<Vec<u8>> {
        match self {
            Self::Http {
                status,
                headers,
                body,
            } => {
                let mut out = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                )
                .into_bytes();
                for (n, v) in headers {
                    out.extend_from_slice(format!("{n}: {v}\r\n").as_bytes());
                }
                out.extend_from_slice(b"\r\n");
                out.extend_from_slice(body);
                Some(out)
            }
            Self::Raw(bytes) => Some(bytes.clone()),
            Self::Hang | Self::Close => None,
        }
    }
}

type Handler = dyn Fn(&RecordedRequest) -> Reply + Send + Sync;

pub struct RawServer {
    url: String,
    log: Arc<Mutex<Vec<RecordedRequest>>>,
    task: Option<JoinHandle<()>>,
    dialer: Option<Arc<MemoryDialer>>,
}

/// The name an in-memory server answers to. It is never resolved.
pub const MEMORY_AUTHORITY: &str = "config-server.test:8888";

/// Connects to an in-memory [`RawServer`]: every dial makes a pipe and hands its far end to a task that serves it.
pub struct MemoryDialer {
    handler: Arc<Handler>,
    log: Arc<Mutex<Vec<RecordedRequest>>>,
    refusing: bool,
    /// Every `(host, port)` the client asked to connect to.
    dialed: Mutex<Vec<(String, u16)>>,
}

impl std::fmt::Debug for MemoryDialer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryDialer")
            .field("refusing", &self.refusing)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Dialer for MemoryDialer {
    async fn dial(&self, host: &str, port: u16) -> io::Result<BoxedIo> {
        self.dialed.lock().unwrap().push((host.to_owned(), port));
        if self.refusing {
            return Err(io::Error::from(io::ErrorKind::ConnectionRefused));
        }
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (handler, log) = (Arc::clone(&self.handler), Arc::clone(&self.log));
        tokio::spawn(async move {
            if let Some(parked) = serve(server, &handler, &log).await {
                // A hung connection stays open for as long as the runtime lives.
                let _held = parked;
                std::future::pending::<()>().await;
            }
        });
        Ok(Box::new(client))
    }
}

impl RawServer {
    /// Listen on a free loopback port and answer every request with `handler`.
    pub async fn start(handler: impl Fn(&RecordedRequest) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let task = tokio::spawn({
            let log = Arc::clone(&log);
            async move {
                // Held so that a `Hang` keeps its connection open for as long as the server lives.
                let mut parked: Vec<TcpStream> = Vec::new();
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    if let Some(stream) = serve(stream, &handler, &log).await {
                        parked.push(stream);
                    }
                }
            }
        });
        Self {
            url: format!("http://{addr}"),
            log,
            task: Some(task),
            dialer: None,
        }
    }

    /// A server that needs no socket: connect to it through [`RawServer::dialer`]. Its URL is `http://` and
    /// [`MEMORY_AUTHORITY`].
    pub fn in_memory(handler: impl Fn(&RecordedRequest) -> Reply + Send + Sync + 'static) -> Self {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dialer = Arc::new(MemoryDialer {
            handler: Arc::new(handler),
            log: Arc::clone(&log),
            refusing: false,
            dialed: Mutex::default(),
        });
        Self {
            url: format!("http://{MEMORY_AUTHORITY}"),
            log,
            task: None,
            dialer: Some(dialer),
        }
    }

    /// Where nothing listens: every connection is refused.
    pub fn in_memory_refusing() -> Self {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dialer = Arc::new(MemoryDialer {
            handler: Arc::new(|_: &RecordedRequest| Reply::Close),
            log: Arc::clone(&log),
            refusing: true,
            dialed: Mutex::default(),
        });
        Self {
            url: format!("http://{MEMORY_AUTHORITY}"),
            log,
            task: None,
            dialer: Some(dialer),
        }
    }

    /// How to connect to an in-memory server.
    pub fn dialer(&self) -> Arc<dyn Dialer> {
        Arc::clone(self.dialer.as_ref().expect("an in-memory server")) as Arc<dyn Dialer>
    }

    /// Where the client connected, for an in-memory server: one `(host, port)` per connection.
    pub fn dialed(&self) -> Vec<(String, u16)> {
        self.dialer
            .as_ref()
            .map(|d| d.dialed.lock().unwrap().clone())
            .unwrap_or_default()
    }

    /// `http://127.0.0.1:port`, the form `BaseUrl` accepts.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for RawServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// The URL of a loopback port where nothing listens, so a connection is refused.
pub async fn unreachable_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

/// Read one request, answer it, and return the stream if it must stay open.
async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    handler: &Arc<Handler>,
    log: &Arc<Mutex<Vec<RecordedRequest>>>,
) -> Option<S> {
    let request = read_request(&mut stream).await?;
    let reply = handler(&request);
    log.lock().unwrap().push(request);
    match reply.render() {
        Some(bytes) => {
            let _ = stream.write_all(&bytes).await;
            let _ = stream.shutdown().await;
            None
        }
        None if matches!(reply, Reply::Hang) => Some(stream),
        None => None,
    }
}

async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Option<RecordedRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(RecordedRequest {
        method,
        target,
        headers,
        body,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
