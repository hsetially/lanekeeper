//! The syntax of a personal token and the keyed hash the store holds instead of it (S7, S11).
//!
//! A token is `lkpat_` plus 256 random bits in unpadded base64url: 49 bytes, one spelling per secret. The store never sees it.
//! It holds `HMAC-SHA-256(key, token text)` under a key the database never sees (a runtime secret from Secret Manager), so a copy
//! of the table lets nobody use a token, and finds a token by an indexed lookup of that hash.
//!
//! This file depends on the rest of the crate for nothing, so the fuzz target `mcp_pat_token` includes it as it is.

use std::fmt;

use aws_lc_rs::hmac;
use aws_lc_rs::rand::fill;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

/// What every personal token starts with. It also tells a personal token from a JWT without parsing either.
pub const PREFIX: &str = "lkpat_";
/// The random bytes of a token: 256 bits.
pub const SECRET_BYTES: usize = 32;
/// The length of a token in bytes: the prefix and 43 base64url characters.
pub const TOKEN_LEN: usize = PREFIX.len() + 43;
/// The shortest HMAC key accepted: as long as the digest.
pub const MIN_KEY_BYTES: usize = 32;
/// The longest HMAC key accepted. A longer secret is a mistake, not extra strength.
pub const MAX_KEY_BYTES: usize = 1024;
/// The longest key text read before decoding it.
const MAX_KEY_TEXT: usize = 2048;

/// Whether `text` starts like a personal token. Cheap and loose: it only routes a credential to the right verifier.
#[must_use]
pub fn looks_like_personal_token(text: &str) -> bool {
    text.starts_with(PREFIX)
}

/// A token of exactly the shape this server issues: the prefix, 43 base64url characters, and no other spelling of the same 256
/// bits. Borrowed from the text it was parsed from, which is not copied.
#[derive(Clone, Copy)]
pub struct PersonalToken<'a>(&'a str);

impl<'a> PersonalToken<'a> {
    /// Parse a token. `None` for anything else: the wrong length or prefix, a character outside the alphabet, or a spelling that
    /// is not the canonical one (the last character carries two bits that must be zero).
    #[must_use]
    pub fn parse(text: &'a str) -> Option<Self> {
        if text.len() != TOKEN_LEN {
            return None;
        }
        let body = text.strip_prefix(PREFIX)?;
        // The decoder wants room for its own estimate (a whole number of 3-byte groups), which is a little more than 32 bytes.
        let mut bytes = [0_u8; SECRET_BYTES + 4];
        // The engine refuses padding and non-zero trailing bits, so the text is the one spelling of its bytes.
        let written = URL_SAFE_NO_PAD.decode_slice(body, &mut bytes).ok()?;
        bytes.zeroize();
        (written == SECRET_BYTES).then_some(Self(text))
    }

    /// The text, exactly as presented.
    #[must_use]
    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

impl fmt::Debug for PersonalToken<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PersonalToken([redacted])")
    }
}

/// A new token and nothing else: the CSPRNG could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the random generator is unavailable")]
pub struct RandomError;

/// A fresh token from the system CSPRNG. The caller shows it once and keeps only [`HmacKey::lookup`] of it.
///
/// # Errors
///
/// [`RandomError`] when the generator fails.
pub fn generate() -> Result<String, RandomError> {
    let mut bytes = [0_u8; SECRET_BYTES];
    fill(&mut bytes).map_err(|_| RandomError)?;
    let mut text = String::with_capacity(TOKEN_LEN);
    text.push_str(PREFIX);
    URL_SAFE_NO_PAD.encode_string(bytes, &mut text);
    bytes.zeroize();
    Ok(text)
}

/// The HMAC of a token: what the store holds and looks a token up by. Not a credential on its own (it is keyed), but it is never
/// logged or returned either.
#[derive(Clone, Copy)]
pub struct TokenHmac([u8; 32]);

impl TokenHmac {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Constant-time equality. The store's row is compared with the hash computed for the presented token (S7).
    #[must_use]
    pub fn ct_matches(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }
}

impl PartialEq for TokenHmac {
    fn eq(&self, other: &Self) -> bool {
        self.ct_matches(other)
    }
}

impl Eq for TokenHmac {}

impl std::hash::Hash for TokenHmac {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for TokenHmac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenHmac([redacted])")
    }
}

/// Why a key was refused. Carries nothing of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("the token key is not base64url")]
    Encoding,
    #[error("the token key is shorter than 32 bytes")]
    TooShort,
    #[error("the token key is longer than 1024 bytes")]
    TooLong,
}

/// The key that turns a token into the hash the store holds (S7). One key per deployment, from Secret Manager; replacing it ends
/// every token, which at seven days is acceptable.
#[derive(Clone)]
pub struct HmacKey(hmac::Key);

impl HmacKey {
    /// A key from raw bytes: 32 to 1024 of them.
    ///
    /// # Errors
    ///
    /// [`KeyError::TooShort`] or [`KeyError::TooLong`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KeyError> {
        if bytes.len() < MIN_KEY_BYTES {
            return Err(KeyError::TooShort);
        }
        if bytes.len() > MAX_KEY_BYTES {
            return Err(KeyError::TooLong);
        }
        Ok(Self(hmac::Key::new(hmac::HMAC_SHA256, bytes)))
    }

    /// A key from the text of a secret: unpadded base64url of 32 to 1024 bytes, with surrounding whitespace ignored (a secret
    /// stored from a file ends in a newline). The decoded bytes are wiped as soon as the key is built.
    ///
    /// # Errors
    ///
    /// [`KeyError`] for text that is not base64url or whose decoded length is out of range.
    pub fn from_secret_text(text: &str) -> Result<Self, KeyError> {
        let text = text.trim();
        if text.len() > MAX_KEY_TEXT {
            return Err(KeyError::TooLong);
        }
        let mut bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| KeyError::Encoding)?;
        let key = Self::from_bytes(&bytes);
        bytes.zeroize();
        key
    }

    /// The hash of a token under this key.
    #[must_use]
    pub fn lookup(&self, token: &PersonalToken<'_>) -> TokenHmac {
        let tag = hmac::sign(&self.0, token.as_str().as_bytes());
        let mut out = [0_u8; 32];
        // HMAC-SHA-256 is 32 bytes; the copy cannot be shorter or longer.
        out.copy_from_slice(tag.as_ref());
        TokenHmac(out)
    }
}

impl fmt::Debug for HmacKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HmacKey([redacted])")
    }
}
