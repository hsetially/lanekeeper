//! A throwaway CA and a server certificate, for the tests that speak real TLS to the fake Google server.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{RootCertStore, ServerConfig};

pub struct TestPki {
    issuer: Issuer<'static, KeyPair>,
    cert_der: CertificateDer<'static>,
    cert_pem: String,
    key_pem: String,
}

impl TestPki {
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let key = KeyPair::generate().unwrap();
        let key_pem = key.serialize_pem();
        let certified = CertifiedIssuer::self_signed(params, key).unwrap();
        Self::from_pem(&certified.pem(), &key_pem)
    }

    /// A CA made earlier (by another process, for example): its certificate and its private key as PEM.
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Self {
        let key = KeyPair::from_pem(key_pem).unwrap();
        let issuer = Issuer::from_ca_cert_pem(cert_pem, key).unwrap();
        let cert_der = CertificateDer::from(pem::parse(cert_pem).unwrap().into_contents());
        Self {
            issuer,
            cert_der,
            cert_pem: cert_pem.to_owned(),
            key_pem: key_pem.to_owned(),
        }
    }

    /// The CA certificate and its private key, as PEM, to hand to a child process.
    pub fn pem(&self) -> (&str, &str) {
        (&self.cert_pem, &self.key_pem)
    }

    pub fn roots(&self) -> RootCertStore {
        let mut store = RootCertStore::empty();
        store.add(self.cert_der.clone()).unwrap();
        store
    }

    /// A TLS 1.3 server configuration valid for `localhost` and the loopback address.
    pub fn loopback_server_config(&self) -> Arc<ServerConfig> {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
        params
            .subject_alt_names
            .push(SanType::IpAddress(std::net::IpAddr::from([127, 0, 0, 1])));
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        let chain = vec![cert.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        Arc::new(
            ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(chain, key)
                .unwrap(),
        )
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
