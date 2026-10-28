//! P11: the hub starts in under 10 s (budget `P11`, `hub/startup`, p95), through the real composition (plan 18, T8).
//!
//! The sample is the wall-clock time from spawning a new process to its `/readyz` answering 200. The process is the `hub_dev` example,
//! which is `hub::process::entry` and so `process::run` and `process::start`, the code the binary runs and nothing else: settings from
//! the environment, the telemetry, the software edges (the only seam: secrets in files, a KMS with software keys, no Google), the
//! database pool and the schema check over TLS 1.3 with the server verified, the web certificate, both CAs and their certificates, the
//! four listeners, the event bus, the Git mirrors, the identity module, the registry and its ten jobs, the write paths and their four,
//! the MCP server, the job runner, the drain and the readiness checks. Nothing is composed by hand here, so a module that is added to
//! the composition is in the number the day it is added, which is what the old harness (a hand-built subset) could not promise.
//!
//! The state is the one a restarting replica finds, which is the case the budget is about: a Postgres 16 with every migration applied
//! (and the tree index that the first start persisted from the fixtures' 60 tenant branches, by the hub's own code) and a mirror volume
//! that already holds the two target-scale repositories of the fixtures. The registry tables hold no agent data: nothing in the start
//! sequence reads a registry table (the schema check reads the catalog, and the registry's warm-up starts after readiness on purpose,
//! `registry.rs`), so the time to ready does not depend on how many rows the agents' versions would fill them with; what the 40
//! swimlanes of the design add is to memory and to the first reads, which prompt 16 measures. The first start fills that volume by
//! fetching both repositories over HTTPS from servers in this process (a first start on an empty volume waits for the network and is
//! outside the budget); it is not measured. Then 2 warm-up starts and 10 measured ones. A start is measured from just before the spawn
//! to the first `/readyz` that says 200, so process start, the runtime, dynamic linking and every step above are in the number.
//! Writes `target/perf-results/hub_startup.json` and prints the phases of the last start (from the hub's own `starting` and
//! `listening` log lines), so a miss shows which part grew.
//!
//! **Memory** (P11's second half) is recorded and not judged here: one more start, the resident set read from `/proc` when the hub is
//! ready and again after 100 signed-in users have made 6,000 requests to the read API (the session, user and registry caches warm; every
//! request is a new TLS 1.3 connection), and the kernel's high-water mark. Writes `target/perf-results/hub_composed_rss_mib.json` with
//! those three figures as its samples, in that order. The budget `P11.memory_mib` (1,536 MiB per replica at design scale) is judged
//! by `hub-platform`'s harness for the platform's share and by prompt 16 for the whole hub with design-scale data; this figure is the
//! whole process with the fixtures' mirrors and an empty registry, so it is a floor for that one and not a verdict.
//!
//! What it does not show: real Google latency (the software KMS and the file secrets answer at once, so the four KMS calls and about
//! seven Secret Manager reads of a real start are not in the number; the hub runs them concurrently, each bounded by
//! `LK_START_TIMEOUT_SECS`, 8 s by default, which is the most they can add), and the registry's warm-up, which starts after readiness
//! on purpose.
//!
//! Needs Docker (testcontainers), the `git` binary, and the fixtures (`cargo xtask gen-fixtures --scale target`; `LK_FIXTURES_DIR`
//! names another directory). Linux (`/proc`).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::too_many_lines,
    clippy::cast_precision_loss
)]

#[path = "../tests/support/browser.rs"]
mod browser;
#[path = "../tests/support/child.rs"]
mod child;
#[path = "../tests/support/git_remote.rs"]
mod git_remote;
#[path = "../tests/support/net.rs"]
mod net;
#[path = "../../hub-platform/tests/support/perf.rs"]
mod perf;
#[path = "../tests/support/pki.rs"]
mod pki;
#[path = "../tests/support/rig.rs"]
mod rig;
mod support {
    pub(crate) use crate::pki;

    /// What the rig puts in the web root of a hub it starts (`tests/support/mod.rs` has the same, among much that a benchmark has no
    /// use for).
    pub fn write_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            r#"<!doctype html><html><head><meta charset="utf-8"><title>Lanekeeper</title><link rel="stylesheet" nonce="__CSP_NONCE__" href="/assets/app-abc123.css"></head><body><div id="root"></div></body></html>"#,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app-abc123.css"), "body{}").unwrap();
        dir
    }
}
#[path = "../tests/support/tls_pg.rs"]
mod tls_pg;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use domain::{Role, UserStatus};
use git_remote::GitHttpServer;
use rig::Rig;
use rustls::ServerConfig;

const WARM_UP: usize = 2;
const MEASURED: usize = 10;
/// The cold start fetches both repositories at target scale, over a loopback socket, with `git upload-pack`.
const COLD_START: Duration = Duration::from_secs(900);
const USERS: u32 = 100;
const REQUESTS: usize = 6_000;
const CONCURRENCY: usize = 16;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn main() -> ExitCode {
    runtime().block_on(run())
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

/// The two fixture repositories behind HTTPS, as GitHub would serve them, on a thread and a runtime of their own: the benchmark
/// blocks its own thread while it waits for the hub, and a server that shared that runtime would stop answering exactly when the
/// hub's fetch is running.
struct FixtureRemotes {
    base_url: String,
    tenant_url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FixtureRemotes {
    fn start(rig: &Rig, base: PathBuf, tenant: PathBuf) -> Self {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = Arc::new(
            ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![rig.leaf.cert.clone()], rig.leaf.key.clone_key())
                .unwrap(),
        );
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (told, hears) = std::sync::mpsc::channel::<(String, String)>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let base = GitHttpServer::start_tls(base, None, None, Arc::clone(&config)).await;
                let tenant = GitHttpServer::start_tls(tenant, None, None, config).await;
                told.send((base.url_for_host("localhost"), tenant.url_for_host("localhost")))
                    .unwrap();
                let _ = stopped.await;
                drop((base, tenant));
            });
        });
        let (base_url, tenant_url) = hears.recv().unwrap();
        Self {
            base_url,
            tenant_url,
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for FixtureRemotes {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn unix_micros(at: SystemTime) -> i64 {
    i64::try_from(at.duration_since(UNIX_EPOCH).unwrap().as_micros()).unwrap()
}

/// `2026-10-27T12:34:56.789012Z` (the hub's log timestamp) as microseconds since the epoch.
fn log_micros(stamp: &str) -> Option<i64> {
    let (date, time) = stamp.trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day_of_month) = (date.next()??, date.next()??, date.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (hours, minutes, seconds) = (clock.next()??, clock.next()??, clock.next()??);
    // Days since the epoch from the civil date (Howard Hinnant's algorithm).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day_of_month - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let micros: i64 = format!("{fraction:0<6}")[..6].parse().ok()?;
    Some(((days * 24 + hours) * 60 + minutes) * 60_000_000 + seconds * 1_000_000 + micros)
}

/// The time of the first log line whose `message` is `message`, as microseconds since the epoch.
fn logged_at(output: &str, message: &str) -> Option<i64> {
    output.lines().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        (value.pointer("/fields/message")?.as_str()? == message)
            .then(|| log_micros(value.get("timestamp")?.as_str()?))?
    })
}

/// One start: spawn, and the milliseconds until `/readyz` first says 200. Also the child, still running, and when it was spawned.
async fn start_once(rig: &Rig, extra: &[(&str, &str)], within: Duration) -> Option<(f64, child::Child, i64)> {
    let program: PathBuf = tokio::task::spawn_blocking(child::example_binary).await.unwrap();
    let env = rig.env(extra);
    let spawned_at = unix_micros(SystemTime::now());
    let started = Instant::now();
    let child = child::Child::spawn(&program, &[], &env);
    let deadline = started + within;
    let ops = loop {
        if let Some(addrs) = child.listening() {
            break addrs.ops;
        }
        if Instant::now() > deadline {
            eprintln!(
                "startup: the hub never said it was listening. Output:\n{}",
                child.output()
            );
            return None;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    loop {
        if net::http_get(ops, "/readyz").await.status == 200 {
            break;
        }
        if Instant::now() > deadline {
            eprintln!(
                "startup: /readyz never answered 200 within {within:?}. Output:\n{}",
                child.output()
            );
            return None;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    Some((started.elapsed().as_secs_f64() * 1000.0, child, spawned_at))
}

/// Stop a child the way the kubelet does and wait for it.
fn stop(mut child: child::Child) {
    child.signal("TERM");
    let status = child.exit_within(Duration::from_secs(60));
    assert!(status.success(), "the hub did not stop cleanly: {status:?}");
}

/// `VmRSS` or `VmHWM` of a process, in MiB.
fn proc_mib(pid: u32, field: &str) -> f64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let kib: f64 = status
        .lines()
        .find_map(|l| l.strip_prefix(field)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no {field} in /proc/{pid}/status"));
    kib / 1024.0
}

async fn run() -> ExitCode {
    let (base_repo, tenant_repo) = fixture_repos();
    let Some(rig) = Rig::start().await else {
        eprintln!("startup: no Docker, so no Postgres to measure against");
        return ExitCode::FAILURE;
    };
    let remotes = FixtureRemotes::start(&rig, base_repo, tenant_repo);
    // The start-up timeout is the production default and not the rig's 20 s: a start that leans on a long one is not a start in 10 s.
    // The rate limits are lifted for the request burst only; the starts do not make requests.
    let extra: Vec<(&str, &str)> = vec![
        ("LK_GIT_BASE_URL", &remotes.base_url),
        ("LK_GIT_TENANT_URL", &remotes.tenant_url),
        ("LK_START_TIMEOUT_SECS", "8"),
        ("LK_IP_RATE_PER_SEC", "100000"),
        ("LK_IP_BURST", "100000"),
        ("LK_USER_RATE_PER_SEC", "100000"),
        ("LK_USER_BURST", "100000"),
    ];

    // The replica's volume, filled once: the state a restarting replica finds. Ready means both mirrors hold a branch.
    let Some((cold_ms, cold, _)) = start_once(&rig, &extra, COLD_START).await else {
        return ExitCode::FAILURE;
    };
    eprintln!(
        "startup: the first start, which fetched both repositories, took {cold_ms:.0} ms (not measured)"
    );
    stop(cold);

    let mut samples = Vec::with_capacity(MEASURED);
    let mut phases = None;
    for n in 0..WARM_UP + MEASURED {
        let Some((ms, child, spawned_at)) = start_once(&rig, &extra, Duration::from_secs(60)).await else {
            return ExitCode::FAILURE;
        };
        if n >= WARM_UP {
            samples.push(ms);
            let output = child.output();
            phases = Some((
                spawned_at,
                logged_at(&output, "starting"),
                logged_at(&output, "listening"),
                ms,
            ));
        }
        stop(child);
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
    if let Some((spawned_at, starting, listening, ready_ms)) = phases {
        let since = |at: Option<i64>| {
            at.map_or_else(
                || "?".to_owned(),
                |t| format!("{:.0} ms", (t - spawned_at) as f64 / 1000.0),
            )
        };
        eprintln!(
            "startup: phases of the last start: spawn to `starting` (process, runtime, settings, telemetry) {}; to `listening` (every module built, four listeners bound) {}; to /readyz 200 {ready_ms:.0} ms",
            since(starting),
            since(listening),
        );
    }

    // Memory: one more start, ready, then a burst of requests from signed-in people.
    let Some((_, child, _)) = start_once(&rig, &extra, Duration::from_secs(60)).await else {
        return ExitCode::FAILURE;
    };
    let pid = child.id();
    let at_ready = proc_mib(pid, "VmRSS");
    let web = child.listening().expect("the hub said it was listening").web;
    let pool = rig.pg.app_pool().await;
    let browser = browser::Browser::new(pool);
    let mut people = Vec::new();
    for n in 1..=USERS {
        people.push(
            browser
                .signed_in(n, Some(Role::Viewer), UserStatus::Active)
                .await
                .1,
        );
    }
    let ca = rig.ca.cert_der();
    let paths = [
        "/api/v1/me",
        "/api/v1/swimlanes",
        "/api/v1/proposals",
        "/api/v1/findings",
        "/api/v1/prs",
    ];
    let people = Arc::new(people);
    let burst_started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for worker in 0..CONCURRENCY {
        let (people, ca) = (Arc::clone(&people), ca.clone());
        tasks.spawn(async move {
            let mut bad = std::collections::BTreeSet::new();
            let mut count = 0usize;
            for i in (worker..REQUESTS).step_by(CONCURRENCY) {
                let who = &people[i % people.len()];
                let cookie = who.header();
                let reply = net::https_request(
                    web,
                    &ca,
                    "GET",
                    paths[i % paths.len()],
                    &[("Cookie", &cookie)],
                    None,
                )
                .await;
                if reply.status != 200 {
                    count += 1;
                    bad.insert(format!("{} answered {}", paths[i % paths.len()], reply.status));
                }
            }
            (count, bad)
        });
    }
    let (mut bad, mut kinds) = (0, std::collections::BTreeSet::new());
    while let Some(done) = tasks.join_next().await {
        let (count, seen) = done.unwrap();
        bad += count;
        kinds.extend(seen);
    }
    let burst = burst_started.elapsed();
    let after_burst = proc_mib(pid, "VmRSS");
    let high_water = proc_mib(pid, "VmHWM");
    stop(child);
    let path = perf::write_result(
        "hub/composed_rss_mib",
        "mib",
        &[at_ready, after_burst, high_water],
    );
    eprintln!(
        "startup: memory of the whole hub (resident set, MiB): at ready {at_ready:.0}, after {REQUESTS} requests from {USERS} signed-in users in {:.1} s ({bad} not 200) {after_burst:.0}, kernel high-water mark {high_water:.0} (the platform's share alone is judged against 1536 by P11.memory_mib; the whole hub at design scale is prompt 16's) -> {}",
        burst.as_secs_f64(),
        path.display()
    );
    if bad > 0 {
        eprintln!(
            "startup: {bad} of the burst's requests were not 200 ({kinds:?}), so the memory figure is not of a hub that was serving"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
