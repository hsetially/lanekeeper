//! A network for the transport tests: in-memory pipes that the test can count, cut and refuse, with real TLS on top.
//!
//! The agent's `Dialer` is replaced by [`NetDialer`], which hands the agent one end of a `tokio::io::duplex` pipe and
//! gives the other end to the fake hub after a real TLS handshake. Nothing touches a socket, so tests run under
//! `tokio::time::pause` and stay deterministic, and the bytes counted on the agent's end are the TLS records a real
//! network would carry (the handshake included; `Counts::since` takes it out).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use agent::transport::dial::{BoxedIo, Dialer};
use async_trait::async_trait;
use rustls::ServerConfig;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tonic::transport::server::Connected;

// ------------------------------------------------------------------ cutting a connection

/// Cuts one connection from the hub's side: after `kill`, every read and write on it fails.
#[derive(Debug, Default)]
pub struct Switch {
    killed: std::sync::atomic::AtomicBool,
    /// The path is dead but nobody says so: nothing is read or written, and no error is raised.
    silent: std::sync::atomic::AtomicBool,
    wakers: Mutex<Vec<(u8, Waker)>>,
}

impl Switch {
    pub fn go_silent(&self) {
        self.silent.store(true, Ordering::SeqCst);
    }

    fn is_silent(&self) -> bool {
        self.silent.load(Ordering::SeqCst)
    }

    pub fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        for (_, waker) in self.wakers.lock().unwrap().drain(..) {
            waker.wake();
        }
    }

    fn is_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    /// Remember who to wake. One slot per direction, so the list cannot grow.
    fn register(&self, slot: u8, waker: &Waker) {
        let mut wakers = self.wakers.lock().unwrap();
        match wakers.iter_mut().find(|(s, _)| *s == slot) {
            Some((_, existing)) => existing.clone_from(waker),
            None => wakers.push((slot, waker.clone())),
        }
    }
}

/// The hub's end of the pipe, which a test can cut.
pub struct Killable<S> {
    inner: S,
    switch: Arc<Switch>,
}

fn reset() -> io::Error {
    io::Error::from(io::ErrorKind::ConnectionReset)
}

impl<S: AsyncRead + Unpin> AsyncRead for Killable<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.switch.register(0, cx.waker());
        if self.switch.is_killed() {
            return Poll::Ready(Err(reset()));
        }
        if self.switch.is_silent() {
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Killable<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.switch.register(1, cx.waker());
        if self.switch.is_killed() {
            return Poll::Ready(Err(reset()));
        }
        if self.switch.is_silent() {
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.switch.register(1, cx.waker());
        if self.switch.is_killed() {
            return Poll::Ready(Err(reset()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

// ------------------------------------------------------------------ counting bytes

/// Bytes that crossed the agent's end of the pipe, in both directions, summed over every connection.
#[derive(Debug, Default)]
pub struct Counters {
    to_hub: AtomicU64,
    from_hub: AtomicU64,
}

/// A reading of [`Counters`] at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub to_hub: u64,
    pub from_hub: u64,
}

impl Counts {
    pub fn total(self) -> u64 {
        self.to_hub + self.from_hub
    }

    /// What was counted after `earlier`.
    pub fn since(self, earlier: Counts) -> Counts {
        Counts {
            to_hub: self.to_hub - earlier.to_hub,
            from_hub: self.from_hub - earlier.from_hub,
        }
    }
}

/// The agent's end of the pipe, counting what passes.
pub struct Counting<S> {
    inner: S,
    counters: Arc<Counters>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Counting<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &polled {
            self.counters
                .from_hub
                .fetch_add((buf.filled().len() - before) as u64, Ordering::SeqCst);
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counting<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &polled {
            self.counters.to_hub.fetch_add(*n as u64, Ordering::SeqCst);
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

// ------------------------------------------------------------------ the hub's end

/// What the hub learned about the peer from the TLS handshake.
#[derive(Debug, Clone, Default)]
pub struct PeerInfo {
    /// The leaf of the client certificate chain, DER. `None` when the client presented none (a `Join`).
    pub client_cert: Option<Vec<u8>>,
    pub version: Option<rustls::ProtocolVersion>,
}

/// Counts the connections the hub currently holds open, and forgets the connection's switch when it ends, so that a
/// long test does not accumulate one switch (and the wakers inside it) per connection.
struct OpenGuard {
    net: Arc<FakeNet>,
    switch: Arc<Switch>,
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.net.open.fetch_sub(1, Ordering::SeqCst);
        self.net
            .state
            .lock()
            .unwrap()
            .switches
            .retain(|s| !Arc::ptr_eq(s, &self.switch));
    }
}

/// The hub's end of one connection after the TLS handshake, in the shape tonic's server wants.
pub struct HubIo {
    stream: TlsStream<Killable<DuplexStream>>,
    peer: PeerInfo,
    _open: OpenGuard,
}

impl Connected for HubIo {
    type ConnectInfo = PeerInfo;

    fn connect_info(&self) -> PeerInfo {
        self.peer.clone()
    }
}

impl AsyncRead for HubIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for HubIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

// ------------------------------------------------------------------ the network

type Serve = dyn Fn(HubIo) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync;

struct State {
    reachable: bool,
    tls: Arc<ServerConfig>,
    dials: Vec<Instant>,
    switches: Vec<Arc<Switch>>,
}

/// The network between the agent and the fake hub.
pub struct FakeNet {
    state: Mutex<State>,
    counters: Arc<Counters>,
    open: Arc<AtomicUsize>,
    handshake_failures: AtomicUsize,
    serve: Box<Serve>,
}

impl std::fmt::Debug for FakeNet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeNet")
    }
}

impl FakeNet {
    /// A network whose hub speaks TLS with `tls` and serves every accepted connection with `serve`.
    pub fn new<F, Fut>(tls: Arc<ServerConfig>, serve: F) -> Arc<Self>
    where
        F: Fn(HubIo) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Arc::new(Self {
            state: Mutex::new(State {
                reachable: true,
                tls,
                dials: Vec::new(),
                switches: Vec::new(),
            }),
            counters: Arc::new(Counters::default()),
            open: Arc::new(AtomicUsize::new(0)),
            handshake_failures: AtomicUsize::new(0),
            serve: Box::new(move |io| Box::pin(serve(io))),
        })
    }

    /// The dialer to give the agent's transport.
    pub fn dialer(self: &Arc<Self>) -> Arc<dyn Dialer> {
        Arc::new(NetDialer {
            net: Arc::clone(self),
        })
    }

    /// While `false`, every dial is refused, as when the hub's pod is gone.
    pub fn set_reachable(&self, reachable: bool) {
        self.state.lock().unwrap().reachable = reachable;
    }

    /// The hub's TLS settings from now on (for connections made after this call).
    pub fn set_tls(&self, tls: Arc<ServerConfig>) {
        self.state.lock().unwrap().tls = tls;
    }

    /// Cut every connection from the hub's side, as when the hub process dies.
    pub fn kill_connections(&self) {
        let switches = std::mem::take(&mut self.state.lock().unwrap().switches);
        for switch in switches {
            switch.kill();
        }
    }

    /// Make every connection go silent: nothing more is delivered in either direction, and nothing fails. This is a
    /// path that died without a reset, which only keepalive can find.
    pub fn silence_connections(&self) {
        for switch in &self.state.lock().unwrap().switches {
            switch.go_silent();
        }
    }

    /// When each dial was made (the paused clock's time), including the refused ones.
    pub fn dials(&self) -> Vec<Instant> {
        self.state.lock().unwrap().dials.clone()
    }

    pub fn counts(&self) -> Counts {
        Counts {
            to_hub: self.counters.to_hub.load(Ordering::SeqCst),
            from_hub: self.counters.from_hub.load(Ordering::SeqCst),
        }
    }

    /// Connections the hub currently holds open.
    pub fn open_connections(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    /// TLS handshakes the hub refused.
    pub fn handshake_failures(&self) -> usize {
        self.handshake_failures.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct NetDialer {
    net: Arc<FakeNet>,
}

#[async_trait]
impl Dialer for NetDialer {
    async fn dial(&self, _host: &str, _port: u16) -> io::Result<BoxedIo> {
        let net = &self.net;
        let (tls, switch) = {
            let mut state = net.state.lock().unwrap();
            state.dials.push(Instant::now());
            if !state.reachable {
                return Err(io::Error::from(io::ErrorKind::ConnectionRefused));
            }
            let switch = Arc::new(Switch::default());
            state.switches.push(Arc::clone(&switch));
            (Arc::clone(&state.tls), switch)
        };
        let (agent_end, hub_end) = tokio::io::duplex(64 * 1024);
        net.open.fetch_add(1, Ordering::SeqCst);
        let open = OpenGuard {
            net: Arc::clone(net),
            switch: Arc::clone(&switch),
        };
        let net = Arc::clone(net);
        tokio::spawn(async move {
            let hub_end = Killable {
                inner: hub_end,
                switch,
            };
            match TlsAcceptor::from(tls).accept(hub_end).await {
                Ok(stream) => {
                    let (_, connection) = stream.get_ref();
                    let peer = PeerInfo {
                        client_cert: connection
                            .peer_certificates()
                            .and_then(|chain| chain.first())
                            .map(|leaf| leaf.to_vec()),
                        version: connection.protocol_version(),
                    };
                    (net.serve)(HubIo {
                        stream,
                        peer,
                        _open: open,
                    })
                    .await;
                }
                Err(_) => {
                    net.handshake_failures.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        Ok(Box::new(Counting {
            inner: agent_end,
            counters: Arc::clone(&self.net.counters),
        }))
    }
}
