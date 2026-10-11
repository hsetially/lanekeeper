//! The hub's own certificate on its listeners (S5, S6): generated in memory with a fresh key, signed through KMS, renewed at
//! half its life.
//!
//! No certificate or key for the hub's TLS identity is stored anywhere. Each replica makes its own key at start-up, has the
//! CA sign a certificate for it exactly as it signs an agent's, and swaps in a fresh one at half the lifetime. A failed
//! renewal is retried with backoff and jitter; the old certificate keeps serving until it expires, and half a lifetime is
//! enough for dozens of attempts.

use std::sync::Arc;
use std::time::Duration;

use rcgen::{CertificateParams, KeyPair};
use rustls::sign::CertifiedKey;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::ca::profile::CertProfile;
use crate::ca::{CaError, CaIssuer, IssuedCertificate, parse_csr};
use crate::tls::{RotatingServerCert, TlsError, leaf_certified_key};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ServerIdentityError {
    #[error("the hub could not make a key for its certificate")]
    Key,
    #[error("the CA could not issue the hub's certificate")]
    Issue(#[from] CaError),
    #[error("the issued certificate cannot be used for TLS")]
    Tls(#[from] TlsError),
}

/// A certificate made now, for a key made now.
pub async fn issue_server_identity(
    ca: &CaIssuer,
    profile: &CertProfile,
) -> Result<(Arc<CertifiedKey>, IssuedCertificate), ServerIdentityError> {
    let key = KeyPair::generate().map_err(|_| ServerIdentityError::Key)?;
    // The hub asks for its own certificate the way an agent does, so there is one issuance path and one set of checks.
    let request = CertificateParams::default()
        .serialize_request(&key)
        .map_err(|_| ServerIdentityError::Key)?;
    let csr = parse_csr(request.der()).map_err(|_| ServerIdentityError::Key)?;
    let issued = ca.issue(profile, &csr).await?;
    let certified = leaf_certified_key(&key, &issued.chain_der)?;
    Ok((certified, issued))
}

/// How long to wait before renewing: half of the certificate's life from the moment it was issued.
#[must_use]
pub fn renewal_delay(issued: &IssuedCertificate) -> Duration {
    let life_ms = issued
        .not_after
        .unix_millis()
        .saturating_sub(issued.issued_at.unix_millis());
    Duration::from_millis(u64::try_from(life_ms / 2).unwrap_or(0))
}

/// The wait before retry number `attempt` (from 0) of a failed renewal: exponential from 30 seconds, capped at 30 minutes,
/// with full jitter.
fn retry_delay(attempt: u32) -> Duration {
    let cap = Duration::from_secs(30 * 60);
    let ceiling = Duration::from_secs(30)
        .saturating_mul(1u32 << attempt.min(10))
        .min(cap);
    ceiling.mul_f64(fastrand::f64().max(0.05))
}

/// Issue a new certificate into `cell` and say how long to wait before the next one: half of the new certificate's life.
pub async fn rotate_once(
    ca: &CaIssuer,
    profile: &CertProfile,
    cell: &RotatingServerCert,
) -> Result<Duration, ServerIdentityError> {
    let (certified, issued) = issue_server_identity(ca, profile).await?;
    cell.replace(certified);
    info!(serial = %issued.serial, "the hub's server certificate was renewed");
    Ok(renewal_delay(&issued))
}

/// Keep `cell` supplied with a fresh certificate until `stop` says stop. `first` is the certificate `cell` holds now.
pub fn spawn_rotation(
    ca: Arc<CaIssuer>,
    profile: CertProfile,
    cell: Arc<RotatingServerCert>,
    first: &IssuedCertificate,
    mut stop: watch::Receiver<bool>,
) -> JoinHandle<()> {
    let mut wait = renewal_delay(first);
    tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = stop.changed() => return,
            }
            match rotate_once(&ca, &profile, &cell).await {
                Ok(next) => {
                    wait = next;
                    attempt = 0;
                }
                Err(error) => {
                    warn!(%error, attempt, "renewing the hub's server certificate failed; trying again");
                    wait = retry_delay(attempt);
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delays_are_jittered_and_never_exceed_the_cap() {
        for attempt in 0..40 {
            let delay = retry_delay(attempt);
            assert!(delay <= Duration::from_secs(30 * 60), "{attempt}: {delay:?}");
            assert!(delay > Duration::ZERO);
        }
        // The first retry is within half a minute: a hiccup is retried soon.
        assert!(retry_delay(0) <= Duration::from_secs(30));
    }

    #[test]
    fn the_retry_ceiling_grows_with_the_attempt_number() {
        let first_ceiling = (0..200).map(|_| retry_delay(0)).max().unwrap_or_default();
        let later_ceiling = (0..200).map(|_| retry_delay(6)).max().unwrap_or_default();
        assert!(later_ceiling > first_ceiling);
    }
}
