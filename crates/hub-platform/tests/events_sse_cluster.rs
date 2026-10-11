//! T5: the SSE endpoint on three replicas over one real Postgres, with real sockets (P13, D66).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::time::{Duration, Instant};

use domain::{DomainEvent, ProposalId, ProposalStatus, UserStatus};
use hub_platform::events::bus::BusSettings;
use hub_platform::events::sse::SseSettings;
use ports::{EventBus, Tx};
use support::event_cluster::{EventCluster, connect, wait_for};
use support::sse_client::{kind_of, parse};

fn proposal(n: i64) -> DomainEvent {
    DomainEvent::ProposalChanged {
        id: ProposalId::from(n),
        status: ProposalStatus::Pending,
    }
}

fn sse_settings() -> SseSettings {
    SseSettings {
        per_user: 50,
        heartbeat: Duration::from_millis(200),
        ..SseSettings::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_hubs_sse_clients_receive_event_within_1s() {
    let Some(cluster) = EventCluster::start(3, BusSettings::default(), sse_settings()).await else {
        return;
    };
    let mut clients = Vec::new();
    for replica in &cluster.replicas {
        for _ in 0..5 {
            let mut conn = connect(replica.addr, "viewer", "/api/v1/events", None).await;
            assert_eq!(conn.status, http::StatusCode::OK);
            assert_eq!(conn.frames.next().await.unwrap(), "retry: 3000\n\n");
            clients.push(conn);
        }
    }
    assert_eq!(clients.len(), 15);

    // Committed on replica 0 through the outbox; heard by clients of all three.
    let mut tx = cluster.replicas[0].pool.begin().await.unwrap();
    cluster.replicas[0]
        .bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), proposal(1))
        .await
        .unwrap();
    let committing = Instant::now();
    tx.commit().await.unwrap();
    for conn in &mut clients {
        let frame = tokio::time::timeout(Duration::from_secs(5), conn.frames.next_event())
            .await
            .expect("an event arrives")
            .unwrap();
        assert_eq!(kind_of(&frame), "proposal_changed");
    }
    assert!(
        committing.elapsed() < Duration::from_secs(1),
        "{:?}",
        committing.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_resumes_on_another_replica_from_its_last_event_id() {
    let Some(cluster) = EventCluster::start(3, BusSettings::default(), sse_settings()).await else {
        return;
    };
    let mut first = connect(cluster.replicas[0].addr, "viewer", "/api/v1/events", None).await;
    assert_eq!(first.frames.next().await.unwrap(), "retry: 3000\n\n");
    cluster.replicas[1].bus.publish(proposal(1)).await.unwrap();
    let seen = parse(&first.frames.next_event().await.unwrap());
    let last_id = seen.id.unwrap();
    drop(first);

    // Missed while the client was away.
    for n in 2..=4 {
        cluster.replicas[1].bus.publish(proposal(n)).await.unwrap();
    }
    // Delivery is at least once: a replica whose relay has not yet read an older row hands it to a stream that subscribed in the
    // meantime, even though the client has seen it (ids below the cursor are late commits to the relay, so they cannot be told
    // apart from it). The "then live" check below is about events after the replay, so replica 2 must have read all four rows
    // before the client arrives; without the wait the test passed or failed on how fast replica 2's relay was.
    wait_for(
        "replica 2's relay to read all four rows",
        Duration::from_secs(5),
        async || cluster.replicas[2].metrics.relayed_total() >= 4,
    )
    .await;
    let mut second = connect(
        cluster.replicas[2].addr,
        "viewer",
        "/api/v1/events",
        Some(&last_id),
    )
    .await;
    assert_eq!(second.status, http::StatusCode::OK);
    let mut ids = Vec::new();
    for _ in 0..3 {
        let frame = second.frames.next_event().await.unwrap();
        assert_eq!(kind_of(&frame), "proposal_changed");
        ids.push(parse(&frame).id.unwrap().parse::<i64>().unwrap());
    }
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "in id order: {ids:?}");
    assert!(ids[0] > last_id.parse::<i64>().unwrap());

    // Then live.
    cluster.replicas[0].bus.publish(proposal(5)).await.unwrap();
    let live = parse(&second.frames.next_event().await.unwrap());
    assert!(live.id.unwrap().parse::<i64>().unwrap() > ids[2]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_over_a_real_socket_ends_when_the_user_is_disabled() {
    let Some(cluster) = EventCluster::start(1, BusSettings::default(), sse_settings()).await else {
        return;
    };
    let mut conn = connect(cluster.replicas[0].addr, "viewer", "/api/v1/events", None).await;
    assert_eq!(conn.frames.next().await.unwrap(), "retry: 3000\n\n");
    cluster.auth.set("viewer", None, UserStatus::Disabled);
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while conn.frames.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the stream ended within a few heartbeats");
    assert_eq!(cluster.replicas[0].sse.active_streams(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_closes_every_stream_with_a_retry_hint() {
    let Some(cluster) = EventCluster::start(1, BusSettings::default(), sse_settings()).await else {
        return;
    };
    let mut conns = Vec::new();
    for _ in 0..3 {
        let mut c = connect(cluster.replicas[0].addr, "viewer", "/api/v1/events", None).await;
        assert_eq!(c.frames.next().await.unwrap(), "retry: 3000\n\n");
        conns.push(c);
    }
    cluster.replicas[0].close_streams();
    for c in &mut conns {
        let last = tokio::time::timeout(Duration::from_secs(5), async {
            let mut last = String::new();
            while let Some(frame) = c.frames.next().await {
                last = frame;
            }
            last
        })
        .await
        .expect("the stream ends");
        assert_eq!(
            last, "retry: 3000\n\n",
            "the final frame tells the client when to come back"
        );
    }
}
