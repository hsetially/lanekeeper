//! T11 (D74, P15): the hub acknowledges a scan delta only after `ReportSink::delta` returned `Ok`, in the order the deltas
//! arrived, once per applied message. An agent that is not acknowledged keeps the version in its spool and resends it, so an ack
//! sent too early is a lost version, and one that never comes is only a resend.
//!
//! The hub side is the real `AgentService` behind the real listener over real mutual TLS; the sink is a scripted double.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use domain::{AgentConfig, ClusterReport, Heartbeat, HeartbeatAction, Hello, ScanDelta, TenantId};
use hub_platform::agent_service::StreamLimits;
use ports::{AgentIdentity, ReportSink, SinkError};
use proto::pb;
use support::stream_rig::{StreamRig, delta};
use tokio::sync::Notify;

/// What the sink was asked and what it answered, with a gate to hold a commit open and a way to fail one.
#[derive(Default)]
struct ScriptedSink {
    started: Mutex<Vec<(String, u64)>>,
    committed: Mutex<Vec<(String, u64)>>,
    gates: Mutex<HashMap<u64, Arc<Notify>>>,
    failures: Mutex<HashMap<u64, SinkError>>,
    /// How long each seq takes to commit.
    delays: Mutex<HashMap<u64, Duration>>,
    heartbeats: Mutex<usize>,
}

impl ScriptedSink {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn hold(&self, seq: u64) -> Arc<Notify> {
        let gate = Arc::new(Notify::new());
        self.gates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seq, Arc::clone(&gate));
        gate
    }

    fn fail(&self, seq: u64, error: SinkError) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seq, error);
    }

    fn delay(&self, seq: u64, by: Duration) {
        self.delays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seq, by);
    }

    fn started(&self) -> Vec<(String, u64)> {
        self.started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn committed(&self) -> Vec<(String, u64)> {
        self.committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn wait_started(&self, seq: u64) {
        for _ in 0..250 {
            if self.started().iter().any(|(_, s)| *s == seq) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the sink was never asked for seq {seq}");
    }
}

#[async_trait]
impl ReportSink for ScriptedSink {
    async fn hello(&self, _: &AgentIdentity, _: Hello) -> Result<AgentConfig, SinkError> {
        Ok(AgentConfig {
            scan_interval_secs: 10,
            heartbeat_interval_secs: 5,
            max_file_bytes: 1_000_000,
            deny_globs: Vec::new(),
            env_allowlist: Vec::new(),
            tenants: vec![TenantId::parse("sit1").unwrap()],
        })
    }

    async fn heartbeat(&self, _: &AgentIdentity, _: Heartbeat) -> Result<HeartbeatAction, SinkError> {
        *self.heartbeats.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Ok(HeartbeatAction::None)
    }

    async fn delta(&self, id: &AgentIdentity, d: ScanDelta) -> Result<(), SinkError> {
        let who = id.swimlane().as_str().to_owned();
        self.started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((who.clone(), d.seq));
        let gate = self
            .gates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&d.seq)
            .cloned();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let delay = self
            .delays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&d.seq)
            .copied();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        if let Some(error) = self
            .failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&d.seq)
        {
            return Err(*error);
        }
        self.committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((who, d.seq));
        Ok(())
    }

    async fn cluster(&self, _: &AgentIdentity, _: ClusterReport) -> Result<(), SinkError> {
        Ok(())
    }
}

const QUIET: Duration = Duration::from_millis(400);
const WITHIN: Duration = Duration::from_secs(5);

#[tokio::test]
async fn ack_is_sent_only_after_delta_commits() {
    let sink = ScriptedSink::new();
    let gate = sink.hold(1);
    let rig = StreamRig::agents(sink.clone(), StreamLimits::default()).await;
    let mut stream = rig.open_agent("sit1").await;

    stream.send_delta(delta(1, false)).await;
    sink.wait_started(1).await;
    // The sink has the delta and has not committed it: there must be no acknowledgement, however long we wait.
    assert_eq!(
        stream.next_ack(QUIET).await,
        None,
        "an ack arrived while the sink was still committing"
    );
    assert!(sink.committed().is_empty());

    // The commit returns; only now does the ack follow, and the sink had committed by then.
    gate.notify_one();
    assert_eq!(stream.next_ack(WITHIN).await, Some(1));
    assert_eq!(sink.committed(), [("sit1".to_owned(), 1)]);
    assert_eq!(
        stream.next_ack(QUIET).await,
        None,
        "one ack per applied message, not two"
    );
}

#[tokio::test]
async fn ack_order_follows_delta_order() {
    let sink = ScriptedSink::new();
    // Earlier deltas take longer, so that anything that acknowledged in the order of completion, or in parallel, would show.
    for seq in 1..=12_u64 {
        sink.delay(seq, Duration::from_millis((13 - seq) * 6));
    }
    let rig = StreamRig::agents(sink.clone(), StreamLimits::default()).await;
    let mut stream = rig.open_agent("sit1").await;

    for seq in 1..=12_u64 {
        stream.send_delta(delta(seq, false)).await;
    }
    let mut acked = Vec::new();
    for _ in 1..=12 {
        acked.push(stream.next_ack(WITHIN).await.expect("an ack for every delta"));
    }
    assert_eq!(
        acked,
        (1..=12).collect::<Vec<_>>(),
        "acks follow the order the deltas arrived in"
    );
    let committed: Vec<u64> = sink.committed().into_iter().map(|(_, s)| s).collect();
    assert_eq!(
        committed,
        (1..=12).collect::<Vec<_>>(),
        "and the sink applied them in that order too"
    );
    assert_eq!(stream.next_ack(QUIET).await, None, "exactly one ack per message");
}

#[tokio::test]
async fn failed_delta_sends_no_ack() {
    let sink = ScriptedSink::new();
    sink.fail(2, SinkError::Unavailable);
    sink.fail(4, SinkError::Invalid);
    sink.fail(5, SinkError::TooLarge);
    let rig = StreamRig::agents(sink.clone(), StreamLimits::default()).await;
    let mut stream = rig.open_agent("sit1").await;

    for seq in 1..=6_u64 {
        stream.send_delta(delta(seq, false)).await;
    }
    let mut acked = Vec::new();
    while let Some(seq) = stream.next_ack(QUIET).await {
        acked.push(seq);
    }
    assert_eq!(
        acked,
        [1, 3, 6],
        "the deltas the sink refused are not acknowledged, so the agent keeps them in its spool and resends"
    );
    // The stream is still usable: a refusal costs the agent a resend, not its connection.
    stream
        .send(pb::agent_message::Kind::Heartbeat(pb::Heartbeat {
            scan_seq: 7,
            merkle_root: vec![7_u8; 32].into(),
            file_count: 1,
        }))
        .await;
    stream.send_delta(delta(7, false)).await;
    assert_eq!(stream.next_ack(WITHIN).await, Some(7));
    assert_eq!(*sink.heartbeats.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_delta_the_sink_does_not_finish_in_time_sends_no_ack() {
    let sink = ScriptedSink::new();
    let _never = sink.hold(1);
    let limits = StreamLimits {
        sink_timeout: Duration::from_millis(300),
        ..StreamLimits::default()
    };
    let rig = StreamRig::agents(sink.clone(), limits).await;
    let mut stream = rig.open_agent("sit1").await;

    stream.send_delta(delta(1, false)).await;
    sink.wait_started(1).await;
    assert_eq!(
        stream.next_ack(Duration::from_millis(900)).await,
        None,
        "a commit that did not finish is not an acknowledged one"
    );
    stream.send_delta(delta(2, false)).await;
    assert_eq!(stream.next_ack(WITHIN).await, Some(2), "the stream went on");
}

#[tokio::test]
async fn a_continuation_message_of_a_multi_part_delta_is_acked_as_the_sink_accepts_it() {
    let sink = ScriptedSink::new();
    let last = sink.hold(3);
    let rig = StreamRig::agents(sink.clone(), StreamLimits::default()).await;
    let mut stream = rig.open_agent("sit1").await;

    // One logical delta in three messages: the first two say `more`, and each has its own seq and its own ack.
    for (seq, more) in [(1, true), (2, true), (3, false)] {
        let mut part = delta(seq, more);
        part.part = u32::try_from(seq).unwrap() - 1;
        stream.send_delta(part).await;
    }
    assert_eq!(
        stream.next_ack(WITHIN).await,
        Some(1),
        "acked as the sink accepted the first part"
    );
    assert_eq!(stream.next_ack(WITHIN).await, Some(2));
    sink.wait_started(3).await;
    assert_eq!(
        stream.next_ack(QUIET).await,
        None,
        "the last part is not acked until it is applied"
    );
    last.notify_one();
    assert_eq!(stream.next_ack(WITHIN).await, Some(3));
}

#[tokio::test]
async fn an_ack_goes_only_to_the_stream_that_sent_the_delta() {
    let sink = ScriptedSink::new();
    let rig = StreamRig::agents(sink.clone(), StreamLimits::default()).await;
    let mut sit1 = rig.open_agent("sit1").await;
    let mut sit2 = rig.open_agent("sit2").await;

    sit1.send_delta(delta(5, false)).await;
    assert_eq!(sit1.next_ack(WITHIN).await, Some(5));
    assert_eq!(
        sit2.next_ack(QUIET).await,
        None,
        "another swimlane's stream hears nothing"
    );
    sit2.send_delta(delta(5, false)).await;
    assert_eq!(sit2.next_ack(WITHIN).await, Some(5));
    assert_eq!(
        sink.committed(),
        [("sit1".to_owned(), 5), ("sit2".to_owned(), 5)],
        "the sink saw each under the identity of its own certificate"
    );
}

#[tokio::test]
async fn acks_are_counted_and_the_count_is_the_number_of_acks_sent() {
    let sink = ScriptedSink::new();
    sink.fail(3, SinkError::Unavailable);
    let rig = StreamRig::agents(sink.clone(), StreamLimits::default()).await;
    let mut stream = rig.open_agent("sit1").await;
    for seq in 1..=4_u64 {
        stream.send_delta(delta(seq, false)).await;
    }
    let mut n = 0;
    while stream.next_ack(QUIET).await.is_some() {
        n += 1;
    }
    assert_eq!(n, 3);
    assert_eq!(rig.metrics.acks_sent(), 3);
}

/// The ack carries the delta's own `seq` and nothing else from it, whatever the number.
#[tokio::test]
async fn the_ack_carries_the_seq_of_the_delta_it_acknowledges() {
    let sink = ScriptedSink::new();
    let rig = StreamRig::agents(sink.clone(), StreamLimits::default()).await;
    let mut stream = rig.open_agent("sit1").await;
    for seq in [u64::MAX, 0, 41] {
        stream.send_delta(delta(seq, false)).await;
        assert_eq!(stream.next_ack(WITHIN).await, Some(seq));
    }
}
