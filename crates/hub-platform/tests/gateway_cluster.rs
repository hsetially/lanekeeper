//! T4: the gateway across several hub replicas on one real Postgres (D62, P-none, S6).
//!
//! Fake agents speak the real protocol over real mutual TLS; replicas forward to each other over real mutual TLS with
//! certificates from a second CA. The acceptance test is `three_hubs_agent_moves_during_traffic_no_request_lost`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::cast_possible_truncation,
    clippy::print_stderr
)]

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use domain::{AgentReply, AgentStatus, HubCommand, NfsPath, RequestId, SwimlaneId};
use futures::StreamExt;
use hub_platform::gateway::metrics::{ForwardOutcome, RequestOutcome, RequestPath, ServeOutcome};
use ports::conformance::{AgentGatewayScenario, agent_gateway};
use ports::{AgentGateway, GatewayError};
use support::cluster::{ClusterOptions, TestCluster};
use support::fake_agent::{AgentLink, Behavior, FakeAgent};

fn swimlane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

fn read(id: &str) -> HubCommand {
    HubCommand::ReadFile {
        request_id: RequestId::parse(id).unwrap(),
        path: NfsPath::parse("app/a.yml").unwrap(),
    }
}

fn is_reply_to(reply: &AgentReply, id: &str) -> bool {
    matches!(reply, AgentReply::Op(op) if op.ok && op.request_id.as_str() == id)
}

async fn eventually(what: &str, mut condition: impl AsyncFnMut() -> bool) {
    for _ in 0..250 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Poll the gateway's `status` until `done` accepts it. A plain function over plain arguments, so that the future is `Send`.
async fn wait_for_status(
    gateway: &hub_platform::gateway::HubGateway,
    s: &SwimlaneId,
    what: &str,
    done: fn(AgentStatus) -> bool,
) {
    for _ in 0..250 {
        if done(gateway.status(s).await) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// What one request of the traffic test came to.
type Outcome = (String, Result<AgentReply, GatewayError>);

const SECOND: Duration = Duration::from_secs(1);

// ------------------------------------------------------------------------------------------- acceptance test

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_hubs_agent_moves_during_traffic_no_request_lost() {
    let Some(cluster) = TestCluster::start(3).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let s = swimlane("sit1");
    let addr = |i: usize| cluster.hubs[i].agent_addr;
    let echo = Behavior::Echo {
        delay: Duration::from_millis(5),
    };

    let mut link_a = agent.connect(&cluster.agent_ca, addr(0), echo).await.unwrap();
    let completed = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<Vec<Outcome>>> = Arc::default();

    // 500 requests, 12 at a time, from all three replicas: every request names itself, so a reply can be checked against it.
    let traffic = {
        let (cluster_hubs, completed, results, s) = (
            cluster
                .hubs
                .iter()
                .map(|h| Arc::clone(&h.gateway))
                .collect::<Vec<_>>(),
            Arc::clone(&completed),
            Arc::clone(&results),
            s.clone(),
        );
        tokio::spawn(async move {
            futures::stream::iter(0..500usize)
                .for_each_concurrent(12, |i| {
                    let (gateway, completed, results, s) = (
                        Arc::clone(&cluster_hubs[i % 3]),
                        Arc::clone(&completed),
                        Arc::clone(&results),
                        s.clone(),
                    );
                    async move {
                        let id = format!("req-{i}");
                        // The outer timeout is the test's own: a request that hangs fails here instead of waiting forever.
                        let result = tokio::time::timeout(
                            Duration::from_secs(20),
                            gateway.request(&s, read(&id), Duration::from_secs(3)),
                        )
                        .await
                        .unwrap_or_else(|_| panic!("request {id} hung past every timeout"));
                        results
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push((id, result));
                        completed.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await;
        })
    };

    let wait_for = async |count: usize| {
        eventually("traffic to progress", async || {
            completed.load(Ordering::SeqCst) >= count
        })
        .await;
    };

    // A to B: the agent connects to B while A's stream is still open, then A's is closed (the agent noticed its old
    // connection dead only after the new one was up).
    wait_for(100).await;
    let mut link_b = agent.connect(&cluster.agent_ca, addr(1), echo).await.unwrap();
    link_a.close().await;
    // B to C: the old stream goes first, so for a moment no replica holds the agent.
    wait_for(300).await;
    link_b.close().await;
    let link_c = agent.connect(&cluster.agent_ca, addr(2), echo).await.unwrap();
    tokio::time::timeout(Duration::from_secs(60), traffic)
        .await
        .expect("traffic finished")
        .unwrap();

    let results = std::mem::take(&mut *results.lock().unwrap_or_else(PoisonError::into_inner));
    assert_eq!(results.len(), 500, "every request ended");
    let (mut replied, mut failed) = (0usize, HashMap::<String, usize>::new());
    for (id, result) in &results {
        match result {
            Ok(reply) => {
                assert!(
                    is_reply_to(reply, id),
                    "the reply to {id} is for another request: {reply:?}"
                );
                replied += 1;
            }
            // A typed result is a clean ending. Which one depends on where the request was when the agent moved.
            Err(error) => *failed.entry(format!("{error:?}")).or_default() += 1,
        }
    }
    assert!(
        replied >= 150,
        "most requests were answered: {replied} replied, failures {failed:?}"
    );
    assert_eq!(replied + failed.values().sum::<usize>(), 500);
    eprintln!("moved A->B->C under traffic: {replied} replied, failures by type {failed:?}");

    // The agents' streams carried what was answered, and nothing was answered twice or for the wrong id.
    let delivered = link_a.received().len() + link_b.received().len() + link_c.received().len();
    assert!(
        delivered >= replied,
        "{delivered} commands reached an agent stream, {replied} were answered"
    );

    // After the move everything converges on C: from every replica, at once.
    for i in 0..3 {
        for n in 0..10 {
            let id = format!("after-{i}-{n}");
            let reply = cluster.hubs[i]
                .gateway
                .request(&s, read(&id), Duration::from_secs(3))
                .await
                .unwrap();
            assert!(is_reply_to(&reply, &id));
        }
    }
    let row: (String, i64) =
        sqlx::query_as("SELECT replica_id, epoch FROM agent_connections WHERE swimlane_id = 'sit1'")
            .fetch_one(&cluster.pool)
            .await
            .unwrap();
    assert_eq!(row, ("hub-2".to_owned(), 3), "C holds the third registration");
}

// ----------------------------------------------------------------------------------------------- routing

#[tokio::test]
async fn a_request_reaches_the_agent_from_every_replica_locally_or_forwarded() {
    let Some(cluster) = TestCluster::start(3).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let link = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::echo())
        .await
        .unwrap();
    let s = swimlane("sit1");

    for (i, hub) in cluster.hubs.iter().enumerate() {
        let id = format!("from-{i}");
        let reply = hub.gateway.request(&s, read(&id), SECOND * 3).await.unwrap();
        assert!(is_reply_to(&reply, &id), "{reply:?}");
        assert_eq!(hub.gateway.status(&s).await, AgentStatus::Connected);
    }
    assert_eq!(link.received().len(), 3);
    // The owner served its own request locally and the two others forwarded.
    let metrics = |i: usize| &cluster.hubs[i].gateway_metrics;
    assert_eq!(
        metrics(1).requests_total(RequestPath::Local, RequestOutcome::Replied),
        1
    );
    assert_eq!(
        metrics(0).requests_total(RequestPath::Forwarded, RequestOutcome::Replied),
        1
    );
    assert_eq!(
        metrics(2).requests_total(RequestPath::Forwarded, RequestOutcome::Replied),
        1
    );
    assert_eq!(metrics(0).forwards_total(ForwardOutcome::Replied), 1);
    assert_eq!(
        metrics(1).served_total(ServeOutcome::Answered),
        2,
        "the owner answered two forwards"
    );
}

#[tokio::test]
async fn status_is_never_seen_then_connected_then_disconnected() {
    let Some(cluster) = TestCluster::start(2).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let s = swimlane("sit1");
    assert_eq!(cluster.hubs[0].gateway.status(&s).await, AgentStatus::NeverSeen);
    assert_eq!(
        cluster.hubs[0].gateway.request(&s, read("r-1"), SECOND).await,
        Err(GatewayError::NotConnected)
    );

    let agent = cluster.agent("sit1").await;
    let mut link = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::echo())
        .await
        .unwrap();
    // The caller's own read of the registry may be up to a second old: wait for it, never sleep a fixed time.
    eventually("hub 0 to see the agent", async || {
        cluster.hubs[0].gateway.status(&s).await == AgentStatus::Connected
    })
    .await;

    link.close().await;
    eventually("hub 0 to see the agent leave", async || {
        cluster.hubs[0].gateway.status(&s).await == AgentStatus::Disconnected
    })
    .await;
    assert_eq!(
        cluster.hubs[0].gateway.request(&s, read("r-2"), SECOND).await,
        Err(GatewayError::NotConnected)
    );
}

#[tokio::test]
async fn a_forwarded_request_times_out_at_its_own_deadline() {
    let Some(cluster) = TestCluster::start(2).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let _link = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::Silent)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let result = cluster.hubs[0]
        .gateway
        .request(&swimlane("sit1"), read("r-1"), Duration::from_millis(300))
        .await;
    assert_eq!(result, Err(GatewayError::Timeout));
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "the owner reports the timeout at the request's deadline, not after the transport's slack: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn the_in_flight_cap_of_an_agent_holds_for_forwarded_requests_too() {
    let Some(cluster) = TestCluster::start(2).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let _link = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::Silent)
        .await
        .unwrap();
    let s = swimlane("sit1");
    let gateway = Arc::clone(&cluster.hubs[0].gateway);
    let calls: Vec<_> = (0..16)
        .map(|i| {
            let (gateway, s) = (Arc::clone(&gateway), s.clone());
            tokio::spawn(async move {
                gateway
                    .request(&s, read(&format!("r-{i}")), Duration::from_secs(30))
                    .await
            })
        })
        .collect();
    let owner = cluster.hubs[1].gateway.local();
    eventually("sixteen requests waiting at the owner", async || {
        owner.get(&s).is_some_and(|stream| stream.waiting() == 16)
    })
    .await;
    assert_eq!(
        gateway.request(&s, read("one-too-many"), SECOND * 2).await,
        Err(GatewayError::Busy),
        "the seventeenth is refused at the owner and the refusal travels back typed"
    );
    // Closing the agent's stream completes all sixteen with Closed, through the forward, not with a timeout.
    owner.get(&s).unwrap().close();
    for call in calls {
        assert_eq!(call.await.unwrap(), Err(GatewayError::Closed));
    }
}

#[tokio::test]
async fn a_request_to_a_replica_that_is_gone_fails_unavailable_without_hanging() {
    let Some(mut cluster) = TestCluster::start(2).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let _link = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::echo())
        .await
        .unwrap();
    // Replica 1 crashes: its replica listener goes away, and its row in the registry is still fresh for another 30 seconds.
    cluster.hubs[1].kill();
    let started = std::time::Instant::now();
    let result = cluster.hubs[0]
        .gateway
        .request(&swimlane("sit1"), read("r-1"), Duration::from_secs(5))
        .await;
    assert_eq!(result, Err(GatewayError::Unavailable));
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        cluster.hubs[0]
            .gateway_metrics
            .requests_total(RequestPath::Forwarded, RequestOutcome::Unavailable),
        1
    );
}

#[tokio::test]
async fn a_stale_route_is_refused_by_the_owner_and_the_caller_retries_once_on_a_fresh_read() {
    // The caller keeps its read of the registry for a minute, so it forwards on a route that has since gone stale.
    let mut options = ClusterOptions::new(3);
    options.settings.route_cache_ttl = Duration::from_secs(60);
    let Some(cluster) = TestCluster::start_with(options).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let s = swimlane("sit1");
    let mut on_b = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::echo())
        .await
        .unwrap();
    let caller = &cluster.hubs[0];
    assert!(is_reply_to(
        &caller
            .gateway
            .request(&s, read("warm"), SECOND * 3)
            .await
            .unwrap(),
        "warm"
    ));
    assert_eq!(
        caller.gateway_metrics.forwards_total(ForwardOutcome::StaleEpoch),
        0
    );

    // The agent leaves B and connects to C. Replica A still believes it is on B at epoch 1.
    on_b.close().await;
    let on_c = agent
        .connect(&cluster.agent_ca, cluster.hubs[2].agent_addr, Behavior::echo())
        .await
        .unwrap();
    eventually("B to release its registration", async || {
        cluster.hubs[1].gateway.local().get(&s).is_none()
    })
    .await;

    let reply = caller
        .gateway
        .request(&s, read("after-move"), SECOND * 3)
        .await
        .unwrap();
    assert!(is_reply_to(&reply, "after-move"));
    assert_eq!(
        caller.gateway_metrics.forwards_total(ForwardOutcome::StaleEpoch),
        1,
        "B refused epoch 1"
    );
    assert_eq!(
        cluster.hubs[1]
            .gateway_metrics
            .served_total(ServeOutcome::StaleEpoch),
        1
    );
    assert_eq!(on_c.received().len(), 1, "and the command went to C exactly once");
}

// --------------------------------------------------------------------------------- replaced and closed streams

#[tokio::test]
async fn a_reconnect_to_the_same_replica_closes_the_old_stream_and_fails_its_requests_with_closed() {
    let Some(cluster) = TestCluster::start(1).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let s = swimlane("sit1");
    let hub = &cluster.hubs[0];
    let old = agent
        .connect(&cluster.agent_ca, hub.agent_addr, Behavior::Silent)
        .await
        .unwrap();
    let in_flight = {
        let (gateway, s) = (Arc::clone(&hub.gateway), s.clone());
        tokio::spawn(async move {
            gateway
                .request(&s, read("on-the-old-stream"), Duration::from_secs(30))
                .await
        })
    };
    eventually("the request to reach the old stream", async || {
        old.received().len() == 1
    })
    .await;

    let new = agent
        .connect(&cluster.agent_ca, hub.agent_addr, Behavior::echo())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), in_flight)
            .await
            .expect("no wait for the 30 s timeout")
            .unwrap(),
        Err(GatewayError::Closed),
        "a request on a replaced stream fails cleanly"
    );
    eventually("the old stream to end", async || old.has_ended()).await;
    assert!(is_reply_to(
        &hub.gateway
            .request(&s, read("on-the-new-stream"), SECOND * 3)
            .await
            .unwrap(),
        "on-the-new-stream"
    ));
    assert_eq!(new.received().len(), 1);
    assert_eq!(
        hub.gateway_metrics.local_streams(),
        1,
        "the replaced stream is no longer held"
    );
    let epoch: i64 = sqlx::query_scalar("SELECT epoch FROM agent_connections WHERE swimlane_id = 'sit1'")
        .fetch_one(&cluster.pool)
        .await
        .unwrap();
    assert_eq!(epoch, 2);
}

#[tokio::test]
async fn a_stream_the_agent_left_for_another_replica_is_closed_when_liveness_notices() {
    let Some(cluster) = TestCluster::start(2).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let s = swimlane("sit1");
    // The agent is on replica 0, and a request is waiting on it.
    let old = agent
        .connect(&cluster.agent_ca, cluster.hubs[0].agent_addr, Behavior::Silent)
        .await
        .unwrap();
    let in_flight = {
        let (gateway, s) = (Arc::clone(&cluster.hubs[0].gateway), s.clone());
        tokio::spawn(async move {
            gateway
                .request(&s, read("waiting"), Duration::from_secs(30))
                .await
        })
    };
    eventually("the request to reach the stream", async || {
        old.received().len() == 1
    })
    .await;

    // It connects to replica 1 without closing the old connection (a network partition that has not been noticed yet).
    let _new = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::echo())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), in_flight)
            .await
            .expect("closed by liveness")
            .unwrap(),
        Err(GatewayError::Closed)
    );
    assert!(cluster.hubs[0].gateway_metrics.superseded_total() >= 1);
    eventually("the old stream to end", async || old.has_ended()).await;
    // And the registration stays with replica 1: the old stream's late release did not clear it.
    let row: (String, i64) =
        sqlx::query_as("SELECT replica_id, epoch FROM agent_connections WHERE swimlane_id = 'sit1'")
            .fetch_one(&cluster.pool)
            .await
            .unwrap();
    assert_eq!(row, ("hub-1".to_owned(), 2));
    assert!(is_reply_to(
        &cluster.hubs[0]
            .gateway
            .request(&s, read("forwarded"), SECOND * 3)
            .await
            .unwrap(),
        "forwarded"
    ));
}

#[tokio::test]
async fn closing_the_stream_releases_the_registration_and_fails_requests_in_flight() {
    let Some(cluster) = TestCluster::start(1).await else {
        return;
    };
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let s = swimlane("sit1");
    let hub = &cluster.hubs[0];
    let mut link = agent
        .connect(&cluster.agent_ca, hub.agent_addr, Behavior::Silent)
        .await
        .unwrap();
    let in_flight = {
        let (gateway, s) = (Arc::clone(&hub.gateway), s.clone());
        tokio::spawn(async move { gateway.request(&s, read("r-1"), Duration::from_secs(30)).await })
    };
    eventually("the request to reach the stream", async || {
        link.received().len() == 1
    })
    .await;
    link.close().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), in_flight)
            .await
            .unwrap()
            .unwrap(),
        Err(GatewayError::Closed)
    );
    eventually("the registration to be released", async || {
        let address: String =
            sqlx::query_scalar("SELECT replica_address FROM agent_connections WHERE swimlane_id = 'sit1'")
                .fetch_one(&cluster.pool)
                .await
                .unwrap();
        address.is_empty()
    })
    .await;
    assert_eq!(hub.gateway_metrics.local_streams(), 0);
    assert_eq!(hub.gateway.status(&s).await, AgentStatus::Disconnected);
}

#[tokio::test]
async fn a_reply_that_nothing_waits_for_is_dropped_and_counted_and_never_reaches_another_swimlane() {
    let Some(cluster) = TestCluster::start(1).await else {
        return;
    };
    cluster.seed(&["sit1", "sit2"]).await;
    let s1 = swimlane("sit1");
    let hub = &cluster.hubs[0];
    let a1 = cluster.agent("sit1").await;
    let a2 = cluster.agent("sit2").await;
    let silent = a1
        .connect(&cluster.agent_ca, hub.agent_addr, Behavior::Silent)
        .await
        .unwrap();
    let liar = a2
        .connect(&cluster.agent_ca, hub.agent_addr, Behavior::Silent)
        .await
        .unwrap();
    let waiting = {
        let (gateway, s1) = (Arc::clone(&hub.gateway), s1.clone());
        tokio::spawn(async move {
            gateway
                .request(&s1, read("shared-id"), Duration::from_millis(800))
                .await
        })
    };
    eventually("the request to reach sit1", async || silent.received().len() == 1).await;

    // sit2's agent answers "shared-id", which only sit1 is waiting for: dropped, counted.
    let before = hub.agent_metrics.unmatched_replies();
    let answer = HubCommand::ReadFile {
        request_id: RequestId::parse("shared-id").unwrap(),
        path: NfsPath::parse("app/a.yml").unwrap(),
    };
    let reply = support::fake_agent::answer(&answer).unwrap();
    liar.send(proto::convert::FromAgent::Reply(reply).into_proto())
        .await;
    eventually("the stray reply to be counted", async || {
        hub.agent_metrics.unmatched_replies() > before
    })
    .await;
    assert_eq!(
        waiting.await.unwrap(),
        Err(GatewayError::Timeout),
        "sit1's request was not completed by sit2's reply"
    );
}

// ------------------------------------------------------------------------------------------ the port suite

struct ClusterGateway {
    cluster: Arc<TestCluster>,
    /// The replica the callers use.
    caller: usize,
    /// The replica the agents connect to.
    home: usize,
    links: tokio::sync::Mutex<HashMap<String, AgentLink>>,
}

impl ClusterGateway {
    fn gateway(&self) -> &Arc<hub_platform::gateway::HubGateway> {
        &self.cluster.hubs[self.caller].gateway
    }

    async fn attach(&self, s: &SwimlaneId, behavior: Behavior) {
        let agent = FakeAgent::new(&self.cluster.agent_ca, s.as_str()).await;
        let link = agent
            .connect(
                &self.cluster.agent_ca,
                self.cluster.hubs[self.home].agent_addr,
                behavior,
            )
            .await
            .unwrap();
        self.links.lock().await.insert(s.as_str().to_owned(), link);
        // The caller may read the registry up to a second late; the suite expects `Connected` at once.
        wait_for_status(self.gateway(), s, "the caller to see the agent", |st| {
            st == AgentStatus::Connected
        })
        .await;
    }
}

#[async_trait]
impl AgentGateway for ClusterGateway {
    async fn request(
        &self,
        s: &SwimlaneId,
        cmd: HubCommand,
        timeout: Duration,
    ) -> Result<AgentReply, GatewayError> {
        self.gateway().request(s, cmd, timeout).await
    }

    async fn status(&self, s: &SwimlaneId) -> AgentStatus {
        self.gateway().status(s).await
    }
}

#[async_trait]
impl AgentGatewayScenario for ClusterGateway {
    async fn attach_echo_agent(&self, s: &SwimlaneId) {
        self.attach(s, Behavior::echo()).await;
    }

    async fn attach_silent_agent(&self, s: &SwimlaneId) {
        self.attach(s, Behavior::Silent).await;
    }

    async fn detach_agent(&self, s: &SwimlaneId) {
        if let Some(mut link) = self.links.lock().await.remove(s.as_str()) {
            link.close().await;
        }
        wait_for_status(self.gateway(), s, "the caller to see the agent leave", |st| {
            st != AgentStatus::Connected
        })
        .await;
    }
}

#[tokio::test]
async fn agent_gateway_conformance_when_the_agent_is_on_the_calling_replica() {
    let Some(cluster) = TestCluster::start(2).await else {
        return;
    };
    cluster.seed(&["sit1", "sit2"]).await;
    let gateway = ClusterGateway {
        cluster: Arc::new(cluster),
        caller: 0,
        home: 0,
        links: tokio::sync::Mutex::default(),
    };
    agent_gateway(&gateway).await;
}

#[tokio::test]
async fn agent_gateway_conformance_when_the_agent_is_on_another_replica() {
    let mut options = ClusterOptions::new(2);
    // A short read-cache, so that the suite's expectation of `Disconnected` right after a detach is met by a replica that did
    // not hold the stream within a moment.
    options.settings.route_cache_ttl = Duration::from_millis(50);
    let Some(cluster) = TestCluster::start_with(options).await else {
        return;
    };
    cluster.seed(&["sit1", "sit2"]).await;
    let gateway = ClusterGateway {
        cluster: Arc::new(cluster),
        caller: 0,
        home: 1,
        links: tokio::sync::Mutex::default(),
    };
    agent_gateway(&gateway).await;
}
