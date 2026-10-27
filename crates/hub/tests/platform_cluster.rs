//! The platform wired by the hub's own composition code (plan 18, T3; S5, S6, S7, S9): two replicas on one real Postgres, each built
//! by `Platform::build` exactly as `hub serve` builds it, with the edges (Secret Manager, Cloud KMS, the verifier of Google ID
//! tokens) the only software in them.
//!
//! What is proved here that the platform's own cluster tests do not: that the composition puts the pieces together in a way that
//! works. An agent joins through the agent listener of one replica with a Google ID token, connects, and a command sent through the
//! *other* replica reaches it over the replica listener; each listener trusts only its own CA; a CA certificate that does not
//! belong to the KMS key stops the start; killing one replica's tasks fails its readiness and not the other's; the platform's jobs
//! are held by one replica; a push announced to one replica reaches the other's event bus; and everything the platform spawns
//! stops when the hub closes.
//!
//! The clients are the platform's (`platform_support`): a CA over a software KMS key, a gRPC client over real mutual TLS, and a
//! fake agent that speaks the real protocol. `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

mod support;

#[path = "support/git_remote.rs"]
mod git_remote;
#[path = "support/platform_support.rs"]
mod platform_support;
#[path = "support/secrets.rs"]
mod secrets;
#[path = "support/tls_pg.rs"]
mod tls_pg;
// The rig itself (plan 18, T3; shared with `seed.rs`, T7): a database, two CAs, a Git remote and replicas built by the composition.
#[path = "support/cluster.rs"]
#[macro_use]
mod cluster;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{CommitId, DomainEvent, RepoKind};
use futures::StreamExt;
use git_remote::fake_github::{Delivery, push_body};
use hub::session_auth::DenySessions;
use hub::wiring::jobs::PLATFORM_JOBS;
use hub::wiring::{CaKind, CaStep, PlatformError, prepare_cas};
use hub::{Deps, Hub, HubSettings, SessionEdge};
use hub_platform::ca::CaError;
use hub_platform::ca::profile::PeerName;
use hub_platform::clock::Clock;
use hub_platform::tls::{ClientTls, roots_from_der};
use platform_support::ca::{AGENT_CA_KEY, csr_for};
use platform_support::fake_agent::{Behavior, FakeAgent};
use platform_support::grpc::{ClientIdentity, agent_client, client_tls, dial, dial_named, identity_for};
use ports::{AgentGateway, EventBus, EventFilter, EventKind};
use proto::pb;
use tokio::net::TcpListener;
use tonic::Request;
use tower::{Service, ServiceExt};

use cluster::{
    BASE_REPO, Cluster, SERVICE_ACCOUNT, WEBHOOK_SECRET, eventually, join_request, pem_of, read, swimlane,
};

// ------------------------------------------------------------------------------------------------ S5: join, then a command

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_joins_with_a_google_id_token_for_its_registered_account() {
    let cluster = cluster!(2);
    cluster.seed_swimlane("sit1", Some(SERVICE_ACCOUNT)).await;
    cluster.accept_google("gid-ok", SERVICE_ACCOUNT);
    cluster.accept_google("gid-other", "other@project.iam.gserviceaccount.com");

    // `Join` needs no client certificate: the agent has none yet.
    let tls = client_tls(&cluster.agent_ca, None);
    let mut client = agent_client(dial(cluster.agent_addr(0), &tls).await.unwrap());
    let key = rcgen::KeyPair::generate().unwrap();
    let csr = csr_for(&key);

    // Another account's token is the same refusal as a token nobody knows: Unauthenticated, no message.
    for token in ["gid-other", "gid-unknown"] {
        let status = client
            .join(Request::new(join_request("sit1", token, &csr)))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated, "{token}");
        assert_eq!(status.message(), "");
    }
    assert_eq!(cluster.count("SELECT count(*) FROM agents").await, 0);

    let response = client
        .join(Request::new(join_request("sit1", "gid-ok", &csr)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        response.cert_chain_der.len(),
        2,
        "the agent's certificate and the CA's"
    );
    assert_eq!(cluster.count("SELECT count(*) FROM agents").await, 1);
    // The certificate it got is a certificate of the agent CA, and names the swimlane (S5).
    let leaf = x509_parser::parse_x509_certificate(&response.cert_chain_der[0])
        .unwrap()
        .1;
    let uris: Vec<String> = leaf
        .subject_alternative_name()
        .unwrap()
        .unwrap()
        .value
        .general_names
        .iter()
        .filter_map(|n| match n {
            x509_parser::extensions::GeneralName::URI(u) => Some((*u).to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(uris, vec!["spiffe://lanekeeper/swimlane/sit1".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_sent_through_the_other_replica_reaches_the_agent() {
    let cluster = cluster!(2);
    cluster.seed_swimlane("sit1", Some(SERVICE_ACCOUNT)).await;
    cluster.accept_google("gid-ok", SERVICE_ACCOUNT);

    // Join on replica 0 with a real Google-token credential, then connect to replica 0 with the certificate it issued.
    let tls = client_tls(&cluster.agent_ca, None);
    let mut client = agent_client(dial(cluster.agent_addr(0), &tls).await.unwrap());
    let key = rcgen::KeyPair::generate().unwrap();
    let response = client
        .join(Request::new(join_request("sit1", "gid-ok", &csr_for(&key))))
        .await
        .unwrap()
        .into_inner();
    let agent = FakeAgent {
        swimlane: "sit1".to_owned(),
        identity: ClientIdentity {
            key,
            chain_der: response.cert_chain_der.iter().map(|c| c.to_vec()).collect(),
        },
    };
    let link = agent
        .connect(&cluster.agent_ca, cluster.agent_addr(0), Behavior::echo())
        .await
        .unwrap();

    // The command starts on replica 1, which does not hold the stream: it reads who does from Postgres, forwards over the
    // replica listener with mutual TLS under the replica CA, and the answer comes back the same way.
    let s = swimlane("sit1");
    eventually(
        "replica 1 to know where the agent is",
        Duration::from_secs(10),
        async || cluster.replicas[1].platform.gateway.status(&s).await == domain::AgentStatus::Connected,
    )
    .await;
    let reply = cluster.replicas[1]
        .platform
        .gateway
        .request(&s, read("r-1"), Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        matches!(&reply, domain::AgentReply::Op(op) if op.ok && op.request_id.as_str() == "r-1"),
        "{reply:?}"
    );
    assert_eq!(
        link.received().len(),
        1,
        "the command reached the agent's stream, once"
    );
    // And from the replica that holds the stream.
    cluster.replicas[0]
        .platform
        .gateway
        .request(&s, read("r-2"), Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(link.received().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn joins_are_audited_and_the_chain_verifies() {
    use hub_identity::audit::{ChainVerifier, EcdsaP256Verifier, MemoryBucket};

    let cluster = cluster!(1);
    cluster.seed_swimlane("sit1", Some(SERVICE_ACCOUNT)).await;
    cluster.accept_google("gid-ok", SERVICE_ACCOUNT);
    let tls = client_tls(&cluster.agent_ca, None);
    let mut client = agent_client(dial(cluster.agent_addr(0), &tls).await.unwrap());
    for _ in 0..2 {
        client
            .join(Request::new(join_request(
                "sit1",
                "gid-ok",
                &csr_for(&rcgen::KeyPair::generate().unwrap()),
            )))
            .await
            .unwrap();
    }
    // One audit event per join, written in the join's own transaction by the identity module's log (D80, S9).
    assert_eq!(
        cluster
            .count("SELECT count(*) FROM audit_events WHERE action = 'agent_joined'")
            .await,
        2
    );
    let report = ChainVerifier::new(
        cluster.replicas[0].pool.clone(),
        Arc::new(MemoryBucket::new()),
        Arc::new(EcdsaP256Verifier::new()),
    )
    .verify()
    .await
    .unwrap();
    assert!(report.is_clean(), "{:?}", report.failures);
    assert!(report.events_checked >= 2, "{report:?}");
}

// ------------------------------------------------------------------------------------------------ S6: two CAs

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_listener_trusts_only_its_own_ca() {
    let cluster = cluster!(1);
    let platform = &cluster.replicas[0].platform;
    let clock: Arc<dyn Clock> = cluster.agent_ca.clock.clone();

    // An agent's certificate, from the agent CA, presented to the replica listener: refused at the handshake.
    let agent_identity = identity_for(
        &cluster.agent_ca,
        &hub_platform::ca::CertProfile::agent(&swimlane("sit1")),
    )
    .await;
    let replica_roots = roots_from_der(&[cluster.replica_ca.ca_der.clone()]).unwrap();
    let as_agent = ClientTls::new(
        replica_roots,
        Some(agent_identity.certified()),
        Arc::clone(&clock),
    )
    .unwrap();
    let before = platform.replica_stats.handshakes_failed();
    let reached = dial_named(platform.replica_addr, as_agent.connector(), "localhost").await;
    eventually(
        "the replica listener to refuse it",
        Duration::from_secs(5),
        async || reached.is_err() || platform.replica_stats.handshakes_failed() > before,
    )
    .await;

    // A replica's certificate, from the replica CA, presented to the agent listener: refused the same way.
    let replica_identity = identity_for(
        &cluster.replica_ca,
        &hub_platform::ca::CertProfile::replica(
            &PeerName::parse("hub-9").unwrap(),
            &["localhost".to_owned()],
        )
        .unwrap(),
    )
    .await;
    let agent_roots = roots_from_der(&[cluster.agent_ca.ca_der.clone()]).unwrap();
    let as_replica = ClientTls::new(agent_roots, Some(replica_identity.certified()), clock).unwrap();
    let before = platform.agent_stats.handshakes_failed();
    let reached = dial_named(platform.agent_addr, as_replica.connector(), "hub.lanekeeper.test").await;
    eventually(
        "the agent listener to refuse it",
        Duration::from_secs(5),
        async || reached.is_err() || platform.agent_stats.handshakes_failed() > before,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_grpc_listeners_require_client_certificates() {
    let cluster = cluster!(1);
    let platform = &cluster.replicas[0].platform;

    // The agent listener: `Connect` without a certificate is Unauthenticated. (`Join` is the one call that needs none.)
    let tls = client_tls(&cluster.agent_ca, None);
    let mut client = agent_client(dial(platform.agent_addr, &tls).await.unwrap());
    let (tx, rx) = tokio::sync::mpsc::channel::<pb::AgentMessage>(1);
    drop(tx);
    let refused = client
        .connect(Request::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::Unauthenticated);

    // The replica listener: its one call, `Forward`, from a client that presents nothing.
    let replica_roots = roots_from_der(&[cluster.replica_ca.ca_der.clone()]).unwrap();
    let anonymous = ClientTls::new(replica_roots, None, cluster.agent_ca.clock.clone()).unwrap();
    let (mut h2, _) = dial_named_h2(platform.replica_addr, &anonymous).await;
    let call = http::Request::builder()
        .method("POST")
        .uri("/lanekeeper.replica.v1.Replica/Forward")
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        // One empty message (the 5-byte frame header: not compressed, length 0): the service decodes it before it looks at who sent it.
        .body(tonic::body::Body::new(http_body_util::Full::new(
            Bytes::from_static(&[0u8; 5]),
        )))
        .unwrap();
    let response = h2.ready().await.unwrap().call(call).await.unwrap();
    let status = tonic::Status::from_header_map(response.headers()).expect("a gRPC status");
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status:?}");
}

/// A connection to the replica listener, checking its certificate for `localhost`.
async fn dial_named_h2(
    addr: std::net::SocketAddr,
    tls: &ClientTls,
) -> (platform_support::grpc::H2, platform_support::grpc::Negotiated) {
    dial_named(addr, tls.connector(), "localhost").await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ca_certificate_that_does_not_belong_to_the_kms_key_refuses_to_start() {
    let cluster = Cluster::prepare().await;
    let Some(cluster) = cluster else { return };
    let agent_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let replica_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let settings = Cluster::settings(0, &agent_listener, &replica_listener);

    // The agent CA's secret now holds the replica CA's certificate: a CA, a good one, for the other key.
    cluster
        .secrets
        .put("agent-ca-cert", &pem_of(&cluster.replica_ca.ca_der));
    let error = prepare_cas(&settings, &cluster.edges, Duration::from_secs(30))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        PlatformError::Ca {
            kind: CaKind::Agent,
            step: CaStep::Issuer(CaError::KeyMismatch)
        }
    );
    // The message names the CA and its settings and says what is wrong; it holds no key name, no secret name and no certificate.
    let shown = error.to_string();
    assert!(
        shown.contains("the agent CA is not usable (LK_AGENT_CA_KMS_REF, LK_AGENT_CA_CERT_REF)")
            && shown.contains("does not belong to the KMS key"),
        "{shown}"
    );
    assert!(
        !shown.contains(AGENT_CA_KEY) && !shown.contains("agent-ca-cert"),
        "{shown}"
    );

    // And the secret holding no certificate at all is a different, named failure.
    cluster.secrets.put("agent-ca-cert", "not a certificate");
    let error = prepare_cas(&settings, &cluster.edges, Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            PlatformError::Ca {
                kind: CaKind::Agent,
                step: CaStep::Pem(_)
            }
        ),
        "{error:?}"
    );
}

// ------------------------------------------------------------------------------------------------ readiness and ownership

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readiness_of_one_replica_fails_without_failing_the_other() {
    let cluster = cluster!(2);
    eventually("both replicas to be ready", Duration::from_secs(30), async || {
        cluster.readiness(0).await.iter().all(|(_, ready)| *ready)
            && cluster.readiness(1).await.iter().all(|(_, ready)| *ready)
    })
    .await;
    let names: Vec<_> = cluster.readiness(0).await.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["events", "mirrors", "jobs", "tasks"]);

    // Replica 0's tasks, its listeners among them, are cut off as a crash would cut them. Replica 1 does not notice.
    cluster.replicas[0].platform.tasks.abort_all();
    eventually(
        "replica 0 to report it is not ready",
        Duration::from_secs(5),
        async || {
            cluster
                .readiness(0)
                .await
                .iter()
                .any(|(name, ready)| *name == "tasks" && !*ready)
        },
    )
    .await;
    assert!(
        cluster.readiness(1).await.iter().all(|(_, ready)| *ready),
        "{:?}",
        cluster.readiness(1).await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_platform_jobs_are_held_by_one_replica_only() {
    let mut cluster = cluster!(2);
    for job in PLATFORM_JOBS {
        eventually(
            &format!("{job} to be held"),
            Duration::from_secs(30),
            async || {
                cluster
                    .replicas
                    .iter()
                    .map(|r| r.platform.job_metrics.held(job))
                    .sum::<i64>()
                    >= 1
            },
        )
        .await;
    }
    for job in PLATFORM_JOBS {
        let holders: i64 = cluster
            .replicas
            .iter()
            .map(|r| r.platform.job_metrics.held(job))
            .sum();
        assert_eq!(holders, 1, "{job} is held by {holders} replicas");
    }
    // The runner is started once.
    assert!(cluster.replicas[0].platform.start_jobs().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_task_the_platform_spawns_stops_when_the_hub_closes() {
    let mut cluster = cluster!(1);
    eventually("the mirrors to be filled", Duration::from_secs(30), async || {
        cluster.readiness(0).await.iter().all(|(_, ready)| *ready)
    })
    .await;
    let mut replica = cluster.replicas.pop().unwrap();
    let names = replica.platform.tasks.names();
    for expected in [
        "event-listener",
        "event-relay",
        "git-fetch",
        "git-follow",
        "gateway-liveness",
        "grpc-agent",
        "grpc-replica",
        "renew-agent-certificate",
        "renew-replica-certificate",
    ] {
        assert!(names.contains(&expected), "{expected} is not among {names:?}");
    }
    replica.closing.send_replace(true);
    if let Some(jobs) = replica.jobs.take() {
        jobs.shutdown().await;
    }
    let overran = replica.platform.tasks.finish(Duration::from_secs(15)).await;
    assert!(overran.is_empty(), "tasks that did not stop: {overran:?}");
    // The leases are released: another replica can take the jobs at once.
    for job in PLATFORM_JOBS {
        assert_eq!(replica.platform.job_metrics.held(job), 0, "{job}");
    }
}

// ------------------------------------------------------------------------------------------------ Git: the webhook and the bus

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_announced_on_one_replica_reaches_the_other_replicas_bus() {
    let cluster = cluster!(2);
    eventually("both replicas to be ready", Duration::from_secs(30), async || {
        cluster.readiness(0).await.iter().all(|(_, ready)| *ready)
            && cluster.readiness(1).await.iter().all(|(_, ready)| *ready)
    })
    .await;
    // The webhook is a route of the composed hub: a signed push needs no session and a bad signature is refused.
    let hub = Hub::build(Deps {
        settings: HubSettings::default(),
        session: SessionEdge::without_sessions(Arc::new(DenySessions)),
        mounted: Vec::new(),
        modules: cluster.replicas[0].platform.modules.clone(),
        static_files: None,
        checks: Vec::new(),
    })
    .unwrap();
    let mut events = cluster.replicas[1].platform.bus.subscribe(EventFilter {
        swimlanes: None,
        kinds: [EventKind::GitHeadMoved].into(),
    });

    let before = cluster.remote.head("main").unwrap();
    let after = cluster
        .remote
        .commit("main", &[("config/a.yml", Some(b"a: 2\n"))], "base, changed");
    let send = |delivery: &Delivery| {
        let mut request = delivery.request();
        request
            .extensions_mut()
            .insert(hub::client_ip::PeerAddr(std::net::SocketAddr::from((
                [198, 51, 100, 9],
                40000,
            ))));
        let web = hub.web.clone();
        async move { web.oneshot(request).await.unwrap().status() }
    };
    let forged = Delivery::signed(
        "not-the-secret",
        "push",
        "d-forged",
        push_body(BASE_REPO, "refs/heads/main", &before, &after),
    );
    assert_eq!(send(&forged).await, http::StatusCode::UNAUTHORIZED);
    let signed = Delivery::signed(
        WEBHOOK_SECRET,
        "push",
        "d-1",
        push_body(BASE_REPO, "refs/heads/main", &before, &after),
    );
    assert_eq!(send(&signed).await, http::StatusCode::ACCEPTED);
    // A replayed delivery is acknowledged and not acted on again: the guard is on Postgres, shared by the replicas.
    assert_eq!(send(&signed).await, http::StatusCode::ACCEPTED);
    assert_eq!(cluster.count("SELECT count(*) FROM webhook_deliveries").await, 1);

    // Replica 0 fetched, announced the move on the bus (through Postgres), and replica 1 heard it. The announcement of the head the
    // mirrors started on (`before`) can reach replica 1 after it subscribed, because it travels through Postgres too (T5 saw it in
    // about one run in ten): it is another event, not this push's, so it is skipped and only the move to `after` ends the wait.
    let after = CommitId::parse(&after).unwrap();
    let moved = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match events.next().await {
                Some(DomainEvent::GitHeadMoved { repo, branch, to, .. })
                    if repo == RepoKind::Base && branch.as_str() == "main" && to == after =>
                {
                    return to;
                }
                Some(_) => {}
                None => panic!("the bus closed"),
            }
        }
    })
    .await
    .expect("replica 1 heard GitHeadMoved to the pushed commit within 20 seconds");
    assert_eq!(moved, after);
}
