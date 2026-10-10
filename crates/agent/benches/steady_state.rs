//! P4: an agent at steady state uses at most 50 mCPU and 64 MiB (budgets `P4.cpu_mcores` and `P4.memory_mib`).
//!
//! The whole agent (`agent::app::App`, the production wiring apart from the Kubernetes client and the hub, which are
//! the fakes of the tests) runs in this process in real time on two worker threads, as in the container: a TLS 1.3
//! stream to the fake hub, the production walker over a copy of swimlane 1 of the target-scale fixtures (about 2,000
//! files), and the Kubernetes watchers against a fake API server that holds 40 Deployments and 80 Pods. Every 30 s a file
//! changes on the NFS tree and a Pod appears, so the measurement includes the work of a delta and a cluster report.
//!
//! - **CPU** is the process's user plus system time, read from `/proc/self/stat`, over 5 s windows for 120 s after a 20 s
//!   warm-up (the first walk and the first lists). The budget is the 95th percentile of the windows. The process also
//!   holds the fake hub and the fake API server, so this is an upper bound for the agent. The 15-minute full rehash is
//!   outside the windows by construction (a 120 s run contains none); its cost is `agent/full_rehash_2000_files`.
//! - **Memory** is two numbers added. The first is the growth of the resident set from just before the agent starts to
//!   its highest point in the run (`VmRSS` in `/proc/self/status`, sampled each second). The second is the resident set of
//!   the real `agent` binary while it idles at start-up, which is what the process costs before it holds a tree or a
//!   watch. The budget is the maximum. Added together they are an upper bound: the growth also includes the fake hub's
//!   own buffers.
//!
//! About 3 minutes. Writes `target/perf-results/agent_steady_state_cpu_mcores.json` and
//! `agent_steady_state_memory_mib.json`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;
#[path = "../tests/support/mod.rs"]
mod support;

use std::fs;
use std::process::Command;
use std::time::Duration;

use proto::convert::FromAgent;
use support::agent_process::{http_get, spawn_waiting_agent, terminate, wait_for_health};
use support::app_rig::{AppRig, Setup};
use support::fake_kube::FakeKube;
use support::k8s_objects::{deployment, pod};
use support::rig::Rig;
use tokio::time::{Instant, sleep, sleep_until};

const WARM_UP: Duration = Duration::from_secs(20);
const WINDOW: Duration = Duration::from_secs(5);
const WINDOWS: u32 = 24;
const CHANGE_EVERY: Duration = Duration::from_secs(30);

/// Clock ticks per second, as `getconf CLK_TCK` says (100 on every Linux this runs on).
fn clock_ticks_per_second() -> f64 {
    Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(100.0)
}

/// User plus system CPU time of this process, in clock ticks.
fn cpu_ticks() -> u64 {
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    // The command name is in parentheses and may hold spaces; the fields after it are space-separated, state first (3).
    let after = &stat[stat.rfind(')').unwrap() + 2..];
    let field = |n: usize| -> u64 { after.split(' ').nth(n - 3).unwrap().parse().unwrap() };
    field(14) + field(15)
}

/// Resident set size of process `pid`, in MiB.
fn rss_mib(pid: &str) -> f64 {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let kib: f64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .map(|rest| rest.trim().trim_end_matches("kB").trim().parse().unwrap())
        .unwrap();
    kib / 1024.0
}

/// The real binary, started with an environment that parses and a cluster that does not answer, once it has settled:
/// the resident set of an agent before it holds a tree or a watch.
fn idle_binary_rss_mib() -> f64 {
    let tmp = tempfile::tempdir().unwrap();
    let (mut child, port) = spawn_waiting_agent(env!("CARGO_BIN_EXE_agent"), tmp.path());
    wait_for_health(port);
    assert!(http_get(port, "/healthz").is_some());
    std::thread::sleep(Duration::from_secs(3));
    let rss = rss_mib(&child.id().to_string());
    terminate(&child);
    child.wait().unwrap();
    rss
}

fn cluster() -> FakeKube {
    let kube = FakeKube::new("sit1");
    for i in 0..40 {
        let name = format!("svc-{i:02}");
        kube.apply(
            deployment("sit1", &name)
                .env("CONFIG_CLIENT_CACHE_TTL", "20m")
                .env("JAVA_OPTS", "-Xmx512m -XX:+UseG1GC")
                .helm("csp-tenant-data-sit1-1.4.2", "csp-tenant-data-sit1")
                .build(),
        );
        for replica in 0..2 {
            kube.apply(pod(
                "sit1",
                &format!("{name}-{replica}"),
                &name,
                "2026-10-10T11:00:00Z",
            ));
        }
    }
    kube
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (cpu_mcores, growth_mib, files) = runtime.block_on(async {
        let work = tempfile::TempDir::new().unwrap();
        common::copy_tree(&common::swimlane_one(), work.path());
        let files = common::count_files(work.path());
        let kube = cluster();
        let rig = Rig::new();
        let baseline = rss_mib("self");

        let mut app = AppRig::start_on(
            rig,
            Setup {
                kube: Some(kube.clone()),
                source: None,
                root: Some(work),
                ..Setup::default()
            },
        )
        .await;
        app.connected().await;
        sleep(WARM_UP).await;

        let ticks_per_second = clock_ticks_per_second();
        let started = Instant::now();
        let mut cpu = Vec::new();
        let mut highest = rss_mib("self");
        let mut last_ticks = cpu_ticks();
        let mut changes = 0;
        for window in 1..=WINDOWS {
            // Sample the resident set each second; one change every 30 s.
            for second in 1..=WINDOW.as_secs() {
                sleep_until(started + (WINDOW * (window - 1)) + Duration::from_secs(second)).await;
                highest = highest.max(rss_mib("self"));
                if started.elapsed() >= CHANGE_EVERY * (changes + 1) {
                    changes += 1;
                    fs::create_dir_all(app.dir.path().join("steady")).unwrap();
                    fs::write(
                        app.dir.path().join(format!("steady/change-{changes}.yml")),
                        format!("changed: {changes}\r\n"),
                    )
                    .unwrap();
                    kube.apply(pod(
                        "sit1",
                        &format!("svc-00-extra-{changes}"),
                        "svc-00",
                        "2026-10-10T12:00:00Z",
                    ));
                }
            }
            let ticks = cpu_ticks();
            // A few hundred ticks in a window: far inside f64's exact integers.
            #[allow(clippy::cast_precision_loss)]
            let seconds = (ticks - last_ticks) as f64 / ticks_per_second;
            cpu.push(seconds / WINDOW.as_secs_f64() * 1000.0);
            last_ticks = ticks;
        }
        assert!(
            app.is_ready(),
            "the agent must still be connected at the end of the run"
        );
        // The run was not of an agent that did nothing: its changes reached the hub, as deltas and as reports.
        let received = app.rig.server.connection(0).received();
        let deltas = received
            .iter()
            .filter(|m| matches!(m, FromAgent::Delta(_)))
            .count();
        let reports = received
            .iter()
            .filter(|m| matches!(m, FromAgent::Cluster(r) if !r.full))
            .count();
        assert!(
            deltas >= 3 && reports >= 3,
            "{changes} changes made, {deltas} deltas and {reports} cluster reports seen"
        );
        app.stop().await.unwrap();
        (cpu, highest - baseline, files)
    });
    drop(runtime);

    let binary = idle_binary_rss_mib();
    let memory = growth_mib + binary;
    let cpu_written = support::perf::write_result("agent/steady_state_cpu_mcores", "mcores", &cpu_mcores);
    let memory_written = support::perf::write_result("agent/steady_state_memory_mib", "mib", &[memory]);
    let mut sorted = cpu_mcores.clone();
    sorted.sort_by(f64::total_cmp);
    eprintln!(
        "steady state over {files} files, 40 Deployments and 80 Pods:\n  \
         cpu per 5 s window (mCPU), sorted: {sorted:.1?}\n  \
         memory: {growth_mib:.1} MiB growth in the run + {binary:.1} MiB for the idle binary = {memory:.1} MiB\n  -> {}\n  -> {}",
        cpu_written.display(),
        memory_written.display()
    );
}
