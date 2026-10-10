//! The whole agent against a fake hub, for the numbers the budgets are about (T4, P1, P3): a change made on the file
//! system and the time until its delta is at the hub, and the bytes on the wire when nothing happens.
//!
//! The same code runs in virtual time (the fast tests) and in real time (the ignored test and the benchmarks). The
//! network is real TLS 1.3 over in-memory pipes (`FakeNet`), so bytes are counted as a network would carry them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use agent::clock::Clock;
use agent::dispatch::CommandHandler;
use agent::ops::Metrics;
use agent::scan::{ScanHandler, Scanner};
use agent::spool::{Spool, SpoolLimits, SpoolOptions, SpoolVolume};
use agent::transport::session::{Session, SessionConfig};
use agent::tree::TreeSource;
use proto::convert::FromAgent;
use tempfile::TempDir;
use tokio::runtime::{Handle, RuntimeFlavor};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};

use super::fake_hub::ConnHandle;
use super::rig::{Rig, hello};

/// The agent's scanner and session, running against a fake hub.
pub struct Harness {
    pub rig: Rig,
    pub scanner: Scanner,
    /// The spool every delta goes through, on a temporary directory.
    pub spool: Spool,
    spool_files: TempDir,
    pub session: Arc<Session>,
    pub conn: ConnHandle,
    /// When the scanner started: its walks are on a 10 s grid from here.
    pub started: Instant,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Harness {
    /// Start the scanner and the session, and wait until the hub has the first heartbeat.
    pub async fn start(source: Arc<dyn TreeSource>) -> Self {
        Self::start_with(source, Rig::new()).await
    }

    pub async fn start_with(source: Arc<dyn TreeSource>, rig: Rig) -> Self {
        Self::start_with_commands(source, rig, |_scanner| None).await
    }

    /// As [`Harness::start_with`], and the commands other than the scan requests go to the handler `commands` builds
    /// from the scanner (so a handler can tell the scanner's tree about the files it writes).
    pub async fn start_with_commands(
        source: Arc<dyn TreeSource>,
        rig: Rig,
        commands: impl FnOnce(&Scanner) -> Option<Arc<dyn CommandHandler>>,
    ) -> Self {
        let identity = rig.identity_handle().await;
        let clock: Arc<dyn Clock> = rig.clock.clone();
        let spool_dir = TempDir::new().unwrap();
        let (spool, _) = Spool::open(
            SpoolVolume::open(spool_dir.path()).unwrap(),
            SpoolOptions::new(SpoolLimits::new(64 * 1024 * 1024, 100_000)).with_io(spool_io()),
            clock.clone(),
        )
        .unwrap();
        let scanner = Scanner::with_seq(source, clock, spool.sink(), Metrics::detached(), spool.seq());
        let mut scan_handler = ScanHandler::new(scanner.clone(), spool.clone());
        if let Some(commands) = commands(&scanner) {
            scan_handler = scan_handler.with_commands(commands);
        }
        let handler = Arc::new(scan_handler);
        let session = Arc::new(Session::new(
            hello(),
            rig.transport.clone(),
            identity,
            SessionConfig::default(),
        ));
        let started = Instant::now();
        let mut tasks = Vec::new();
        {
            let scanner = scanner.clone();
            tasks.push(tokio::spawn(async move {
                let never = scanner.run().await;
                match never {}
            }));
        }
        {
            let session = session.clone();
            tasks.push(tokio::spawn(async move {
                let never = session.run(handler).await;
                match never {}
            }));
        }
        let conn = rig.server.wait_for_connection(1).await;
        conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
        Self {
            rig,
            scanner,
            spool,
            spool_files: spool_dir,
            session,
            conn,
            started,
            tasks,
        }
    }

    /// The directory the spool lives in.
    pub fn spool_dir(&self) -> &std::path::Path {
        self.spool_files.path()
    }

    /// Wait until `deadline`, make `change` (which returns the path it wrote) and measure how long the hub takes to
    /// get a delta that carries that path.
    pub async fn latency_of(&self, deadline: Instant, change: impl FnOnce() -> String) -> Duration {
        sleep_until(deadline).await;
        let path = change();
        let made = Instant::now();
        self.conn
            .wait_for(
                |m| matches!(m, FromAgent::Delta(d) if d.entries.iter().any(|e| e.path.as_str() == path)),
            )
            .await;
        made.elapsed()
    }
}

/// Where the spool's file operations run: inline on a current-thread runtime, which is what the tests in virtual time use
/// (time must not jump while a `spawn_blocking` task works), and on the blocking pool on a multi-thread runtime, which
/// is the real-time tests and the benchmarks.
pub fn spool_io() -> agent::spool::IoMode {
    match Handle::current().runtime_flavor() {
        RuntimeFlavor::CurrentThread => agent::spool::IoMode::Inline,
        _ => agent::spool::IoMode::Blocking,
    }
}

/// Twelve moments spread over one walk interval, none of them on a tick: the worst case for a change is just after a
/// walk, the best just before one, and a p95 over the spread is what a user sees.
pub fn phase_offsets(interval: Duration, count: u32) -> Vec<Duration> {
    (0..count)
        .map(|i| interval.mul_f64((f64::from(i) + 0.5) / f64::from(count)))
        .collect()
}

/// Make one change at each offset into a walk interval, one walk interval apart, and return the latency of each.
pub async fn measure_oob_latencies(
    harness: &Harness,
    interval: Duration,
    offsets: &[Duration],
    mut change: impl FnMut(usize) -> String,
) -> Vec<Duration> {
    let mut latencies = Vec::with_capacity(offsets.len());
    for (i, offset) in offsets.iter().enumerate() {
        // The offset into the walk interval that is running now, or into the next one if that moment has passed.
        let since = harness.started.elapsed();
        let current_tick = interval.mul_f64((since.as_secs_f64() / interval.as_secs_f64()).floor());
        let mut deadline = harness.started + current_tick + *offset;
        if deadline <= Instant::now() {
            deadline += interval;
        }
        latencies.push(harness.latency_of(deadline, || change(i)).await);
    }
    latencies
}

/// Bytes on the wire per virtual or real minute, for `minutes` windows, after the connection has settled.
pub async fn idle_bytes_per_minute(harness: &Harness, minutes: u32) -> Vec<u64> {
    let net = &harness.rig.server.net;
    let mut windows = Vec::with_capacity(minutes as usize);
    let mut before = net.counts();
    for _ in 0..minutes {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let after = net.counts();
        windows.push(after.since(before).total());
        before = after;
    }
    windows
}
