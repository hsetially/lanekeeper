//! P11: hub memory stays under 1.5 GiB per replica at design scale (budget `P11.memory_mib`, `hub/memory_at_design_scale_mib`, max).
//!
//! This is the platform's share of the hub's memory (plan A21), measured as the resident set of this process while it holds, at
//! design scale: 40 attached agents (real mutual TLS, real `Connect` streams, a hub-side stream and gateway registration each), 100
//! SSE subscribers on real sockets, the blob cache full at its default 512 MiB (a 640 MiB corpus read through it, so it is evicting,
//! not merely filling), and the tree-index cache holding every branch and tag of the target-scale fixture repositories (60 tenant
//! branches). A load runs through all of them for the whole measurement (events committed through the outbox at 20 per second,
//! 50 blob reads per second over the whole corpus, 40 gateway round trips per second), and the resident set is read every 500 ms.
//! The result is the maximum of those readings and of the kernel's own high-water mark, so a peak between two readings counts.
//!
//! The process also holds the fake agents' and clients' ends of every connection, the two test databases' pools and the container
//! client, so the number over-states what the hub alone uses; the other crates' caches and the embedding model are added by prompt
//! 16, which repeats this with everything in. Writes `target/perf-results/hub_memory_at_design_scale_mib.json`.
//!
//! Needs Docker (testcontainers) and the fixtures (`cargo xtask gen-fixtures --scale target`); Linux (`/proc`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

#[path = "../tests/support/mod.rs"]
mod support;

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use domain::{DomainEvent, HubCommand, NfsPath, ProposalId, ProposalStatus, RepoKind, RequestId, SwimlaneId};
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PgBlobStore};
use hub_platform::events::bus::BusSettings;
use hub_platform::events::sse::SseSettings;
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
use hub_platform::git::reader::{GixReader, ReaderSettings};
use ports::{AgentGateway, BlobStore, EventBus, GitReader, Tx};
use support::cluster::TestCluster;
use support::event_cluster::{EventCluster, connect};
use support::fake_agent::Behavior;
use support::perf::write_result;

const AGENTS: usize = 40;
const SSE_CLIENTS: usize = 100;
/// More than the cache holds, so that it is evicting while it is measured.
const CORPUS_BYTES: u64 = 640 * 1024 * 1024;
const LOAD: Duration = Duration::from_secs(20);
const SAMPLE_EVERY: Duration = Duration::from_millis(500);

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    if !runtime.block_on(run()) {
        std::process::exit(1);
    }
}

/// A line of `/proc/self/status` in kB, as MiB.
fn status_mib(field: &str) -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("this harness needs /proc (Linux)");
    let line = status
        .lines()
        .find_map(|l| l.strip_prefix(field))
        .unwrap_or_else(|| panic!("no {field} in /proc/self/status"));
    let kb: f64 = line
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .expect("a number of kB");
    kb / 1024.0
}

async fn run() -> bool {
    let (base, tenant) = common::fixture_repos();

    // 40 attached agents.
    let Some(cluster) = TestCluster::start(1).await else {
        eprintln!("memory_at_design_scale: no Docker, so no Postgres to measure against");
        return false;
    };
    let lanes: Vec<String> = (1..=AGENTS).map(|n| format!("sl-{n:02}")).collect();
    cluster
        .seed(&lanes.iter().map(String::as_str).collect::<Vec<_>>())
        .await;
    let mut links = Vec::new();
    for lane in &lanes {
        let agent = cluster.agent(lane).await;
        let link = agent
            .connect(&cluster.agent_ca, cluster.hubs[0].agent_addr, Behavior::echo())
            .await
            .expect("an agent attaches");
        links.push((agent, link));
    }
    eprintln!("memory_at_design_scale: {AGENTS} agents attached");

    // 100 SSE subscribers, each reading as a browser does.
    let Some(events) = EventCluster::start(
        1,
        BusSettings::default(),
        SseSettings {
            per_user: SSE_CLIENTS * 2,
            ..SseSettings::default()
        },
    )
    .await
    else {
        return false;
    };
    let frames = Arc::new(AtomicU64::new(0));
    for _ in 0..SSE_CLIENTS {
        let mut conn = connect(events.replicas[0].addr, "viewer", "/api/v1/events", None).await;
        assert_eq!(conn.status, http::StatusCode::OK);
        let frames = Arc::clone(&frames);
        tokio::spawn(async move {
            while conn.frames.next_event().await.is_some() {
                frames.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
    eprintln!("memory_at_design_scale: {SSE_CLIENTS} SSE subscribers connected");

    // The blob cache, full.
    let blob_metrics = BlobMetrics::default();
    let store = Arc::new(PgBlobStore::new(
        events.db.app_pool().await,
        BlobSettings::default(),
        blob_metrics.clone(),
    ));
    let mut hashes = Vec::new();
    let mut rng = fastrand::Rng::with_seed(20_260_101);
    let mut total = 0u64;
    while total < CORPUS_BYTES {
        // 100 KiB to 2 MiB, random bytes: distinct, incompressible, the worst case for the cache.
        let size = rng.usize(100 * 1024..=2 * 1024 * 1024);
        let mut bytes = vec![0u8; size];
        rng.fill(&mut bytes);
        total += size as u64;
        hashes.push(store.put(Bytes::from(bytes)).await.unwrap());
    }
    let mut order: Vec<usize> = (0..hashes.len()).collect();
    rng.shuffle(&mut order);
    for i in &order {
        store.get(&hashes[*i]).await.unwrap().expect("a stored blob");
    }
    let held = store.cache().weighted_size();
    let cap = BlobSettings::default().cache_bytes;
    assert!(
        held * 10 >= cap * 9,
        "the blob cache holds {held} of {cap} bytes: not full, so the measurement would understate it"
    );
    eprintln!(
        "memory_at_design_scale: {} blobs, {:.0} MiB through a cache that holds {:.0} MiB",
        hashes.len(),
        total as f64 / 1_048_576.0,
        held as f64 / 1_048_576.0
    );

    // The tree-index cache, at 60 branches.
    let dir = tempfile::tempdir().unwrap();
    let mirror = Mirror::open(
        MirrorSettings::new(
            dir.path(),
            RemoteUrl::local(&format!("file://{}", base.display())),
            RemoteUrl::local(&format!("file://{}", tenant.display())),
        ),
        Arc::new(NoCredentials),
        GitMetrics::default(),
    )
    .unwrap();
    mirror.initialize().await.unwrap();
    let reader = Arc::new(
        GixReader::new(
            Arc::clone(&mirror),
            &ReaderSettings::default(),
            GitMetrics::default(),
        )
        .unwrap(),
    );
    let indexed = {
        let reader = Arc::clone(&reader);
        tokio::task::spawn_blocking(move || {
            let mut commits = 0usize;
            for repo in [RepoKind::Base, RepoKind::Tenant] {
                let mut heads: Vec<_> = reader
                    .branches(repo)
                    .unwrap()
                    .into_iter()
                    .map(|b| b.head)
                    .collect();
                heads.extend(reader.tags(repo).unwrap().into_iter().map(|t| t.commit));
                heads.sort();
                heads.dedup();
                for head in heads {
                    reader.tree_index(repo, &head).unwrap();
                    commits += 1;
                }
            }
            commits
        })
        .await
        .unwrap()
    };
    assert!(
        indexed >= 60,
        "{indexed} commits indexed: the fixtures have 60 tenant branches"
    );
    eprintln!(
        "memory_at_design_scale: {indexed} commits indexed, {} entries cached",
        reader.tree_indexes().weighted_size()
    );

    // The load, and the readings.
    let stop = Arc::new(AtomicBool::new(false));
    let mut loads = Vec::new();
    {
        let (replica_pool, bus) = (events.replicas[0].pool.clone(), events.replicas[0].bus.clone());
        let stop = Arc::clone(&stop);
        loads.push(tokio::spawn(async move {
            let mut n = 0i64;
            while !stop.load(Ordering::Relaxed) {
                let mut tx = replica_pool.begin().await.unwrap();
                bus.publish_in_tx(
                    &mut Tx::from_pg(&mut tx),
                    DomainEvent::ProposalChanged {
                        id: ProposalId::from(n),
                        status: ProposalStatus::Pending,
                    },
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
                n += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }));
    }
    {
        let (store, hashes, stop) = (Arc::clone(&store), hashes.clone(), Arc::clone(&stop));
        loads.push(tokio::spawn(async move {
            let mut rng = fastrand::Rng::with_seed(7);
            while !stop.load(Ordering::Relaxed) {
                let _ = store.get(&hashes[rng.usize(..hashes.len())]).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }));
    }
    {
        let (gateway, lanes, stop) = (
            Arc::clone(&cluster.hubs[0].gateway),
            lanes.clone(),
            Arc::clone(&stop),
        );
        loads.push(tokio::spawn(async move {
            let mut n = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let swimlane = SwimlaneId::parse(&lanes[n % lanes.len()]).unwrap();
                let command = HubCommand::ReadFile {
                    request_id: RequestId::parse(&format!("mem-{n}")).unwrap(),
                    path: NfsPath::parse("app/a.yml").unwrap(),
                };
                let _ = gateway.request(&swimlane, command, Duration::from_secs(3)).await;
                n += 1;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }));
    }

    let mut samples = vec![status_mib("VmRSS:")];
    let until = Instant::now() + LOAD;
    while Instant::now() < until {
        tokio::time::sleep(SAMPLE_EVERY).await;
        samples.push(status_mib("VmRSS:"));
    }
    stop.store(true, Ordering::Relaxed);
    for load in loads {
        let _ = load.await;
    }
    // The kernel's own high-water mark covers a peak between two readings.
    samples.push(status_mib("VmHWM:"));
    assert!(
        frames.load(Ordering::Relaxed) > 0,
        "no SSE subscriber received an event: the load did not run"
    );
    assert!(
        links.iter().all(|(_, link)| !link.has_ended()),
        "an agent was detached during the load"
    );

    let path = write_result("hub/memory_at_design_scale_mib", "mib", &samples);
    let max = samples.iter().copied().fold(0.0, f64::max);
    eprintln!(
        "memory_at_design_scale: {} readings, max {max:.0} MiB (budget 1536) -> {}",
        samples.len(),
        path.display()
    );
    drop(links);
    true
}
