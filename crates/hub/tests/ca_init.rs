//! `hub ca-init` (plan 18, T3, G15; S5, S6): the CA certificate that the first start needs and nothing else makes.
//!
//! The KMS here is a software key behind the same `KmsSigner` port, so the certificate is signed the way KMS signs it. What is
//! proved: the output is exactly one PEM certificate; it is a CA that may sign leaf certificates and no other CA; it carries the
//! KMS key's public key and the name and life that were asked for; and `CaIssuer::new` (the check the hub makes at every start)
//! accepts it for that key and refuses it for another. The command that reads these arguments is the fuzzed `hub_cli_args`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

#[path = "../examples/support/soft_kms.rs"]
mod soft_kms;

use std::sync::Arc;

use domain::Secret;
use hub::ca_init::{CaInitError, run};
use hub::cli::CaInitArgs;
use hub::web_tls::decode_ca_certificate;
use hub_platform::ca::{CaError, CaIssuer};
use hub_platform::clock::SystemClock;
use ports::{KeyRef, KmsError, KmsSigner};
use soft_kms::SoftKms;
use x509_parser::prelude::{FromDer, X509Certificate};

const KEY: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/agent-ca/cryptoKeyVersions/1";
const OTHER: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/other/cryptoKeyVersions/1";

fn args(key: &str, name: &str, days: u32) -> CaInitArgs {
    CaInitArgs {
        kms_ref: KeyRef::parse(key).unwrap(),
        common_name: name.to_owned(),
        valid_days: days,
    }
}

#[tokio::test]
async fn the_certificate_is_a_ca_for_the_kms_key_with_the_name_and_life_asked_for() {
    let key = KeyRef::parse(KEY).unwrap();
    let kms = Arc::new(SoftKms::new(&[&key]));
    let pem = run(
        &args(KEY, "Lanekeeper agent CA", 3650),
        kms.clone(),
        &*kms,
        &SystemClock,
    )
    .await
    .unwrap();

    // Exactly one certificate, in PEM with LF line endings, and nothing else in the output.
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----\n"), "{pem}");
    assert!(pem.trim_end().ends_with("-----END CERTIFICATE-----"), "{pem}");
    assert!(!pem.contains('\r') && !pem.contains("PRIVATE"), "{pem}");
    let der = decode_ca_certificate(&Secret::new(pem)).unwrap();

    let (rest, cert) = X509Certificate::from_der(&der).unwrap();
    assert!(rest.is_empty());
    // A CA that may sign leaf certificates and no other CA (path length 0), and may do nothing else with its key.
    let constraints = cert.basic_constraints().unwrap().unwrap().value;
    assert!(constraints.ca);
    assert_eq!(constraints.path_len_constraint, Some(0));
    let usage = cert.key_usage().unwrap().unwrap().value;
    assert!(usage.key_cert_sign() && !usage.digital_signature() && !usage.key_encipherment());
    assert!(cert.extended_key_usage().unwrap().is_none());
    // The name and the life asked for.
    assert_eq!(
        cert.subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap(),
        "Lanekeeper agent CA"
    );
    let validity = cert.validity();
    assert_eq!(
        validity.not_after.timestamp() - validity.not_before.timestamp(),
        3650 * 24 * 3600
    );
    // The certificate carries the KMS key's public key, so a signature KMS makes verifies under it.
    let spki = kms.public_key_spki_der(&key).unwrap();
    assert_eq!(cert.public_key().raw, spki.as_slice());

    // What the hub checks at every start: the certificate belongs to the key (a probe signature verifies under it).
    let signer: Arc<dyn KmsSigner> = kms.clone();
    CaIssuer::new(Arc::clone(&signer), key, der.clone())
        .await
        .unwrap();
    // And for another key it does not: the hub would refuse to start on it.
    let other = KeyRef::parse(OTHER).unwrap();
    let both = Arc::new(SoftKms::new(&[&other]));
    let error = CaIssuer::new(both, other, der).await.unwrap_err();
    assert_eq!(error, CaError::KeyMismatch);
}

#[tokio::test]
async fn a_key_kms_does_not_know_is_an_error_that_names_the_step_and_makes_nothing() {
    let known = KeyRef::parse(KEY).unwrap();
    let kms = Arc::new(SoftKms::new(&[&known]));
    let error = run(&args(OTHER, "CA", 30), kms.clone(), &*kms, &SystemClock)
        .await
        .unwrap_err();
    assert_eq!(error, CaInitError::PublicKey(KmsError::KeyNotFound));
    let shown = error.to_string();
    assert!(shown.contains("public key") && !shown.contains(OTHER), "{shown}");
}

#[tokio::test]
async fn a_signer_that_fails_after_the_key_was_read_is_an_error_and_no_certificate() {
    // The public key is readable (it is another key's), the signature is not: the ceremony fails whole.
    let known = KeyRef::parse(KEY).unwrap();
    let reads = Arc::new(SoftKms::new(&[&known]));
    let signs = Arc::new(SoftKms::new(&[]));
    let error = run(&args(KEY, "CA", 30), signs, &*reads, &SystemClock)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            CaInitError::Certificate(CaError::Kms(KmsError::KeyNotFound))
        ),
        "{error:?}"
    );
}

#[test]
fn a_certificate_secret_holds_one_certificate_and_no_private_key() {
    let key = rcgen::KeyPair::generate().unwrap();
    let one = rcgen::CertificateParams::new(vec!["ca.example".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap()
        .pem();
    assert!(decode_ca_certificate(&Secret::new(one.clone())).is_ok());
    // Two certificates are two things to trust; a private key is the wrong secret; nothing is nothing.
    for (text, why) in [
        (format!("{one}{one}"), "two certificates"),
        (
            format!("{one}{}", key.serialize_pem()),
            "a private key beside the certificate",
        ),
        (key.serialize_pem(), "only a private key"),
        (String::new(), "empty"),
        (
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_owned(),
            "garbage inside",
        ),
        ("a".repeat(70 * 1024), "too large"),
    ] {
        assert!(decode_ca_certificate(&Secret::new(text)).is_err(), "{why}");
    }
}

/// Found by the fuzz target `hub_pem` in the FINAL step of plan 18, the first time the targets ran on the pinned nightly. A
/// second `BEGIN CERTIFICATE` that is glued to the text before it on its line is not a PEM section, so the grammar skips it and the
/// decoder used to see one certificate and trust it without a word. It is what joining two PEM files where the first has no final
/// newline produces, and it is a second certificate that somebody meant to be there: refused, as a second section is.
#[test]
fn a_second_certificate_marker_that_the_pem_grammar_skips_is_still_refused() {
    let key = rcgen::KeyPair::generate().unwrap();
    let one = rcgen::CertificateParams::new(vec!["ca.example".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap()
        .pem();
    for (text, why) in [
        (
            format!("{one}not pem att{one}"),
            "a marker glued to text before it",
        ),
        (
            format!("{}{one}", one.trim_end()),
            "two files joined with no newline between",
        ),
        (format!("{one}# {one}"), "a marker after a comment mark"),
    ] {
        assert!(decode_ca_certificate(&Secret::new(text)).is_err(), "{why}");
    }
    // And the one certificate, with text around it that carries no second marker, is still a fine secret.
    assert!(decode_ca_certificate(&Secret::new(format!("subject: ca.example\n{one}\n"))).is_ok());
}
