//! `Secret<T>`: redacted in `Debug`, no `Display`, no `Serialize`, no `Clone`, zeroized on drop (S21).

use std::fmt;

use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Deserializer};
use subtle::{Choice, ConstantTimeEq};
use zeroize::Zeroize;

/// A value that must never appear in logs, errors, audit events or responses.
///
/// - `Debug` prints `[redacted]`; there is no `Display`.
/// - There is no `Serialize` and no `Clone`: a secret is moved, or re-read from its source.
/// - Equality is only available through [`Secret::ct_eq`], which is constant time.
/// - The inner value is zeroized on drop.
/// - `Deserialize` exists for `Secret<String>` only, for loading configuration.
pub struct Secret<T: Zeroize>(SecretBox<T>);

impl<T: Zeroize> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(SecretBox::new(Box::new(value)))
    }

    /// Borrow the plaintext. Keep the borrow short and never log or format it.
    pub fn expose(&self) -> &T {
        self.0.expose_secret()
    }
}

impl<T: Zeroize + AsRef<[u8]>> Secret<T> {
    /// Constant-time comparison of two secrets (the length is not hidden).
    pub fn ct_eq(&self, other: &Self) -> Choice {
        self.expose().as_ref().ct_eq(other.expose().as_ref())
    }

    /// Constant-time comparison against candidate bytes, for example a presented token.
    pub fn ct_eq_bytes(&self, candidate: &[u8]) -> Choice {
        self.expose().as_ref().ct_eq(candidate)
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<T: Zeroize> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for Secret<String> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::new)
    }
}
