//! P13: a live update reaches SSE subscribers within 1 s (budget `P13`, `hub-platform/sse_delivery`, p95).
//!
//! Three hub replicas on one real Postgres 16, 100 SSE clients on each over real sockets (300 in all). An event is committed through
//! `publish_in_tx` on one replica (rotating), and the sample is the time from just before the commit until the last of the 300
//! clients has the frame: the full fan-out, not the nearest subscriber. 60 events after 5 warm-up events. Writes
//! `target/perf-results/hub-platform_sse_delivery.json`.
//!
//! Needs Docker (testcontainers), like the integration tests; without it this exits with an error rather than writing a result.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use domain::{DomainEvent, ProposalId, ProposalStatus};
use hub_platform::events::bus::BusSettings;
use hub_platform::events::sse::SseSettings;
use ports::{EventBus, Tx};
use support::event_cluster::{EventCluster, connect};
use support::sse_client::parse;

const REPLICAS: usize = 3;
const CLIENTS_PER_REPLICA: usize = 100;
const WARM_UP: usize = 5;
const MEASURED: usize = 60;

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let ok = runtime.block_on(run());
    if !ok {
        std::process::exit(1);
    }
}

async fn run() -> bool {
    let sse = SseSettings {
        per_user: CLIENTS_PER_REPLICA * 2,
        ..SseSettings::default()
    };
    let Some(cluster) = EventCluster::start(REPLICAS, BusSettings::default(), sse).await else {
        eprintln!("sse_delivery: no Docker, so no Postgres to measure against");
        return false;
    };
    let total_clients = REPLICAS * CLIENTS_PER_REPLICA;
    let events = WARM_UP + MEASURED;
    let origin = Instant::now();

    // arrivals[event][client]: nanoseconds after `origin` at which the client had the frame, 0 if not yet.
    let arrivals: Arc<Vec<Vec<AtomicU64>>> = Arc::new(
        (0..events)
            .map(|_| (0..total_clients).map(|_| AtomicU64::new(0)).collect())
            .collect(),
    );
    let received: Arc<Vec<AtomicU64>> = Arc::new((0..events).map(|_| AtomicU64::new(0)).collect());

    let mut client = 0;
    for replica in &cluster.replicas {
        for _ in 0..CLIENTS_PER_REPLICA {
            let mut conn = connect(replica.addr, "viewer", "/api/v1/events", None).await;
            assert_eq!(conn.status, http::StatusCode::OK);
            let arrivals = Arc::clone(&arrivals);
            let received = Arc::clone(&received);
            let index = client;
            client += 1;
            tokio::spawn(async move {
                while let Some(frame) = conn.frames.next_event().await {
                    let at = origin.elapsed().as_nanos() as u64;
                    let Some(data) = parse(&frame).data else { continue };
                    let value: serde_json::Value = serde_json::from_str(&data).unwrap();
                    let Some(n) = value["id"].as_u64().and_then(|n| usize::try_from(n).ok()) else {
                        continue;
                    };
                    if n < arrivals.len() {
                        arrivals[n][index].store(at.max(1), Ordering::SeqCst);
                        received[n].fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    }
    assert_eq!(client, total_clients);

    let mut samples = Vec::with_capacity(MEASURED);
    for n in 0..events {
        let replica = &cluster.replicas[n % REPLICAS];
        let mut tx = replica.pool.begin().await.unwrap();
        replica
            .bus
            .publish_in_tx(
                &mut Tx::from_pg(&mut tx),
                DomainEvent::ProposalChanged {
                    id: ProposalId::from(i64::try_from(n).unwrap()),
                    status: ProposalStatus::Pending,
                },
            )
            .await
            .unwrap();
        let before_commit = origin.elapsed().as_nanos() as u64;
        tx.commit().await.unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while received[n].load(Ordering::SeqCst) < total_clients as u64 {
            if Instant::now() > deadline {
                eprintln!(
                    "sse_delivery: event {n} reached {} of {total_clients} clients in 10 s",
                    received[n].load(Ordering::SeqCst)
                );
                return false;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let last = arrivals[n]
            .iter()
            .map(|a| a.load(Ordering::SeqCst))
            .max()
            .unwrap();
        if n >= WARM_UP {
            samples.push(last.saturating_sub(before_commit) as f64 / 1e6);
        }
        // Events are spread out, as real ones are: the budget is for one update, not for a flood.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let path = support::perf::write_result("hub-platform/sse_delivery", "ms", &samples);
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    eprintln!(
        "sse_delivery: {MEASURED} events, {REPLICAS} replicas x {CLIENTS_PER_REPLICA} clients, ms: p50 {:.1}, p95 {:.1}, max {:.1}\n-> {}",
        support::perf::percentile(&sorted, 0.5),
        support::perf::percentile(&sorted, 0.95),
        support::perf::percentile(&sorted, 1.0),
        path.display()
    );
    true
}
