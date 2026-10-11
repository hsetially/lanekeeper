//! A throwaway CA and leaf certificates for TLS tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub struct TestCa {
    issuer: CertifiedIssuer<'static, KeyPair>,
}

pub struct Leaf {
    pub cert: CertificateDer<'static>,
    pub cert_pem: String,
    pub key: PrivateKeyDer<'static>,
    pub key_pem: String,
}

impl TestCa {
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let issuer = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        Self { issuer }
    }

    pub fn cert_der(&self) -> CertificateDer<'static> {
        self.issuer.der().clone()
    }

    pub fn cert_pem(&self) -> String {
        self.issuer.pem()
    }

    /// A server certificate for these names, signed by this CA.
    pub fn leaf(&self, names: &[&str]) -> Leaf {
        let key = KeyPair::generate().unwrap();
        let params =
            CertificateParams::new(names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap();
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        Leaf {
            cert: cert.der().clone(),
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        }
    }
}
