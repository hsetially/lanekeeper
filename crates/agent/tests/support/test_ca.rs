//! A throwaway certificate authority that stands in for the hub's KMS-backed CA (S5).
//!
//! It issues real X.509 certificates from the agent's real CSR, so the agent's certificate checks run against
//! genuine DER. A test describes the certificate it wants with an [`IssueSpec`]; the helpers make the good one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytes::Bytes;
use rcgen::string::Ia5String;
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, CertifiedIssuer, DistinguishedName,
    DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, PublicKeyData, SanType,
};
use time::OffsetDateTime;

/// The moment tests treat as "now": 2027-01-15T08:00:00Z, in Unix milliseconds. Any fixed instant would do.
pub const T0_MS: i64 = 1_800_000_000_000;

/// What the certificate should contain. Times are Unix seconds.
#[derive(Debug, Clone)]
pub struct IssueSpec {
    pub uris: Vec<String>,
    pub dns: Vec<String>,
    pub not_before: i64,
    pub not_after: i64,
}

impl IssueSpec {
    /// What the hub issues to an agent: the S5 identity, backdated by five minutes, valid for 24 hours from `now_ms`.
    pub fn agent(swimlane: &str, now_ms: i64) -> Self {
        let now = now_ms / 1000;
        Self {
            uris: vec![format!("spiffe://lanekeeper/swimlane/{swimlane}")],
            dns: Vec::new(),
            not_before: now - 300,
            not_after: now + 24 * 3600,
        }
    }

    pub fn with_uris(mut self, uris: &[&str]) -> Self {
        self.uris = uris.iter().map(|s| (*s).to_owned()).collect();
        self
    }

    pub fn with_dns(mut self, dns: &[&str]) -> Self {
        self.dns = dns.iter().map(|s| (*s).to_owned()).collect();
        self
    }

    pub fn valid(mut self, not_before: i64, not_after: i64) -> Self {
        self.not_before = not_before;
        self.not_after = not_after;
        self
    }
}

pub struct TestCa {
    issuer: CertifiedIssuer<'static, KeyPair>,
}

impl TestCa {
    pub fn new() -> Self {
        let mut params = CertificateParams::new(Vec::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, "Lanekeeper test CA");
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let issuer = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        Self { issuer }
    }

    /// The CA certificate, DER.
    pub fn der(&self) -> Bytes {
        Bytes::copy_from_slice(self.issuer.der())
    }

    /// Issue a certificate for the key in a CSR, as the hub does at `Join`. The chain is the leaf, then the CA.
    pub fn issue_for_csr(&self, csr_der: &[u8], spec: &IssueSpec) -> Vec<Bytes> {
        let csr = CertificateSigningRequestParams::from_der(&csr_der.to_vec().into())
            .expect("the agent's CSR must parse and carry a valid signature");
        self.issue_for_key(&csr.public_key, spec)
    }

    /// Issue a certificate for any public key.
    pub fn issue_for_key(&self, key: &impl PublicKeyData, spec: &IssueSpec) -> Vec<Bytes> {
        let mut params = CertificateParams::default();
        params.not_before = OffsetDateTime::from_unix_timestamp(spec.not_before).unwrap();
        params.not_after = OffsetDateTime::from_unix_timestamp(spec.not_after).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, "lanekeeper agent");
        params.subject_alt_names = spec
            .uris
            .iter()
            .map(|u| SanType::URI(Ia5String::try_from(u.clone()).unwrap()))
            .chain(
                spec.dns
                    .iter()
                    .map(|d| SanType::DnsName(Ia5String::try_from(d.clone()).unwrap())),
            )
            .collect();
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let leaf = params.signed_by(key, &self.issuer).unwrap();
        vec![Bytes::copy_from_slice(leaf.der()), self.der()]
    }
}

impl Default for TestCa {
    fn default() -> Self {
        Self::new()
    }
}
