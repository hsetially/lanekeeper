//! N hub replicas' event buses and SSE endpoints in one process over one real Postgres, for the event tests (D80, P13).
//!
//! Each replica has its own connection pool, its own `PgEventBus` (its own `LISTEN` connection and relay) and its own SSE
//! endpoint on a real socket, as three hub pods would. Clients speak real HTTP/1.1 to them.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{Role, UserStatus};
use http::{Request, StatusCode};
use http_body_util::{BodyExt, Empty};
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::EventMetrics;
use hub_platform::events::sse::{self, SseSettings, SseState};
use hub_platform::events::visibility::RoleVisibility;
use hub_platform::{FakeSessionAuth, GuardedRouter, TEST_USER_HEADER};
use hyper_util::rt::TokioIo;
use sqlx::PgPool;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::pg::TestDb;
use super::sse_client::Frames;

pub struct EventReplica {
    pub name: String,
    pub bus: PgEventBus,
    pub pool: PgPool,
    pub metrics: EventMetrics,
    pub sse: Arc<SseState>,
    pub addr: SocketAddr,
    server: JoinHandle<()>,
    closing: watch::Sender<bool>,
}

impl EventReplica {
    /// Tell the SSE streams of this replica that the hub is shutting down.
    pub fn close_streams(&self) {
        self.closing.send_replace(true);
    }
}

impl Drop for EventReplica {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub struct EventCluster {
    pub db: TestDb,
    pub auth: Arc<FakeSessionAuth>,
    pub replicas: Vec<EventReplica>,
    stop: watch::Sender<bool>,
}

impl EventCluster {
    /// `None` when there is no Docker and it is not required (the helper has said so).
    pub async fn start(replicas: usize, bus: BusSettings, sse: SseSettings) -> Option<Self> {
        let db = TestDb::ready().await?;
        let auth = Arc::new(
            FakeSessionAuth::new()
                .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
                .with_user("admin", Some(Role::Admin), UserStatus::Active),
        );
        let (stop, stop_rx) = watch::channel(false);
        let mut out = Vec::new();
        for i in 0..replicas {
            let pool = db.app_pool().await;
            let metrics = EventMetrics::default();
            let event_bus = PgEventBus::new(pool.clone(), bus.clone(), metrics.clone());
            event_bus.start(stop_rx.clone()).await;
            let (closing, closing_rx) = watch::channel(false);
            let state = SseState::new(
                event_bus.source(),
                Arc::clone(&auth) as _,
                Arc::new(RoleVisibility),
                sse.clone(),
                closing_rx,
                metrics.clone(),
            );
            let router = sse::mount(
                GuardedRouter::new(Arc::clone(&auth) as _, None),
                Arc::clone(&state),
            );
            let (router, _) = router.into_parts().unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let _ = axum::serve(listener, router).await;
            });
            out.push(EventReplica {
                name: format!("hub-{i}"),
                bus: event_bus,
                pool,
                metrics,
                sse: state,
                addr,
                server,
                closing,
            });
        }
        Some(Self {
            db,
            auth,
            replicas: out,
            stop,
        })
    }
}

impl Drop for EventCluster {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// An open SSE connection over a real socket.
pub struct SseConn {
    pub status: StatusCode,
    pub frames: Frames<http_body_util::BodyDataStream<hyper::body::Incoming>>,
}

/// Open `GET {path}` on `addr` as `user`, with an optional `Last-Event-ID`. Returns once the response head has arrived.
pub async fn connect(addr: SocketAddr, user: &str, path: &str, last_event_id: Option<&str>) -> SseConn {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut request = Request::builder()
        .uri(path)
        .header(http::header::HOST, "localhost")
        .header(TEST_USER_HEADER, user);
    if let Some(id) = last_event_id {
        request = request.header("last-event-id", id);
    }
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        sender.send_request(request.body(Empty::<Bytes>::new()).unwrap()),
    )
    .await
    .expect("the response head arrives")
    .unwrap();
    SseConn {
        status: response.status(),
        frames: Frames::new(response.into_body().into_data_stream()),
    }
}

/// Wait for `condition` for up to `within`, polling every 10 ms. Panics with `what` on a timeout.
pub async fn wait_for(what: &str, within: Duration, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if condition().await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
