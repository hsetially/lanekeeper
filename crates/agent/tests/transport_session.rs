//! The session with the hub (T3, S6, S11): what it sends first, what it does with each kind of message from the hub,
//! how it reconnects, and how a certificate is renewed over the stream.
//!
//! Everything runs against the fake hub on the in-memory network, under `tokio::time::pause`, so waits of hours cost
//! nothing and the retry delays can be compared exactly with the backoff schedule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent::backoff::{Backoff, ceiling};
use agent::config::Settings;
use agent::identity::joiner::{IdentityHandle, Joiner};
use agent::identity::store::MemoryCertStore;
use agent::identity::{KeyMaterial, RenewError};
use agent::transport::session::{ConnectionState, Session, SessionConfig};
use agent::transport::{Outbox, OutboxError};
use bytes::Bytes;
use domain::{AgentReply, OpError};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use support::fake_hub::{ConnHandle, HubServer, ScriptedIdTokens};
use support::rig::{Recorder, Rig, WI_TOKEN, hello};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

struct Running {
    rig: Rig,
    session: Arc<Session>,
    recorder: Arc<Recorder>,
    identity: Arc<IdentityHandle>,
    task: JoinHandle<Infallible>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Running {
    async fn start() -> Self {
        Self::start_with(SessionConfig::default()).await
    }

    async fn start_with(config: SessionConfig) -> Self {
        let rig = Rig::new();
        let identity = rig.identity_handle().await;
        Self::start_on(rig, identity, config)
    }

    fn start_on(rig: Rig, identity: Arc<IdentityHandle>, config: SessionConfig) -> Self {
        let session = Arc::new(Session::new(
            hello(),
            rig.transport.clone(),
            identity.clone(),
            config,
        ));
        let recorder = Recorder::new();
        let task = {
            let (session, recorder) = (session.clone(), recorder.clone());
            tokio::spawn(async move { session.run(recorder).await })
        };
        Self {
            rig,
            session,
            recorder,
            identity,
            task,
        }
    }

    fn server(&self) -> &Arc<HubServer> {
        &self.rig.server
    }

    async fn connected(&self) {
        self.state_is(ConnectionState::Connected).await;
    }

    async fn state_is(&self, wanted: ConnectionState) {
        tokio::time::timeout(
            Duration::from_secs(600),
            self.session.state().wait_for(|s| *s == wanted),
        )
        .await
        .unwrap_or_else(|_| panic!("the session never reached {wanted:?}"))
        .unwrap();
    }

    /// The first connection, once the handler is running on it.
    async fn first_connection(&self) -> ConnHandle {
        let conn = self.server().wait_for_connection(1).await;
        conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
        conn
    }
}

fn is_op_result(request_id: &str) -> impl Fn(&FromAgent) -> bool + '_ {
    move |m| matches!(m, FromAgent::Reply(AgentReply::Op(r)) if r.request_id.as_str() == request_id)
}

#[tokio::test(start_paused = true)]
async fn the_session_says_hello_and_hands_the_handler_the_hubs_configuration() {
    let rig = Rig::new();
    rig.server.set_config(pb::AgentConfig {
        scan_interval_secs: 60,
        heartbeat_interval_secs: 30,
        max_file_bytes: 1024,
        deny_globs: vec!["*.secret".into()],
        env_allowlist: vec!["CONFIG_CLIENT_CACHE_TTL".into()],
        tenants: vec!["sit1".into(), "sit2".into()],
    });
    let identity = rig.identity_handle().await;
    let running = Running::start_on(rig, identity, SessionConfig::default());
    assert_eq!(*running.session.state().borrow(), ConnectionState::Disconnected);

    let conn = running.first_connection().await;
    running.connected().await;

    // The first message is the Hello, with the agent's own facts.
    let hello_sent = conn.hello();
    assert_eq!(hello_sent, hello());
    // The handler got the hub's settings, clamped by the same rules as every other hub value.
    let links = running.recorder.links();
    assert_eq!(links.len(), 1);
    let tunables = &links[0].tunables;
    assert_eq!(tunables.scan_interval, Duration::from_secs(60));
    assert_eq!(tunables.heartbeat_interval, Duration::from_secs(30));
    assert_eq!(tunables.max_file_bytes, 1024);
    assert_eq!(tunables.tenants.len(), 2);
    // And what the handler queued reached the hub.
    assert!(matches!(
        conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await,
        FromAgent::Heartbeat(h) if h.scan_seq == 1
    ));
    assert_eq!(running.session.stats().connections(), 1);
}

#[tokio::test(start_paused = true)]
async fn invalid_hub_message_is_dropped_stream_survives() {
    use pb::hub_message::Kind;
    let running = Running::start().await;
    let conn = running.first_connection().await;
    running.connected().await;

    // 1. A write to a path that escapes the root: refused, and the hub is told so by request id.
    conn.send_raw(pb::HubMessage {
        kind: Some(Kind::WriteFile(pb::WriteFile {
            request_id: "req-bad".into(),
            path: "../etc/passwd".into(),
            expected: Some(pb::Expected {
                state: Some(pb::expected::State::Absent(pb::expected::Absent {})),
            }),
            content: Bytes::from_static(b"x"),
        })),
    })
    .await;
    // 2. A configuration that would make the agent scan in a loop: dropped.
    conn.send_raw(pb::HubMessage {
        kind: Some(Kind::AgentConfig(pb::AgentConfig {
            scan_interval_secs: 0,
            heartbeat_interval_secs: 10,
            ..pb::AgentConfig::default()
        })),
    })
    .await;
    // 3. A message this build knows nothing about: skipped.
    conn.send_raw(pb::HubMessage { kind: None }).await;
    // 4. A command whose request id cannot be read: dropped, with nobody to tell.
    conn.send_raw(pb::HubMessage {
        kind: Some(Kind::ReadFile(pb::ReadFile {
            request_id: String::new(),
            path: "../x".into(),
        })),
    })
    .await;
    // 5. Then a good one, which must still arrive.
    conn.send(ToAgent::Ack(5)).await;

    let refusal = conn.wait_for(is_op_result("req-bad")).await;
    let FromAgent::Reply(AgentReply::Op(result)) = refusal else {
        panic!("an OpResult");
    };
    assert!(!result.ok);
    assert_eq!(result.error, Some(OpError::Denied));
    assert_eq!(result.current_hash, None);

    tokio::time::timeout(Duration::from_secs(30), async {
        while running.recorder.links()[0].received.is_empty() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the valid message never reached the handler: the stream did not survive the invalid ones");
    assert_eq!(
        running.recorder.links()[0].received,
        vec![ToAgent::Ack(5)],
        "only the valid message reaches the handler"
    );
    let stats = running.session.stats();
    assert_eq!(stats.invalid_messages(), 3, "the write, the config and the read");
    assert_eq!(stats.unknown_messages(), 1);
    // The stream survived all of it: no reconnect, and the answer to the hostile write was the only reply sent.
    assert_eq!(running.server().connection_count(), 1);
    assert_eq!(*running.session.state().borrow(), ConnectionState::Connected);
    let replies = conn
        .received()
        .into_iter()
        .filter(|m| matches!(m, FromAgent::Reply(_)))
        .count();
    assert_eq!(replies, 1);
}

// ------------------------------------------------------------------ reconnecting

/// The gaps between the dials the hub's network saw, in order.
fn gaps(dials: &[Instant]) -> Vec<Duration> {
    dials.windows(2).map(|w| w[1] - w[0]).collect()
}

/// A timer fires on a whole millisecond, never early, so a wait of `delay` is observed as `delay` plus less than 2 ms.
#[track_caller]
fn assert_waited(gap: Duration, delay: Duration, what: &str) {
    assert!(
        gap >= delay && gap < delay + Duration::from_millis(2),
        "{what}: waited {gap:?}, the schedule says {delay:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn reconnect_delays_follow_the_backoff_schedule() {
    let rig = Rig::new();
    let identity = rig.identity_handle().await;
    rig.server.net.set_reachable(false);
    let seed = 42;
    let before = rig.server.net.dials().len();
    let running = Running::start_on(
        rig,
        identity,
        SessionConfig {
            seed: Some(seed),
            ..SessionConfig::default()
        },
    );
    sleep(Duration::from_secs(20 * 60)).await;

    let dials = running.server().net.dials();
    let dials = &dials[before..];
    assert!(dials.len() > 12, "only {} attempts in 20 minutes", dials.len());
    // Exactly the schedule of `Backoff`: full jitter, doubling from 1 s, capped at 60 s.
    let mut expected = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(60), seed);
    for (n, gap) in gaps(dials).into_iter().enumerate() {
        let delay = expected.next_delay();
        assert_waited(gap, delay, &format!("gap {n}"));
        let most = ceiling(
            Duration::from_secs(1),
            Duration::from_secs(60),
            u32::try_from(n).unwrap(),
        );
        assert!(delay <= most);
        assert!(
            gap <= Duration::from_secs(60) + Duration::from_millis(2),
            "S6: never longer than a minute"
        );
    }
    // Every one of those dials was refused, and each refusal was counted as a failed attempt.
    assert_eq!(
        usize::try_from(running.session.stats().failed_attempts()).unwrap(),
        dials.len()
    );
}

#[tokio::test(start_paused = true)]
async fn a_flapping_hub_does_not_reset_the_backoff() {
    let running = Running::start_with(SessionConfig {
        seed: Some(7),
        ..SessionConfig::default()
    })
    .await;
    let net = running.server().net.clone();
    let first_dial = net.dials().len();
    // The hub accepts, sends its configuration, and drops the connection at once, five times in a row.
    for n in 1..=5 {
        running.server().wait_for_connection(n).await;
        running.connected().await;
        net.kill_connections();
        running.state_is(ConnectionState::Disconnected).await;
    }
    let dials = net.dials();
    let dials = &dials[first_dial..];
    let mut expected = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(60), 7);
    for (n, gap) in gaps(dials).into_iter().enumerate() {
        assert_waited(
            gap,
            expected.next_delay(),
            &format!("gap {n} (a short connection must not start the schedule over)"),
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_stayed_up_starts_the_schedule_over() {
    let running = Running::start_with(SessionConfig {
        seed: Some(7),
        ..SessionConfig::default()
    })
    .await;
    let net = running.server().net.clone();
    // Make the schedule long first: three connections that die at once.
    for n in 1..=3 {
        running.server().wait_for_connection(n).await;
        running.connected().await;
        net.kill_connections();
        running.state_is(ConnectionState::Disconnected).await;
    }
    // Then one that lives for a minute.
    running.server().wait_for_connection(4).await;
    running.connected().await;
    sleep(Duration::from_secs(60)).await;
    let killed_at = Instant::now();
    net.kill_connections();
    let conn5 = running.server().wait_for_connection(5).await;
    let waited = net.dials().last().copied().unwrap() - killed_at;
    assert!(
        waited <= Duration::from_secs(1),
        "after a good connection the first delay is at most the base (1 s), not {waited:?}"
    );
    drop(conn5);
}

#[tokio::test(start_paused = true)]
async fn a_refusal_from_the_hub_is_retried_and_counted() {
    let rig = Rig::new();
    let identity = rig.identity_handle().await;
    rig.server.set_refuse_connect(true);
    let running = Running::start_on(rig, identity, SessionConfig::default());
    sleep(Duration::from_secs(5 * 60)).await;
    assert!(running.session.stats().failed_attempts() >= 5);
    assert_eq!(running.session.stats().connections(), 0);
    assert_eq!(*running.session.state().borrow(), ConnectionState::Disconnected);
    assert!(running.recorder.links().is_empty(), "the handler never ran");

    running.server().set_refuse_connect(false);
    tokio::time::timeout(Duration::from_secs(61), running.connected())
        .await
        .expect("the session must connect within one backoff cap once the hub allows it");
}

#[tokio::test(start_paused = true)]
async fn a_hub_that_never_sends_its_configuration_is_abandoned_after_30_seconds() {
    let rig = Rig::new();
    let identity = rig.identity_handle().await;
    rig.server.set_send_config(false);
    let running = Running::start_on(rig, identity, SessionConfig::default());
    running.server().wait_for_connection(1).await;
    let started = Instant::now();
    sleep(Duration::from_secs(29)).await;
    assert_eq!(
        running.session.stats().failed_attempts(),
        0,
        "still waiting at 29 s"
    );
    sleep(Duration::from_secs(2)).await;
    assert_eq!(running.session.stats().failed_attempts(), 1, "gave up at 30 s");
    assert!(started.elapsed() < Duration::from_secs(40));
    assert!(running.recorder.links().is_empty(), "the handler never ran");
    // And it tries again, with the hub fixed.
    running.server().set_send_config(true);
    tokio::time::timeout(Duration::from_secs(90), running.connected())
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_hub_ending_the_stream_makes_the_session_connect_again() {
    let running = Running::start().await;
    let conn = running.first_connection().await;
    running.connected().await;
    conn.end_stream();
    running.server().wait_for_connection(2).await;
    running.connected().await;
    assert_eq!(running.session.stats().connections(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_outbox_belongs_to_one_connection() {
    let running = Running::start().await;
    running.first_connection().await;
    running.connected().await;
    running.server().net.kill_connections();
    running.server().wait_for_connection(2).await;
    running
        .server()
        .connection(1)
        .wait_for(|m| matches!(m, FromAgent::Heartbeat(_)))
        .await;

    let outboxes = running.recorder.outboxes();
    assert_eq!(outboxes.len(), 2);
    let stale: &Outbox = &outboxes[0];
    assert!(stale.is_closed());
    let message = FromAgent::Heartbeat(domain::Heartbeat {
        scan_seq: 99,
        merkle_root: domain::ContentHash::from_bytes([9; 32]),
        file_count: 0,
    });
    assert_eq!(stale.try_send(message), Err(OutboxError::Closed));
    // Nothing queued on the first connection was replayed on the second: its only heartbeat is its own.
    let heartbeats = running
        .server()
        .connection(1)
        .received()
        .into_iter()
        .filter(|m| matches!(m, FromAgent::Heartbeat(_)))
        .count();
    assert_eq!(heartbeats, 1);
}

// ------------------------------------------------------------------ renewal over the stream

fn csr() -> Bytes {
    KeyMaterial::generate().unwrap().csr_der().unwrap()
}

#[tokio::test(start_paused = true)]
async fn renewal_while_disconnected_fails_at_once() {
    let rig = Rig::new();
    let identity = rig.identity_handle().await;
    rig.server.net.set_reachable(false);
    let running = Running::start_on(rig, identity, SessionConfig::default());
    let joiner_channel: &dyn agent::identity::joiner::RenewalChannel = &*running.session;
    assert_eq!(
        joiner_channel.renew(csr()).await.err(),
        Some(RenewError::NotConnected)
    );
}

#[tokio::test(start_paused = true)]
async fn the_renewal_answer_goes_to_the_caller_and_not_to_the_handler() {
    let running = Running::start().await;
    let conn = running.first_connection().await;
    running.connected().await;
    let request = csr();

    let channel: &dyn agent::identity::joiner::RenewalChannel = &*running.session;
    let issued = channel.renew(request.clone()).await.unwrap();

    assert!(!issued.cert_chain_der.is_empty());
    let FromAgent::CertRenewal { csr_der } = conn
        .wait_for(|m| matches!(m, FromAgent::CertRenewal { .. }))
        .await
    else {
        panic!("a renewal request");
    };
    assert_eq!(csr_der, request, "the hub got the CSR the caller made");
    assert!(
        running.recorder.links()[0].received.is_empty(),
        "the handler must never see the certificate"
    );
}

#[tokio::test(start_paused = true)]
async fn a_renewal_the_hub_never_answers_times_out_and_leaves_nothing_behind() {
    let running = Running::start().await;
    running.first_connection().await;
    running.connected().await;
    running.server().set_answer_renewals(false);
    let channel: &dyn agent::identity::joiner::RenewalChannel = &*running.session;

    let started = Instant::now();
    assert_eq!(channel.renew(csr()).await.err(), Some(RenewError::Timeout));
    assert_eq!(started.elapsed(), Duration::from_secs(30));

    // A later renewal works: the abandoned one did not block the slot.
    running.server().set_answer_renewals(true);
    channel.renew(csr()).await.unwrap();
    assert_eq!(running.session.stats().unsolicited_renewals(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_renewal_in_flight_when_the_connection_dies_fails_as_not_connected() {
    let running = Running::start().await;
    running.first_connection().await;
    running.connected().await;
    running.server().set_answer_renewals(false);
    let session = running.session.clone();
    let renewal = tokio::spawn(async move {
        let channel: &dyn agent::identity::joiner::RenewalChannel = &*session;
        channel.renew(csr()).await
    });
    sleep(Duration::from_secs(1)).await;
    running.server().net.kill_connections();
    assert_eq!(renewal.await.unwrap().err(), Some(RenewError::NotConnected));
}

fn settings() -> Settings {
    Settings::from_env(&support::valid_env(Path::new("/nonexistent"))).unwrap()
}

#[tokio::test(start_paused = true)]
async fn renewed_cert_is_presented_on_the_next_handshake() {
    // The whole path: the joiner renews at half the lifetime over the session's stream, publishes the new identity,
    // and the session reconnects with it (A22), so the hub sees the new certificate on the next connection.
    let rig = Rig::new();
    let handle = rig.identity_handle().await;
    let first_leaf = handle.current().chain_der()[0].to_vec();
    let first_not_after = handle.current().not_after();
    let running = Running::start_on(rig, handle.clone(), SessionConfig::default());

    let joiner = Arc::new(Joiner::new(
        &settings(),
        ScriptedIdTokens::new(Ok(WI_TOKEN)),
        None,
        running.rig.transport.clone(),
        Arc::new(MemoryCertStore::new()),
        running.rig.clock.clone(),
    ));
    let mut published = handle.subscribe();
    let maintain = {
        let (joiner, handle, session) = (joiner.clone(), handle.clone(), running.session.clone());
        tokio::spawn(async move { joiner.maintain(&handle, &*session).await })
    };

    let conn1 = running.first_connection().await;
    assert_eq!(conn1.client_cert().as_deref(), Some(first_leaf.as_slice()));

    // Half the lifetime later: a little under 12 hours, because the certificate is backdated by five minutes.
    tokio::time::timeout(Duration::from_secs(13 * 3600), published.changed())
        .await
        .expect("the joiner never renewed")
        .unwrap();

    let conn2 = running.server().wait_for_connection(2).await;
    let second_leaf = conn2
        .client_cert()
        .expect("the second connection presents a certificate");
    assert_ne!(
        second_leaf, first_leaf,
        "the renewed certificate, not the old one"
    );
    assert_eq!(
        second_leaf.as_slice(),
        handle.current().chain_der()[0].as_ref(),
        "and it is the one the handle holds now"
    );
    let renewals = conn1
        .received()
        .into_iter()
        .filter(|m| matches!(m, FromAgent::CertRenewal { .. }))
        .count();
    assert_eq!(renewals, 1, "the renewal went over the first stream");
    assert_eq!(running.session.stats().connections(), 2);
    assert!(
        running.identity.current().not_after() > first_not_after,
        "the new certificate lives past the old one"
    );
    maintain.abort();
}

// ------------------------------------------------------------------ the session does not care what carries it

#[test]
fn hello_is_built_from_the_settings() {
    let settings = settings();
    let hello = agent::transport::session::hello(&settings).unwrap();
    assert_eq!(hello.swimlane.as_str(), "sit1");
    assert_eq!(hello.cluster.as_str(), "gke-sit1");
    assert_eq!(hello.project.as_str(), "bank-sit");
    assert_eq!(hello.nfs_server.as_str(), "10.1.2.3");
    assert_eq!(hello.export.as_str(), "/export/csp");
    assert_eq!(hello.mount_root.as_str(), "/nonexistent");
    assert_eq!(hello.agent_version.as_str(), env!("CARGO_PKG_VERSION"));
}

const SPAM_LIMIT: usize = 5000;

/// A handler that queues heartbeats as fast as the outbox takes them, and counts how many it got in.
#[derive(Default)]
struct Spammer {
    accepted: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl agent::transport::LinkHandler for Spammer {
    async fn handle(&self, link: agent::transport::Link) {
        let mut n = 0_usize;
        loop {
            // A handler that never waits would starve the paused clock, so it stops at a number far beyond the queue.
            if n >= SPAM_LIMIT {
                std::future::pending::<()>().await;
            }
            n += 1;
            let beat = FromAgent::Heartbeat(domain::Heartbeat {
                scan_seq: n as u64,
                merkle_root: domain::ContentHash::from_bytes([1; 32]),
                file_count: n as u64,
            });
            if link.outbox.send(beat).await.is_err() {
                return;
            }
            self.accepted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

fn spammer_session(
    transport: Arc<support::scripted_transport::ScriptedTransport>,
    identity: Arc<IdentityHandle>,
    config: SessionConfig,
) -> (Arc<Session>, Arc<Spammer>, JoinHandle<Infallible>) {
    let session = Arc::new(Session::new(hello(), transport, identity, config));
    let spammer = Arc::new(Spammer::default());
    let task = {
        let (session, spammer) = (session.clone(), spammer.clone());
        tokio::spawn(async move { session.run(spammer).await })
    };
    (session, spammer, task)
}

fn accepted(spammer: &Spammer) -> usize {
    spammer.accepted.load(std::sync::atomic::Ordering::SeqCst)
}

#[tokio::test(start_paused = true)]
async fn the_session_runs_over_any_transport_and_a_stalled_hub_slows_the_handler_down() {
    use support::scripted_transport::ScriptedTransport;
    let identity = Rig::new().identity_handle().await;
    let transport = ScriptedTransport::new();
    let (session, spammer, task) = spammer_session(transport.clone(), identity, SessionConfig::default());

    // The hub speaks first after Hello, as the real one does.
    let mut hub = transport.accept(1).await;
    let first = hub.from_agent.recv().await.unwrap();
    assert!(matches!(first.kind, Some(pb::agent_message::Kind::Hello(_))));
    hub.send(ToAgent::Config(domain::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: Vec::new(),
        env_allowlist: Vec::new(),
        tenants: Vec::new(),
    }))
    .await;
    tokio::time::timeout(
        Duration::from_secs(60),
        session.state().wait_for(|s| *s == ConnectionState::Connected),
    )
    .await
    .unwrap()
    .unwrap();

    // Now the hub stops reading. The handler fills the outbox, and then it waits: it is not allowed to go on.
    sleep(Duration::from_secs(10)).await;
    let stuck_at = accepted(&spammer);
    let room = SessionConfig::default().outbox.messages;
    assert!(
        (room..=room + 2).contains(&stuck_at),
        "{stuck_at} messages accepted by a hub that reads nothing; the queue holds {room}"
    );
    sleep(Duration::from_secs(300)).await;
    assert_eq!(accepted(&spammer), stuck_at, "still stuck, and not growing");

    // The hub reads again, and the handler gets going.
    tokio::spawn(async move { while hub.from_agent.recv().await.is_some() {} });
    sleep(Duration::from_secs(1)).await;
    assert_eq!(
        accepted(&spammer),
        SPAM_LIMIT,
        "every message went through once the hub read again"
    );
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_renewed_certificate_waits_for_an_empty_outbox_but_not_for_ever() {
    use support::scripted_transport::ScriptedTransport;
    let rig = Rig::new();
    let handle = rig.identity_handle().await;
    let transport = ScriptedTransport::new();
    let (_session, _spammer, task) =
        spammer_session(transport.clone(), handle.clone(), SessionConfig::default());

    let mut hub = transport.accept(1).await;
    hub.from_agent.recv().await.unwrap();
    hub.send(ToAgent::Config(domain::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: Vec::new(),
        env_allowlist: Vec::new(),
        tenants: Vec::new(),
    }))
    .await;
    // The hub reads nothing from here on, so the outbox stays full.

    // The joiner renews at half the lifetime, straight from the fake hub, and publishes the new identity.
    let joiner = Arc::new(Joiner::new(
        &settings(),
        ScriptedIdTokens::new(Ok(WI_TOKEN)),
        None,
        rig.transport.clone(),
        Arc::new(MemoryCertStore::new()),
        rig.clock.clone(),
    ));
    let mut published = handle.subscribe();
    let maintain = {
        let (joiner, handle, hub) = (joiner, handle.clone(), rig.server.hub.clone());
        tokio::spawn(async move { joiner.maintain(&handle, &*hub).await })
    };
    // The joiner renews at half the lifetime, which is a little under 12 hours from now (the certificate is backdated by
    // five minutes). Wait for the moment the new identity is published, and time everything from there.
    tokio::time::timeout(Duration::from_secs(13 * 3600), published.changed())
        .await
        .expect("the joiner never renewed")
        .unwrap();
    sleep(Duration::from_secs(10)).await;
    assert_eq!(transport.opened(), 1, "a busy outbox: no reconnect yet");

    sleep(Duration::from_secs(15)).await;
    assert_eq!(
        transport.opened(),
        1,
        "still inside the 30 s the session is willing to wait"
    );
    sleep(Duration::from_secs(10)).await;
    assert_eq!(
        transport.opened(),
        2,
        "after 30 s it reconnects anyway, so that the new certificate is used"
    );
    maintain.abort();
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_path_that_dies_silently_is_found_by_keepalive_and_the_session_connects_again() {
    let running = Running::start().await;
    running.first_connection().await;
    running.connected().await;
    running.server().net.silence_connections();

    // Nothing is delivered and nothing fails. The first keepalive ping goes out after 30 s, and the agent gives the
    // answer 20 s: until then the session has no way to know.
    sleep(Duration::from_secs(25)).await;
    assert_eq!(*running.session.state().borrow(), ConnectionState::Connected);
    sleep(Duration::from_secs(20)).await;
    assert_eq!(
        *running.session.state().borrow(),
        ConnectionState::Connected,
        "45 s: the ping has not gone unanswered for 20 s yet"
    );

    let second = tokio::time::timeout(Duration::from_secs(60), running.server().wait_for_connection(2)).await;
    assert!(
        second.is_ok(),
        "keepalive must have ended the dead connection and the session reconnected"
    );
    running.connected().await;
    assert_eq!(running.session.stats().connections(), 2);
}
