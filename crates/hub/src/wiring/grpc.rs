//! The two certificate authorities and the hub's own certificates (plan 18, T3; S5, S6).
//!
//! The hub holds no private key for either CA. A CA is a Cloud KMS key version and the public certificate that `hub ca-init` made
//! for it. At start the hub reads each certificate from Secret Manager, has `CaIssuer::new` prove with a probe signature that KMS
//! signs under the certificate's key (a mismatch refuses to start: every certificate issued would fail to verify), and has the CA
//! issue the replica's own server certificate, which a rotation task then renews at half its life.
//!
//! Two CAs, so that an agent's certificate is never accepted as a replica's: the agent listener trusts the agent CA and presents a
//! certificate from it; the replica listener trusts the replica CA only. The two setups run at the same time, each under the start
//! timeout, because the library's own probe allows 30 seconds and the sum of four KMS calls must not eat P11's ten.

use std::sync::Arc;
use std::time::Duration;

use hub_platform::ca::profile::{CertProfile, ProfileError};
use hub_platform::ca::{CaError, CaIssuer, IssuedCertificate};
use hub_platform::server_identity::{ServerIdentityError, issue_server_identity};
use hub_platform::tls::RotatingServerCert;
use ports::SecretError;
use tracing::error;

use crate::config::{CaSettings, PlatformSettings};
use crate::edges::Edges;
use crate::web_tls::{PemError, decode_ca_certificate};

/// Which of the two authorities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaKind {
    Agent,
    Replica,
}

impl CaKind {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Replica => "replica",
        }
    }

    /// The settings that name this CA, for the message that says it is not usable.
    #[must_use]
    pub fn variables(self) -> &'static str {
        match self {
            Self::Agent => "LK_AGENT_CA_KMS_REF, LK_AGENT_CA_CERT_REF",
            Self::Replica => "LK_REPLICA_CA_KMS_REF, LK_REPLICA_CA_CERT_REF",
        }
    }
}

/// What went wrong with a CA. Every message is fixed text: none carries a name, a key or a certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CaStep {
    #[error("the CA certificate secret could not be read: {0}")]
    Secret(SecretError),
    #[error("the CA certificate cannot be used: {0}")]
    Pem(PemError),
    #[error("{0}")]
    Issuer(CaError),
    #[error("the hub's own certificate could not be issued: {0}")]
    Server(ServerIdentityError),
    #[error("the hub's own certificate profile is not valid: {0}")]
    Profile(ProfileError),
    #[error("setting it up took too long")]
    TimedOut,
}

/// One authority, ready: the issuer, and this replica's server certificate from it.
pub struct Ca {
    pub issuer: Arc<CaIssuer>,
    /// This replica's server certificate from this CA, renewed in place by the rotation task.
    pub cert: Arc<RotatingServerCert>,
    /// The certificate in `cert` when it was issued, for the rotation's first wait.
    pub issued: IssuedCertificate,
    pub profile: CertProfile,
    /// The CA certificate, DER: the root peers are trusted under.
    pub root_der: Vec<u8>,
}

impl std::fmt::Debug for Ca {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ca")
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

/// Both authorities.
#[derive(Debug)]
pub struct Cas {
    pub agent: Ca,
    pub replica: Ca,
}

impl Ca {
    async fn load(
        ca: &CaSettings,
        profile: Result<CertProfile, ProfileError>,
        edges: &Edges,
    ) -> Result<Self, CaStep> {
        let profile = profile.map_err(CaStep::Profile)?;
        let secret = edges.secrets.get(&ca.cert_ref).await.map_err(CaStep::Secret)?;
        let root_der = decode_ca_certificate(&secret).map_err(CaStep::Pem)?;
        // The probe: KMS signs a digest and the signature must verify under the certificate's key.
        let authority = CaIssuer::new(Arc::clone(&edges.signer), ca.kms_ref.clone(), root_der.clone())
            .await
            .map_err(CaStep::Issuer)?
            .with_clock(Arc::clone(&edges.clock));
        let authority = Arc::new(authority);
        let (certified, issued) = issue_server_identity(&authority, &profile)
            .await
            .map_err(CaStep::Server)?;
        Ok(Self {
            issuer: authority,
            cert: RotatingServerCert::new(certified),
            issued,
            profile,
            root_der,
        })
    }
}

/// `load`, or `TimedOut` when it takes longer than `within`.
async fn bounded(
    within: Duration,
    load: impl std::future::Future<Output = Result<Ca, CaStep>>,
) -> Result<Ca, CaStep> {
    tokio::time::timeout(within, load)
        .await
        .unwrap_or(Err(CaStep::TimedOut))
}

/// Set up both authorities at the same time, each under `within`. When both fail, both are logged and the agent CA's is returned:
/// two failures at once are usually one cause (KMS or Secret Manager unreachable), and the first symptom alone would send a
/// person the wrong way.
pub async fn prepare_cas(
    settings: &PlatformSettings,
    edges: &Edges,
    within: Duration,
) -> Result<Cas, crate::wiring::PlatformError> {
    use crate::wiring::PlatformError;

    let agent_profile = CertProfile::hub_server(&settings.replica_name, &settings.agent_hosts);
    let replica_profile = CertProfile::replica(
        &settings.replica_name,
        &[settings.replica_address.host().to_owned()],
    );
    let (agent, replica) = tokio::join!(
        bounded(within, Ca::load(&settings.agent_ca, agent_profile, edges)),
        bounded(within, Ca::load(&settings.replica_ca, replica_profile, edges)),
    );
    match (agent, replica) {
        (Ok(agent), Ok(replica)) => Ok(Cas { agent, replica }),
        (Err(agent), Err(replica)) => {
            error!(
                ca = CaKind::Replica.name(),
                error = %PlatformError::Ca { kind: CaKind::Replica, step: replica },
                "the hub could not start"
            );
            Err(PlatformError::Ca {
                kind: CaKind::Agent,
                step: agent,
            })
        }
        (Err(step), Ok(_)) => Err(PlatformError::Ca {
            kind: CaKind::Agent,
            step,
        }),
        (Ok(_), Err(step)) => Err(PlatformError::Ca {
            kind: CaKind::Replica,
            step,
        }),
    }
}
