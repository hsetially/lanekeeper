//! What the Google REST methods send and receive, as typed, bounded values.
//!
//! Names are validated before they reach a URL (a `..` or a `?` in a name must not rewrite the request), and the integers
//! that Google's JSON writes as strings are read either way.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ports::KeyRef;
use serde::{Deserialize, Deserializer};

/// A name segment: letters, digits, `_` and `-`, at most 63 characters. Project ids, locations, key rings and keys.
#[must_use]
pub fn is_name_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 63
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A project id or number, as it appears in a resource name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectId(String);

impl ProjectId {
    /// Letters, digits and `-`, 1 to 63 characters. (Domain-scoped ids such as `example.com:project` are refused.)
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        (is_name_segment(text) && !text.contains('_')).then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `projects/P/locations/L/keyRings/R/cryptoKeys/K`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoKeyName(String);

/// `projects/P/locations/L/keyRings/R/cryptoKeys/K/cryptoKeyVersions/N`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoKeyVersionName(String);

fn split_key(parts: &[&str]) -> Option<String> {
    let [projects, p, locations, l, key_rings, r, crypto_keys, k] = parts else {
        return None;
    };
    (*projects == "projects"
        && *locations == "locations"
        && *key_rings == "keyRings"
        && *crypto_keys == "cryptoKeys"
        && [p, l, r, k].iter().all(|s| is_name_segment(s)))
    .then(|| parts.join("/"))
}

impl CryptoKeyName {
    #[must_use]
    pub fn parse(key: &KeyRef) -> Option<Self> {
        let parts: Vec<&str> = key.as_str().split('/').collect();
        split_key(&parts).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `version` is a version of this key.
    #[must_use]
    pub fn owns(&self, version: &CryptoKeyVersionName) -> bool {
        version.key().as_str() == self.0
    }
}

impl CryptoKeyVersionName {
    #[must_use]
    pub fn parse(key: &KeyRef) -> Option<Self> {
        let parts: Vec<&str> = key.as_str().split('/').collect();
        let [head @ .., versions, n] = parts.as_slice() else {
            return None;
        };
        let valid_version = !n.is_empty() && n.len() <= 19 && n.bytes().all(|b| b.is_ascii_digit());
        (*versions == "cryptoKeyVersions" && valid_version && split_key(head).is_some())
            .then(|| Self(parts.join("/")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key this is a version of.
    #[must_use]
    pub fn key(&self) -> CryptoKeyName {
        let end = self.0.rfind("/cryptoKeyVersions/").unwrap_or(self.0.len());
        CryptoKeyName(self.0[..end].to_owned())
    }
}

/// A signed or unsigned 64-bit integer that Google writes as a JSON string and a client may send either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Int64(pub i64);

impl<'de> Deserialize<'de> for Int64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Number(i64),
            Text(String),
        }
        match Either::deserialize(d)? {
            Either::Number(n) => Ok(Self(n)),
            Either::Text(t) => t.parse().map(Self).map_err(serde::de::Error::custom),
        }
    }
}

impl Int64 {
    /// Whether this is the CRC-32C `expected`.
    #[must_use]
    pub fn is_crc32c(self, expected: u32) -> bool {
        self.0 == i64::from(expected)
    }
}

/// Standard base64, as in Google's JSON for `bytes` fields.
#[must_use]
pub fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn from_b64(text: &str) -> Option<Vec<u8>> {
    STANDARD.decode(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> KeyRef {
        KeyRef::parse(text).unwrap()
    }

    const VERSION: &str =
        "projects/p1/locations/europe-west2/keyRings/lk/cryptoKeys/agent-ca/cryptoKeyVersions/3";

    #[test]
    fn a_version_name_has_exactly_the_documented_shape() {
        let version = CryptoKeyVersionName::parse(&key(VERSION));
        assert_eq!(version.as_ref().map(CryptoKeyVersionName::as_str), Some(VERSION));
        assert_eq!(
            version.map(|v| v.key().as_str().to_owned()).as_deref(),
            Some("projects/p1/locations/europe-west2/keyRings/lk/cryptoKeys/agent-ca")
        );
        for bad in [
            "projects/p1/locations/l/keyRings/r/cryptoKeys/k",
            "projects/p1/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/",
            "projects/p1/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/latest",
            "projects/p1/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1/extra",
            "projects/../locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1",
            "projects/p1/locations/l/keyRings/r/cryptoKeys/./cryptoKeyVersions/1",
            "projects/p1/locations/l/keyRings/r/cryptoKeys/k:evil/cryptoKeyVersions/1",
            "projects/p1/locations/l/keyRings//cryptoKeys/k/cryptoKeyVersions/1",
            "organizations/p1/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1",
            "fake/kms/keys/envelope/versions/1",
        ] {
            assert!(CryptoKeyVersionName::parse(&key(bad)).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_crypto_key_name_owns_its_versions_and_only_them() {
        let k = CryptoKeyName::parse(&key(
            "projects/p1/locations/europe-west2/keyRings/lk/cryptoKeys/agent-ca",
        ));
        let version = CryptoKeyVersionName::parse(&key(VERSION));
        assert!(matches!((&k, &version), (Some(k), Some(v)) if k.owns(v)));
        let other = CryptoKeyVersionName::parse(&key(
            "projects/p1/locations/europe-west2/keyRings/lk/cryptoKeys/other/cryptoKeyVersions/3",
        ));
        assert!(matches!((&k, &other), (Some(k), Some(v)) if !k.owns(v)));
        assert!(
            CryptoKeyName::parse(&key(VERSION)).is_none(),
            "a version is not a key"
        );
    }

    #[test]
    fn project_ids_are_path_safe() {
        assert!(ProjectId::parse("lk-prod-123").is_some());
        assert!(ProjectId::parse("123456789").is_some());
        for bad in [
            "",
            "a/b",
            "..",
            "a b",
            "a:b",
            "a.b",
            "a?b",
            "a_b",
            &"x".repeat(64),
        ] {
            assert!(ProjectId::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn int64_reads_numbers_and_strings() {
        assert_eq!(
            serde_json::from_str::<Int64>("\"4294967295\"").ok(),
            Some(Int64(4_294_967_295))
        );
        assert_eq!(serde_json::from_str::<Int64>("17").ok(), Some(Int64(17)));
        assert!(serde_json::from_str::<Int64>("\"abc\"").is_err());
        assert!(serde_json::from_str::<Int64>("1.5").is_err());
    }
}
