//! A hub that restarts again and again, and what the agent looks like after each time (T3, S6).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use agent::transport::session::{ConnectionState, Session, SessionConfig};
use proto::convert::FromAgent;
use tokio::runtime::Handle;
use tokio::time::sleep;

use super::rig::{Recorder, Rig, hello};

/// What was measured after each connection was up and had carried traffic.
#[derive(Debug)]
pub struct Report {
    /// Live tasks in the runtime.
    pub tasks: Vec<usize>,
    /// Resident memory, in 4 KiB pages.
    pub pages: Vec<u64>,
}

/// Resident set size in 4 KiB pages, from `/proc/self/statm`.
fn resident_pages() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    statm.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// Run a session against a hub that is killed and brought back `restarts` times, checking after each restart that the
/// agent came back with a whole connection: `Hello`, the configuration, a running handler, and traffic from it.
///
/// Everything that is not the agent is kept from growing: the hub forgets old streams, the recorder keeps only the
/// latest link, and the network forgets finished connections. So what this measures is the agent and its libraries.
pub async fn run(restarts: usize) -> Report {
    let rig = Rig::new();
    let identity = rig.identity_handle().await;
    let session = Arc::new(Session::new(
        hello(),
        rig.transport.clone(),
        identity,
        SessionConfig::default(),
    ));
    let recorder = Recorder::lean();
    let run = {
        let (session, recorder) = (session.clone(), recorder.clone());
        tokio::spawn(async move { session.run(recorder).await })
    };

    let mut report = Report {
        tasks: Vec::new(),
        pages: Vec::new(),
    };
    for n in 1..=restarts + 1 {
        let conn = rig.server.wait_for_connection(n).await;
        conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
        tokio::time::timeout(
            Duration::from_secs(600),
            session.state().wait_for(|s| *s == ConnectionState::Connected),
        )
        .await
        .unwrap_or_else(|_| panic!("connection {n}: the session never became connected"))
        .unwrap();
        assert_eq!(conn.hello(), hello(), "connection {n}");

        // Long enough to count as a good connection, so that restarts are answered quickly.
        sleep(Duration::from_secs(6)).await;
        rig.server.forget_connections_before(n);
        report.tasks.push(Handle::current().metrics().num_alive_tasks());
        report.pages.push(resident_pages());

        if n <= restarts {
            // The hub process dies and is gone for five seconds.
            rig.server.restart(Duration::from_secs(5)).await;
        }
    }
    // Let whatever is still shutting down finish.
    sleep(Duration::from_secs(5)).await;

    assert_eq!(
        rig.server.connection_count(),
        restarts + 1,
        "one stream per restart, and the first"
    );
    assert_eq!(
        usize::try_from(session.stats().connections()).unwrap(),
        restarts + 1
    );
    assert_eq!(
        recorder.links_started(),
        restarts + 1,
        "the handler ran on every connection"
    );
    assert_eq!(
        rig.server.net.open_connections(),
        1,
        "only the live connection is open on the hub's side"
    );
    assert_eq!(
        recorder.stale_outboxes(),
        0,
        "the outbox of each old connection was closed before the next connection began"
    );
    assert!(!recorder.outboxes()[0].is_closed(), "and the live one is open");
    run.abort();
    report
}
