//! T3: the hub's own certificate (S5, S6): made in memory, signed through KMS, renewed at half its life.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::server_identity::{issue_server_identity, renewal_delay, rotate_once};
use hub_platform::tls::RotatingServerCert;
use support::ca::TestCa;
use x509_parser::prelude::{FromDer, X509Certificate};

fn profile() -> CertProfile {
    CertProfile::hub_server(
        &PeerName::parse("hub-0").unwrap(),
        &["hub.lanekeeper.test".to_owned()],
    )
    .unwrap()
}

fn serial_of(cell: &RotatingServerCert) -> String {
    let current = cell.current();
    let (_, cert) = X509Certificate::from_der(current.end_entity_cert().unwrap().as_ref()).unwrap();
    hex::encode(cert.raw_serial())
}

#[tokio::test]
async fn renewal_at_half_lifetime_the_delay_is_half_of_the_life_from_issuance() {
    let ca = TestCa::new().await;
    let (_, issued) = issue_server_identity(&ca.issuer, &profile()).await.unwrap();
    // 24 hours of life from the moment of issuance (the five minutes of back-dating are not life), so 12 hours.
    assert_eq!(renewal_delay(&issued), Duration::from_secs(12 * 3600));
    assert_eq!(
        issued.not_after.unix_millis() - issued.issued_at.unix_millis(),
        24 * 3600 * 1000
    );
}

#[tokio::test]
async fn renewal_at_half_lifetime_swaps_in_a_new_certificate_with_a_new_key() {
    let ca = TestCa::new().await;
    let (first, issued) = issue_server_identity(&ca.issuer, &profile()).await.unwrap();
    let cell = RotatingServerCert::new(first);
    let before = serial_of(&cell);
    let key_before = cell.current().key.public_key().map(|k| k.as_ref().to_vec());

    ca.clock.advance_secs(12 * 3600);
    let next = rotate_once(&ca.issuer, &profile(), &cell).await.unwrap();

    assert_ne!(serial_of(&cell), before, "a new certificate is served");
    assert_ne!(
        cell.current().key.public_key().map(|k| k.as_ref().to_vec()),
        key_before,
        "for a new key: nothing is reused from the first"
    );
    assert_eq!(
        next,
        renewal_delay(&issued),
        "and the next renewal is another half life away"
    );
}

#[tokio::test]
async fn a_failed_renewal_keeps_serving_the_old_certificate() {
    let ca = TestCa::new().await;
    let (first, _) = issue_server_identity(&ca.issuer, &profile()).await.unwrap();
    let cell = RotatingServerCert::new(first);
    let before = serial_of(&cell);
    // Push the clock past the CA certificate's own end of life: issuing now fails.
    ca.clock.advance_secs(11 * 365 * 24 * 3600);
    let error = rotate_once(&ca.issuer, &profile(), &cell).await.unwrap_err();
    assert!(matches!(
        error,
        hub_platform::server_identity::ServerIdentityError::Issue(_)
    ));
    assert_eq!(serial_of(&cell), before);
}
