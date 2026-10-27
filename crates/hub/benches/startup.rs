//! P11: the hub starts in under 10 s (budget `P11`, `hub/startup`, p95).
//!
//! The sample is the wall-clock time from spawning a new process to its `/readyz` answering 200, with a real Postgres 16 whose
//! migrations are already applied and mirrors of the target-scale fixture repositories already on the replica's volume (a restart
//! of a running replica, which is the case the budget is about; the first start fetches both repositories instead and is bounded by
//! the network, not by this code). The process is this benchmark's own executable run again with `LK_STARTUP_CHILD=1`: it does
//! what the hub does after it has read its settings, in the order it does it (telemetry, the pool, both mirrors, the reader and its
//! hashing pool, the outbox bus and its `LISTEN` connection, the blob store, the Git announcer, fetch worker and webhook, the
//! composed router, both listeners), then polls its own operations listener until `/readyz` is 200, prints `READY` and waits for
//! its stdin to close. The parent measures from just before the spawn to that line, so process start, the runtime and dynamic
//! linking are in the number. 2 warm-up starts, then 10 measured. Writes `target/perf-results/hub_startup.json`.
//!
//! Not in the number: the TLS handshake of the pool's first connection and the web listener's certificate (the benchmark's database
//! and listeners are plain, so that it needs no PKI; a handshake is a few milliseconds), and the identity and registry modules of
//! prompts 03a and 05, which prompt 16 adds to this measurement.
//!
//! Needs Docker (testcontainers) and the fixtures (`cargo xtask gen-fixtures --scale target`).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::too_many_lines
)]

#[path = "../../hub-platform/tests/support/perf.rs"]
mod perf;
#[path = "../../hub-platform/tests/support/pg.rs"]
mod pg;

use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::RepoKind;
use hub::blobs::BlobsModule;
use hub::git::MirrorsReady;
use hub::serve::{ServeLimits, serve};
use hub::{Deps, EventsModule, GitModule, Hub, HubSettings, SessionEdge};
use hub_platform::FakeSessionAuth;
use hub_platform::blobs::endpoint::{BlobState, DenyHashes};
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PgBlobStore};
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::EventMetrics;
use hub_platform::events::sse::{SseSettings, SseState};
use hub_platform::events::visibility::RoleVisibility;
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::deliveries::PgDeliveryLog;
use hub_platform::git::fetch_queue::FetchQueue;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
use hub_platform::git::payload::RepoFullName;
use hub_platform::git::reader::{GixReader, ReaderSettings};
use hub_platform::git::sync::{
    Announcer, FetchWorker, MirrorSync, MirrorSyncer, RetrySettings, StartupRetry, spawn_initialize,
};
use hub_platform::git::webhook::{WebhookSettings, WebhookState};
use ports::EventBus;
use ports::fakes::FakeSecretSource;
use sqlx::postgres::PgPoolOptions;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const WARM_UP: usize = 2;
const MEASURED: usize = 10;
const CHILD: &str = "LK_STARTUP_CHILD";

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn main() -> ExitCode {
    if std::env::var_os(CHILD).is_some() {
        return runtime().block_on(child());
    }
    runtime().block_on(parent())
}

fn fixture_repos() -> (PathBuf, PathBuf) {
    let base = std::env::var_os("LK_FIXTURES_DIR").map_or_else(
        || {
            std::env::var_os("CARGO_TARGET_DIR")
                .map_or_else(
                    || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
                    PathBuf::from,
                )
                .join("fixtures/target")
        },
        PathBuf::from,
    );
    let (b, t) = (
        base.join("repos/configuration-base-saas.git"),
        base.join("repos/csp-tenant-data.git"),
    );
    for repo in [&b, &t] {
        assert!(
            repo.join("HEAD").is_file(),
            "no fixture repository at {}. Run `cargo xtask gen-fixtures --scale target` first.",
            repo.display()
        );
    }
    (b, t)
}

// ------------------------------------------------------------------------------------------------------ the parent

async fn parent() -> ExitCode {
    let (base, tenant) = fixture_repos();
    let Some(db) = pg::TestDb::ready().await else {
        eprintln!("startup: no Docker, so no Postgres to measure against");
        return ExitCode::FAILURE;
    };
    let (base_url, tenant_url) = (
        format!("file://{}", base.display()),
        format!("file://{}", tenant.display()),
    );
    // The replica's volume, filled once: the state a restarting replica finds.
    let volume = tempfile::tempdir().unwrap();
    {
        let mirror = Mirror::open(
            MirrorSettings::new(
                volume.path(),
                RemoteUrl::local(&base_url),
                RemoteUrl::local(&tenant_url),
            ),
            Arc::new(NoCredentials),
            GitMetrics::default(),
        )
        .unwrap();
        mirror.initialize().await.unwrap();
        assert!(mirror.ready());
    }
    let exe = std::env::current_exe().unwrap();
    let mut samples = Vec::with_capacity(MEASURED);
    for n in 0..WARM_UP + MEASURED {
        let started = Instant::now();
        let mut child = tokio::process::Command::new(&exe)
            .env(CHILD, "1")
            .env("LK_STARTUP_DB_URL", db.app_url())
            .env("LK_STARTUP_MIRROR_DIR", volume.path())
            .env("LK_STARTUP_BASE_URL", &base_url)
            .env("LK_STARTUP_TENANT_URL", &tenant_url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let ready = tokio::time::timeout(Duration::from_secs(60), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                if line == "READY" {
                    return true;
                }
            }
            false
        })
        .await;
        let took = started.elapsed();
        if ready != Ok(true) {
            eprintln!("startup: start {n} did not become ready ({ready:?}) within 60 s");
            return ExitCode::FAILURE;
        }
        // Closing its stdin is how the child is told to stop.
        drop(child.stdin.take());
        let _ = tokio::time::timeout(Duration::from_secs(30), child.wait()).await;
        if n >= WARM_UP {
            samples.push(took.as_secs_f64() * 1000.0);
        }
    }
    let path = perf::write_result("hub/startup", "ms", &samples);
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    eprintln!(
        "startup: {} starts, ms: p50 {:.0}, p95 {:.0}, max {:.0} (budget 10000) -> {}",
        samples.len(),
        perf::percentile(&sorted, 0.5),
        perf::percentile(&sorted, 0.95),
        perf::percentile(&sorted, 1.0),
        path.display()
    );
    ExitCode::SUCCESS
}

// ------------------------------------------------------------------------------------------------------- the child

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

/// What the hub does at start-up, in order, until it is ready; then `READY`, and wait to be told to stop.
async fn child() -> ExitCode {
    let settings = HubSettings::default();
    let _telemetry = hub::telemetry::init(&settings).expect("telemetry");

    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&env("LK_STARTUP_DB_URL"))
        .await
        .expect("the database is reachable");

    let git_metrics = GitMetrics::default();
    let mirror = Mirror::open(
        MirrorSettings::new(
            env("LK_STARTUP_MIRROR_DIR"),
            RemoteUrl::local(&env("LK_STARTUP_BASE_URL")),
            RemoteUrl::local(&env("LK_STARTUP_TENANT_URL")),
        ),
        Arc::new(NoCredentials),
        git_metrics.clone(),
    )
    .expect("the mirrors open");
    let (closing_tx, closing) = watch::channel(false);
    let _initialize = spawn_initialize(Arc::clone(&mirror), StartupRetry::default(), closing.clone());
    let _reader = GixReader::new(
        Arc::clone(&mirror),
        &ReaderSettings::default(),
        git_metrics.clone(),
    )
    .expect("the reader");

    let event_metrics = EventMetrics::default();
    let bus = PgEventBus::new(pool.clone(), BusSettings::default(), event_metrics.clone());
    let _bus_tasks = bus.start(closing.clone()).await;
    let auth = Arc::new(FakeSessionAuth::new());
    let sse = SseState::new(
        bus.source(),
        Arc::clone(&auth) as _,
        Arc::new(RoleVisibility),
        SseSettings::default(),
        closing.clone(),
        event_metrics,
    );
    let bus: Arc<dyn EventBus> = Arc::new(bus);

    let announcer = Announcer::start(Arc::clone(&mirror), Arc::clone(&bus), git_metrics.clone()).await;
    let queue = FetchQueue::new(256);
    let syncer = Arc::new(MirrorSyncer::new(Arc::clone(&mirror), Arc::clone(&announcer)));
    let _worker =
        FetchWorker::new(Arc::clone(&queue), syncer, RetrySettings::default()).spawn(closing.clone());
    let _sync = MirrorSync::new(Arc::clone(&mirror), announcer, Arc::clone(&bus)).spawn(closing);
    let webhook = WebhookState::new(
        WebhookSettings::new(
            "webhook-secret",
            vec![
                (
                    RepoKind::Base,
                    RepoFullName::parse("acme/configuration-base-saas").unwrap(),
                ),
                (
                    RepoKind::Tenant,
                    RepoFullName::parse("acme/csp-tenant-data").unwrap(),
                ),
            ],
        ),
        Arc::new(FakeSecretSource::new().with("webhook-secret", "bench")),
        Arc::new(PgDeliveryLog::new(pool.clone())),
        queue,
        git_metrics,
    );
    let blobs = BlobState::new(
        Arc::new(PgBlobStore::new(
            pool.clone(),
            BlobSettings::default(),
            BlobMetrics::default(),
        )),
        Arc::new(DenyHashes::default()),
    );

    let hub = Hub::build(Deps {
        settings,
        session: SessionEdge::without_sessions(auth),
        mounted: Vec::new(),
        modules: vec![
            Arc::new(EventsModule::new(sse)),
            Arc::new(GitModule::new(Arc::new(webhook))),
            Arc::new(BlobsModule::new(blobs)),
        ],
        static_files: None,
        checks: vec![
            ("postgres", Arc::new(hub::db::PostgresReady(pool))),
            ("mirrors", Arc::new(MirrorsReady::new(mirror))),
        ],
    })
    .expect("the hub composes");

    let web = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ops = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ops_addr = ops.local_addr().unwrap();
    tokio::spawn(serve(
        web,
        None,
        hub.web.clone(),
        std::future::pending(),
        ServeLimits::default(),
    ));
    tokio::spawn(serve(
        ops,
        None,
        hub.ops.clone(),
        std::future::pending(),
        ServeLimits::default(),
    ));

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if ready(ops_addr).await {
            break;
        }
        if Instant::now() > deadline {
            eprintln!("startup child: /readyz never answered 200");
            return ExitCode::FAILURE;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    println!("READY");
    // The parent closes our stdin when it has the line.
    let _ = tokio::task::spawn_blocking(|| {
        use std::io::Read;
        let mut sink = [0u8; 64];
        while matches!(std::io::stdin().read(&mut sink), Ok(n) if n > 0) {}
    })
    .await;
    let _ = closing_tx.send(true);
    ExitCode::SUCCESS
}

/// One `GET /readyz` over a plain connection: true on a 200.
async fn ready(addr: std::net::SocketAddr) -> bool {
    use tokio::io::AsyncWriteExt;
    let Ok(mut stream) = TcpStream::connect(addr).await else {
        return false;
    };
    if stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .is_err()
    {
        return false;
    }
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer).await;
    answer.starts_with("HTTP/1.1 200")
}
