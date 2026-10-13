//! What a certificate says (S5, S6): the identity in the SAN, how long it lives and what it may be used for.
//!
//! The requester never decides any of this. The hub builds the profile from what it verified (the swimlane whose credential
//! was checked, the sentinel's registered name, its own replica id), and the CSR contributes only a public key.
//!
//! | Kind | SAN | EKU |
//! |---|---|---|
//! | Agent | URI `spiffe://lanekeeper/swimlane/<id>` | client auth |
//! | Sentinel | URI `spiffe://lanekeeper/sentinel/<name>` | client auth |
//! | Hub server | URI `spiffe://lanekeeper/hub/<replica>` and DNS names | server auth |
//! | Replica | URI `spiffe://lanekeeper/hub/<replica>` and DNS names | client auth and server auth |
//!
//! Every certificate is for a key that is not a CA, lives 24 hours and is backdated five minutes for clock skew.

use std::fmt;
use std::time::Duration;

use domain::{SwimlaneId, Timestamp};
use rcgen::string::Ia5String;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose, SanType,
    SerialNumber,
};
use time::OffsetDateTime;

/// Every agent certificate carries exactly one URI SAN: this prefix followed by the swimlane id. The agent checks the same
/// string at join, so a disagreement shows up there.
pub const AGENT_SAN_PREFIX: &str = "spiffe://lanekeeper/swimlane/";
pub const SENTINEL_SAN_PREFIX: &str = "spiffe://lanekeeper/sentinel/";
pub const HUB_SAN_PREFIX: &str = "spiffe://lanekeeper/hub/";

/// How long a certificate is valid (S5).
pub const CERT_LIFETIME: Duration = Duration::from_secs(24 * 3600);
/// `notBefore` is this long before issuance, because machines disagree about the time by seconds.
pub const BACKDATE: Duration = Duration::from_secs(5 * 60);

/// A name that goes into a URI SAN: `[A-Za-z0-9][A-Za-z0-9._-]{0,252}`. Nothing that could end the path segment, start a
/// query or smuggle another identity in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerName(String);

impl PeerName {
    pub const MAX_LEN: usize = 253;

    pub fn parse(s: &str) -> Result<Self, ProfileError> {
        let bytes = s.as_bytes();
        let first_ok = bytes.first().is_some_and(u8::is_ascii_alphanumeric);
        let rest_ok = bytes
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
        if bytes.is_empty() || bytes.len() > Self::MAX_LEN || !first_ok || !rest_ok {
            return Err(ProfileError::BadName);
        }
        Ok(Self(s.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("the name cannot be part of an identity")]
    BadName,
    #[error("a server certificate needs at least one DNS name")]
    NoDnsName,
    #[error("a DNS name is not valid")]
    BadDnsName,
    #[error("a certificate carries at most {MAX_DNS_NAMES} DNS names")]
    TooManyDnsNames,
}

/// The most DNS names one certificate carries.
pub const MAX_DNS_NAMES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CertKind {
    Agent,
    Sentinel,
    HubServer,
    Replica,
}

/// The identity and use of one certificate to be issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertProfile {
    kind: CertKind,
    uri: String,
    dns: Vec<String>,
}

impl CertProfile {
    /// The agent of `swimlane`: a client certificate only.
    #[must_use]
    pub fn agent(swimlane: &SwimlaneId) -> Self {
        Self {
            kind: CertKind::Agent,
            uri: format!("{AGENT_SAN_PREFIX}{}", swimlane.as_str()),
            dns: Vec::new(),
        }
    }

    /// The sentinel daemon of the NFS VM `name`: a client certificate only.
    #[must_use]
    pub fn sentinel(name: &PeerName) -> Self {
        Self {
            kind: CertKind::Sentinel,
            uri: format!("{SENTINEL_SAN_PREFIX}{name}"),
            dns: Vec::new(),
        }
    }

    /// The hub's own certificate on the agent listener: a server certificate for the names agents connect to.
    pub fn hub_server(replica: &PeerName, dns_names: &[String]) -> Result<Self, ProfileError> {
        Ok(Self {
            kind: CertKind::HubServer,
            uri: format!("{HUB_SAN_PREFIX}{replica}"),
            dns: checked_dns(dns_names)?,
        })
    }

    /// A hub replica, to its peers: both a client and a server certificate.
    pub fn replica(replica: &PeerName, dns_names: &[String]) -> Result<Self, ProfileError> {
        Ok(Self {
            kind: CertKind::Replica,
            uri: format!("{HUB_SAN_PREFIX}{replica}"),
            dns: checked_dns(dns_names)?,
        })
    }

    #[must_use]
    pub fn kind(&self) -> CertKind {
        self.kind
    }

    /// The URI SAN, which is the identity.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// The certificate's fields as of `now`, with a serial number the caller made at random.
    pub(crate) fn params(
        &self,
        now: Timestamp,
        serial: SerialNumber,
    ) -> Result<CertificateParams, ProfileError> {
        let at = |ms: i64| {
            OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).map_err(|_| ProfileError::BadName)
        };
        let lifetime_ms = i64::try_from(CERT_LIFETIME.as_millis()).unwrap_or(i64::MAX);
        let backdate_ms = i64::try_from(BACKDATE.as_millis()).unwrap_or(i64::MAX);
        let mut params = CertificateParams::default();
        params.not_before = at(now.unix_millis().saturating_sub(backdate_ms))?;
        params.not_after = at(now.unix_millis().saturating_add(lifetime_ms))?;
        params.serial_number = Some(serial);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, self.common_name());
        params.distinguished_name = dn;
        let mut names = vec![SanType::URI(
            Ia5String::try_from(self.uri.clone()).map_err(|_| ProfileError::BadName)?,
        )];
        for dns in &self.dns {
            names.push(SanType::DnsName(
                Ia5String::try_from(dns.clone()).map_err(|_| ProfileError::BadDnsName)?,
            ));
        }
        params.subject_alt_names = names;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = match self.kind {
            CertKind::Agent | CertKind::Sentinel => vec![ExtendedKeyUsagePurpose::ClientAuth],
            CertKind::HubServer => vec![ExtendedKeyUsagePurpose::ServerAuth],
            CertKind::Replica => vec![
                ExtendedKeyUsagePurpose::ServerAuth,
                ExtendedKeyUsagePurpose::ClientAuth,
            ],
        };
        params.use_authority_key_identifier_extension = true;
        Ok(params)
    }

    fn common_name(&self) -> &'static str {
        match self.kind {
            CertKind::Agent => "lanekeeper agent",
            CertKind::Sentinel => "lanekeeper sentinel",
            CertKind::HubServer => "lanekeeper hub",
            CertKind::Replica => "lanekeeper hub replica",
        }
    }
}

/// Lower-case hostnames only: letters, digits, hyphens and dots, labels of 1 to 63, 253 in all, no wildcard.
fn checked_dns(names: &[String]) -> Result<Vec<String>, ProfileError> {
    if names.is_empty() {
        return Err(ProfileError::NoDnsName);
    }
    if names.len() > MAX_DNS_NAMES {
        return Err(ProfileError::TooManyDnsNames);
    }
    for name in names {
        let labels_ok = name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        });
        if name.is_empty() || name.len() > 253 || !labels_ok {
            return Err(ProfileError::BadDnsName);
        }
    }
    Ok(names.to_vec())
}
