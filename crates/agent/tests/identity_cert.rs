//! The checks on an issued certificate before it is stored (S5): our key, a bounded lifetime, and exactly the
//! identity `spiffe://lanekeeper/swimlane/<our swimlane>`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use agent::identity::cert::{AGENT_SAN_PREFIX, ClientIdentity, MAX_LIFETIME_SECS};
use agent::identity::{CertProblem, KeyMaterial};
use bytes::Bytes;
use domain::{SwimlaneId, Timestamp};
use rcgen::KeyPair;
use support::test_ca::{IssueSpec, T0_MS, TestCa};

const NOW: Timestamp = Timestamp::from_unix_millis(T0_MS);

fn swimlane() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

/// A key, its CA-issued chain for `spec`, and the CA.
fn issued(spec: &IssueSpec) -> (KeyMaterial, Vec<Bytes>) {
    let ca = TestCa::new();
    let key = KeyMaterial::generate().unwrap();
    let chain = ca.issue_for_csr(&key.csr_der().unwrap(), spec);
    (key, chain)
}

fn verify(spec: &IssueSpec) -> Result<ClientIdentity, CertProblem> {
    let (key, chain) = issued(spec);
    ClientIdentity::verify(key, chain, &swimlane(), NOW)
}

fn good() -> IssueSpec {
    IssueSpec::agent("sit1", T0_MS)
}

#[test]
fn the_san_prefix_is_the_s5_form() {
    assert_eq!(AGENT_SAN_PREFIX, "spiffe://lanekeeper/swimlane/");
}

#[test]
fn issued_cert_san_must_be_swimlane_form() {
    // The S5 form is accepted, and the identity reports what the certificate says.
    let identity = verify(&good()).unwrap();
    assert_eq!(identity.swimlane(), &swimlane());
    assert_eq!(identity.chain_der().len(), 2);
    assert_eq!(identity.not_before().unix_millis(), T0_MS - 300_000);
    assert_eq!(identity.not_after().unix_millis(), T0_MS + 24 * 3600 * 1000);

    let wrong_identity = |uri: &str| {
        assert_eq!(
            verify(&good().with_uris(&[uri])).unwrap_err(),
            CertProblem::WrongIdentity,
            "{uri}"
        );
    };
    // The old comment form in proto/agent.proto, which S5 overrules (A5).
    wrong_identity("spiffe://lanekeeper/agent/sit1");
    // Another swimlane's identity.
    wrong_identity("spiffe://lanekeeper/swimlane/sit2");
    // A sentinel can never act as an agent.
    wrong_identity("spiffe://lanekeeper/sentinel/sit1");
    // Prefix and suffix tricks.
    wrong_identity("spiffe://lanekeeper/swimlane/sit1x");
    wrong_identity("spiffe://lanekeeper/swimlane/sit1/extra");
    wrong_identity("spiffe://lanekeeper/swimlane/SIT1");
    wrong_identity("spiffe://lanekeeper/swimlane/");
    wrong_identity("spiffe://lanekeeper/swimlane/sit1 ");
    wrong_identity("spiffe://other.example/swimlane/sit1");
    wrong_identity("spiffe://lanekeeper/swimlane/sit1?x=1");
}

#[test]
fn exactly_one_uri_san_is_required_and_other_san_types_are_ignored() {
    // Two URI SANs are refused even when both are right: the identity must be unambiguous.
    let right = "spiffe://lanekeeper/swimlane/sit1";
    assert_eq!(
        verify(&good().with_uris(&[right, right])).unwrap_err(),
        CertProblem::SeveralUriSans
    );
    assert_eq!(
        verify(&good().with_uris(&[right, "spiffe://lanekeeper/agent/sit1"])).unwrap_err(),
        CertProblem::SeveralUriSans
    );
    // No URI SAN at all, with and without other SAN types.
    assert_eq!(verify(&good().with_uris(&[])).unwrap_err(), CertProblem::NoUriSan);
    assert_eq!(
        verify(&good().with_uris(&[]).with_dns(&["sit1.example"])).unwrap_err(),
        CertProblem::NoUriSan
    );
    // A DNS SAN that a later hub adds does not lock agents out.
    assert!(verify(&good().with_dns(&["agent.lanekeeper.example"])).is_ok());
}

#[test]
fn rejects_issued_cert_with_wrong_key_san_or_lifetime() {
    // Wrong key: a certificate issued for someone else's key.
    let ca = TestCa::new();
    let ours = KeyMaterial::generate().unwrap();
    let other = KeyPair::generate().unwrap();
    let chain = ca.issue_for_key(&other, &good());
    assert_eq!(
        ClientIdentity::verify(ours, chain, &swimlane(), NOW).unwrap_err(),
        CertProblem::WrongKey
    );

    let now_s = T0_MS / 1000;
    // A lifetime over the bound (S5 says 24 hours; the hub may backdate a little).
    assert_eq!(
        verify(&good().valid(now_s - 300, now_s - 300 + MAX_LIFETIME_SECS + 1)).unwrap_err(),
        CertProblem::LifetimeTooLong
    );
    assert_eq!(
        verify(&good().valid(now_s, now_s + 10 * 365 * 86_400)).unwrap_err(),
        CertProblem::LifetimeTooLong
    );
    // Exactly the bound is fine.
    assert!(verify(&good().valid(now_s - 300, now_s - 300 + MAX_LIFETIME_SECS)).is_ok());
    // Already expired, not valid yet, and ends before it begins.
    assert_eq!(
        verify(&good().valid(now_s - 7200, now_s - 3600)).unwrap_err(),
        CertProblem::Expired
    );
    assert_eq!(
        verify(&good().valid(now_s - 7200, now_s)).unwrap_err(),
        CertProblem::Expired,
        "a certificate that ends this very second is expired"
    );
    assert_eq!(
        verify(&good().valid(now_s + 3600, now_s + 7200)).unwrap_err(),
        CertProblem::NotYetValid
    );
    assert!(
        verify(&good().valid(now_s + 100, now_s + 86_400)).is_ok(),
        "a few minutes of clock skew are tolerated"
    );
    assert_eq!(
        verify(&good().valid(now_s + 100, now_s + 100)).unwrap_err(),
        CertProblem::NoLifetime
    );
    assert_eq!(
        verify(&good().valid(now_s + 200, now_s - 200)).unwrap_err(),
        CertProblem::NoLifetime
    );
}

#[test]
fn garbage_and_empty_chains_are_refused() {
    let (key, chain) = issued(&good());
    assert_eq!(
        ClientIdentity::verify(key, Vec::new(), &swimlane(), NOW).unwrap_err(),
        CertProblem::EmptyChain
    );
    let (key, _) = issued(&good());
    assert_eq!(
        ClientIdentity::verify(
            key,
            vec![Bytes::from_static(b"not a certificate")],
            &swimlane(),
            NOW
        )
        .unwrap_err(),
        CertProblem::Unparseable
    );
    // A bad certificate behind a good leaf still fails: the whole chain is stored, so the whole chain is checked.
    let (key, chain2) = issued(&good());
    let mut bad_tail = vec![chain2[0].clone(), Bytes::from_static(b"\x30\x03\x02\x01\x01")];
    assert_eq!(
        ClientIdentity::verify(key, bad_tail.clone(), &swimlane(), NOW).unwrap_err(),
        CertProblem::Unparseable
    );
    // Trailing bytes after a certificate are refused.
    bad_tail.clear();
    let mut padded = chain[0].to_vec();
    padded.extend_from_slice(b"\x00\x00");
    let (key, _) = issued(&good());
    assert_eq!(
        ClientIdentity::verify(key, vec![Bytes::from(padded)], &swimlane(), NOW).unwrap_err(),
        CertProblem::Unparseable
    );
}

#[test]
fn identity_debug_is_redacted() {
    let identity = verify(&good()).unwrap();
    let key_der = identity.key().pkcs8_der().to_vec();
    let key_pem = identity.key().to_pem();
    for shown in [format!("{identity:?}"), format!("{identity:#?}")] {
        assert!(shown.contains("redacted"), "{shown}");
        assert!(shown.contains("sit1"), "the swimlane is not secret: {shown}");
        assert!(
            !shown.contains(key_pem.expose().lines().nth(1).unwrap()),
            "{shown}"
        );
        assert!(!shown.contains(&hex(&key_der[key_der.len() - 40..])), "{shown}");
        assert!(!shown.contains(&format!("{key_der:?}")), "{shown}");
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
