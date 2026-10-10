//! Shutting the session down (T7, S6): what is queued reaches the hub, the stream is closed in an orderly way, and the
//! session returns, whatever it was doing (connecting, waiting to retry, or talking to a hub that has gone quiet).
//!
//! Under `tokio::time::pause`, against the fake hub on the in-memory network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::transport::session::{Link, LinkHandler, Session, SessionConfig};
use async_trait::async_trait;
use domain::{ContentHash, Heartbeat};
use proto::convert::FromAgent;
use support::rig::{Rig, hello};
use tokio::time::{Instant, sleep, timeout};

fn beat(n: u64) -> FromAgent {
    FromAgent::Heartbeat(Heartbeat {
        scan_seq: n,
        merkle_root: ContentHash::from_bytes([7; 32]),
        file_count: n,
    })
}

/// Queues `count` heartbeats the moment the link starts, asks the session to shut down, and then idles like a real
/// handler would.
struct QueueThenStop {
    session: std::sync::Mutex<Option<Arc<Session>>>,
    count: u64,
}

#[async_trait]
impl LinkHandler for QueueThenStop {
    async fn handle(&self, link: Link) {
        for n in 1..=self.count {
            link.outbox.send(beat(n)).await.unwrap();
        }
        let session = self.session.lock().unwrap().clone().unwrap();
        session.shutdown();
        let _keep = link;
        std::future::pending::<()>().await;
    }
}

async fn session_for(rig: &Rig, config: SessionConfig) -> Arc<Session> {
    let identity = rig.identity_handle().await;
    Arc::new(Session::new(hello(), rig.transport.clone(), identity, config))
}

#[tokio::test(start_paused = true)]
async fn shutdown_flushes_and_exits() {
    let rig = Rig::new();
    let session = session_for(&rig, SessionConfig::default()).await;
    let handler = Arc::new(QueueThenStop {
        session: std::sync::Mutex::new(Some(session.clone())),
        count: 40,
    });
    let running = {
        let (session, handler) = (session.clone(), handler.clone());
        tokio::spawn(async move { session.serve(handler).await })
    };
    timeout(Duration::from_secs(30), running).await.unwrap().unwrap();

    let conn = rig.server.connection(0);
    conn.wait_closed().await;
    let beats = conn
        .received()
        .into_iter()
        .filter(|m| matches!(m, FromAgent::Heartbeat(_)))
        .count();
    assert_eq!(
        beats, 40,
        "every message queued before the shutdown reached the hub"
    );
    assert_eq!(
        *session.state().borrow(),
        agent::transport::session::ConnectionState::Disconnected
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_before_the_first_connection_returns_at_once() {
    let rig = Rig::new();
    let session = session_for(&rig, SessionConfig::default()).await;
    rig.server.net.set_reachable(false);
    let running = {
        let session = session.clone();
        tokio::spawn(async move { session.serve(Arc::new(support::rig::Recorder::default())).await })
    };
    sleep(Duration::from_secs(3)).await;
    assert!(!running.is_finished(), "still retrying");
    let asked = Instant::now();
    session.shutdown();
    timeout(Duration::from_secs(1), running).await.unwrap().unwrap();
    assert!(asked.elapsed() < Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn shutdown_while_connecting_does_not_wait_for_the_connect_timeout() {
    let rig = Rig::new();
    // A hub that accepts the connection and never speaks: the session waits for the configuration for 30 s.
    rig.server.set_send_config(false);
    let session = session_for(&rig, SessionConfig::default()).await;
    let running = {
        let session = session.clone();
        tokio::spawn(async move { session.serve(Arc::new(support::rig::Recorder::default())).await })
    };
    sleep(Duration::from_secs(5)).await;
    session.shutdown();
    timeout(Duration::from_secs(1), running).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_silent_hub_cannot_hold_the_shutdown_for_more_than_the_flush_and_close_waits() {
    let rig = Rig::new();
    let config = SessionConfig {
        flush_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    };
    let session = session_for(&rig, config).await;
    let handler = Arc::new(support::rig::Recorder::default());
    let running = {
        let (session, handler) = (session.clone(), handler.clone());
        tokio::spawn(async move { session.serve(handler).await })
    };
    rig.server.wait_for_connection(1).await;
    sleep(Duration::from_secs(1)).await;
    // The path dies without a reset, with a message waiting that can never be delivered.
    rig.server.net.silence_connections();
    let outbox = handler.outboxes().into_iter().next().unwrap();
    for n in 0..30 {
        let _ = outbox.try_send(beat(n));
    }
    let asked = Instant::now();
    session.shutdown();
    timeout(Duration::from_secs(60), running).await.unwrap().unwrap();
    // Handing on (at most the flush timeout) and then waiting for the hub to end its side (a fixed two seconds).
    let waited = asked.elapsed();
    assert!(waited <= Duration::from_secs(8), "waited {waited:?}");
}
