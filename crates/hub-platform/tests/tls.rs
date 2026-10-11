//! T3: the agent listener (S5, S6). TLS 1.3 only, client certificates verified against the agent CA, and the identity a
//! handler sees is the one in the verified certificate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hub_platform::listener::{GrpcLimits, PeerTls};
use hub_platform::tls::{IdentityError, PeerCert, PeerIdentity, roots_from_der};
use proto::pb;
use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, SanType};
use support::ca::TestCa;
use support::grpc::{HUB_NAME, TestServer, agent_client, agent_identity, client_tls, dial, dial_with};
use tokio::net::TcpStream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::service::Routes;
use tonic::{Request, Response, Status, Streaming};

/// An `Agent` service that says who the listener thinks the caller is, and nothing else.
#[derive(Clone)]
struct WhoAmI;

#[async_trait::async_trait]
impl pb::agent_server::Agent for WhoAmI {
    async fn join(&self, _: Request<pb::JoinRequest>) -> Result<Response<pb::JoinResponse>, Status> {
        Err(Status::unauthenticated(""))
    }

    type ConnectStream = ReceiverStream<Result<pb::HubMessage, Status>>;

    async fn connect(
        &self,
        request: Request<Streaming<pb::AgentMessage>>,
    ) -> Result<Response<Self::ConnectStream>, Status> {
        let peer = request.extensions().get::<PeerTls>().cloned();
        let Some(PeerTls(Some(cert))) = peer else {
            return Err(Status::unauthenticated(""));
        };
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        // The swimlane the certificate names, as the sequence number of an acknowledgement: enough to compare.
        let tag = match &cert.identity {
            PeerIdentity::Agent(swimlane) => u64::from(swimlane.as_str().bytes().last().unwrap_or(0)),
            PeerIdentity::Sentinel(_) => 1,
            PeerIdentity::Hub(_) => 2,
        };
        tx.send(Ok(pb::HubMessage {
            kind: Some(pb::hub_message::Kind::Ack(pb::Ack { seq: tag })),
        }))
        .await
        .unwrap();
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

fn routes() -> Routes {
    Routes::new(proto::grpc::agent_server(WhoAmI))
}

async fn connect_as(server: &TestServer, ca: &TestCa, swimlane: Option<&str>) -> Result<u64, Status> {
    let identity = match swimlane {
        Some(s) => Some(agent_identity(ca, s).await),
        None => None,
    };
    let tls = client_tls(ca, identity.as_ref());
    let h2 = dial(server.addr, &tls)
        .await
        .map_err(|e| Status::unavailable(e.to_string()))?;
    let mut client = agent_client(h2);
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(pb::AgentMessage::default()).await.unwrap();
    let mut stream = client
        .connect(Request::new(ReceiverStream::new(rx)))
        .await?
        .into_inner();
    let message = stream.message().await?.expect("one message");
    match message.kind {
        Some(pb::hub_message::Kind::Ack(ack)) => Ok(ack.seq),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn the_handler_sees_the_identity_in_the_verified_certificate() {
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    assert_eq!(
        connect_as(&server, &ca, Some("sit1")).await.unwrap(),
        u64::from(b'1')
    );
    assert_eq!(
        connect_as(&server, &ca, Some("sit7")).await.unwrap(),
        u64::from(b'7')
    );
    server.shutdown().await;
}

#[tokio::test]
async fn a_connection_without_a_certificate_has_no_peer_identity() {
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    // The handshake succeeds (Join needs it to), and the service refuses: no certificate, no identity.
    let status = connect_as(&server, &ca, None).await.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status:?}");
    server.shutdown().await;
}

#[tokio::test]
async fn agent_listener_negotiates_tls13_and_h2_and_does_not_resume() {
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    let identity = agent_identity(&ca, "sit1").await;
    let tls = client_tls(&ca, Some(&identity));
    let (_h2, first) = dial_with(server.addr, tls.connector()).await.unwrap();
    assert!(first.tls13 && first.h2, "{first:?}");
    let (_h2, second) = dial_with(server.addr, tls.connector()).await.unwrap();
    assert!(
        !second.resumed,
        "a certificate is checked again on every connection"
    );
    server.shutdown().await;
}

/// The hub's test clock as rustls' time, so that a client built by hand checks the certificates by the same time the hub issued
/// them at (the certificates are dated in 2027; the machine's clock is not).
#[derive(Debug)]
struct TestTime(Arc<support::clock::ManualClock>);

impl rustls::time_provider::TimeProvider for TestTime {
    fn current_time(&self) -> Option<rustls::pki_types::UnixTime> {
        use hub_platform::clock::Clock;
        let ms = u64::try_from(self.0.now().unix_millis()).unwrap();
        Some(rustls::pki_types::UnixTime::since_unix_epoch(
            Duration::from_millis(ms),
        ))
    }
}

fn hand_built_client(
    ca: &TestCa,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> tokio_rustls::TlsConnector {
    let roots = roots_from_der(&[ca.ca_der.clone()]).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config =
        rustls::ClientConfig::builder_with_details(provider, Arc::new(TestTime(ca.clock.clone())))
            .with_protocol_versions(versions)
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

#[tokio::test]
async fn agent_listener_refuses_tls12() {
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    // The control: the same hand-built client over TLS 1.3 connects, so what fails below is the protocol version.
    let (_h2, control) = dial_with(server.addr, hand_built_client(&ca, &[&rustls::version::TLS13]))
        .await
        .unwrap();
    assert!(control.tls13);
    // A client that offers TLS 1.2 and nothing else.
    let result = dial_with(server.addr, hand_built_client(&ca, &[&rustls::version::TLS12])).await;
    assert!(result.is_err(), "the hub does not speak TLS 1.2");
    // The server's side of the same failure is counted, so an operator sees a peer that cannot connect.
    for _ in 0..50 {
        if server.stats.handshakes_failed() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(server.stats.handshakes_failed(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn forged_expired_and_wrong_swimlane_certs_are_rejected_forged() {
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    // A certificate with the right name from a CA the hub does not trust. Its chain is valid, for someone else's root.
    let other = TestCa::new().await;
    let forged = agent_identity(&other, "sit1").await;
    let tls = client_tls(&ca, Some(&forged));
    let outcome = async {
        let h2 = dial(server.addr, &tls).await?;
        let mut client = agent_client(h2);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(pb::AgentMessage::default()).await.unwrap();
        let mut stream = client
            .connect(Request::new(ReceiverStream::new(rx)))
            .await
            .map_err(std::io::Error::other)?
            .into_inner();
        stream.message().await.map_err(std::io::Error::other)
    }
    .await;
    assert!(
        outcome.is_err(),
        "a certificate from another CA never reaches a handler: {outcome:?}"
    );
    assert!(server.stats.handshakes_failed() >= 1);
    server.shutdown().await;
}

#[tokio::test]
async fn forged_expired_and_wrong_swimlane_certs_are_rejected_expired() {
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    let identity = agent_identity(&ca, "sit1").await;
    // While it is valid, it works.
    assert!(connect_as_identity(&server, &ca, &identity).await.is_ok());
    // 25 hours later, by the hub's clock, the same certificate is expired. (The client shares the clock, so the failure is
    // the server refusing the client certificate.)
    ca.clock.advance_secs(25 * 3600);
    // The hub's own certificate has expired too by now; renew it, as the rotation job would, so that what fails is the
    // client's certificate and not the server's.
    server.reissue_server_cert(&ca).await;
    let outcome = connect_as_identity(&server, &ca, &identity).await;
    assert!(outcome.is_err(), "an expired certificate is refused: {outcome:?}");
    assert!(
        server.stats.handshakes_failed() >= 1,
        "the hub is the one that refused it"
    );
    // A certificate issued now is accepted, so it was the expiry and nothing else.
    let fresh = agent_identity(&ca, "sit1").await;
    assert!(connect_as_identity(&server, &ca, &fresh).await.is_ok());
    server.shutdown().await;
}

async fn connect_as_identity(
    server: &TestServer,
    ca: &TestCa,
    identity: &support::grpc::ClientIdentity,
) -> Result<u64, Box<dyn std::error::Error>> {
    let tls = client_tls(ca, Some(identity));
    let h2 = dial(server.addr, &tls).await?;
    let mut client = agent_client(h2);
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(pb::AgentMessage::default()).await?;
    let mut stream = client
        .connect(Request::new(ReceiverStream::new(rx)))
        .await?
        .into_inner();
    let message = stream.message().await?.ok_or("no message")?;
    match message.kind {
        Some(pb::hub_message::Kind::Ack(ack)) => Ok(ack.seq),
        _ => Err("unexpected message".into()),
    }
}

#[tokio::test]
async fn a_server_only_certificate_is_not_accepted_as_a_client_certificate() {
    use hub_platform::ca::parse_csr;
    use hub_platform::ca::profile::{CertProfile, PeerName};
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    // The hub's own certificate: same CA, a hub identity, but for server authentication only. Presenting it as a client
    // certificate must fail at the handshake, whatever its name says.
    let key = KeyPair::generate().unwrap();
    let csr = parse_csr(&support::ca::csr_for(&key)).unwrap();
    let profile =
        CertProfile::hub_server(&PeerName::parse("hub-9").unwrap(), &["hub-9.test".to_owned()]).unwrap();
    let issued = ca.issuer.issue(&profile, &csr).await.unwrap();
    let identity = support::grpc::ClientIdentity {
        key,
        chain_der: issued.chain_der.iter().map(|c| c.to_vec()).collect(),
    };
    let outcome = connect_as_identity(&server, &ca, &identity).await;
    assert!(outcome.is_err(), "{outcome:?}");
    assert!(server.stats.handshakes_failed() >= 1);
    server.shutdown().await;
}

#[tokio::test]
async fn a_connection_that_never_finishes_its_handshake_is_closed() {
    let ca = TestCa::new().await;
    let limits = GrpcLimits {
        handshake_timeout: Duration::from_millis(150),
        ..GrpcLimits::default()
    };
    let server = TestServer::start_with(&ca, routes(), limits).await;
    let silent = TcpStream::connect(server.addr).await.unwrap();
    for _ in 0..100 {
        if server.stats.handshakes_failed() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(server.stats.handshakes_failed(), 1);
    drop(silent);
    server.shutdown().await;
}

#[tokio::test]
async fn connections_over_the_limit_are_closed_without_an_answer() {
    let ca = TestCa::new().await;
    let limits = GrpcLimits {
        max_connections: 2,
        ..GrpcLimits::default()
    };
    let server = TestServer::start_with(&ca, routes(), limits).await;
    let first = TcpStream::connect(server.addr).await.unwrap();
    let second = TcpStream::connect(server.addr).await.unwrap();
    // Let the listener accept both and take their permits.
    for _ in 0..100 {
        if server.stats.accepted() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let third = TcpStream::connect(server.addr).await.unwrap();
    for _ in 0..100 {
        if server.stats.refused_at_limit.load(Ordering::Relaxed) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(server.stats.refused_at_limit.load(Ordering::Relaxed), 1);
    drop((first, second, third));
    server.shutdown().await;
}

// ---------------------------------------------------------------------------------------- reading the identity

/// A leaf with exactly these names, from a throwaway CA. TLS verification is not involved: `PeerCert::from_leaf` reads a
/// certificate that has already been verified, and what it must refuse is a verified certificate that names no identity.
fn leaf_with(uris: &[&str], dns: &[&str]) -> Vec<u8> {
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().unwrap();
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, "x");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.subject_alt_names = uris
        .iter()
        .map(|u| SanType::URI((*u).try_into().unwrap()))
        .chain(dns.iter().map(|d| SanType::DnsName((*d).try_into().unwrap())))
        .collect();
    let key = KeyPair::generate().unwrap();
    params.signed_by(&key, &ca).unwrap().der().to_vec()
}

#[test]
fn peer_cert_reads_the_three_kinds_of_identity() {
    let agent = PeerCert::from_leaf(&leaf_with(&["spiffe://lanekeeper/swimlane/sit1"], &[])).unwrap();
    assert!(matches!(&agent.identity, PeerIdentity::Agent(s) if s.as_str() == "sit1"));
    let sentinel = PeerCert::from_leaf(&leaf_with(&["spiffe://lanekeeper/sentinel/nfs-vm-1"], &[])).unwrap();
    assert!(matches!(&sentinel.identity, PeerIdentity::Sentinel(n) if n.as_str() == "nfs-vm-1"));
    let hub =
        PeerCert::from_leaf(&leaf_with(&["spiffe://lanekeeper/hub/hub-0"], &["hub-0.hub.svc"])).unwrap();
    assert!(matches!(&hub.identity, PeerIdentity::Hub(n) if n.as_str() == "hub-0"));
}

#[test]
fn peer_cert_refuses_certificates_that_name_no_known_identity() {
    let cases: &[(&[&str], &[&str], IdentityError)] = &[
        (&[], &["hub.test"], IdentityError::NotOneUri),
        (
            &[
                "spiffe://lanekeeper/swimlane/sit1",
                "spiffe://lanekeeper/swimlane/sit2",
            ],
            &[],
            IdentityError::NotOneUri,
        ),
        (
            &["spiffe://lanekeeper/other/x"],
            &[],
            IdentityError::UnknownIdentity,
        ),
        (
            &["https://lanekeeper/swimlane/sit1"],
            &[],
            IdentityError::UnknownIdentity,
        ),
        (
            &["spiffe://lanekeeper/swimlane/SIT1"],
            &[],
            IdentityError::UnknownIdentity,
        ),
        (
            &["spiffe://lanekeeper/swimlane/a/b"],
            &[],
            IdentityError::UnknownIdentity,
        ),
        (
            &["spiffe://lanekeeper/swimlane/"],
            &[],
            IdentityError::UnknownIdentity,
        ),
        (
            &["spiffe://lanekeeper/sentinel/a/b"],
            &[],
            IdentityError::UnknownIdentity,
        ),
        (
            &["spiffe://other-trust-domain/swimlane/sit1"],
            &[],
            IdentityError::UnknownIdentity,
        ),
        (
            &["spiffe://lanekeeper/swimlane/sit1"],
            &["hub.test"],
            IdentityError::UnexpectedNames,
        ),
    ];
    for (uris, dns, want) in cases {
        let error = PeerCert::from_leaf(&leaf_with(uris, dns)).unwrap_err();
        assert_eq!(&error, want, "{uris:?} {dns:?}");
    }
    assert_eq!(
        PeerCert::from_leaf(b"not a certificate").unwrap_err(),
        IdentityError::Unreadable
    );
}

#[test]
fn roots_from_der_refuses_nothing_and_garbage() {
    assert!(
        roots_from_der::<Vec<u8>>(&[]).is_err(),
        "no trust anchors would trust no one, silently"
    );
    assert!(roots_from_der(&[b"garbage".to_vec()]).is_err());
}

#[tokio::test]
async fn a_client_that_dials_another_name_fails_name_verification() {
    let ca = TestCa::new().await;
    let server = TestServer::start(&ca, routes()).await;
    let tls = client_tls(&ca, None);
    let tcp = TcpStream::connect(server.addr).await.unwrap();
    let wrong = rustls::pki_types::ServerName::try_from("not-the-hub.test").unwrap();
    assert!(
        tls.connector().connect(wrong, tcp).await.is_err(),
        "the hub certificate names {HUB_NAME} only"
    );
    server.shutdown().await;
}
