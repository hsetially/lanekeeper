//! The client certificate and the checks that make it safe to store and use (S5).
//!
//! The hub issues the certificate, so the agent trusts it only as far as it has to: it must be for our own key, its
//! lifetime must be bounded, and its identity must be exactly `spiffe://lanekeeper/swimlane/<our swimlane>`. A hub (or
//! anything standing in for it) that issues anything else is refused at join, loudly, instead of failing at the first
//! handshake. The chain's signature is not checked here: the hub's TLS certificate is checked against the pinned CA
//! when the agent connects, and the hub checks our certificate against its own CA on every stream.

use std::fmt;

use bytes::Bytes;
use domain::{SwimlaneId, Timestamp};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

use super::error::CertProblem;
use super::key::KeyMaterial;

/// Every agent certificate carries exactly one URI SAN: this prefix followed by the swimlane id (S5, decision A5).
///
/// This is the one place the form is written down. The hub (prompt 03b) issues it and the agent checks it, so a
/// disagreement shows up at join.
pub const AGENT_SAN_PREFIX: &str = "spiffe://lanekeeper/swimlane/";

/// The longest lifetime accepted: 24 hours (S5) plus an hour for the hub to backdate `notBefore` and for rounding.
pub const MAX_LIFETIME_SECS: i64 = 25 * 3600;

/// A certificate whose `notBefore` is this far ahead of our clock is still accepted: nodes disagree by seconds.
const CLOCK_SKEW_SECS: i64 = 5 * 60;

/// A certificate chain for our key that passed [`ClientIdentity::verify`], with the key it belongs to.
pub struct ClientIdentity {
    swimlane: SwimlaneId,
    key: KeyMaterial,
    chain: Vec<Bytes>,
    not_before: Timestamp,
    not_after: Timestamp,
}

impl ClientIdentity {
    /// Check an issued chain (leaf first) against our key and swimlane, as of `now`.
    ///
    /// The key is consumed: on success it becomes part of the identity, on failure it is wiped.
    pub fn verify(
        key: KeyMaterial,
        chain_der: Vec<Bytes>,
        swimlane: &SwimlaneId,
        now: Timestamp,
    ) -> Result<Self, CertProblem> {
        let (not_before, not_after) = check(&key, &chain_der, swimlane, now)?;
        Ok(Self {
            swimlane: swimlane.clone(),
            key,
            chain: chain_der,
            not_before: Timestamp::from_unix_millis(not_before.saturating_mul(1000)),
            not_after: Timestamp::from_unix_millis(not_after.saturating_mul(1000)),
        })
    }

    pub fn swimlane(&self) -> &SwimlaneId {
        &self.swimlane
    }

    pub fn key(&self) -> &KeyMaterial {
        &self.key
    }

    /// The certificates, DER, leaf first.
    pub fn chain_der(&self) -> &[Bytes] {
        &self.chain
    }

    pub fn not_before(&self) -> Timestamp {
        self.not_before
    }

    pub fn not_after(&self) -> Timestamp {
        self.not_after
    }
}

impl fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientIdentity")
            .field("swimlane", &self.swimlane.as_str())
            .field("certificates", &self.chain.len())
            .field("not_before_ms", &self.not_before.unix_millis())
            .field("not_after_ms", &self.not_after.unix_millis())
            .field("key", &format_args!("[redacted]"))
            .finish()
    }
}

/// The validity window of the leaf, in Unix seconds, once every check has passed.
fn check(
    key: &KeyMaterial,
    chain_der: &[Bytes],
    swimlane: &SwimlaneId,
    now: Timestamp,
) -> Result<(i64, i64), CertProblem> {
    let leaf_der = chain_der.first().ok_or(CertProblem::EmptyChain)?;
    // The whole chain is stored and later presented, so every certificate in it must parse.
    for der in chain_der {
        parse(der)?;
    }
    let leaf = parse(leaf_der)?;

    if leaf.public_key().raw != key.spki_der() {
        return Err(CertProblem::WrongKey);
    }

    let mut uris = Vec::new();
    let san = leaf
        .subject_alternative_name()
        .map_err(|_| CertProblem::Unparseable)?;
    if let Some(san) = san {
        for name in &san.value.general_names {
            if let GeneralName::URI(uri) = name {
                uris.push(*uri);
            }
        }
    }
    match uris.as_slice() {
        [] => return Err(CertProblem::NoUriSan),
        [uri] => {
            if uri.strip_prefix(AGENT_SAN_PREFIX) != Some(swimlane.as_str()) {
                return Err(CertProblem::WrongIdentity);
            }
        }
        _ => return Err(CertProblem::SeveralUriSans),
    }

    let validity = leaf.validity();
    let (not_before, not_after) = (validity.not_before.timestamp(), validity.not_after.timestamp());
    if not_after <= not_before {
        return Err(CertProblem::NoLifetime);
    }
    if not_after.saturating_sub(not_before) > MAX_LIFETIME_SECS {
        return Err(CertProblem::LifetimeTooLong);
    }
    let now_s = now.unix_millis().div_euclid(1000);
    if now_s >= not_after {
        return Err(CertProblem::Expired);
    }
    if not_before > now_s.saturating_add(CLOCK_SKEW_SECS) {
        return Err(CertProblem::NotYetValid);
    }
    Ok((not_before, not_after))
}

/// One DER certificate and nothing after it.
fn parse(der: &[u8]) -> Result<X509Certificate<'_>, CertProblem> {
    match X509Certificate::from_der(der) {
        Ok(([], cert)) => Ok(cert),
        _ => Err(CertProblem::Unparseable),
    }
}
