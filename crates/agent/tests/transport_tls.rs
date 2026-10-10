//! TLS toward the hub (T3, S6): TLS 1.3 only, one pinned CA, a client certificate only when there is an identity.
//!
//! Each test runs a real handshake between the agent's `client_config` and a rustls server, over an in-memory pipe.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io;
use std::path::Path;
use std::sync::Arc;

use agent::clock::Clock;
use agent::identity::{ClientIdentity, KeyMaterial};
use agent::transport::tls::{
    ALPN_H2, HubRoots, MAX_CA_CERTIFICATES, MAX_CA_FILE_BYTES, classify, client_config,
};
use agent::transport::{TlsError, TlsFailure};
use domain::SwimlaneId;
use rustls::ProtocolVersion;
use rustls::pki_types::ServerName;
use support::clock::TestClock;
use support::test_ca::{IssueSpec, T0_MS, TestCa};
use support::tls::{HubVersions, server_config};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const HUB_NAME: &str = "hub.lanekeeper.test";

struct ClientSide {
    version: Option<ProtocolVersion>,
    alpn: Option<Vec<u8>>,
}

struct ServerSide {
    client_cert: Option<Vec<u8>>,
}

struct Outcome {
    client: io::Result<ClientSide>,
    server: io::Result<ServerSide>,
}

/// One handshake over an in-memory pipe.
async fn handshake(
    client: Arc<rustls::ClientConfig>,
    server: Arc<rustls::ServerConfig>,
    name: &str,
) -> Outcome {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let tls = TlsAcceptor::from(server).accept(server_io).await?;
        let client_cert = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .map(|leaf| leaf.to_vec());
        Ok(ServerSide { client_cert })
    });
    let name = ServerName::try_from(name.to_owned()).unwrap();
    let client = TlsConnector::from(client).connect(name, client_io).await;
    let (client, keep_open) = match client {
        Ok(tls) => {
            let info = {
                let (_, connection) = tls.get_ref();
                ClientSide {
                    version: connection.protocol_version(),
                    alpn: connection.alpn_protocol().map(<[u8]>::to_vec),
                }
            };
            (Ok(info), Some(tls))
        }
        Err(error) => (Err(error), None),
    };
    // The client end must outlive the server's accept: the server writes its session tickets after the handshake.
    let server = server.await.unwrap();
    drop(keep_open);
    Outcome { client, server }
}

/// A hub with its own CA, the agent's roots pinned to it, and a clock that matches the agent's certificates.
struct Setup {
    ca: TestCa,
    roots: HubRoots,
    clock: Arc<TestClock>,
}

impl Setup {
    fn new() -> Self {
        let ca = TestCa::new();
        let roots = HubRoots::from_pem(ca.pem().as_bytes()).unwrap();
        Self {
            ca,
            roots,
            clock: Arc::new(TestClock::starting_at(T0_MS)),
        }
    }

    fn hub(&self, versions: HubVersions) -> Arc<rustls::ServerConfig> {
        server_config(&self.ca, &[HUB_NAME], &self.ca, versions, self.clock.clone())
    }

    /// An identity the hub's CA issued, as the join would produce.
    fn identity(&self) -> ClientIdentity {
        let key = KeyMaterial::generate().unwrap();
        let csr = key.csr_der().unwrap();
        let spec = IssueSpec::agent("sit1", T0_MS);
        let chain = self.ca.issue_for_csr(&csr, &spec);
        ClientIdentity::verify(key, chain, &SwimlaneId::parse("sit1").unwrap(), self.clock.now()).unwrap()
    }
}

#[tokio::test]
async fn client_config_is_tls13_only() {
    let setup = Setup::new();
    // A hub that would happily speak TLS 1.2 still ends up on 1.3: the agent offers nothing else.
    let out = handshake(
        client_config(&setup.roots, None).unwrap(),
        setup.hub(HubVersions::Both),
        HUB_NAME,
    )
    .await;
    let client = out.client.unwrap();
    assert_eq!(client.version, Some(ProtocolVersion::TLSv1_3));
    assert_eq!(client.alpn.as_deref(), Some(ALPN_H2), "gRPC needs h2");
    out.server.unwrap();
}

#[tokio::test]
async fn tls12_only_server_is_refused() {
    let setup = Setup::new();
    let out = handshake(
        client_config(&setup.roots, None).unwrap(),
        setup.hub(HubVersions::Tls12Only),
        HUB_NAME,
    )
    .await;
    let error = out.client.err().expect("a TLS 1.2-only hub must be refused");
    assert_eq!(classify(&error), TlsFailure::Version, "{error}");
    assert!(
        out.server.is_err(),
        "the hub must not complete a handshake either"
    );
}

#[tokio::test]
async fn server_cert_from_other_ca_is_refused() {
    let setup = Setup::new();
    let impostor = TestCa::new();
    let hub = server_config(
        &impostor,
        &[HUB_NAME],
        &setup.ca,
        HubVersions::Tls13Only,
        setup.clock.clone(),
    );
    let out = handshake(client_config(&setup.roots, None).unwrap(), hub, HUB_NAME).await;
    let error = out
        .client
        .err()
        .expect("a certificate from another CA must be refused");
    assert_eq!(classify(&error), TlsFailure::Certificate, "{error}");
}

#[tokio::test]
async fn a_certificate_for_another_name_is_refused() {
    let setup = Setup::new();
    // Right CA, wrong host: the pin is on the CA, and the name must still match.
    let hub = server_config(
        &setup.ca,
        &["other.example.com"],
        &setup.ca,
        HubVersions::Tls13Only,
        setup.clock.clone(),
    );
    let out = handshake(client_config(&setup.roots, None).unwrap(), hub, HUB_NAME).await;
    let error = out
        .client
        .err()
        .expect("a certificate for another name must be refused");
    assert_eq!(classify(&error), TlsFailure::Certificate, "{error}");
}

#[tokio::test]
async fn the_client_certificate_is_sent_only_when_there_is_an_identity() {
    let setup = Setup::new();
    let identity = setup.identity();

    let without = handshake(
        client_config(&setup.roots, None).unwrap(),
        setup.hub(HubVersions::Tls13Only),
        HUB_NAME,
    )
    .await;
    without.client.unwrap();
    assert_eq!(
        without.server.unwrap().client_cert,
        None,
        "Join has no client certificate"
    );

    let with = handshake(
        client_config(&setup.roots, Some(&identity)).unwrap(),
        setup.hub(HubVersions::Tls13Only),
        HUB_NAME,
    )
    .await;
    with.client.unwrap();
    let presented = with
        .server
        .unwrap()
        .client_cert
        .expect("Connect presents the certificate");
    assert_eq!(presented.as_slice(), identity.chain_der()[0].as_ref());
}

#[tokio::test]
async fn a_client_certificate_from_a_foreign_ca_is_not_accepted_by_the_hub() {
    // The reverse check, to show the hub-side verifier in these tests really verifies: a certificate the hub's CA did
    // not issue fails the handshake on the hub's side.
    let setup = Setup::new();
    let foreign = TestCa::new();
    let key = KeyMaterial::generate().unwrap();
    let chain = foreign.issue_for_csr(&key.csr_der().unwrap(), &IssueSpec::agent("sit1", T0_MS));
    let identity =
        ClientIdentity::verify(key, chain, &SwimlaneId::parse("sit1").unwrap(), setup.clock.now()).unwrap();
    let out = handshake(
        client_config(&setup.roots, Some(&identity)).unwrap(),
        setup.hub(HubVersions::Tls13Only),
        HUB_NAME,
    )
    .await;
    assert!(out.server.is_err());
}

// ------------------------------------------------------------------ the pinned CA file

#[test]
fn the_pinned_ca_accepts_one_or_more_certificates() {
    let (a, b) = (TestCa::new(), TestCa::new());
    HubRoots::from_pem(a.pem().as_bytes()).unwrap();
    HubRoots::from_pem(format!("{}\n{}", a.pem(), b.pem()).as_bytes()).unwrap();
}

#[test]
fn a_ca_file_with_nothing_usable_in_it_is_refused() {
    let ca = TestCa::new();
    let key_block = pem::encode(&pem::Pem::new("PRIVATE KEY", vec![1, 2, 3]));
    let too_many = (0..=MAX_CA_CERTIFICATES).map(|_| ca.pem()).collect::<String>();
    let cases: [(&str, Vec<u8>, TlsError); 6] = [
        ("empty", Vec::new(), TlsError::CaEmpty),
        ("only text", b"hello".to_vec(), TlsError::CaEmpty),
        (
            "a key, not a certificate",
            key_block.into_bytes(),
            TlsError::CaMalformed,
        ),
        (
            "not a certificate inside",
            pem::encode(&pem::Pem::new("CERTIFICATE", vec![1, 2, 3])).into_bytes(),
            TlsError::CaRejected,
        ),
        (
            "broken base64",
            b"-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n".to_vec(),
            TlsError::CaMalformed,
        ),
        ("too many", too_many.into_bytes(), TlsError::CaTooMany),
    ];
    for (what, bytes, expected) in cases {
        assert_eq!(HubRoots::from_pem(&bytes).err(), Some(expected), "{what}");
    }
}

#[test]
fn the_ca_file_is_read_with_a_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let ca = TestCa::new();
    let path = dir.path().join("hub-ca.pem");
    std::fs::write(&path, ca.pem()).unwrap();
    HubRoots::load(&path).unwrap();

    // A file of any size is read only up to the limit, then refused.
    let big = dir.path().join("big.pem");
    std::fs::write(&big, vec![b'x'; MAX_CA_FILE_BYTES + 1]).unwrap();
    assert_eq!(HubRoots::load(&big).err(), Some(TlsError::CaTooLarge));

    assert_eq!(
        HubRoots::load(&dir.path().join("missing.pem")).err(),
        Some(TlsError::CaUnreadable)
    );
    assert_eq!(
        HubRoots::load(dir.path()).err(),
        Some(TlsError::CaUnreadable),
        "a directory is not a CA"
    );
    assert_eq!(HubRoots::load(Path::new("/")).err(), Some(TlsError::CaUnreadable));
}
