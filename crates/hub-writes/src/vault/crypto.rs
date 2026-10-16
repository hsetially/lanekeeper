//! Envelope encryption of a GitHub token (S8): a fresh AES-256-GCM data key for every token, wrapped by KMS. The associated data
//! is the owner's user id and the credential id, for the data key's wrapping and for the token's encryption alike, so a stored
//! ciphertext opens only for the user and the row it was made for.
//!
//! The plaintext token exists in three places only: the caller's `Secret<String>`, the buffer the cipher works in (overwritten
//! by the ciphertext as it is sealed, and zeroized on drop after it is opened) and the `Secret` that [`open`] returns. The data
//! key is a `Secret<[u8; 32]>`, dropped (and so zeroized) before the function that made it returns.

use std::borrow::Borrow;

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use domain::{Secret, UserId};
use ports::{KmsEnvelope, KmsError, WrappedKey};
use zeroize::{Zeroize, Zeroizing};

/// The ciphertext of AES-256-GCM carries a 16-byte tag after the encrypted bytes.
const TAG_LEN: usize = 16;

/// Why a token could not be sealed or opened. None of these carries a token, a key or the associated data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// KMS did not answer, or refused the request.
    #[error("the key service is unavailable")]
    Kms,
    /// The ciphertext does not open for this owner: it was changed, moved or made with other associated data.
    #[error("the credential does not decrypt")]
    Decrypt,
    /// The system could not make a key or a nonce.
    #[error("no random bytes were available")]
    Random,
}

impl From<KmsError> for CryptoError {
    fn from(e: KmsError) -> Self {
        match e {
            KmsError::Decrypt | KmsError::KeyNotFound => Self::Decrypt,
            KmsError::Invalid | KmsError::Unavailable => Self::Kms,
        }
    }
}

/// A token as it is stored: the wrapped data key, the nonce and the encrypted bytes with their tag.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed {
    pub wrapped: WrappedKey,
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sealed").finish_non_exhaustive()
    }
}

/// A plaintext type [`open`] can return: bytes that become a value, and zeroize when dropped. `String` is the real one; the tests
/// use a type that records when it is zeroized.
pub trait Plaintext: Zeroize + Sized {
    /// `None` when the bytes are not a value of this type. The buffer is consumed either way.
    fn from_plain(bytes: Vec<u8>) -> Option<Self>;
}

impl Plaintext for String {
    fn from_plain(bytes: Vec<u8>) -> Option<Self> {
        String::from_utf8(bytes).ok()
    }
}

/// The associated data of a credential: a label, then the user's tenant id, object id and the credential id, each with its
/// length, so no two (user, credential) pairs give the same bytes.
#[must_use]
pub fn associated_data(user: &UserId, credential_id: i64) -> Vec<u8> {
    let mut aad = Vec::with_capacity(96);
    aad.extend_from_slice(b"lanekeeper/github-token/v1");
    for part in [user.tid().as_str(), user.oid().as_str()] {
        aad.extend_from_slice(&(part.len() as u64).to_be_bytes());
        aad.extend_from_slice(part.as_bytes());
    }
    aad.extend_from_slice(&credential_id.to_be_bytes());
    aad
}

fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut bytes = [0_u8; N];
    aws_lc_rs::rand::fill(&mut bytes).map_err(|_| CryptoError::Random)?;
    Ok(bytes)
}

fn cipher(dek: &Secret<[u8; 32]>) -> Result<LessSafeKey, CryptoError> {
    let key = UnboundKey::new(&AES_256_GCM, dek.expose()).map_err(|_| CryptoError::Random)?;
    Ok(LessSafeKey::new(key))
}

/// A fresh data key, as a `Secret`. The bytes it was made from are wiped as soon as they are in the `Secret` (and on an early
/// return), so the one copy that remains is the one that is zeroized when the `Secret` drops.
fn new_data_key() -> Result<Secret<[u8; 32]>, CryptoError> {
    let raw = Zeroizing::new(random::<32>()?);
    Ok(Secret::new(*raw))
}

/// Encrypt `token` under a data key made for it, and have KMS wrap that key. `aad` is [`associated_data`].
pub async fn seal(kms: &dyn KmsEnvelope, token: &Secret<String>, aad: &[u8]) -> Result<Sealed, CryptoError> {
    seal_with(kms, token, aad, new_data_key).await
}

/// [`seal`] with the data key's source named, so that a test can watch the key: `new_key` makes the key, and this function owns
/// it from then on. Whatever happens, the key is dropped (and so zeroized) before this function returns; it is not stored, not
/// returned and not held across anything that outlives the call.
async fn seal_with<K: Borrow<Secret<[u8; 32]>>>(
    kms: &dyn KmsEnvelope,
    token: &Secret<String>,
    aad: &[u8],
    new_key: impl FnOnce() -> Result<K, CryptoError>,
) -> Result<Sealed, CryptoError> {
    let nonce = random::<NONCE_LEN>()?;
    let (wrapped, ciphertext) = {
        let dek = new_key()?;
        let key = cipher(dek.borrow())?;
        // The room for the tag is reserved, so the buffer never moves and no copy of the plaintext is left behind.
        let mut buffer = Vec::with_capacity(token.expose().len() + TAG_LEN);
        buffer.extend_from_slice(token.expose().as_bytes());
        key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buffer)
            .map_err(|_| CryptoError::Decrypt)?;
        let wrapped = kms.wrap(dek.borrow(), aad).await?;
        // `dek` is dropped, and so zeroized, at the end of this block, and on every early return above.
        (wrapped, buffer)
    };
    Ok(Sealed {
        wrapped,
        nonce,
        ciphertext,
    })
}

/// Decrypt a stored token for the owner `aad` names. The result is a `Secret`: it zeroizes when dropped.
pub async fn open<T: Plaintext>(
    kms: &dyn KmsEnvelope,
    sealed: &Sealed,
    aad: &[u8],
) -> Result<Secret<T>, CryptoError> {
    if sealed.ciphertext.len() < TAG_LEN {
        return Err(CryptoError::Decrypt);
    }
    let dek = kms.unwrap(&sealed.wrapped, aad).await?;
    let key = cipher(&dek)?;
    drop(dek);
    let mut buffer = sealed.ciphertext.clone();
    let opened = key
        .open_in_place(
            Nonce::assume_unique_for_key(sealed.nonce),
            Aad::from(aad),
            &mut buffer,
        )
        .map(|plain| plain.len());
    let Ok(plain_len) = opened else {
        // Whatever the cipher left in the buffer is not a token, but it is wiped like one.
        buffer.zeroize();
        return Err(CryptoError::Decrypt);
    };
    buffer.truncate(plain_len);
    match T::from_plain(buffer) {
        Some(value) => Ok(Secret::new(value)),
        None => Err(CryptoError::Decrypt),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Borrow;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use ports::conformance::sample::user_id;
    use ports::fakes::FakeKmsEnvelope;

    use super::*;

    const TOKEN: &str = "github_pat_11ABCDEFG0abcdefghijklmnopqrstuvwxyz0123456789";

    fn token() -> Secret<String> {
        Secret::new(TOKEN.to_owned())
    }

    #[tokio::test]
    async fn a_token_opens_for_its_owner_and_row() {
        let kms = FakeKmsEnvelope::new();
        let aad = associated_data(&user_id(1), 7);
        let sealed = seal(&kms, &token(), &aad).await.unwrap();
        let opened: Secret<String> = open(&kms, &sealed, &aad).await.unwrap();
        assert_eq!(opened.expose(), TOKEN);
        assert_eq!(sealed.ciphertext.len(), TOKEN.len() + TAG_LEN);
        assert!(
            !sealed
                .ciphertext
                .windows(8)
                .any(|w| TOKEN.as_bytes().windows(8).any(|t| t == w)),
            "the ciphertext holds a run of the token"
        );
    }

    #[tokio::test]
    async fn swapped_associated_data_fails_decryption() {
        let kms = FakeKmsEnvelope::new();
        let sealed = seal(&kms, &token(), &associated_data(&user_id(1), 7))
            .await
            .unwrap();
        for (what, aad) in [
            ("another credential id", associated_data(&user_id(1), 8)),
            ("another user", associated_data(&user_id(2), 7)),
            ("nothing", Vec::new()),
        ] {
            let got = open::<String>(&kms, &sealed, &aad).await;
            assert_eq!(got.unwrap_err(), CryptoError::Decrypt, "{what}");
        }
    }

    /// The data key's wrapping is bound to the owner too: KMS refuses to unwrap it for other associated data, so the cipher is
    /// never even tried. (The cipher would refuse as well, which `swapped_associated_data_fails_decryption` shows.)
    #[tokio::test]
    async fn the_wrapped_key_only_unwraps_for_the_owner() {
        let kms = FakeKmsEnvelope::new();
        let aad = associated_data(&user_id(1), 7);
        let sealed = seal(&kms, &token(), &aad).await.unwrap();
        let other = associated_data(&user_id(2), 7);
        assert_eq!(
            kms.unwrap(&sealed.wrapped, &other).await.unwrap_err(),
            KmsError::Decrypt
        );
        assert!(kms.unwrap(&sealed.wrapped, &aad).await.is_ok());
    }

    #[tokio::test]
    async fn ciphertext_moved_to_another_user_fails() {
        let kms = FakeKmsEnvelope::new();
        let sealed = seal(&kms, &token(), &associated_data(&user_id(1), 7))
            .await
            .unwrap();
        // The row is copied to user 2 under the same credential id.
        let theirs = associated_data(&user_id(2), 7);
        assert!(open::<String>(&kms, &sealed, &theirs).await.is_err());
    }

    #[tokio::test]
    async fn a_changed_ciphertext_nonce_or_tag_fails() {
        let kms = FakeKmsEnvelope::new();
        let aad = associated_data(&user_id(1), 7);
        let sealed = seal(&kms, &token(), &aad).await.unwrap();
        for i in [0, TOKEN.len() / 2, TOKEN.len(), TOKEN.len() + TAG_LEN - 1] {
            let mut s = sealed.clone();
            s.ciphertext[i] ^= 1;
            assert!(open::<String>(&kms, &s, &aad).await.is_err(), "byte {i}");
        }
        let mut s = sealed.clone();
        s.nonce[0] ^= 1;
        assert!(open::<String>(&kms, &s, &aad).await.is_err());
        let mut s = sealed.clone();
        s.ciphertext.truncate(TAG_LEN - 1);
        assert!(open::<String>(&kms, &s, &aad).await.is_err());
        let mut s = sealed;
        s.ciphertext.clear();
        assert!(open::<String>(&kms, &s, &aad).await.is_err());
    }

    /// Records every data key KMS is asked to wrap.
    #[derive(Default)]
    struct Spy {
        inner: FakeKmsEnvelope,
        keys: std::sync::Mutex<Vec<[u8; 32]>>,
    }

    #[async_trait]
    impl KmsEnvelope for Spy {
        async fn wrap(&self, dek: &Secret<[u8; 32]>, aad: &[u8]) -> Result<WrappedKey, KmsError> {
            self.keys.lock().unwrap().push(*dek.expose());
            self.inner.wrap(dek, aad).await
        }

        async fn unwrap(&self, w: &WrappedKey, aad: &[u8]) -> Result<Secret<[u8; 32]>, KmsError> {
            self.inner.unwrap(w, aad).await
        }
    }

    #[tokio::test]
    async fn fresh_data_key_per_token() {
        let spy = Spy::default();
        let aad = associated_data(&user_id(1), 7);
        let a = seal(&spy, &token(), &aad).await.unwrap();
        let b = seal(&spy, &token(), &aad).await.unwrap();
        let keys = spy.keys.lock().unwrap().clone();
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1], "two tokens shared a data key");
        assert_ne!(keys[0], [0; 32]);
        // The same token twice gives different nonces and different ciphertexts.
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.wrapped.ciphertext, b.wrapped.ciphertext);
    }

    static ZEROIZED: AtomicUsize = AtomicUsize::new(0);

    /// A plaintext that counts the times it is zeroized, which `Secret` does when it drops.
    struct Probe(Vec<u8>);

    impl Zeroize for Probe {
        fn zeroize(&mut self) {
            self.0.zeroize();
            ZEROIZED.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Plaintext for Probe {
        fn from_plain(bytes: Vec<u8>) -> Option<Self> {
            Some(Self(bytes))
        }
    }

    #[tokio::test]
    async fn plaintext_token_is_zeroized_after_use() {
        let kms = FakeKmsEnvelope::new();
        let aad = associated_data(&user_id(1), 7);
        let sealed = seal(&kms, &token(), &aad).await.unwrap();
        let before = ZEROIZED.load(Ordering::SeqCst);
        {
            let opened: Secret<Probe> = open(&kms, &sealed, &aad).await.unwrap();
            assert_eq!(opened.expose().0, TOKEN.as_bytes());
            assert_eq!(
                ZEROIZED.load(Ordering::SeqCst),
                before,
                "zeroized while still in use"
            );
        }
        assert_eq!(
            ZEROIZED.load(Ordering::SeqCst),
            before + 1,
            "the plaintext was not zeroized when dropped"
        );
    }

    /// A data key that tells when it is dropped. `seal_with` owns whatever its key factory returns, so this is what shows how long
    /// the key lives: `Secret` zeroizes its bytes when it drops, and a `Secret` that has not dropped has not been wiped.
    struct Probed {
        key: Secret<[u8; 32]>,
        dropped: Arc<AtomicBool>,
    }

    impl Probed {
        fn new(bytes: [u8; 32]) -> (Self, Arc<AtomicBool>) {
            let dropped = Arc::new(AtomicBool::new(false));
            (
                Self {
                    key: Secret::new(bytes),
                    dropped: dropped.clone(),
                },
                dropped,
            )
        }
    }

    impl Borrow<Secret<[u8; 32]>> for Probed {
        fn borrow(&self) -> &Secret<[u8; 32]> {
            &self.key
        }
    }

    impl Drop for Probed {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    /// A KMS that records the key it is asked to wrap and whether that key is still alive at that moment, then answers `reply`.
    struct Watching {
        inner: FakeKmsEnvelope,
        dropped: Arc<AtomicBool>,
        seen: std::sync::Mutex<Vec<([u8; 32], bool)>>,
        refuse: bool,
    }

    #[async_trait]
    impl KmsEnvelope for Watching {
        async fn wrap(&self, dek: &Secret<[u8; 32]>, aad: &[u8]) -> Result<WrappedKey, KmsError> {
            let alive = !self.dropped.load(Ordering::SeqCst);
            self.seen.lock().unwrap().push((*dek.expose(), alive));
            if self.refuse {
                return Err(KmsError::Unavailable);
            }
            self.inner.wrap(dek, aad).await
        }

        async fn unwrap(&self, w: &WrappedKey, aad: &[u8]) -> Result<Secret<[u8; 32]>, KmsError> {
            self.inner.unwrap(w, aad).await
        }
    }

    const KEY: [u8; 32] = [0xA5; 32];

    fn watching(dropped: &Arc<AtomicBool>, refuse: bool) -> Watching {
        Watching {
            inner: FakeKmsEnvelope::new(),
            dropped: dropped.clone(),
            seen: std::sync::Mutex::default(),
            refuse,
        }
    }

    /// S8, acceptance 3. The data key is a `Secret`, KMS wraps exactly that key while it is alive, it is the key the token is
    /// encrypted under, no copy of it is left in what `seal` returns, and it is dropped (which zeroizes it) before `seal` returns.
    /// What cannot be shown from safe Rust is the wiped memory itself: that is `Secret`'s own property, and the probe shows that
    /// it is `Secret`'s drop that `seal` relies on and that nothing keeps the key past the call.
    #[tokio::test]
    async fn data_key_is_zeroized_after_wrap() {
        let (key, dropped) = Probed::new(KEY);
        let kms = watching(&dropped, false);
        let aad = associated_data(&user_id(1), 7);
        let sealed = seal_with(&kms, &token(), &aad, || Ok(key)).await.unwrap();

        assert!(
            dropped.load(Ordering::SeqCst),
            "the data key outlived the call to seal"
        );
        let seen = kms.seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![(KEY, true)],
            "KMS is given the key while it is alive, once"
        );

        // The token was encrypted under that key: the bytes KMS wrapped are the bytes that open it.
        let mut buffer = sealed.ciphertext.clone();
        let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &KEY).unwrap());
        let plain = cipher
            .open_in_place(
                Nonce::assume_unique_for_key(sealed.nonce),
                Aad::from(&aad[..]),
                &mut buffer,
            )
            .unwrap();
        assert_eq!(plain, TOKEN.as_bytes());

        // And no copy of it was left in what is returned.
        for (what, bytes) in [
            ("nonce", sealed.nonce.to_vec()),
            ("ciphertext", sealed.ciphertext.clone()),
            ("wrapped key", sealed.wrapped.ciphertext.to_vec()),
        ] {
            assert!(
                !bytes.windows(8).any(|w| KEY.windows(8).any(|k| k == w)),
                "{what} holds the key"
            );
        }
    }

    /// The key is wiped when `seal` gives up, too: a KMS that refuses must not leave it behind.
    #[tokio::test]
    async fn data_key_is_zeroized_when_kms_refuses() {
        let (key, dropped) = Probed::new(KEY);
        let kms = watching(&dropped, true);
        let got = seal_with(&kms, &token(), &associated_data(&user_id(1), 7), || Ok(key)).await;
        assert_eq!(got.unwrap_err(), CryptoError::Kms);
        assert!(
            dropped.load(Ordering::SeqCst),
            "the data key outlived the failed call"
        );
        assert_eq!(kms.seen.lock().unwrap().len(), 1);
    }

    /// `seal` takes its key from `new_data_key`, which hands out a `Secret` of 32 random bytes and nothing printable.
    #[tokio::test]
    async fn the_data_key_seal_uses_is_a_fresh_secret() {
        let a: Secret<[u8; 32]> = new_data_key().unwrap();
        let b: Secret<[u8; 32]> = new_data_key().unwrap();
        assert_ne!(a.expose(), b.expose());
        assert_ne!(a.expose(), &[0_u8; 32]);
        assert_eq!(format!("{a:?}"), "[redacted]");
        // The real `seal` wraps keys made this way: a key that is not a `Secret` does not reach KMS.
        let spy = Spy::default();
        seal(&spy, &token(), &associated_data(&user_id(1), 7))
            .await
            .unwrap();
        assert_eq!(spy.keys.lock().unwrap().len(), 1);
    }

    #[test]
    fn errors_and_debug_hold_nothing() {
        let sealed = Sealed {
            wrapped: WrappedKey {
                key_version: ports::KeyRef::parse("k/1").unwrap(),
                ciphertext: bytes::Bytes::from_static(b"wrapped-key-marker"),
            },
            nonce: [9; NONCE_LEN],
            ciphertext: b"ciphertext-marker".to_vec(),
        };
        let shown = format!("{sealed:?}");
        assert!(!shown.contains("marker"));
        for e in [CryptoError::Kms, CryptoError::Decrypt, CryptoError::Random] {
            assert!(!format!("{e} {e:?}").contains("github_pat"));
        }
    }

    #[test]
    fn associated_data_names_the_user_the_part_and_the_credential() {
        let one = user_id(1);
        // The same two ids in the other order are another user.
        let swapped = UserId::new(one.oid().clone(), one.tid().clone());
        assert_ne!(associated_data(&one, 1), associated_data(&swapped, 1));
        assert_ne!(associated_data(&one, 1), associated_data(&one, 2));
        assert_ne!(associated_data(&one, 1), associated_data(&user_id(2), 1));
        assert_eq!(associated_data(&one, 1), associated_data(&one, 1));
        assert!(associated_data(&one, 1).starts_with(b"lanekeeper/github-token/v1"));
    }
}
