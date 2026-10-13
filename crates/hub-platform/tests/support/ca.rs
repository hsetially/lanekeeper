//! A CA for the tests: a real P-256 key behind the `KmsSigner` trait, a self-signed CA certificate made by the same
//! ceremony code the hub uses, and the `CaIssuer` under test. The CA's private key never leaves `SoftKmsSigner`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use hub_platform::ca::{CaIssuer, ceremony};
use hub_platform::clock::Clock;
use ports::{KeyRef, KmsSigner};
use rcgen::{CertificateParams, KeyPair};

use super::clock::ManualClock;
use super::soft_kms::SoftKmsSigner;

pub const AGENT_CA_KEY: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/agent-ca/cryptoKeyVersions/1";
pub const REPLICA_CA_KEY: &str =
    "projects/p/locations/l/keyRings/r/cryptoKeys/replica-ca/cryptoKeyVersions/1";

pub struct TestCa {
    pub soft: Arc<SoftKmsSigner>,
    pub key: KeyRef,
    pub ca_der: Vec<u8>,
    pub issuer: CaIssuer,
    pub clock: Arc<ManualClock>,
}

impl TestCa {
    pub fn clock_now_ms(&self) -> i64 {
        self.clock.now().unix_millis()
    }

    pub async fn new() -> Self {
        Self::with_key(AGENT_CA_KEY).await
    }

    pub async fn with_key(name: &str) -> Self {
        let key = KeyRef::parse(name).unwrap();
        let soft = Arc::new(SoftKmsSigner::new(&[&key]));
        Self::over(soft, key).await
    }

    /// A CA over an existing signer (for example a spy around it).
    pub async fn over(soft: Arc<SoftKmsSigner>, key: KeyRef) -> Self {
        let clock = ManualClock::at_t0();
        let signer: Arc<dyn KmsSigner> = soft.clone();
        Self::over_signer(soft, signer, key, clock).await
    }

    pub async fn over_signer(
        soft: Arc<SoftKmsSigner>,
        signer: Arc<dyn KmsSigner>,
        key: KeyRef,
        clock: Arc<ManualClock>,
    ) -> Self {
        let spki = soft.public_key_spki_der(&key).unwrap();
        let ca_der = ceremony::self_signed_ca(
            Arc::clone(&signer),
            key.clone(),
            &spki,
            "Lanekeeper test CA",
            clock.now(),
            Duration::from_secs(10 * 365 * 24 * 3600),
        )
        .await
        .unwrap();
        let issuer = CaIssuer::new(signer, key.clone(), ca_der.clone())
            .await
            .unwrap()
            .with_clock(clock.clone());
        Self {
            soft,
            key,
            ca_der,
            issuer,
            clock,
        }
    }
}

/// A PKCS#10 request for `key`, with no requested names, the way the agent makes it.
pub fn csr_for(key: &KeyPair) -> Vec<u8> {
    let params = CertificateParams::default();
    params.serialize_request(key).unwrap().der().to_vec()
}

pub fn fresh_csr() -> (KeyPair, Vec<u8>) {
    let key = KeyPair::generate().unwrap();
    let der = csr_for(&key);
    (key, der)
}
