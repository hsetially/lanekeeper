use domain::Secret;

use crate::{KeyRef, KmsEnvelope, KmsError, KmsSigner, SecretError, SecretSource, WrappedKey};

/// `known` is a key the signer holds; `unknown` is one it does not.
///
/// - Signing with a known key gives a non-empty signature that depends on the digest.
/// - An unknown key is `KeyNotFound`.
pub async fn kms_signer<S: KmsSigner + ?Sized>(s: &S, known: &KeyRef, unknown: &KeyRef) {
    let a = s.sign_digest(known.clone(), [1; 32]).await.expect("sign");
    let b = s.sign_digest(known.clone(), [2; 32]).await.expect("sign");
    assert!(!a.as_bytes().is_empty() && a.as_bytes().len() <= 1024);
    assert_ne!(a, b, "different digests give different signatures");
    assert_eq!(
        s.sign_digest(unknown.clone(), [1; 32]).await.unwrap_err(),
        KmsError::KeyNotFound
    );
}

/// - Unwrapping with the same associated data returns the data key; any other associated data, a tampered
///   or truncated ciphertext fails with `Decrypt` (S7: the ciphertext is bound to its owner).
/// - Wrapping without associated data is `Invalid`.
/// - The ciphertext does not contain the plaintext key, and wrapping twice gives different ciphertexts.
pub async fn kms_envelope<E: KmsEnvelope + ?Sized>(e: &E) {
    let key = [0x5au8; 32];
    let dek = Secret::new(key);
    let aad = b"user:1/credential:2";

    let wrapped = e.wrap(&dek, aad).await.expect("wrap");
    assert!(
        !wrapped.ciphertext.windows(key.len()).any(|w| w == key),
        "the plaintext key must not appear in the ciphertext"
    );
    let back = e.unwrap(&wrapped, aad).await.expect("unwrap with the same aad");
    assert!(bool::from(back.ct_eq(&dek)), "round trip");

    assert_eq!(
        e.unwrap(&wrapped, b"user:2/credential:2").await.unwrap_err(),
        KmsError::Decrypt,
        "another owner's associated data"
    );
    assert_eq!(e.unwrap(&wrapped, b"").await.unwrap_err(), KmsError::Decrypt);

    let mut tampered = wrapped.ciphertext.to_vec();
    if let Some(last) = tampered.last_mut() {
        *last ^= 1;
    }
    let tampered = WrappedKey {
        key_version: wrapped.key_version.clone(),
        ciphertext: tampered.into(),
    };
    assert_eq!(
        e.unwrap(&tampered, aad).await.unwrap_err(),
        KmsError::Decrypt,
        "tampered ciphertext"
    );

    let truncated = WrappedKey {
        key_version: wrapped.key_version.clone(),
        ciphertext: wrapped.ciphertext.slice(..wrapped.ciphertext.len() / 2),
    };
    assert_eq!(
        e.unwrap(&truncated, aad).await.unwrap_err(),
        KmsError::Decrypt,
        "truncated ciphertext"
    );

    assert_eq!(
        e.wrap(&dek, b"").await.unwrap_err(),
        KmsError::Invalid,
        "associated data is required"
    );

    let again = e.wrap(&dek, aad).await.unwrap();
    assert_ne!(wrapped.ciphertext, again.ciphertext, "a fresh nonce per wrap");
}

/// `name` holds `value`; `missing` is a valid name that holds nothing.
///
/// - Reading returns the value; a missing secret is `NotFound`; malformed names are `InvalidName`.
/// - Error messages contain neither the name nor the value.
pub async fn secret_source<S: SecretSource + ?Sized>(s: &S, name: &str, value: &str, missing: &str) {
    let got = s.get(name).await.expect("known secret");
    assert!(bool::from(got.ct_eq_bytes(value.as_bytes())), "value round trip");
    assert!(
        !format!("{got:?}").contains(value),
        "Debug must not print the value"
    );

    let err = s.get(missing).await.unwrap_err();
    assert_eq!(err, SecretError::NotFound);
    let long = "x".repeat(256);
    for bad in ["", "has space", "../etc/passwd", long.as_str()] {
        let err = s.get(bad).await.unwrap_err();
        assert_eq!(err, SecretError::InvalidName, "name {bad:?}");
    }
    for err in [SecretError::NotFound, SecretError::InvalidName] {
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains(name) && !shown.contains(value));
    }
}
