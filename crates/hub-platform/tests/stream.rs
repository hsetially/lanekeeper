//! T3: one agent's stream handle (rule 5): a bounded queue, a bounded number of requests in flight, and a way to close it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use domain::{ContentHash, HubCommand, SwimlaneId};
use futures::StreamExt;
use hub_platform::stream::{Busy, ConnectedAgent, MAX_IN_FLIGHT, OUTBOUND_QUEUE, SendError};
use proto::convert::ToAgent;

fn command() -> ToAgent {
    ToAgent::Command(HubCommand::RequestFullScan)
}

fn agent() -> (
    std::sync::Arc<ConnectedAgent>,
    tokio_stream::wrappers::ReceiverStream<Result<proto::pb::HubMessage, tonic::Status>>,
) {
    ConnectedAgent::new(SwimlaneId::parse("sit1").unwrap())
}

#[test]
fn the_limits_are_the_documented_ones() {
    assert_eq!(OUTBOUND_QUEUE, 64);
    assert_eq!(MAX_IN_FLIGHT, 16);
}

#[tokio::test]
async fn the_outbound_queue_holds_64_messages_and_then_says_full() {
    let (agent, mut stream) = agent();
    for _ in 0..OUTBOUND_QUEUE {
        agent.try_send(command()).unwrap();
    }
    assert_eq!(agent.try_send(command()), Err(SendError::Full));
    // Reading one makes room for exactly one.
    assert!(stream.next().await.is_some());
    assert_eq!(agent.try_send(command()), Ok(()));
    assert_eq!(agent.try_send(command()), Err(SendError::Full));
}

#[tokio::test(start_paused = true)]
async fn send_waits_for_room_for_the_time_it_was_given_and_no_longer() {
    let (agent, mut stream) = agent();
    for _ in 0..OUTBOUND_QUEUE {
        agent.try_send(command()).unwrap();
    }
    let started = tokio::time::Instant::now();
    assert_eq!(
        agent.send(command(), Duration::from_secs(5)).await,
        Err(SendError::Full)
    );
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    // With room it returns at once.
    stream.next().await.unwrap().unwrap();
    assert_eq!(agent.send(command(), Duration::from_secs(5)).await, Ok(()));
}

#[tokio::test]
async fn at_most_sixteen_requests_are_in_flight_and_a_finished_one_frees_its_slot() {
    let (agent, _stream) = agent();
    let held: Vec<_> = (0..MAX_IN_FLIGHT)
        .map(|_| agent.reserve_in_flight().unwrap())
        .collect();
    assert_eq!(agent.free_in_flight(), 0);
    assert_eq!(agent.reserve_in_flight().unwrap_err(), Busy);
    drop(held);
    assert_eq!(agent.free_in_flight(), MAX_IN_FLIGHT);
    assert!(agent.reserve_in_flight().is_ok());
}

#[tokio::test]
async fn a_dropped_stream_is_closed_for_every_sender() {
    let (agent, stream) = agent();
    assert!(!agent.is_closed());
    drop(stream);
    assert_eq!(agent.try_send(command()), Err(SendError::Closed));
    assert!(agent.is_closed());
}

#[tokio::test]
async fn close_wakes_everyone_watching() {
    let (agent, _stream) = agent();
    let mut watcher = agent.closed();
    assert!(!*watcher.borrow());
    agent.close();
    tokio::time::timeout(Duration::from_secs(1), watcher.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*watcher.borrow());
    assert!(agent.is_closed());
}

#[tokio::test]
async fn the_messages_come_out_in_the_order_they_went_in() {
    let (agent, mut stream) = agent();
    agent.try_send(ToAgent::Ack(1)).unwrap();
    agent.try_send(ToAgent::Ack(2)).unwrap();
    agent
        .try_send(ToAgent::Command(HubCommand::RequestDelta {
            since_root: ContentHash::from_bytes([3; 32]),
        }))
        .unwrap();
    let mut kinds = Vec::new();
    for _ in 0..3 {
        kinds.push(stream.next().await.unwrap().unwrap().kind.unwrap());
    }
    assert!(matches!(&kinds[0], proto::pb::hub_message::Kind::Ack(a) if a.seq == 1));
    assert!(matches!(&kinds[1], proto::pb::hub_message::Kind::Ack(a) if a.seq == 2));
    assert!(matches!(&kinds[2], proto::pb::hub_message::Kind::RequestDelta(_)));
}
