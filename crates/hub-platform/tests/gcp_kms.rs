//! T2: Cloud KMS signing and envelope encryption (S5, S6, S7).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use bytes::Bytes;
use domain::Secret;
use hub_platform::gcp::http::HttpLimits;
use hub_platform::gcp::{CallLimits, RetryPolicy};
use ports::{KeyRef, KmsEnvelope, KmsError, KmsSigner, WrappedKey};
use support::fake_google::{
    ENVELOPE_KEY, Fault, Harness, OTHER_ENVELOPE_KEY, Route, SIGN_KEY, UNKNOWN_SIGN_KEY,
};
use support::log_capture::LogCapture;

fn key(text: &str) -> KeyRef {
    KeyRef::parse(text).unwrap()
}

#[tokio::test(start_paused = true)]
async fn kms_signer_signature_verifies_against_the_key_and_the_request_carries_the_digest() {
    let h = Harness::new();
    let digest = [7u8; 32];
    let signature = h.signer().sign_digest(key(SIGN_KEY), digest).await.unwrap();
    assert!(
        h.fake.verifies(&key(SIGN_KEY), &digest, signature.as_bytes()),
        "a real ECDSA signature over the digest"
    );
    assert!(
        !h.fake.verifies(&key(SIGN_KEY), &[8u8; 32], signature.as_bytes()),
        "and over no other digest"
    );

    let sign = h
        .fake
        .requests()
        .into_iter()
        .find(|r| r.route == Some(Route::Sign))
        .unwrap();
    assert_eq!(sign.method, "POST");
    assert_eq!(sign.path, format!("/v1/{SIGN_KEY}:asymmetricSign"));
    assert_eq!(sign.authorization.as_deref(), Some("Bearer ya29.fake-token-1"));
    assert!(sign.body["digest"]["sha256"].is_string());
    assert!(
        sign.body["digestCrc32c"].is_string(),
        "the digest's checksum is sent"
    );
}

#[tokio::test(start_paused = true)]
async fn kms_signer_refuses_an_answer_that_fails_its_checksum() {
    let h = Harness::new();
    h.fake.script(Route::Sign, Fault::DamagedAnswer, 1);
    assert_eq!(
        h.signer().sign_digest(key(SIGN_KEY), [1; 32]).await.unwrap_err(),
        KmsError::Unavailable,
        "a damaged signature is never put into a certificate"
    );
    // And the next call, undamaged, is fine: nothing was cached or poisoned.
    assert!(h.signer().sign_digest(key(SIGN_KEY), [1; 32]).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn kms_signer_malformed_key_names_never_reach_the_network() {
    let h = Harness::new();
    for bad in [
        "agent-ca",
        "projects/p/locations/l/keyRings/r/cryptoKeys/k",
        "projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/latest",
        "projects/../locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1",
        "projects/p/locations/l/keyRings/r/cryptoKeys/k:evil/cryptoKeyVersions/1",
        "projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1/../../x",
    ] {
        assert_eq!(
            h.signer().sign_digest(key(bad), [0; 32]).await.unwrap_err(),
            KmsError::Invalid,
            "{bad}"
        );
    }
    assert!(h.fake.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn kms_signer_public_key_is_the_spki_of_the_key() {
    let h = Harness::new();
    let public = h.signer().public_key(&key(SIGN_KEY)).await.unwrap();
    assert_eq!(
        Some(public.spki_der().to_vec()),
        h.fake.public_key_spki_der(&key(SIGN_KEY))
    );

    assert_eq!(
        h.signer().public_key(&key(UNKNOWN_SIGN_KEY)).await.unwrap_err(),
        KmsError::KeyNotFound
    );
    h.fake.script(Route::PublicKey, Fault::DamagedAnswer, 1);
    assert_eq!(
        h.signer().public_key(&key(SIGN_KEY)).await.unwrap_err(),
        KmsError::Unavailable
    );
}

#[tokio::test(start_paused = true)]
async fn kms_envelope_wrap_binds_aad() {
    let h = Harness::new();
    let envelope = h.envelope();
    let dek = Secret::new([0x42u8; 32]);

    let wrapped = envelope.wrap(&dek, b"user:1/credential:2").await.unwrap();
    assert!(wrapped.key_version.as_str().starts_with(ENVELOPE_KEY));
    assert!(
        wrapped.key_version.as_str().contains("/cryptoKeyVersions/"),
        "the version that wrapped it is kept"
    );
    let encrypt = h
        .fake
        .requests()
        .into_iter()
        .find(|r| r.route == Some(Route::Encrypt))
        .unwrap();
    assert!(
        encrypt.body["additionalAuthenticatedData"].is_string(),
        "the associated data goes to KMS"
    );

    let opened = envelope.unwrap(&wrapped, b"user:1/credential:2").await.unwrap();
    assert!(bool::from(opened.ct_eq(&dek)));
    assert_eq!(
        envelope
            .unwrap(&wrapped, b"user:2/credential:2")
            .await
            .unwrap_err(),
        KmsError::Decrypt,
        "another owner's associated data fails with Decrypt"
    );
    assert_eq!(
        envelope.unwrap(&wrapped, b"").await.unwrap_err(),
        KmsError::Decrypt
    );
}

#[tokio::test(start_paused = true)]
async fn kms_envelope_refuses_what_it_should_not_send() {
    let h = Harness::new();
    let envelope = h.envelope();
    let dek = Secret::new([1u8; 32]);

    // KMS itself accepts empty associated data; the ciphertext would then belong to nobody.
    assert_eq!(envelope.wrap(&dek, b"").await.unwrap_err(), KmsError::Invalid);
    assert_eq!(
        envelope.wrap(&dek, &[b'a'; 1025]).await.unwrap_err(),
        KmsError::Invalid
    );

    let wrapped = envelope.wrap(&dek, b"u/c").await.unwrap();
    let before = h.fake.requests().len();
    // A key version of a different key, or one that is not a key name at all: Decrypt, without asking KMS.
    for version in [
        format!("{OTHER_ENVELOPE_KEY}/cryptoKeyVersions/1"),
        "fake/kms/keys/envelope/versions/1".to_owned(),
    ] {
        let foreign = WrappedKey {
            key_version: key(&version),
            ciphertext: wrapped.ciphertext.clone(),
        };
        assert_eq!(
            envelope.unwrap(&foreign, b"u/c").await.unwrap_err(),
            KmsError::Decrypt
        );
    }
    let huge = WrappedKey {
        key_version: wrapped.key_version.clone(),
        ciphertext: Bytes::from(vec![0u8; 4096]),
    };
    assert_eq!(
        envelope.unwrap(&huge, b"u/c").await.unwrap_err(),
        KmsError::Decrypt
    );
    assert_eq!(h.fake.requests().len(), before, "none of these reached KMS");
}

#[tokio::test(start_paused = true)]
async fn kms_envelope_refuses_a_key_that_is_not_a_crypto_key_name() {
    let h = Harness::new();
    for bad in [SIGN_KEY, "projects/p/locations/l/keyRings/r", "token-envelope"] {
        assert!(
            hub_platform::gcp::kms::CloudKmsEnvelope::new(h.api.clone(), h.kms_origin.clone(), &key(bad))
                .is_err(),
            "{bad}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn kms_envelope_refuses_damaged_answers_in_both_directions() {
    let h = Harness::new();
    let envelope = h.envelope();
    let dek = Secret::new([9u8; 32]);

    h.fake.script(Route::Encrypt, Fault::DamagedAnswer, 1);
    assert_eq!(
        envelope.wrap(&dek, b"u/c").await.unwrap_err(),
        KmsError::Unavailable
    );

    let wrapped = envelope.wrap(&dek, b"u/c").await.unwrap();
    h.fake.script(Route::Decrypt, Fault::DamagedAnswer, 1);
    assert_eq!(
        envelope.unwrap(&wrapped, b"u/c").await.unwrap_err(),
        KmsError::Unavailable
    );
    assert!(envelope.unwrap(&wrapped, b"u/c").await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn kms_calls_retry_with_jitter_and_time_out() {
    // Two 503s and then the signature: three tries, two retries counted.
    let h = Harness::new();
    h.fake.script(Route::Sign, Fault::Status(503), 2);
    h.signer().sign_digest(key(SIGN_KEY), [3; 32]).await.unwrap();
    assert_eq!(h.fake.count(Route::Sign), 3);
    assert!(
        h.exposition()
            .contains("lanekeeper_google_api_retries_total{operation=\"KmsSign\"} 2")
    );

    // A cut connection is a transport failure and is retried the same way.
    let h = Harness::new();
    h.fake.script(Route::Sign, Fault::Disconnect, 1);
    h.signer().sign_digest(key(SIGN_KEY), [3; 32]).await.unwrap();
    assert_eq!(h.fake.count(Route::Sign), 2);

    // Three 503s: bounded, then Unavailable. A 400 is not retried at all.
    let h = Harness::new();
    h.fake.script(Route::Sign, Fault::Status(503), 3);
    assert_eq!(
        h.signer().sign_digest(key(SIGN_KEY), [3; 32]).await.unwrap_err(),
        KmsError::Unavailable
    );
    assert_eq!(h.fake.count(Route::Sign), 3);
    h.fake.script(Route::Sign, Fault::Status(400), 1);
    assert_eq!(
        h.signer().sign_digest(key(SIGN_KEY), [3; 32]).await.unwrap_err(),
        KmsError::Invalid
    );
    assert_eq!(h.fake.count(Route::Sign), 4);
    assert!(
        h.exposition()
            .contains(r#"operation="KmsSign",outcome="Unavailable""#)
    );
    assert!(
        h.exposition()
            .contains(r#"operation="KmsSign",outcome="Refused""#)
    );
}

#[tokio::test(start_paused = true)]
async fn kms_calls_time_out_per_attempt_and_overall() {
    // A server that never answers: each try gives up after the attempt timeout, the policy allows two tries, and the
    // whole call is over in about two attempts' time, far inside the deadline.
    let http = HttpLimits {
        attempt_timeout: Duration::from_secs(2),
        ..HttpLimits::default()
    };
    let call = CallLimits {
        retry: RetryPolicy {
            max_attempts: 2,
            base: Duration::from_millis(100),
            cap: Duration::from_millis(100),
        },
        deadline: Duration::from_secs(30),
    };
    let h = Harness::with_limits(call, http);
    h.fake.script(Route::Sign, Fault::Hang, 2);
    let started = tokio::time::Instant::now();
    assert_eq!(
        h.signer().sign_digest(key(SIGN_KEY), [3; 32]).await.unwrap_err(),
        KmsError::Unavailable
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(4) && took <= Duration::from_millis(4100),
        "two attempts and one jittered wait: {took:?}"
    );
    assert_eq!(h.fake.count(Route::Sign), 2);
    assert!(
        h.exposition()
            .contains(r#"operation="KmsSign",outcome="Timeout""#)
    );

    // The deadline cuts a call short even when the policy would allow more tries.
    let http = HttpLimits {
        attempt_timeout: Duration::from_secs(10),
        ..HttpLimits::default()
    };
    let call = CallLimits {
        retry: RetryPolicy {
            max_attempts: 100,
            base: Duration::from_millis(10),
            cap: Duration::from_millis(10),
        },
        deadline: Duration::from_secs(25),
    };
    let h = Harness::with_limits(call, http);
    h.fake.script(Route::Sign, Fault::Hang, 100);
    let started = tokio::time::Instant::now();
    assert_eq!(
        h.signer().sign_digest(key(SIGN_KEY), [3; 32]).await.unwrap_err(),
        KmsError::Unavailable
    );
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(25),
        "the deadline, to the second"
    );
}

#[tokio::test(start_paused = true)]
async fn a_401_renews_the_token_once_and_only_once() {
    let h = Harness::new();
    h.signer().sign_digest(key(SIGN_KEY), [1; 32]).await.unwrap();
    assert_eq!(h.fake.count(Route::Token), 1);

    // The token is revoked behind the hub's back: the next call gets a 401, renews, and succeeds.
    h.fake.revoke_tokens();
    h.signer().sign_digest(key(SIGN_KEY), [2; 32]).await.unwrap();
    assert_eq!(h.fake.count(Route::Token), 2);
    assert_eq!(h.fake.count(Route::Sign), 3, "one refused, one accepted");

    // A server that keeps saying 401 does not make the client loop.
    h.fake.script(Route::Sign, Fault::Status(401), 5);
    assert_eq!(
        h.signer().sign_digest(key(SIGN_KEY), [3; 32]).await.unwrap_err(),
        KmsError::Unavailable
    );
    assert_eq!(h.fake.count(Route::Sign), 5, "two more tries, not five");
    assert_eq!(h.fake.count(Route::Token), 3, "one renewal");
}

#[tokio::test(start_paused = true)]
async fn kms_values_never_appear_in_logs_debug_or_metrics() {
    let (logs, _guard) = LogCapture::install();
    let h = Harness::new();
    let dek = Secret::new([0xA5u8; 32]);
    let planted_aad = b"user:PLANTED/credential:7";

    let wrapped = h.envelope().wrap(&dek, planted_aad).await.unwrap();
    h.envelope().unwrap(&wrapped, planted_aad).await.unwrap();
    h.signer().sign_digest(key(SIGN_KEY), [0x5Au8; 32]).await.unwrap();
    // Failure paths log.
    h.fake.script(Route::Sign, Fault::Status(503), 3);
    let _ = h.signer().sign_digest(key(SIGN_KEY), [0x5Au8; 32]).await;
    h.fake.script(Route::Decrypt, Fault::Status(400), 1);
    let _ = h.envelope().unwrap(&wrapped, planted_aad).await;
    h.fake.script(Route::Encrypt, Fault::DamagedAnswer, 1);
    let _ = h.envelope().wrap(&dek, planted_aad).await;

    // `wrapped` and `dek` are the values themselves; their Debug is the port's own, which names the key version (a
    // public name) but must not show bytes. Everything this crate prints must show neither bytes nor names.
    let values = format!("{wrapped:?} {dek:?}");
    let ours = format!(
        "{:?} {:?} {} {}",
        h.signer(),
        h.envelope(),
        logs.text(),
        h.exposition()
    );
    let dek_b64 = base64_of(&[0xA5u8; 32]);
    let digest_b64 = base64_of(&[0x5Au8; 32]);
    let ciphertext_b64 = base64_of(&wrapped.ciphertext);
    for secret in [
        dek_b64.as_str(),
        digest_b64.as_str(),
        ciphertext_b64.as_str(),
        "PLANTED",
        "ya29.fake-token",
        "a5a5a5a5",
    ] {
        assert!(
            !values.contains(secret) && !ours.contains(secret),
            "{secret} leaked"
        );
    }
    // Key names are public configuration and may appear in a component's Debug, but logs and metric labels carry codes
    // only (S10): a label of key names would also make the number of series depend on configuration.
    let emitted = format!("{} {}", logs.text(), h.exposition());
    for name in ["agent-ca", "token-envelope", "lk-test-project"] {
        assert!(!emitted.contains(name), "{name} leaked into logs or metrics");
    }
    assert!(!logs.is_empty());
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
