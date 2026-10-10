//! Joining the hub and keeping the certificate fresh (S5).
//!
//! [`Joiner`] owns the whole life of the client certificate:
//!
//! 1. **Join** ([`Joiner::join`]): make a key and a CSR in memory, get a credential (a Google ID token from the metadata
//!    server, or a join token as the fallback), call `Join` on the hub, check the certificate that comes back, and store
//!    it. `Join` needs no client certificate, so it works when there is no working certificate.
//! 2. **Start-up** ([`Joiner::establish`]): use the stored certificate if it is still good, otherwise join.
//! 3. **Renewal** ([`Joiner::maintain`]): at half the lifetime, renew over the open stream with a new key; if that keeps
//!    failing, join again once less than a tenth of the lifetime is left.
//!
//! The transport (T3) supplies the two hub-facing traits, [`JoinClient`] and [`RenewalChannel`], and reads the current
//! identity from the [`IdentityHandle`] each time it connects, so a renewed certificate is used from the next connect.

use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{ShortText, SwimlaneId};
use proto::convert::{IssuedCert, JoinCredential, JoinParams, JoinSubject};
use tokio::sync::watch;
use tokio::time::sleep;
use tracing::{error, info, warn};

use super::cert::ClientIdentity;
use super::error::{IdTokenError, IdentityError, JoinError, RenewError, StoreError};
use super::idtoken::IdTokenSource;
use super::jointoken::JoinTokenSource;
use super::key::KeyMaterial;
use super::schedule::{Phase, RenewalSchedule};
use super::store::CertStore;
use crate::backoff::Backoff;
use crate::clock::Clock;
use crate::config::{JoinMode, Settings};

/// Retry schedule of a join: the first retry within 5 s, then up to a minute apart (S6's cap).
pub const JOIN_BACKOFF_BASE: Duration = Duration::from_secs(5);
pub const JOIN_BACKOFF_CAP: Duration = Duration::from_secs(60);
/// Retry schedule of a renewal. A renewal has hours of slack, so it backs off further than a join does.
pub const RENEW_BACKOFF_BASE: Duration = Duration::from_secs(10);
pub const RENEW_BACKOFF_CAP: Duration = Duration::from_secs(600);

/// A call to the hub gets this long (rule 5). The transport has its own, shorter limits.
const HUB_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// No sleep is longer than this, so a clock that jumps is noticed within ten minutes.
const MAX_SLEEP: Duration = Duration::from_secs(600);
/// The shortest sleep, so that a deadline that is a few milliseconds away does not become a busy loop.
const MIN_SLEEP: Duration = Duration::from_millis(1);

/// The hub's `Join`, which needs no client certificate. The transport (T3) implements it over TLS without client
/// authentication.
#[async_trait]
pub trait JoinClient: Send + Sync + fmt::Debug {
    async fn join(&self, params: JoinParams) -> Result<IssuedCert, JoinError>;
}

/// Certificate renewal over the open stream: the agent sends a CSR in a `CertRenewalRequest` and the hub answers with a
/// `CertRenewalResponse`. The session (T3) implements it.
#[async_trait]
pub trait RenewalChannel: Send + Sync {
    async fn renew(&self, csr_der: Bytes) -> Result<IssuedCert, RenewError>;
}

/// The identity the next connection should use. [`Joiner::maintain`] replaces it on every renewal; a connection that
/// already exists keeps the [`Arc`] it was made with.
#[derive(Debug)]
pub struct IdentityHandle {
    tx: watch::Sender<Arc<ClientIdentity>>,
}

impl IdentityHandle {
    pub fn new(initial: ClientIdentity) -> Self {
        Self {
            tx: watch::Sender::new(Arc::new(initial)),
        }
    }

    /// The identity to connect with now.
    pub fn current(&self) -> Arc<ClientIdentity> {
        Arc::clone(&self.tx.borrow())
    }

    /// A receiver that reports each new identity, so the session can reconnect once a renewal has been stored (A22).
    pub fn subscribe(&self) -> watch::Receiver<Arc<ClientIdentity>> {
        self.tx.subscribe()
    }

    fn publish(&self, identity: ClientIdentity) {
        self.tx.send_replace(Arc::new(identity));
    }
}

#[derive(Debug)]
pub struct Joiner {
    swimlane: SwimlaneId,
    audience: ShortText,
    mode: JoinMode,
    id_tokens: Arc<dyn IdTokenSource>,
    join_tokens: Option<Arc<dyn JoinTokenSource>>,
    hub: Arc<dyn JoinClient>,
    store: Arc<dyn CertStore>,
    clock: Arc<dyn Clock>,
    seed: Option<u64>,
}

impl Joiner {
    /// `join_tokens` is the fallback source, present when `LK_JOIN_TOKEN_SECRET` is set.
    pub fn new(
        settings: &Settings,
        id_tokens: Arc<dyn IdTokenSource>,
        join_tokens: Option<Arc<dyn JoinTokenSource>>,
        hub: Arc<dyn JoinClient>,
        store: Arc<dyn CertStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            swimlane: settings.swimlane.clone(),
            audience: settings.hub_audience.clone(),
            mode: settings.join_mode,
            id_tokens,
            join_tokens,
            hub,
            store,
            clock,
            seed: None,
        }
    }

    /// Fix the seed of the retry jitter, so a test sees the same delays every time.
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    fn backoff(&self, base: Duration, cap: Duration) -> Backoff {
        self.seed.map_or_else(
            || Backoff::new(base, cap),
            |seed| Backoff::with_seed(base, cap, seed),
        )
    }

    // ------------------------------------------------------------ join

    /// One attempt to join, start to finish. The new certificate is stored and returned.
    pub async fn join(&self) -> Result<ClientIdentity, IdentityError> {
        // A join token is single-use, so find out first that the certificate can be stored.
        self.store.probe().await?;
        let credential = self.credential().await?;
        let key = KeyMaterial::generate()?;
        let params = JoinParams {
            subject: JoinSubject::Agent(self.swimlane.clone()),
            csr_der: key.csr_der()?,
            credential,
        };
        let issued = tokio::time::timeout(HUB_CALL_TIMEOUT, self.hub.join(params))
            .await
            .map_err(|_| JoinError::Unavailable)??;
        let identity = ClientIdentity::verify(key, issued.cert_chain_der, &self.swimlane, self.clock.now())?;
        self.keep(&identity).await;
        info!(
            swimlane = %self.swimlane,
            not_after_ms = identity.not_after().unix_millis(),
            "joined the hub"
        );
        Ok(identity)
    }

    /// The credential for this join: Workload Identity, or the join token, as the mode says.
    async fn credential(&self) -> Result<JoinCredential, IdentityError> {
        match self.mode {
            JoinMode::Token => self.join_token().await,
            JoinMode::WorkloadIdentity => self.workload_identity().await,
            JoinMode::Auto => match self.workload_identity().await {
                // Only a metadata server that is not there lets the join token take over. One that answers with an
                // error, or a hub that refuses the token, is a fault to fix and never a reason to try another way in.
                Err(IdentityError::IdToken(IdTokenError::Unavailable)) if self.join_tokens.is_some() => {
                    warn!("Workload Identity is not available here; joining with the join token");
                    self.join_token().await
                }
                other => other,
            },
        }
    }

    async fn workload_identity(&self) -> Result<JoinCredential, IdentityError> {
        let token = self.id_tokens.id_token(&self.audience).await?;
        Ok(JoinCredential::GoogleIdToken(token))
    }

    async fn join_token(&self) -> Result<JoinCredential, IdentityError> {
        let source = self
            .join_tokens
            .as_ref()
            .ok_or(IdentityError::NoJoinTokenSecret)?;
        Ok(JoinCredential::JoinToken(source.join_token().await?))
    }

    /// Store `identity`. A failure is loud but not fatal: the hub has issued a good certificate and the agent can use it;
    /// only a restart before the next successful save would have to join again.
    async fn keep(&self, identity: &ClientIdentity) {
        if let Err(error) = self.store.save(identity).await {
            error!(%error, "could not store the certificate; it is lost if the agent restarts");
        }
    }

    // ------------------------------------------------------------ start-up

    /// One pass at start-up: the stored certificate if it is still good, otherwise a join.
    pub async fn try_establish(&self) -> Result<ClientIdentity, IdentityError> {
        match self.store.load().await {
            Ok(Some(stored)) => {
                let now = self.clock.now();
                match ClientIdentity::verify(stored.key, stored.chain_der, &self.swimlane, now) {
                    Ok(identity) => {
                        let schedule = RenewalSchedule::new(identity.not_before(), identity.not_after());
                        if schedule.phase(now) != Phase::Rejoin {
                            info!(
                                not_after_ms = identity.not_after().unix_millis(),
                                "using the stored certificate"
                            );
                            return Ok(identity);
                        }
                        info!("the stored certificate is close to expiry; joining again");
                    }
                    Err(problem) => info!(%problem, "the stored certificate cannot be used; joining again"),
                }
            }
            Ok(None) => info!("no stored certificate; joining"),
            Err(StoreError::Malformed) => warn!("the stored key or certificate is damaged; joining again"),
            Err(error) => return Err(error.into()),
        }
        self.join().await
    }

    /// Keep trying, with backoff, until the agent has a certificate. The caller reports "not ready" meanwhile; dropping
    /// the future stops the attempts.
    pub async fn establish(&self) -> ClientIdentity {
        let mut backoff = self.backoff(JOIN_BACKOFF_BASE, JOIN_BACKOFF_CAP);
        loop {
            match self.try_establish().await {
                Ok(identity) => return identity,
                Err(error) => {
                    warn!(%error, "no client certificate yet; retrying");
                    sleep(backoff.next_delay()).await;
                }
            }
        }
    }

    // ------------------------------------------------------------ renewal

    /// One renewal over the stream, with a new key. The new certificate is stored and returned.
    pub async fn renew(&self, channel: &dyn RenewalChannel) -> Result<ClientIdentity, IdentityError> {
        let key = KeyMaterial::generate()?;
        let issued = tokio::time::timeout(HUB_CALL_TIMEOUT, channel.renew(key.csr_der()?))
            .await
            .map_err(|_| RenewError::Timeout)??;
        let identity = ClientIdentity::verify(key, issued.cert_chain_der, &self.swimlane, self.clock.now())?;
        self.keep(&identity).await;
        info!(
            not_after_ms = identity.not_after().unix_millis(),
            "renewed the certificate over the stream"
        );
        Ok(identity)
    }

    /// Keep the certificate in `handle` fresh, for as long as this future is polled. It never returns.
    ///
    /// Wait until half the lifetime has passed, then renew over `channel`, retrying with backoff. Once less than a tenth
    /// is left (or the certificate has expired), stop renewing and join again, which needs no working certificate.
    pub async fn maintain(&self, handle: &IdentityHandle, channel: &dyn RenewalChannel) -> Infallible {
        let mut renew_backoff = self.backoff(RENEW_BACKOFF_BASE, RENEW_BACKOFF_CAP);
        let mut join_backoff = self.backoff(JOIN_BACKOFF_BASE, JOIN_BACKOFF_CAP);
        loop {
            let current = handle.current();
            let schedule = RenewalSchedule::new(current.not_before(), current.not_after());
            let now = self.clock.now();
            match schedule.phase(now) {
                Phase::Wait { until } => sleep(until_wall(now, until, MAX_SLEEP)).await,
                Phase::Renew { rejoin_at } => match self.renew(channel).await {
                    Ok(renewed) => {
                        handle.publish(renewed);
                        renew_backoff.reset();
                    }
                    Err(error) => {
                        warn!(%error, "renewal over the stream failed; will retry");
                        // Never sleep past the point where joining again takes over.
                        let delay = renew_backoff.next_delay();
                        sleep(delay.min(until_wall(now, rejoin_at, MAX_SLEEP))).await;
                    }
                },
                Phase::Rejoin => match self.join().await {
                    Ok(joined) => {
                        handle.publish(joined);
                        renew_backoff.reset();
                        join_backoff.reset();
                    }
                    Err(error) => {
                        warn!(%error, "joining again failed; will retry");
                        sleep(join_backoff.next_delay()).await;
                    }
                },
            }
        }
    }
}

/// How long until the wall-clock time `until`, from `now`, between [`MIN_SLEEP`] and `longest`.
fn until_wall(now: domain::Timestamp, until: domain::Timestamp, longest: Duration) -> Duration {
    let ms = until.unix_millis().saturating_sub(now.unix_millis());
    Duration::from_millis(u64::try_from(ms).unwrap_or(0)).clamp(MIN_SLEEP, longest)
}

#[cfg(test)]
mod tests {
    use domain::Timestamp;

    use super::*;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    #[test]
    fn sleeps_are_bounded_on_both_sides() {
        let long = Duration::from_secs(600);
        assert_eq!(until_wall(ts(0), ts(5_000), long), Duration::from_secs(5));
        assert_eq!(until_wall(ts(0), ts(10_000_000), long), long);
        assert_eq!(until_wall(ts(1_000), ts(1_000), long), MIN_SLEEP);
        assert_eq!(
            until_wall(ts(2_000), ts(1_000), long),
            MIN_SLEEP,
            "a deadline in the past"
        );
        assert_eq!(until_wall(ts(i64::MIN), ts(i64::MAX), long), long);
    }
}
