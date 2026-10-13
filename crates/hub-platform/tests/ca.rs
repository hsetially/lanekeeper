//! T3: the agent CA (S5, S6). Certificates are built by rcgen and signed through `KmsSigner`; the CSR is proof of
//! possession and nothing else.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use domain::SwimlaneId;
use hub_platform::ca::csr::{CsrError, MAX_CSR_BYTES, parse_csr};
use hub_platform::ca::profile::{CERT_LIFETIME, CertProfile, PeerName, ProfileError};
use hub_platform::ca::{CaError, CaIssuer};
use hub_platform::clock::Clock;
use ports::{KeyRef, KmsError, KmsSigner, Signature};
use proptest::prelude::*;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    PKCS_ECDSA_P384_SHA384, PKCS_ED25519, SanType,
};
use support::ca::{TestCa, csr_for, fresh_csr};
use support::clock::T0_MS;
use support::soft_kms::SoftKmsSigner;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

fn swimlane(id: &str) -> SwimlaneId {
    SwimlaneId::parse(id).unwrap()
}

fn parse(der: &[u8]) -> X509Certificate<'_> {
    let (rest, cert) = X509Certificate::from_der(der).unwrap();
    assert!(rest.is_empty());
    cert
}

/// The decoded fields a reviewer cares about, as text: what the certificate says, not its bytes.
fn describe(der: &[u8]) -> serde_json::Value {
    let cert = parse(der);
    let san: Vec<String> = cert
        .subject_alternative_name()
        .unwrap()
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .map(|name| match name {
                    GeneralName::URI(uri) => format!("uri:{uri}"),
                    GeneralName::DNSName(dns) => format!("dns:{dns}"),
                    other => format!("other:{other:?}"),
                })
                .collect()
        })
        .unwrap_or_default();
    let eku = cert.extended_key_usage().unwrap().map(|ext| {
        let eku = ext.value;
        serde_json::json!({
            "client_auth": eku.client_auth,
            "server_auth": eku.server_auth,
            "any": eku.any,
            "other": eku.other.len(),
        })
    });
    let basic = cert
        .basic_constraints()
        .unwrap()
        .map(|ext| serde_json::json!({"ca": ext.value.ca, "critical": ext.critical}));
    let usage = cert.key_usage().unwrap().map(|ext| {
        serde_json::json!({
            "digital_signature": ext.value.digital_signature(),
            "key_cert_sign": ext.value.key_cert_sign(),
            "crl_sign": ext.value.crl_sign(),
        })
    });
    serde_json::json!({
        "version": cert.version().0,
        "subject": cert.subject().to_string(),
        "san": san,
        "not_before_unix": cert.validity().not_before.timestamp(),
        "not_after_unix": cert.validity().not_after.timestamp(),
        "lifetime_secs": cert.validity().not_after.timestamp() - cert.validity().not_before.timestamp(),
        "eku": eku,
        "basic_constraints": basic,
        "key_usage": usage,
        "signature_algorithm": cert.signature_algorithm.algorithm.to_id_string(),
        "serial_bytes": cert.raw_serial().len(),
    })
}

#[tokio::test]
async fn an_agent_certificate_has_the_swimlane_san_and_nothing_else() {
    let ca = TestCa::new().await;
    let (_key, csr_der) = fresh_csr();
    let csr = parse_csr(&csr_der).unwrap();
    let issued = ca
        .issuer
        .issue(&CertProfile::agent(&swimlane("sit1")), &csr)
        .await
        .unwrap();

    assert_eq!(issued.chain_der.len(), 2, "leaf, then the CA");
    assert_eq!(issued.chain_der[1].as_ref(), ca.ca_der.as_slice());
    insta::assert_json_snapshot!("agent_certificate_profile", describe(&issued.chain_der[0]));

    let now = ca.clock.now().unix_millis();
    assert_eq!(now, T0_MS);
    assert_eq!(issued.not_after.unix_millis(), now + 24 * 3600 * 1000);
    assert_eq!(issued.not_before.unix_millis(), now - 5 * 60 * 1000);
    assert_eq!(CERT_LIFETIME, Duration::from_secs(24 * 3600));
}

#[tokio::test]
async fn the_issued_certificate_verifies_under_the_ca_key_and_carries_the_csr_key() {
    let ca = TestCa::new().await;
    let (key, csr_der) = fresh_csr();
    let issued = ca
        .issuer
        .issue(
            &CertProfile::agent(&swimlane("sit1")),
            &parse_csr(&csr_der).unwrap(),
        )
        .await
        .unwrap();
    let leaf = parse(&issued.chain_der[0]);
    let ca_cert = parse(&ca.ca_der);
    leaf.verify_signature(Some(ca_cert.public_key()))
        .expect("signed by the KMS key");
    assert_eq!(leaf.issuer(), ca_cert.subject());
    assert_eq!(
        leaf.public_key().subject_public_key.data.as_ref(),
        rcgen::PublicKeyData::der_bytes(&key),
        "the certificate is for the requester's key"
    );
}

#[tokio::test]
async fn a_sentinel_certificate_is_a_client_certificate_for_its_name_only() {
    let ca = TestCa::new().await;
    let (_key, csr_der) = fresh_csr();
    let profile = CertProfile::sentinel(&PeerName::parse("nfs-vm-1.internal").unwrap());
    let issued = ca
        .issuer
        .issue(&profile, &parse_csr(&csr_der).unwrap())
        .await
        .unwrap();
    insta::assert_json_snapshot!("sentinel_certificate_profile", describe(&issued.chain_der[0]));
}

#[tokio::test]
async fn a_hub_server_certificate_is_server_auth_with_its_dns_names() {
    let ca = TestCa::new().await;
    let (_key, csr_der) = fresh_csr();
    let profile = CertProfile::hub_server(
        &PeerName::parse("hub-0").unwrap(),
        &["hub.lanekeeper.test".to_owned()],
    )
    .unwrap();
    let issued = ca
        .issuer
        .issue(&profile, &parse_csr(&csr_der).unwrap())
        .await
        .unwrap();
    insta::assert_json_snapshot!("hub_server_certificate_profile", describe(&issued.chain_der[0]));
}

#[tokio::test]
async fn a_replica_certificate_is_both_client_and_server_auth() {
    let ca = TestCa::new().await;
    let (_key, csr_der) = fresh_csr();
    let profile = CertProfile::replica(
        &PeerName::parse("hub-1").unwrap(),
        &["hub-1.hub.lanekeeper.svc".to_owned()],
    )
    .unwrap();
    let issued = ca
        .issuer
        .issue(&profile, &parse_csr(&csr_der).unwrap())
        .await
        .unwrap();
    insta::assert_json_snapshot!("replica_certificate_profile", describe(&issued.chain_der[0]));
}

#[test]
fn peer_names_cannot_smuggle_a_path_into_a_san() {
    for bad in [
        "",
        "a/b",
        "x/../swimlane/sit1",
        "has space",
        "-leading-dash",
        "tab\tname",
        "ünicode",
        "semi;colon",
        "query?x=1",
        "frag#ment",
        "at@sign",
        "colon:name",
    ] {
        assert!(
            PeerName::parse(bad).is_err(),
            "{bad:?} must not become part of a URI"
        );
    }
    assert!(PeerName::parse(&"a".repeat(254)).is_err());
    assert!(PeerName::parse(&"a".repeat(253)).is_ok());
    assert!(PeerName::parse("nfs-vm-1.example.internal").is_ok());
}

#[test]
fn dns_names_in_a_server_profile_are_checked_too() {
    let name = PeerName::parse("hub-0").unwrap();
    assert_eq!(
        CertProfile::hub_server(&name, &[]).unwrap_err(),
        ProfileError::NoDnsName,
        "a server certificate with no name matches nothing"
    );
    for bad in ["", "*.evil.test", "with space", "a..b", "UPPER.test x"] {
        assert_eq!(
            CertProfile::hub_server(&name, &[bad.to_owned()]).unwrap_err(),
            ProfileError::BadDnsName,
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn csr_san_and_subject_are_ignored() {
    let ca = TestCa::new().await;
    // A request that asks for everything: someone else's swimlane, a server name, a CA, a CN of its own.
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, "evil");
    params.subject_alt_names = vec![
        SanType::URI("spiffe://lanekeeper/swimlane/other".try_into().unwrap()),
        SanType::DnsName("hub.lanekeeper.test".try_into().unwrap()),
    ];
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let csr_der = params.serialize_request(&key).unwrap().der().to_vec();

    let csr = parse_csr(&csr_der).expect("the request is well formed; what it asks for is not read");
    let issued = ca
        .issuer
        .issue(&CertProfile::agent(&swimlane("sit1")), &csr)
        .await
        .unwrap();
    let described = describe(&issued.chain_der[0]);
    assert_eq!(
        described["san"],
        serde_json::json!(["uri:spiffe://lanekeeper/swimlane/sit1"])
    );
    assert!(
        !described["subject"].as_str().unwrap().contains("evil"),
        "{described}"
    );
    assert_eq!(described["basic_constraints"]["ca"], false);
    assert_eq!(described["eku"]["client_auth"], true);
    assert_eq!(described["eku"]["server_auth"], false);
}

#[test]
fn a_csr_with_a_damaged_self_signature_is_refused() {
    let (_key, mut der) = fresh_csr();
    // The signature is the last thing in the DER; flip a bit well inside it.
    let at = der.len() - 8;
    der[at] ^= 0x01;
    assert!(matches!(
        parse_csr(&der).unwrap_err(),
        CsrError::BadSignature | CsrError::Malformed
    ));
}

#[test]
fn a_csr_signed_by_another_key_is_refused() {
    // Proof of possession: the request names key A but was signed with key B. Splice A's key into B's request.
    let (key_a, csr_a) = fresh_csr();
    let (key_b, csr_b) = fresh_csr();
    let point_a = rcgen::PublicKeyData::der_bytes(&key_a).to_vec();
    let point_b = {
        let key = KeyPair::from_der_and_sign_algo(
            &rustls::pki_types::PrivateKeyDer::try_from(key_b.serialize_der()).unwrap(),
            &rcgen::PKCS_ECDSA_P256_SHA256,
        )
        .unwrap();
        rcgen::PublicKeyData::der_bytes(&key).to_vec()
    };
    let mut spliced = csr_b.clone();
    let at = spliced
        .windows(point_b.len())
        .position(|w| w == point_b.as_slice())
        .expect("the point is in the request");
    spliced[at..at + point_b.len()].copy_from_slice(&point_a);
    assert_ne!(spliced, csr_a);
    assert!(parse_csr(&spliced).is_err());
}

#[test]
fn only_p256_keys_are_accepted() {
    for alg in [&PKCS_ECDSA_P384_SHA384, &PKCS_ED25519] {
        let key = KeyPair::generate_for(alg).unwrap();
        let der = csr_for(&key);
        assert_eq!(parse_csr(&der).unwrap_err(), CsrError::UnsupportedKey, "{alg:?}");
    }
}

#[test]
fn a_csr_over_the_size_cap_or_with_trailing_bytes_is_refused() {
    let (_key, der) = fresh_csr();
    assert!(der.len() < MAX_CSR_BYTES);
    let mut padded = der.clone();
    padded.extend_from_slice(&[0; 4]);
    assert!(parse_csr(&padded).is_err(), "bytes after the request");
    let big = vec![0x30; MAX_CSR_BYTES + 1];
    assert_eq!(parse_csr(&big).unwrap_err(), CsrError::TooLarge);
    assert_eq!(parse_csr(&[]).unwrap_err(), CsrError::Malformed);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn parse_csr_never_panics_and_only_accepts_p256_with_a_valid_signature(
        bytes in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        if let Ok(csr) = parse_csr(&bytes) {
            prop_assert_eq!(csr.public_key_point().len(), 65);
        }
    }

    #[test]
    fn a_mutated_valid_csr_never_panics_and_never_changes_the_key_silently(
        index in 0usize..200,
        value in any::<u8>(),
    ) {
        let (_key, der) = fresh_csr();
        let original = parse_csr(&der).unwrap();
        let mut mutated = der.clone();
        let at = index % mutated.len();
        mutated[at] = value;
        if let Ok(csr) = parse_csr(&mutated) {
            // Accepted means the signature checked out; then the key is the original key or the request is a new,
            // internally consistent one (which cannot happen by flipping a byte).
            prop_assert_eq!(csr.public_key_point(), original.public_key_point());
        }
    }
}

#[tokio::test]
async fn two_certificates_for_the_same_key_have_different_serials() {
    // rcgen derives a default serial from the public key, so a renewal for the same key would repeat it.
    let ca = TestCa::new().await;
    let (_key, csr_der) = fresh_csr();
    let csr = parse_csr(&csr_der).unwrap();
    let profile = CertProfile::agent(&swimlane("sit1"));
    let first = ca.issuer.issue(&profile, &csr).await.unwrap();
    let second = ca.issuer.issue(&profile, &csr).await.unwrap();
    assert_ne!(first.serial, second.serial);
    let leaf = parse(&first.chain_der[0]);
    assert_eq!(hex::encode(leaf.raw_serial()), first.serial);
    assert!(
        leaf.raw_serial().len() <= 20 && leaf.raw_serial()[0] & 0x80 == 0,
        "RFC 5280: positive, at most 20 bytes"
    );
}

#[tokio::test]
async fn a_ca_certificate_that_does_not_match_the_kms_key_is_refused_at_start_up() {
    let a = KeyRef::parse(support::ca::AGENT_CA_KEY).unwrap();
    let b = KeyRef::parse(support::ca::REPLICA_CA_KEY).unwrap();
    let soft = Arc::new(SoftKmsSigner::new(&[&a, &b]));
    let ca_for_a = TestCa::over(soft.clone(), a.clone()).await;
    // The certificate of key A, with KMS asked to sign with key B.
    let signer: Arc<dyn KmsSigner> = soft;
    let error = CaIssuer::new(signer, b, ca_for_a.ca_der.clone())
        .await
        .unwrap_err();
    assert_eq!(error, CaError::KeyMismatch);
}

#[tokio::test]
async fn a_ca_certificate_that_is_not_a_certificate_is_refused() {
    let a = KeyRef::parse(support::ca::AGENT_CA_KEY).unwrap();
    let soft: Arc<dyn KmsSigner> = Arc::new(SoftKmsSigner::new(&[&a]));
    assert_eq!(
        CaIssuer::new(soft, a, b"not a certificate".to_vec())
            .await
            .unwrap_err(),
        CaError::BadCaCertificate
    );
}

/// A signer that fails, to show that no certificate comes out of a failed signature.
#[derive(Debug)]
struct FailingSigner(KmsError);

#[async_trait::async_trait]
impl KmsSigner for FailingSigner {
    async fn sign_digest(&self, _key: KeyRef, _digest: [u8; 32]) -> Result<Signature, KmsError> {
        Err(self.0)
    }
}

#[tokio::test]
async fn a_kms_failure_means_no_certificate() {
    let ca = TestCa::new().await;
    // The issuer that signed the CA works; one over a failing signer starts only when the probe is skipped, so build it
    // from the working one and then swap the signer for the issuing path.
    let broken = CaIssuer::new_unchecked(
        Arc::new(FailingSigner(KmsError::Unavailable)),
        ca.key.clone(),
        ca.ca_der.clone(),
    )
    .unwrap();
    let (_key, csr_der) = fresh_csr();
    let error = broken
        .issue(
            &CertProfile::agent(&swimlane("sit1")),
            &parse_csr(&csr_der).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error, CaError::Kms(KmsError::Unavailable));
}
