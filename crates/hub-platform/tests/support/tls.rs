//! A throwaway CA and a server certificate, for the tests that speak real TLS to the fake Google server.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{RootCertStore, ServerConfig};

pub struct TestPki {
    issuer: CertifiedIssuer<'static, KeyPair>,
}

impl TestPki {
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let issuer = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        Self { issuer }
    }

    pub fn roots(&self) -> RootCertStore {
        let mut store = RootCertStore::empty();
        store.add(self.issuer.der().clone()).unwrap();
        store
    }

    /// A server configuration for `host`, signed by this CA. `versions` picks the protocol versions it offers.
    pub fn server_config(
        &self,
        host: &str,
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> Arc<ServerConfig> {
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec![host.to_owned()]).unwrap();
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        let chain: Vec<CertificateDer<'static>> = vec![cert.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        Arc::new(config)
    }
}
