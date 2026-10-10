//! The hub's side of TLS for the transport tests: a server configuration that trusts the test CA for client
//! certificates, with the clock under the test's control.
//!
//! The agent's certificates are minted around the test clock, which starts in 2027, while rustls checks validity
//! against the time it is given. The hub side therefore gets the test clock as its time source. The agent side is
//! the production configuration and uses the real clock; the hub certificate is valid for centuries.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use agent::clock::Clock;
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::time_provider::TimeProvider;
use rustls::version::{TLS12, TLS13};
use rustls::{RootCertStore, ServerConfig, SupportedProtocolVersion};

use super::test_ca::TestCa;

/// Which protocol versions the fake hub offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubVersions {
    /// TLS 1.2 and 1.3, like a hub that has not been hardened.
    Both,
    Tls13Only,
    Tls12Only,
}

impl HubVersions {
    fn list(self) -> Vec<&'static SupportedProtocolVersion> {
        match self {
            Self::Both => vec![&TLS13, &TLS12],
            Self::Tls13Only => vec![&TLS13],
            Self::Tls12Only => vec![&TLS12],
        }
    }
}

/// The test clock as rustls' notion of time.
#[derive(Debug)]
struct ClockTime(Arc<dyn Clock>);

impl TimeProvider for ClockTime {
    fn current_time(&self) -> Option<UnixTime> {
        let ms = u64::try_from(self.0.now().unix_millis()).unwrap();
        Some(UnixTime::since_unix_epoch(Duration::from_millis(ms)))
    }
}

/// A hub that presents a certificate for `names` from `server_ca`, and accepts (but does not require) client
/// certificates from `client_ca`. `Join` has no client certificate, so the TLS layer cannot require one; the service
/// does, for `Connect`.
pub fn server_config(
    server_ca: &TestCa,
    names: &[&str],
    client_ca: &TestCa,
    versions: HubVersions,
    clock: Arc<dyn Clock>,
) -> Arc<ServerConfig> {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let mut client_roots = RootCertStore::empty();
    client_roots
        .add(CertificateDer::from(client_ca.der().to_vec()))
        .unwrap();
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(client_roots), Arc::clone(&provider))
        .allow_unauthenticated()
        .build()
        .unwrap();
    let identity = server_ca.issue_server(names);
    let mut config = ServerConfig::builder_with_details(provider, Arc::new(ClockTime(clock)))
        .with_protocol_versions(&versions.list())
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(identity.chain, identity.key)
        .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}
