//! T4: the request table and the local stream of one agent (D62, rule 5). No database and no network: a stream is a
//! `ConnectedAgent` whose outbound queue the test reads, and the "agent" answers by calling `complete`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{AgentReply, HubCommand, NfsPath, OpResult, RequestId, ServiceRef, SwimlaneId};
use hub_platform::gateway::correlation::{Accepts, Delivery, Pending, RegisterError};
use hub_platform::gateway::local::{Epoch, LocalStream, LocalStreams};
use hub_platform::stream::{ConnectedAgent, MAX_IN_FLIGHT};
use ports::GatewayError;
use proto::convert::ToAgent;
use proto::pb;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;

fn rid(s: &str) -> RequestId {
    RequestId::parse(s).unwrap()
}

fn swimlane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

fn read(id: &str) -> HubCommand {
    HubCommand::ReadFile {
        request_id: rid(id),
        path: NfsPath::parse("app/a.yml").unwrap(),
    }
}

fn op(id: &str) -> AgentReply {
    AgentReply::Op(OpResult {
        request_id: rid(id),
        ok: true,
        error: None,
        current_hash: None,
    })
}

fn file(id: &str) -> AgentReply {
    AgentReply::File {
        request_id: rid(id),
        path: NfsPath::parse("app/a.yml").unwrap(),
        hash: ports::content_hash(b"x"),
        bytes: Bytes::from_static(b"x"),
    }
}

fn notify(id: &str) -> AgentReply {
    AgentReply::Notify {
        request_id: rid(id),
        status: 200,
    }
}

fn accepts(cmd: &HubCommand) -> Accepts {
    Accepts::for_command(cmd).expect("a command with a request id").1
}

// ------------------------------------------------------------------------------------------------- Pending

#[tokio::test]
async fn reply_completes_the_waiting_request_by_id() {
    let pending = Pending::new(8);
    let ticket = pending.register(rid("r-1"), accepts(&read("r-1"))).unwrap();
    assert_eq!(pending.complete(op("r-1")), Delivery::Delivered);
    assert_eq!(ticket.wait(Duration::from_secs(1)).await, Ok(op("r-1")));
    assert_eq!(pending.len(), 0, "a completed request leaves the table");
}

#[tokio::test]
async fn reply_with_an_unknown_or_already_answered_id_is_unmatched() {
    let pending = Pending::new(8);
    assert_eq!(pending.complete(op("nobody")), Delivery::Unmatched);
    let ticket = pending.register(rid("r-1"), accepts(&read("r-1"))).unwrap();
    assert_eq!(pending.complete(op("r-1")), Delivery::Delivered);
    assert_eq!(
        pending.complete(op("r-1")),
        Delivery::Unmatched,
        "a second answer to the same request is dropped"
    );
    assert!(ticket.wait(Duration::from_secs(1)).await.is_ok());
}

#[tokio::test]
async fn the_same_request_id_cannot_be_waited_on_twice_at_once() {
    let pending = Pending::new(8);
    let first = pending.register(rid("r-1"), accepts(&read("r-1"))).unwrap();
    assert_eq!(
        pending.register(rid("r-1"), accepts(&read("r-1"))).err(),
        Some(RegisterError::Duplicate)
    );
    drop(first);
    assert!(
        pending.register(rid("r-1"), accepts(&read("r-1"))).is_ok(),
        "the id is free again once the first caller is gone"
    );
}

#[tokio::test]
async fn close_completes_every_waiter_with_closed_and_refuses_new_ones() {
    let pending = Pending::new(8);
    let a = pending.register(rid("a"), accepts(&read("a"))).unwrap();
    let b = pending.register(rid("b"), accepts(&read("b"))).unwrap();
    pending.close();
    assert_eq!(a.wait(Duration::from_secs(1)).await, Err(GatewayError::Closed));
    assert_eq!(b.wait(Duration::from_secs(1)).await, Err(GatewayError::Closed));
    assert_eq!(
        pending.register(rid("c"), accepts(&read("c"))).err(),
        Some(RegisterError::Closed),
        "a request registered after the stream closed would wait forever"
    );
    assert_eq!(pending.len(), 0);
}

#[tokio::test]
async fn the_table_is_bounded() {
    let pending = Pending::new(2);
    let _a = pending.register(rid("a"), accepts(&read("a"))).unwrap();
    let _b = pending.register(rid("b"), accepts(&read("b"))).unwrap();
    assert_eq!(
        pending.register(rid("c"), accepts(&read("c"))).err(),
        Some(RegisterError::Full)
    );
}

#[tokio::test]
async fn a_reply_of_a_kind_the_command_cannot_have_is_not_delivered() {
    let pending = Pending::new(8);
    let ticket = pending.register(rid("r-1"), accepts(&read("r-1"))).unwrap();
    // A ReadFile is answered by file content or by an operation result, never by a notify result.
    assert_eq!(pending.complete(notify("r-1")), Delivery::Unmatched);
    assert_eq!(
        pending.len(),
        1,
        "the request is still waiting for its real answer"
    );
    assert_eq!(pending.complete(file("r-1")), Delivery::Delivered);
    assert_eq!(ticket.wait(Duration::from_secs(1)).await, Ok(file("r-1")));
}

#[tokio::test(start_paused = true)]
async fn waiting_times_out_and_frees_the_id() {
    let pending = Pending::new(8);
    let ticket = pending.register(rid("r-1"), accepts(&read("r-1"))).unwrap();
    assert_eq!(
        ticket.wait(Duration::from_millis(50)).await,
        Err(GatewayError::Timeout)
    );
    assert_eq!(pending.len(), 0);
    assert_eq!(
        pending.complete(op("r-1")),
        Delivery::Unmatched,
        "a late answer to a request nobody waits for is dropped"
    );
}

#[tokio::test]
async fn which_replies_each_command_accepts() {
    let service = ServiceRef::new("ns", "svc").unwrap();
    let cluster = HubCommand::RequestClusterReport { request_id: rid("c") };
    let restart = HubCommand::RestartDeployment {
        request_id: rid("d"),
        service,
    };
    let notify_cmd = HubCommand::NotifyConfigServer {
        request_id: rid("n"),
        paths: vec![NfsPath::parse("a.yml").unwrap()],
    };
    let (_, restart_accepts) = Accepts::for_command(&restart).unwrap();
    let (_, notify_accepts) = Accepts::for_command(&notify_cmd).unwrap();
    let (_, cluster_accepts) = Accepts::for_command(&cluster).unwrap();
    assert!(restart_accepts.allows(&op("d")));
    assert!(!restart_accepts.allows(&notify("d")));
    assert!(notify_accepts.allows(&notify("n")) && notify_accepts.allows(&op("n")));
    assert!(cluster_accepts.allows(&op("c")));
    assert!(
        Accepts::for_command(&HubCommand::RequestFullScan).is_none(),
        "a command without a request id has no reply to wait for"
    );
}

// ----------------------------------------------------------------------------------------------- LocalStream

struct Rig {
    stream: Arc<LocalStream>,
    queue: ReceiverStream<Result<pb::HubMessage, Status>>,
}

fn rig(name: &str, epoch: i64) -> Rig {
    let (agent, queue) = ConnectedAgent::new(swimlane(name));
    Rig {
        stream: LocalStream::new(agent, Epoch::new(epoch)),
        queue,
    }
}

impl Rig {
    async fn next_command(&mut self) -> HubCommand {
        let message = tokio::time::timeout(Duration::from_secs(5), self.queue.next())
            .await
            .expect("a message within 5 seconds")
            .expect("the queue is open")
            .expect("a message, not a status");
        match ToAgent::from_proto(message).unwrap().unwrap() {
            ToAgent::Command(command) => command,
            other => panic!("expected a command, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn request_writes_the_command_and_returns_the_reply_for_its_id() {
    let mut rig = rig("sit1", 3);
    let stream = Arc::clone(&rig.stream);
    let call = tokio::spawn(async move { stream.request(read("r-1"), Duration::from_secs(5)).await });
    assert_eq!(rig.next_command().await, read("r-1"));
    assert!(rig.stream.complete(op("r-1")));
    assert_eq!(call.await.unwrap(), Ok(op("r-1")));
    assert_eq!(
        rig.stream.agent().free_in_flight(),
        MAX_IN_FLIGHT,
        "the in-flight slot is released"
    );
}

#[tokio::test]
async fn a_command_without_a_request_id_is_written_and_acknowledged_at_once() {
    let mut rig = rig("sit1", 1);
    let reply = rig
        .stream
        .request(HubCommand::RequestFullScan, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(rig.next_command().await, HubCommand::RequestFullScan);
    assert_eq!(
        reply,
        op("ack"),
        "the agent answers a scan request with deltas, not with a reply"
    );
    let reply = rig
        .stream
        .request(
            HubCommand::RequestDelta {
                since_root: ports::content_hash(b"root"),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(reply, op("ack"));
}

#[tokio::test]
async fn more_than_the_in_flight_cap_is_busy() {
    let rig = rig("sit1", 1);
    let mut calls = Vec::new();
    for i in 0..MAX_IN_FLIGHT {
        let stream = Arc::clone(&rig.stream);
        let id = format!("r-{i}");
        calls.push(tokio::spawn(async move {
            stream.request(read(&id), Duration::from_secs(30)).await
        }));
    }
    for _ in 0..200 {
        if rig.stream.agent().free_in_flight() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(rig.stream.agent().free_in_flight(), 0, "every slot is taken");
    assert_eq!(
        rig.stream
            .request(read("one-too-many"), Duration::from_secs(1))
            .await,
        Err(GatewayError::Busy)
    );
    rig.stream.close();
    for call in calls {
        assert_eq!(call.await.unwrap(), Err(GatewayError::Closed));
    }
}

#[tokio::test]
async fn closing_the_stream_completes_requests_in_flight_with_closed() {
    let mut rig = rig("sit1", 1);
    let stream = Arc::clone(&rig.stream);
    let call = tokio::spawn(async move { stream.request(read("r-1"), Duration::from_secs(30)).await });
    let _ = rig.next_command().await;
    rig.stream.close();
    assert_eq!(call.await.unwrap(), Err(GatewayError::Closed));
    assert_eq!(
        rig.stream.request(read("r-2"), Duration::from_secs(1)).await,
        Err(GatewayError::Closed),
        "a request on a closed stream fails at once"
    );
    assert!(rig.stream.agent().is_closed());
}

#[tokio::test]
async fn a_reply_only_completes_a_request_on_its_own_stream() {
    let mut a = rig("sit1", 1);
    let mut b = rig("sit2", 1);
    let (sa, sb) = (Arc::clone(&a.stream), Arc::clone(&b.stream));
    let call_a = tokio::spawn(async move { sa.request(read("same-id"), Duration::from_secs(5)).await });
    let call_b = tokio::spawn(async move { sb.request(read("same-id"), Duration::from_secs(5)).await });
    let _ = a.next_command().await;
    let _ = b.next_command().await;
    assert!(a.stream.complete(op("same-id")));
    assert_eq!(call_a.await.unwrap(), Ok(op("same-id")));
    assert!(
        !a.stream.complete(op("same-id")),
        "stream A has nothing left to answer"
    );
    assert!(b.stream.complete(file("same-id")));
    assert_eq!(call_b.await.unwrap(), Ok(file("same-id")));
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_request_frees_its_slot_and_its_id() {
    let mut rig = rig("sit1", 1);
    let result = rig.stream.request(read("r-1"), Duration::from_millis(100)).await;
    assert_eq!(result, Err(GatewayError::Timeout));
    assert_eq!(rig.stream.agent().free_in_flight(), MAX_IN_FLIGHT);
    let _ = rig.next_command().await;
    // The same id may be used again: a caller retrying a timed-out command (idempotent by request id).
    let stream = Arc::clone(&rig.stream);
    let again = tokio::spawn(async move { stream.request(read("r-1"), Duration::from_secs(5)).await });
    let _ = rig.next_command().await;
    assert!(rig.stream.complete(op("r-1")));
    assert_eq!(again.await.unwrap(), Ok(op("r-1")));
}

#[tokio::test]
async fn a_request_the_agent_cannot_read_is_busy_not_blocked() {
    // Nobody reads the queue (64 messages), so the 65th cannot be written within the wait.
    let (agent, _queue) = ConnectedAgent::new(swimlane("sit1"));
    let stream = LocalStream::new(agent, Epoch::new(1));
    for i in 0..hub_platform::stream::OUTBOUND_QUEUE {
        stream
            .request(
                HubCommand::RequestDelta {
                    since_root: ports::content_hash(format!("{i}").as_bytes()),
                },
                Duration::from_secs(1),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        stream
            .request(HubCommand::RequestFullScan, Duration::from_secs(1))
            .await,
        Err(GatewayError::Busy)
    );
}

// ---------------------------------------------------------------------------------------------- LocalStreams

#[tokio::test]
async fn inserting_a_stream_for_a_swimlane_returns_the_one_it_replaces() {
    let streams = LocalStreams::new(16);
    let first = rig("sit1", 1).stream;
    let second = rig("sit1", 2).stream;
    assert!(streams.insert(Arc::clone(&first)).unwrap().is_none());
    let replaced = streams
        .insert(Arc::clone(&second))
        .unwrap()
        .expect("the first is replaced");
    assert!(Arc::ptr_eq(&replaced, &first));
    assert_eq!(streams.get(&swimlane("sit1")).unwrap().epoch(), Epoch::new(2));
    // Removing the replaced stream (its handler ending late) must not remove its successor.
    assert!(!streams.remove_if(&first));
    assert!(streams.get(&swimlane("sit1")).is_some());
    assert!(streams.remove_if(&second));
    assert!(streams.get(&swimlane("sit1")).is_none());
    assert_eq!(streams.len(), 0);
}

#[tokio::test]
async fn the_table_of_streams_is_bounded() {
    let streams = LocalStreams::new(2);
    assert!(streams.insert(rig("a", 1).stream).is_ok());
    assert!(streams.insert(rig("b", 1).stream).is_ok());
    assert!(
        streams.insert(rig("c", 1).stream).is_err(),
        "a third swimlane does not fit"
    );
    assert!(
        streams.insert(rig("a", 2).stream).is_ok(),
        "replacing a swimlane's stream needs no room"
    );
    assert_eq!(streams.snapshot().len(), 2);
}
