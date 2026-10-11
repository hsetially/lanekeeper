//! TLS for the agent listener and for hub peers (S5, S6): TLS 1.3 only, one pinned CA, and the peer's identity read from the
//! verified certificate.
//!
//! The configuration is built here, not by tonic's TLS features, so that three facts are written in one place and cannot be
//! loosened by a feature flag elsewhere in the workspace:
//!
//! - TLS 1.3 and nothing older, on the stack's one crypto provider (aws-lc-rs);
//! - the only trust anchor is the CA we were given: the system roots are never consulted;
//! - a client certificate is *optional* at the handshake, because `Join` happens before an agent has one, and *required* by
//!   every other RPC through [`PeerCert`]. Which service a certificate may call is decided by the SAN prefix, so an agent
//!   certificate cannot call `Sentinel` or the replica `Forward`, and the reverse.
//!
//! Sessions are not resumed: a resumed session would skip the certificate check, and a certificate lives 24 hours.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use domain::{SwimlaneId, Timestamp};
use rcgen::KeyPair;
use rustls::client::ResolvesClientCert;
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::{ClientHello, NoServerSessionStorage, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::time_provider::TimeProvider;
use rustls::{ClientConfig, RootCertStore, ServerConfig, SignatureScheme};
use tokio_rustls::TlsAcceptor;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};
use zeroize::Zeroize;

use crate::ca::profile::{AGENT_SAN_PREFIX, HUB_SAN_PREFIX, PeerName, SENTINEL_SAN_PREFIX};
use crate::clock::Clock;

/// The ALPN name for HTTP/2, which gRPC needs.
pub const ALPN_H2: &[u8] = b"h2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TlsError {
    #[error("a trust anchor is not a usable certificate")]
    Roots,
    #[error("the TLS configuration was refused")]
    Config,
    #[error("the certificate chain or its key is not usable")]
    Key,
}

/// Trust anchors from DER certificates: exactly what was configured and nothing else.
pub fn roots_from_der<T: AsRef<[u8]>>(certificates: &[T]) -> Result<RootCertStore, TlsError> {
    if certificates.is_empty() {
        return Err(TlsError::Roots);
    }
    let mut store = RootCertStore::empty();
    for der in certificates {
        store
            .add(CertificateDer::from(der.as_ref().to_vec()))
            .map_err(|_| TlsError::Roots)?;
    }
    Ok(store)
}

/// The hub's clock as rustls' notion of time, so that certificate expiry is checked against a clock a test can move.
struct ClockTime(Arc<dyn Clock>);

impl fmt::Debug for ClockTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClockTime")
    }
}

impl TimeProvider for ClockTime {
    fn current_time(&self) -> Option<UnixTime> {
        let ms = u64::try_from(self.0.now().unix_millis()).ok()?;
        Some(UnixTime::since_unix_epoch(Duration::from_millis(ms)))
    }
}

/// A certificate chain and its key, ready for the handshake. Built from a key this process generated a moment ago, never
/// from a key loaded from anywhere (`tests/ca_key_use.rs` allows exactly this function to turn a key into DER).
pub fn leaf_certified_key<T: AsRef<[u8]>>(
    key: &KeyPair,
    chain_der: &[T],
) -> Result<Arc<CertifiedKey>, TlsError> {
    let mut der = key.serialize_der();
    let signing = {
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der.as_slice()));
        aws_lc_rs::sign::any_ecdsa_type(&private)
    };
    der.zeroize();
    let signing = signing.map_err(|_| TlsError::Key)?;
    let chain = chain_der
        .iter()
        .map(|c| CertificateDer::from(c.as_ref().to_vec()))
        .collect();
    Ok(Arc::new(CertifiedKey::new(chain, signing)))
}

/// The hub's server certificate, swapped in place when it is renewed. Connections already open keep the certificate they
/// started with; the next handshake gets the new one.
pub struct RotatingServerCert {
    current: RwLock<Arc<CertifiedKey>>,
}

impl fmt::Debug for RotatingServerCert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RotatingServerCert")
    }
}

impl RotatingServerCert {
    #[must_use]
    pub fn new(initial: Arc<CertifiedKey>) -> Arc<Self> {
        Arc::new(Self {
            current: RwLock::new(initial),
        })
    }

    pub fn replace(&self, next: Arc<CertifiedKey>) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = next;
    }

    #[must_use]
    pub fn current(&self) -> Arc<CertifiedKey> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }
}

impl ResolvesServerCert for RotatingServerCert {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}

/// The server side of mutual TLS: our certificate, and client certificates from one CA, optional at the handshake.
#[derive(Clone)]
pub struct ServerTls {
    config: Arc<ServerConfig>,
}

impl fmt::Debug for ServerTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServerTls")
    }
}

impl ServerTls {
    /// A listener for peers whose certificates `client_roots` issued. `clock` decides whether a certificate has expired.
    pub fn new(
        client_roots: RootCertStore,
        server_cert: Arc<RotatingServerCert>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, TlsError> {
        let provider = Arc::new(aws_lc_rs::default_provider());
        let verifier =
            WebPkiClientVerifier::builder_with_provider(Arc::new(client_roots), Arc::clone(&provider))
                .allow_unauthenticated()
                .build()
                .map_err(|_| TlsError::Roots)?;
        let mut config = ServerConfig::builder_with_details(provider, Arc::new(ClockTime(clock)))
            // TLS 1.3 and nothing older (S6). Not the library's "safe defaults", which also allow 1.2.
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| TlsError::Config)?
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(server_cert);
        config.alpn_protocols = vec![ALPN_H2.to_vec()];
        // No tickets, no session cache: every connection proves its certificate again.
        config.session_storage = Arc::new(NoServerSessionStorage {});
        config.send_tls13_tickets = 0;
        Ok(Self {
            config: Arc::new(config),
        })
    }

    #[must_use]
    pub fn acceptor(&self) -> TlsAcceptor {
        TlsAcceptor::from(Arc::clone(&self.config))
    }

    #[must_use]
    pub fn config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.config)
    }
}

/// The client side of TLS toward the hub or a replica: TLS 1.3 only, one pinned CA, and a client certificate when there is
/// one (not for `Join`).
#[derive(Clone)]
pub struct ClientTls {
    config: Arc<ClientConfig>,
}

impl fmt::Debug for ClientTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientTls")
    }
}

#[derive(Debug)]
struct PresentedIdentity(Arc<CertifiedKey>);

impl ResolvesClientCert for PresentedIdentity {
    fn resolve(&self, _hints: &[&[u8]], _schemes: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// A client certificate that is the current one of a [`RotatingServerCert`]: a replica presents the certificate it serves with,
/// and when that is renewed the next handshake presents the new one.
#[derive(Debug)]
struct RotatingIdentity(Arc<RotatingServerCert>);

impl ResolvesClientCert for RotatingIdentity {
    fn resolve(&self, _hints: &[&[u8]], _schemes: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
        Some(self.0.current())
    }

    fn has_certs(&self) -> bool {
        true
    }
}

impl ClientTls {
    pub fn new(
        roots: RootCertStore,
        identity: Option<Arc<CertifiedKey>>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, TlsError> {
        match identity {
            Some(identity) => Self::build(roots, Some(Arc::new(PresentedIdentity(identity))), clock),
            None => Self::build(roots, None, clock),
        }
    }

    /// Present whatever certificate `identity` holds at the time of each handshake (a replica toward its peers, T4).
    pub fn new_rotating(
        roots: RootCertStore,
        identity: Arc<RotatingServerCert>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, TlsError> {
        Self::build(roots, Some(Arc::new(RotatingIdentity(identity))), clock)
    }

    fn build(
        roots: RootCertStore,
        identity: Option<Arc<dyn ResolvesClientCert>>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, TlsError> {
        let provider = Arc::new(aws_lc_rs::default_provider());
        let builder = ClientConfig::builder_with_details(provider, Arc::new(ClockTime(clock)))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| TlsError::Config)?
            .with_root_certificates(roots);
        let mut config = match identity {
            Some(identity) => builder.with_client_cert_resolver(identity),
            None => builder.with_no_client_auth(),
        };
        config.alpn_protocols = vec![ALPN_H2.to_vec()];
        config.resumption = rustls::client::Resumption::disabled();
        Ok(Self {
            config: Arc::new(config),
        })
    }

    #[must_use]
    pub fn connector(&self) -> tokio_rustls::TlsConnector {
        tokio_rustls::TlsConnector::from(Arc::clone(&self.config))
    }
}

// ------------------------------------------------------------------------------------------------ peer identity

/// Who a verified client certificate says its holder is. The kind decides which service it may call.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PeerIdentity {
    /// `spiffe://lanekeeper/swimlane/<id>`: an in-cluster agent, for that swimlane.
    Agent(SwimlaneId),
    /// `spiffe://lanekeeper/sentinel/<name>`: the sentinel daemon of an NFS VM.
    Sentinel(PeerName),
    /// `spiffe://lanekeeper/hub/<replica>`: another hub replica.
    Hub(PeerName),
}

/// What the hub knows about the certificate a peer presented. Inserted into the request extensions by the listener, after the
/// TLS layer verified the chain, so a handler reads it and never a header or a message field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCert {
    pub identity: PeerIdentity,
    /// Lower-case hex, as stored in `agents.cert_serial`.
    pub serial: String,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

/// Why a certificate that passed TLS verification does not name an identity the hub knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("the certificate could not be read")]
    Unreadable,
    #[error("the certificate has no URI name, or more than one")]
    NotOneUri,
    #[error("the certificate's URI is not a Lanekeeper identity")]
    UnknownIdentity,
    #[error("the certificate has names that an identity certificate of its kind does not carry")]
    UnexpectedNames,
}

impl PeerCert {
    /// Read the identity from the leaf of a chain whose signature, validity and use TLS has already checked.
    pub fn from_leaf(leaf_der: &[u8]) -> Result<Self, IdentityError> {
        let (rest, cert) = X509Certificate::from_der(leaf_der).map_err(|_| IdentityError::Unreadable)?;
        if !rest.is_empty() {
            return Err(IdentityError::Unreadable);
        }
        let mut uris = Vec::new();
        let mut others = 0usize;
        if let Some(san) = cert
            .subject_alternative_name()
            .map_err(|_| IdentityError::Unreadable)?
        {
            for name in &san.value.general_names {
                match name {
                    GeneralName::URI(uri) => uris.push(*uri),
                    _ => others += 1,
                }
            }
        }
        let [uri] = uris.as_slice() else {
            return Err(IdentityError::NotOneUri);
        };
        let identity = if let Some(id) = uri.strip_prefix(AGENT_SAN_PREFIX) {
            PeerIdentity::Agent(SwimlaneId::parse(id).map_err(|_| IdentityError::UnknownIdentity)?)
        } else if let Some(name) = uri.strip_prefix(SENTINEL_SAN_PREFIX) {
            PeerIdentity::Sentinel(PeerName::parse(name).map_err(|_| IdentityError::UnknownIdentity)?)
        } else if let Some(name) = uri.strip_prefix(HUB_SAN_PREFIX) {
            PeerIdentity::Hub(PeerName::parse(name).map_err(|_| IdentityError::UnknownIdentity)?)
        } else {
            return Err(IdentityError::UnknownIdentity);
        };
        // An agent or sentinel certificate is the URI and nothing else; only hub certificates carry DNS names.
        if others > 0 && !matches!(identity, PeerIdentity::Hub(_)) {
            return Err(IdentityError::UnexpectedNames);
        }
        let validity = cert.validity();
        Ok(Self {
            identity,
            serial: hex::encode(cert.raw_serial()),
            not_before: Timestamp::from_unix_millis(validity.not_before.timestamp().saturating_mul(1000)),
            not_after: Timestamp::from_unix_millis(validity.not_after.timestamp().saturating_mul(1000)),
        })
    }

    /// The identity of the peer on the other end of a server-side connection, if it presented a certificate.
    pub fn from_connection(connection: &rustls::ServerConnection) -> Option<Result<Self, IdentityError>> {
        let leaf = connection.peer_certificates()?.first()?;
        Some(Self::from_leaf(leaf.as_ref()))
    }
}
