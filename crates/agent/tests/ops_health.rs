//! `/healthz`, `/readyz` and `/metrics` as the kubelet and Prometheus see them (T7, S16): over a real socket on the
//! loopback interface, in real time, plus the liveness rules in virtual time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::clock::SystemClock;
use agent::config::BaseUrl;
use agent::http::{HttpClient, HttpRequest, HttpResponse};
use agent::ops::health::{ServerLimits, serve_with};
use agent::ops::{Health, Metrics, NotReady};
use agent::transport::session::ConnectionState;
use support::clock::TestClock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::sleep;

struct Server {
    base: BaseUrl,
    addr: std::net::SocketAddr,
    health: Arc<Health>,
    task: JoinHandle<std::convert::Infallible>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(limits: ServerLimits) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let health = Health::new(Arc::new(SystemClock));
    let task = tokio::spawn(serve_with(listener, health.clone(), Metrics::new(), limits));
    Server {
        base: BaseUrl::parse(&format!("http://{addr}"), "http").unwrap(),
        addr,
        health,
        task,
    }
}

async fn get(server: &Server, target: &str) -> HttpResponse {
    HttpClient::new(Duration::from_secs(5), 1 << 20)
        .send(&server.base, HttpRequest::get(target))
        .await
        .unwrap()
}

fn text(response: &HttpResponse) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

#[tokio::test]
async fn the_three_endpoints_answer_over_a_real_socket() {
    let server = serve(ServerLimits::default()).await;
    let (tx, rx) = watch::channel(ConnectionState::Connecting);
    server.health.follow_connection(rx);

    let health = get(&server, "/healthz").await;
    assert_eq!((health.status, text(&health).as_str()), (200, "ok\n"));

    let not_ready = get(&server, "/readyz").await;
    assert_eq!(
        (not_ready.status, text(&not_ready).as_str()),
        (503, "not ready: connecting\n")
    );
    tx.send(ConnectionState::Connected).unwrap();
    let ready = get(&server, "/readyz").await;
    assert_eq!((ready.status, text(&ready).as_str()), (200, "ready\n"));

    let metrics = get(&server, "/metrics").await;
    assert_eq!(metrics.status, 200);
    assert!(text(&metrics).contains("lanekeeper_agent_files_tracked"));
}

#[tokio::test]
async fn readyz_only_while_connected() {
    let server = serve(ServerLimits::default()).await;
    assert_eq!(
        server.health.ready(),
        Err(NotReady::NoSession),
        "before a session exists"
    );
    let (tx, rx) = watch::channel(ConnectionState::Disconnected);
    server.health.follow_connection(rx);
    for (state, ready) in [
        (ConnectionState::Disconnected, 503),
        (ConnectionState::Connecting, 503),
        (ConnectionState::Connected, 200),
        (ConnectionState::Disconnected, 503),
        (ConnectionState::Connected, 200),
    ] {
        tx.send(state).unwrap();
        assert_eq!(get(&server, "/readyz").await.status, ready, "{state:?}");
    }
    server.health.begin_shutdown();
    let shutting = get(&server, "/readyz").await;
    assert_eq!(
        (shutting.status, text(&shutting).as_str()),
        (503, "not ready: shutting down\n")
    );
    // Shutting down is not a reason to be killed: the liveness probe still passes.
    assert_eq!(get(&server, "/healthz").await.status, 200);
}

/// One raw request on a fresh connection; the whole reply as text.
async fn raw(addr: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut reply))
        .await
        .expect("the server closes the connection after one answer")
        .unwrap();
    String::from_utf8_lossy(&reply).into_owned()
}

#[tokio::test]
async fn only_get_and_head_on_three_paths_and_nothing_leaks() {
    let server = serve(ServerLimits::default()).await;
    let post = raw(
        server.addr,
        "POST /healthz HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    )
    .await;
    assert!(post.starts_with("HTTP/1.1 405"), "{post}");
    assert!(post.to_ascii_lowercase().contains("allow: get, head"), "{post}");

    let missing = raw(server.addr, "GET /debug/pprof HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");

    // A path traversal, a query and an absolute URI are just paths nobody serves.
    for target in [
        "/../etc/passwd",
        "/healthz/",
        "/metrics?x=1&y=%00",
        "http://evil/healthz",
    ] {
        let reply = raw(server.addr, &format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n")).await;
        assert!(
            reply.starts_with("HTTP/1.1 404") || reply.starts_with("HTTP/1.1 200"),
            "{target}: {reply}"
        );
    }

    let head = raw(server.addr, "HEAD /healthz HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.ends_with("\r\n\r\n"), "a HEAD answer has no body: {head:?}");

    let ok = raw(server.addr, "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").await;
    let lower = ok.to_ascii_lowercase();
    assert!(lower.contains("cache-control: no-store") && lower.contains("x-content-type-options: nosniff"));
    assert!(!lower.contains("\r\nserver:"), "no software banner: {ok}");

    // Garbage is answered with a closed connection, not a panic, and the server keeps serving.
    let garbage = raw(
        server.addr,
        "\x16\x03\x01\x02\x00\x01\x00\x01\x7f\x03\x03 not http\r\n\r\n",
    )
    .await;
    assert!(!garbage.contains("200"), "{garbage}");
    assert_eq!(get(&server, "/healthz").await.status, 200);
}

#[tokio::test]
async fn a_client_that_sends_nothing_is_cut_off_and_the_connection_limit_holds() {
    let limits = ServerLimits {
        connections: 3,
        header_timeout: Duration::from_millis(300),
        connection_timeout: Duration::from_millis(600),
    };
    let server = serve(limits).await;

    // Three clients connect and say nothing: they fill the limit.
    let mut idle = Vec::new();
    for _ in 0..3 {
        idle.push(TcpStream::connect(server.addr).await.unwrap());
    }
    sleep(Duration::from_millis(100)).await;
    // The fourth is closed without an answer, at once.
    let mut fourth = TcpStream::connect(server.addr).await.unwrap();
    let mut buffer = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_millis(250), fourth.read(&mut buffer))
        .await
        .expect("closed at once, not held until a timeout");
    assert_eq!(read.unwrap_or(0), 0);

    // The idle ones are cut off after the header timeout, and then the port serves again.
    sleep(Duration::from_millis(900)).await;
    for stream in &mut idle {
        let n = tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buffer))
            .await
            .expect("an idle client is closed by the server")
            .unwrap_or(0);
        assert_eq!(n, 0);
    }
    assert_eq!(get(&server, "/healthz").await.status, 200);
}

// ------------------------------------------------------------------------------------------ liveness, virtual time

#[tokio::test(start_paused = true)]
async fn healthz_ok_while_loops_tick() {
    let clock = Arc::new(TestClock::starting_at(1_791_633_600_000));
    let health = Health::new(clock);
    let progress = health.progress("scanner progress", Duration::from_secs(60));
    let _alive = health.task("scanner");
    assert!(health.unhealthy().is_empty());

    // A loop that keeps ticking is healthy for as long as it ticks.
    for _ in 0..20 {
        sleep(Duration::from_secs(30)).await;
        progress.tick();
        assert!(health.unhealthy().is_empty());
    }
    // One that stops is unhealthy once its allowed age has passed, and not before.
    sleep(Duration::from_secs(59)).await;
    assert!(health.unhealthy().is_empty());
    sleep(Duration::from_secs(2)).await;
    assert_eq!(health.unhealthy(), vec!["scanner progress"]);
    // And it recovers when the loop moves again.
    progress.tick();
    assert!(health.unhealthy().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_loop_that_ended_fails_healthz_at_once_and_a_hub_outage_does_not() {
    let clock = Arc::new(TestClock::starting_at(1_791_633_600_000));
    let health = Health::new(clock);
    let guard = health.task("session");
    let _other = health.task("scanner");
    let (tx, rx) = watch::channel(ConnectionState::Disconnected);
    health.follow_connection(rx);

    // Not connected for a day: not ready, but alive. Restarting the container would not bring the hub back.
    sleep(Duration::from_secs(24 * 3600)).await;
    assert!(health.ready().is_err());
    assert!(health.unhealthy().is_empty());
    drop(tx);

    drop(guard);
    assert_eq!(health.unhealthy(), vec!["session"]);
}
