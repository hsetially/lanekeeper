//! `Join` (S5, S9): an agent proves who it is and gets a 24-hour client certificate for exactly that identity.
//!
//! ```text
//!   rate limit -> read the CSR -> check the credential -> issue the certificate (KMS) -> one transaction:
//!       consume the join token (token path), record the agent's certificate, write the audit event -> commit
//! ```
//!
//! The certificate is issued before the transaction opens, so no database transaction is held across the KMS call. If the
//! transaction fails (a second join consumed the token first, the audit write failed), the certificate is dropped unsent: an
//! agent never holds a certificate whose join was not recorded.
//!
//! Every refusal reaches the caller as the same `Unauthenticated` status with no message; the reason is a code in the log.

pub mod google;
pub mod limits;
pub mod token;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{ShortText, SwimlaneId, Timestamp};
use ports::{AuditAction, AuditActor, AuditEvent, AuditLog, AuthError, TokenVerifier, Tx};
use proto::convert::{IssuedCert, JoinCredential, JoinParams, JoinSubject};
use sqlx::PgPool;
use tracing::{debug, warn};
use zeroize::Zeroize;

use crate::agent_metrics::{AgentMetrics, CredentialKind, JoinOutcome};
use crate::ca::{CaError, CaIssuer, CertProfile, parse_csr};
use crate::clock::Clock;
pub use limits::{JoinLimiter, JoinLimits};

/// A whole join, from the request to the certificate (rule 5).
const JOIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Why a join did not succeed. The caller sees [`JoinError::status_code`] and nothing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JoinError {
    /// The credential, the request or the identity was not acceptable. `code` says why, for the log only.
    #[error("join refused")]
    Refused(Refusal),
    #[error("too many join attempts")]
    RateLimited,
    /// Something the hub depends on is down; retrying later may work.
    #[error("join is unavailable")]
    Unavailable,
}

/// The log code of a refusal. Never sent to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    WrongKind,
    BadRequest,
    BadIdToken,
    UnknownOrUnregisteredSwimlane,
    EmailMismatch,
    NoLiveToken,
    TokenAlreadyUsed,
}

impl Refusal {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::WrongKind => "join_wrong_kind",
            Self::BadRequest => "join_bad_request",
            Self::BadIdToken => "join_bad_id_token",
            Self::UnknownOrUnregisteredSwimlane => "join_swimlane_unknown_or_unregistered",
            Self::EmailMismatch => "join_email_mismatch",
            Self::NoLiveToken => "join_no_live_token",
            Self::TokenAlreadyUsed => "join_token_already_used",
        }
    }
}

impl JoinError {
    /// The gRPC code the caller gets. Credential and identity failures are all `Unauthenticated`.
    #[must_use]
    pub fn status_code(self) -> tonic::Code {
        match self {
            Self::Refused(_) => tonic::Code::Unauthenticated,
            Self::RateLimited => tonic::Code::ResourceExhausted,
            Self::Unavailable => tonic::Code::Unavailable,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JoinSettings {
    /// The audience Google ID tokens must carry: the hub's own name.
    pub hub_audience: ShortText,
    pub limits: JoinLimits,
}

pub struct JoinService {
    pool: PgPool,
    verifier: Arc<dyn TokenVerifier>,
    audit: Arc<dyn AuditLog>,
    ca: Arc<CaIssuer>,
    clock: Arc<dyn Clock>,
    audience: ShortText,
    limiter: JoinLimiter,
    metrics: AgentMetrics,
}

impl std::fmt::Debug for JoinService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinService").finish_non_exhaustive()
    }
}

impl JoinService {
    #[must_use]
    pub fn new(
        pool: PgPool,
        verifier: Arc<dyn TokenVerifier>,
        audit: Arc<dyn AuditLog>,
        ca: Arc<CaIssuer>,
        clock: Arc<dyn Clock>,
        settings: JoinSettings,
    ) -> Self {
        Self {
            pool,
            verifier,
            audit,
            ca,
            clock,
            audience: settings.hub_audience,
            limiter: JoinLimiter::new(settings.limits),
            metrics: AgentMetrics::default(),
        }
    }

    /// Count joins in `metrics`, which the hub registers in its exposition.
    #[must_use]
    pub fn with_metrics(mut self, metrics: AgentMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// Join as an agent. `remote` is the peer's address, for the per-address limit.
    pub async fn join_agent(
        &self,
        remote: Option<IpAddr>,
        params: JoinParams,
    ) -> Result<IssuedCert, JoinError> {
        let credential = match &params.credential {
            JoinCredential::GoogleIdToken(_) => CredentialKind::GoogleIdToken,
            JoinCredential::JoinToken(_) => CredentialKind::JoinToken,
        };
        let result = match tokio::time::timeout(JOIN_TIMEOUT, self.join_agent_inner(remote, params)).await {
            Ok(result) => result,
            Err(_elapsed) => Err(JoinError::Unavailable),
        };
        match &result {
            Ok(_) => {
                debug!("an agent joined");
                self.metrics.join(credential, JoinOutcome::Issued);
            }
            Err(JoinError::Refused(reason)) => {
                warn!(code = reason.code(), "join refused");
                self.metrics.join(credential, JoinOutcome::Refused);
            }
            Err(JoinError::RateLimited) => {
                warn!(code = "join_rate_limited", "join refused");
                self.metrics.join(credential, JoinOutcome::RateLimited);
            }
            Err(JoinError::Unavailable) => {
                warn!(code = "join_unavailable", "join failed");
                self.metrics.join(credential, JoinOutcome::Unavailable);
            }
        }
        result
    }

    async fn join_agent_inner(
        &self,
        remote: Option<IpAddr>,
        params: JoinParams,
    ) -> Result<IssuedCert, JoinError> {
        let JoinParams {
            subject,
            csr_der,
            credential,
        } = params;
        let JoinSubject::Agent(swimlane) = subject else {
            return Err(JoinError::Refused(Refusal::WrongKind));
        };
        if !self.limiter.allow(swimlane.as_str(), remote) {
            return Err(JoinError::RateLimited);
        }
        let csr = parse_csr(&csr_der).map_err(|_| JoinError::Refused(Refusal::BadRequest))?;

        // The credential decides; nothing is written yet.
        let token_id = match &credential {
            JoinCredential::GoogleIdToken(jwt) => {
                self.check_google(&swimlane, jwt.expose()).await?;
                None
            }
            JoinCredential::JoinToken(presented) => {
                let mut hash = token::hash_token(presented);
                let found = token::find_live(&self.pool, swimlane.as_str(), &hash).await;
                hash.zeroize();
                match found {
                    Ok(id) => Some(id),
                    Err(token::TokenError::NoMatch) => return Err(JoinError::Refused(Refusal::NoLiveToken)),
                    Err(token::TokenError::Unavailable) => return Err(JoinError::Unavailable),
                }
            }
        };
        drop(credential);

        let issued = self
            .ca
            .issue(&CertProfile::agent(&swimlane), &csr)
            .await
            .map_err(ca_error)?;

        // One transaction: the token use, the agent's certificate and the audit event commit together or not at all.
        let mut tx = self.pool.begin().await.map_err(|_| JoinError::Unavailable)?;
        if let Some(id) = token_id {
            match token::consume(&mut tx, id).await {
                Ok(true) => {}
                Ok(false) => return Err(JoinError::Refused(Refusal::TokenAlreadyUsed)),
                Err(_) => return Err(JoinError::Unavailable),
            }
        }
        record_certificate(&mut tx, &swimlane, &issued.serial, issued.not_after, true)
            .await
            .map_err(|_| JoinError::Unavailable)?;
        let event = AuditEvent {
            swimlane: Some(swimlane.clone()),
            ..AuditEvent::new(
                self.clock.now(),
                AuditActor::Agent {
                    swimlane: swimlane.clone(),
                },
                AuditAction::AgentJoined,
                ports::AuditVia::System,
            )
        };
        self.audit
            .record(&mut Tx::from_pg(&mut tx), event)
            .await
            .map_err(|_| JoinError::Unavailable)?;
        tx.commit().await.map_err(|_| JoinError::Unavailable)?;

        Ok(IssuedCert {
            cert_chain_der: issued.chain_der,
            not_after: issued.not_after,
        })
    }

    async fn check_google(&self, swimlane: &SwimlaneId, jwt: &str) -> Result<(), JoinError> {
        let identity = self
            .verifier
            .google_id_token(jwt, self.audience.as_str())
            .await
            .map_err(|error| match error {
                AuthError::Unavailable => JoinError::Unavailable,
                _ => JoinError::Refused(Refusal::BadIdToken),
            })?;
        let registered = google::registered_service_account(&self.pool, swimlane.as_str())
            .await
            .map_err(|_| JoinError::Unavailable)?;
        if google::email_matches(&identity, registered.as_deref()) {
            Ok(())
        } else if registered.as_deref().is_none_or(str::is_empty) {
            Err(JoinError::Refused(Refusal::UnknownOrUnregisteredSwimlane))
        } else {
            Err(JoinError::Refused(Refusal::EmailMismatch))
        }
    }

    /// Renew the certificate of an agent whose stream is already authenticated by its current certificate (S5). The identity
    /// is the stream's, never the request's; the request contributes its key.
    pub async fn renew_agent(&self, swimlane: &SwimlaneId, csr_der: &Bytes) -> Result<IssuedCert, JoinError> {
        let csr = parse_csr(csr_der).map_err(|_| JoinError::Refused(Refusal::BadRequest))?;
        let issued = self
            .ca
            .issue(&CertProfile::agent(swimlane), &csr)
            .await
            .map_err(ca_error)?;
        let mut tx = self.pool.begin().await.map_err(|_| JoinError::Unavailable)?;
        record_certificate(&mut tx, swimlane, &issued.serial, issued.not_after, false)
            .await
            .map_err(|_| JoinError::Unavailable)?;
        let event = AuditEvent {
            swimlane: Some(swimlane.clone()),
            ..AuditEvent::new(
                self.clock.now(),
                AuditActor::Agent {
                    swimlane: swimlane.clone(),
                },
                AuditAction::AgentCertificateRenewed,
                ports::AuditVia::System,
            )
        };
        self.audit
            .record(&mut Tx::from_pg(&mut tx), event)
            .await
            .map_err(|_| JoinError::Unavailable)?;
        tx.commit().await.map_err(|_| JoinError::Unavailable)?;
        Ok(IssuedCert {
            cert_chain_der: issued.chain_der,
            not_after: issued.not_after,
        })
    }
}

fn ca_error(error: CaError) -> JoinError {
    match error {
        CaError::Profile(_) => JoinError::Refused(Refusal::BadRequest),
        _ => JoinError::Unavailable,
    }
}

/// Record the agent's current certificate (A20): `joined_at` on a join, the serial and expiry always.
async fn record_certificate(
    tx: &mut sqlx::PgConnection,
    swimlane: &SwimlaneId,
    serial: &str,
    not_after: Timestamp,
    joined: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agents (swimlane_id, joined_at, cert_serial, cert_expires_at) \
         VALUES ($1, CASE WHEN $4 THEN now() END, $2, to_timestamp($3::double precision / 1000.0)) \
         ON CONFLICT (swimlane_id) DO UPDATE SET \
           joined_at = CASE WHEN $4 THEN now() ELSE agents.joined_at END, \
           cert_serial = excluded.cert_serial, cert_expires_at = excluded.cert_expires_at",
    )
    .bind(swimlane.as_str())
    .bind(serial)
    .bind(not_after.unix_millis())
    .bind(joined)
    .execute(tx)
    .await?;
    Ok(())
}
