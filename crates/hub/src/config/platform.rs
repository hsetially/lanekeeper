//! The platform's settings (plan 18, T3, G10; S5, S6, S7): the two certificate authorities, this replica's identity, the name
//! agents are told to ask Google for a token for, and the names agents reach the hub by.
//!
//! The hub holds no private key for either CA: a CA is a Cloud KMS key version (`*_CA_KMS_REF`, a resource name) and the CA
//! certificate that `hub ca-init` made for it (`*_CA_CERT_REF`, the name of a Secret Manager secret; the certificate is public).
//! None of these names looks like a secret to the pod contract, and none is one.
//!
//! The group is all or nothing, as the web certificate is: a pod that sets some of it is a half-written deployment and is
//! refused by name; a process built from the library's defaults has none, and `require_serving` refuses that.

use std::net::SocketAddr;

use domain::ShortText;
use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::gateway::forward::PeerAddress;
use hub_platform::gcp::wire::CryptoKeyVersionName;
use ports::KeyRef;

use super::ConfigError;
use super::reader::Reader;

/// Where the agent listener binds unless `LK_AGENT_ADDR` says otherwise (HARDENING.md: the `agent` port).
pub const DEFAULT_AGENT_ADDR: &str = "0.0.0.0:8444";
/// Where the replica listener binds unless `LK_REPLICA_ADDR` says otherwise (the `replica` port).
pub const DEFAULT_REPLICA_ADDR: &str = "0.0.0.0:8445";

/// One certificate authority: the KMS key that signs for it and its certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaSettings {
    /// A Cloud KMS crypto key version (`projects/P/locations/L/keyRings/R/cryptoKeys/K/cryptoKeyVersions/N`).
    pub kms_ref: KeyRef,
    /// The Secret Manager secret that holds the CA certificate as PEM.
    pub cert_ref: String,
}

#[derive(Debug, Clone)]
pub struct PlatformSettings {
    /// The agent and sentinel listener (mTLS).
    pub agent_addr: SocketAddr,
    /// The replica-to-replica listener (mTLS).
    pub replica_addr: SocketAddr,
    /// The CA that issues agent, sentinel and agent-listener certificates.
    pub agent_ca: CaSettings,
    /// The CA that issues the replicas' own certificates, so an agent certificate is never accepted as a replica's.
    pub replica_ca: CaSettings,
    /// This replica's stable name: the `StatefulSet` pod name. It is in the certificate's identity and in `agent_connections`.
    pub replica_name: PeerName,
    /// Where the other replicas dial this one: `host:port`, the pod's name behind the headless service. The host is the DNS
    /// name on the replica certificate.
    pub replica_address: PeerAddress,
    /// The audience a Google ID token must carry to join: the hub's own name.
    pub audience: ShortText,
    /// The DNS names agents reach the agent endpoint by (D25). They go on the agent listener's certificate.
    pub agent_hosts: Vec<String>,
}

impl PlatformSettings {
    /// `None` when none of the group is set. The listeners' addresses have defaults, so they do not count.
    pub(super) fn read(reader: &Reader<'_>) -> Result<Option<Self>, ConfigError> {
        let agent_addr = reader.addr("LK_AGENT_ADDR", DEFAULT_AGENT_ADDR)?;
        let replica_addr = reader.addr("LK_REPLICA_ADDR", DEFAULT_REPLICA_ADDR)?;
        let kms = |name: &'static str| -> Result<Option<KeyRef>, ConfigError> {
            let Some(text) = reader.parse(name)? else {
                return Ok(None);
            };
            let key = KeyRef::parse(&text).map_err(|_| ConfigError::Invalid(name))?;
            // The version, not the key: a signature is made by one version, and the CA certificate belongs to it.
            CryptoKeyVersionName::parse(&key).ok_or(ConfigError::Invalid(name))?;
            Ok(Some(key))
        };
        let agent_kms = kms("LK_AGENT_CA_KMS_REF")?;
        let agent_cert = reader.secret_ref("LK_AGENT_CA_CERT_REF")?;
        let replica_kms = kms("LK_REPLICA_CA_KMS_REF")?;
        let replica_cert = reader.secret_ref("LK_REPLICA_CA_CERT_REF")?;
        let replica_name = reader
            .parse("LK_REPLICA_NAME")?
            .map(|text| PeerName::parse(&text).map_err(|_| ConfigError::Invalid("LK_REPLICA_NAME")))
            .transpose()?;
        let replica_address = reader
            .parse("LK_REPLICA_ADDRESS")?
            .map(|text| {
                let address =
                    PeerAddress::parse(&text).map_err(|_| ConfigError::Invalid("LK_REPLICA_ADDRESS"))?;
                // Its host is the DNS name on this replica's certificate, which the CA's profile only takes in lower case: a host
                // that the address accepts and the certificate refuses would be found at start, after the CA was reached.
                let probe =
                    PeerName::parse("probe").map_err(|_| ConfigError::Invalid("LK_REPLICA_ADDRESS"))?;
                CertProfile::replica(&probe, &[address.host().to_owned()])
                    .map_err(|_| ConfigError::Invalid("LK_REPLICA_ADDRESS"))?;
                Ok::<_, ConfigError>(address)
            })
            .transpose()?;
        let audience = reader
            .parse("LK_HUB_AUDIENCE")?
            .map(|text| ShortText::parse(&text).map_err(|_| ConfigError::Invalid("LK_HUB_AUDIENCE")))
            .transpose()?;
        let agent_hosts = reader
            .parse("LK_AGENT_ENDPOINT_HOSTS")?
            .map(|text| hosts(&text))
            .transpose()?;

        if agent_kms.is_none()
            && agent_cert.is_none()
            && replica_kms.is_none()
            && replica_cert.is_none()
            && replica_name.is_none()
            && replica_address.is_none()
            && audience.is_none()
            && agent_hosts.is_none()
        {
            return Ok(None);
        }
        Ok(Some(Self {
            agent_addr,
            replica_addr,
            agent_ca: CaSettings {
                kms_ref: agent_kms.ok_or(ConfigError::Missing("LK_AGENT_CA_KMS_REF"))?,
                cert_ref: agent_cert.ok_or(ConfigError::Missing("LK_AGENT_CA_CERT_REF"))?,
            },
            replica_ca: CaSettings {
                kms_ref: replica_kms.ok_or(ConfigError::Missing("LK_REPLICA_CA_KMS_REF"))?,
                cert_ref: replica_cert.ok_or(ConfigError::Missing("LK_REPLICA_CA_CERT_REF"))?,
            },
            replica_name: replica_name.ok_or(ConfigError::Missing("LK_REPLICA_NAME"))?,
            replica_address: replica_address.ok_or(ConfigError::Missing("LK_REPLICA_ADDRESS"))?,
            audience: audience.ok_or(ConfigError::Missing("LK_HUB_AUDIENCE"))?,
            agent_hosts: agent_hosts.ok_or(ConfigError::Missing("LK_AGENT_ENDPOINT_HOSTS"))?,
        }))
    }
}

/// A comma-separated list of DNS names, at least one and at most what one certificate carries. Each is checked by the profile
/// that will put it on the certificate, so the settings cannot accept a name the CA would refuse later.
fn hosts(text: &str) -> Result<Vec<String>, ConfigError> {
    // An empty entry is a stray comma, which is a typo to refuse and not a name to skip.
    let names: Vec<String> = text.split(',').map(|name| name.trim().to_owned()).collect();
    let probe = PeerName::parse("probe").map_err(|_| ConfigError::Invalid("LK_AGENT_ENDPOINT_HOSTS"))?;
    CertProfile::hub_server(&probe, &names).map_err(|_| ConfigError::Invalid("LK_AGENT_ENDPOINT_HOSTS"))?;
    Ok(names)
}
