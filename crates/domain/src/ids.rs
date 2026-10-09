//! Validated identifiers. Every constructor checks its input; errors carry no input text.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why an identifier was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("identifier is empty")]
    Empty,
    #[error("identifier is too long")]
    TooLong,
    #[error("identifier contains a character that is not allowed")]
    BadChar,
    #[error("identifier has the wrong shape")]
    BadShape,
}

fn is_lower_alnum(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit()
}

/// Implements `Display`, `FromStr`, `Serialize` and `Deserialize` through a `parse(&str)` constructor
/// and an `as_str()` accessor.
macro_rules! string_id {
    ($name:ident) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

/// A swimlane: exactly one GKE cluster. A slug, `[a-z0-9-]{1,63}`, starting with a letter or digit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SwimlaneId(String);

impl SwimlaneId {
    pub fn parse(s: &str) -> Result<Self, IdError> {
        let b = s.as_bytes();
        if b.is_empty() {
            return Err(IdError::Empty);
        }
        if b.len() > 63 {
            return Err(IdError::TooLong);
        }
        if !is_lower_alnum(b[0]) || !b.iter().all(|&c| is_lower_alnum(c) || c == b'-') {
            return Err(IdError::BadChar);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
string_id!(SwimlaneId);

/// A tenant id, which equals the tenant branch name (D84). `[a-z0-9_-]{1,63}`, starting with a letter or
/// digit. No dots, so the id can never be confused with a file extension.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TenantId(String);

impl TenantId {
    pub fn parse(s: &str) -> Result<Self, IdError> {
        let b = s.as_bytes();
        if b.is_empty() {
            return Err(IdError::Empty);
        }
        if b.len() > 63 {
            return Err(IdError::TooLong);
        }
        if !is_lower_alnum(b[0]) || !b.iter().all(|&c| is_lower_alnum(c) || c == b'-' || c == b'_') {
            return Err(IdError::BadChar);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
string_id!(TenantId);

/// SHA-256 of file content (the identity of a blob).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash([u8; 32]);

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl ContentHash {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 64 hex characters, either case.
    pub fn parse(s: &str) -> Result<Self, IdError> {
        let b = s.as_bytes();
        if b.len() != 64 {
            return Err(IdError::BadShape);
        }
        let mut out = [0_u8; 32];
        for (i, pair) in b.chunks_exact(2).enumerate() {
            let (Some(hi), Some(lo)) = (hex_val(pair[0]), hex_val(pair[1])) else {
                return Err(IdError::BadChar);
            };
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0_u8; 64];
        for (i, byte) in self.0.iter().enumerate() {
            buf[2 * i] = HEX[usize::from(byte >> 4)];
            buf[2 * i + 1] = HEX[usize::from(byte & 0x0f)];
        }
        // The buffer only holds ASCII hex digits.
        f.write_str(core::str::from_utf8(&buf).map_err(|_| fmt::Error)?)
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

impl FromStr for ContentHash {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A Git object id: 40 hex characters (SHA-1) or 64 (SHA-256), stored lower case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitId(String);

impl CommitId {
    pub fn parse(s: &str) -> Result<Self, IdError> {
        if s.len() != 40 && s.len() != 64 {
            return Err(IdError::BadShape);
        }
        if !s.bytes().all(|c| hex_val(c).is_some()) {
            return Err(IdError::BadChar);
        }
        Ok(Self(s.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First seven characters, for labels.
    pub fn short(&self) -> &str {
        &self.0[..7]
    }
}
string_id!(CommitId);

/// A GUID in 8-4-4-4-12 form (Entra tenant and object ids), stored lower case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(String);

impl Guid {
    pub fn parse(s: &str) -> Result<Self, IdError> {
        let b = s.as_bytes();
        if b.len() != 36 {
            return Err(IdError::BadShape);
        }
        for (i, &c) in b.iter().enumerate() {
            let dash = matches!(i, 8 | 13 | 18 | 23);
            if dash && c != b'-' {
                return Err(IdError::BadShape);
            }
            if !dash && hex_val(c).is_none() {
                return Err(IdError::BadChar);
            }
        }
        Ok(Self(s.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
string_id!(Guid);

/// A user, keyed by Entra tenant id and object id. Written `tid:oid`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId {
    tid: Guid,
    oid: Guid,
}

impl UserId {
    pub fn new(tid: Guid, oid: Guid) -> Self {
        Self { tid, oid }
    }

    pub fn tid(&self) -> &Guid {
        &self.tid
    }

    pub fn oid(&self) -> &Guid {
        &self.oid
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.tid, self.oid)
    }
}

impl FromStr for UserId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (tid, oid) = s.split_once(':').ok_or(IdError::BadShape)?;
        Ok(Self {
            tid: Guid::parse(tid)?,
            oid: Guid::parse(oid)?,
        })
    }
}

impl Serialize for UserId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for UserId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Defines an opaque token id: 1-128 characters from `[A-Za-z0-9._-]`.
macro_rules! token_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn parse(s: &str) -> Result<Self, IdError> {
                if s.is_empty() {
                    return Err(IdError::Empty);
                }
                if s.len() > 128 {
                    return Err(IdError::TooLong);
                }
                if !s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')) {
                    return Err(IdError::BadChar);
                }
                Ok(Self(s.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        string_id!($name);
    };
}

token_id!(
    /// Correlates a request across logs, audit and agent commands.
    RequestId
);
token_id!(
    /// Client-supplied key that makes a write safe to retry (D66).
    IdempotencyKey
);
token_id!(
    /// A draft created by MCP `propose_change`.
    DraftId
);

/// Defines a numeric database id.
macro_rules! numeric_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl From<i64> for $name {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

numeric_id!(
    /// Position of an event in the audit chain.
    AuditId
);
numeric_id!(ProposalId);
numeric_id!(DocId);
numeric_id!(FindingId);
numeric_id!(
    /// A PR link record (D77).
    PrLinkId
);
numeric_id!(
    /// A PR job (D77).
    PrJobId
);

/// A Kubernetes object name or namespace: an RFC 1123 subdomain, 1-253 characters.
fn k8s_name(s: &str) -> Result<(), IdError> {
    let b = s.as_bytes();
    if b.is_empty() {
        return Err(IdError::Empty);
    }
    if b.len() > 253 {
        return Err(IdError::TooLong);
    }
    let edge_ok = |c: u8| is_lower_alnum(c);
    if !edge_ok(b[0]) || !edge_ok(b[b.len() - 1]) {
        return Err(IdError::BadChar);
    }
    if !b.iter().all(|&c| is_lower_alnum(c) || c == b'-' || c == b'.') {
        return Err(IdError::BadChar);
    }
    Ok(())
}

/// A Deployment: namespace and name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ServiceRef {
    namespace: String,
    name: String,
}

impl ServiceRef {
    pub fn new(namespace: &str, name: &str) -> Result<Self, IdError> {
        k8s_name(namespace)?;
        k8s_name(name)?;
        Ok(Self {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl<'de> Deserialize<'de> for ServiceRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            namespace: String,
            name: String,
        }
        let raw = Raw::deserialize(d)?;
        Self::new(&raw.namespace, &raw.name).map_err(serde::de::Error::custom)
    }
}

/// A Kubernetes Job: name and uid (D72).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct JobRef {
    name: String,
    uid: String,
}

impl JobRef {
    pub fn new(name: &str, uid: &str) -> Result<Self, IdError> {
        k8s_name(name)?;
        if uid.is_empty() {
            return Err(IdError::Empty);
        }
        if uid.len() > 64 || !uid.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(IdError::BadChar);
        }
        Ok(Self {
            name: name.to_owned(),
            uid: uid.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn uid(&self) -> &str {
        &self.uid
    }
}

impl<'de> Deserialize<'de> for JobRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            name: String,
            uid: String,
        }
        let raw = Raw::deserialize(d)?;
        Self::new(&raw.name, &raw.uid).map_err(serde::de::Error::custom)
    }
}
