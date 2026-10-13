//! The workload's access token from the metadata server (T2, S7).
//!
//! With Workload Identity the node's metadata server answers for the pod's own Google service account: no key file, no
//! secret in the image. The token is cached until a minute before it expires, one fetch at a time (callers that arrive
//! while one is running wait for it), and it is a `Secret`: it prints as redacted and is wiped when it drops.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use domain::Secret;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::warn;

use super::api::{ApiError, Transport};
use super::http::{GoogleRequest, Origin};
use super::metrics::{Operation, Outcome};

/// The longest token the hub accepts. Google's are about 200 bytes.
const MAX_TOKEN_BYTES: usize = 8 * 1024;
/// A token that claims to live longer than a day is not one the metadata server issued.
const MAX_LIFETIME_SECONDS: u64 = 86_400;
/// The token is renewed this long before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
const TOKEN_PATH: &str = "/computeMetadata/v1/instance/service-accounts/default/token";

/// Why no token could be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("no access token is available")]
    Unavailable,
    #[error("the metadata server's answer is not an access token")]
    Malformed,
}

/// Where access tokens come from. Production has one implementation, [`WorkloadIdentityTokens`]; the seam lets a test
/// count fetches and revoke a token, and lets the opt-in test against a real project borrow a developer's token.
#[async_trait]
pub trait AccessTokens: Send + Sync + fmt::Debug + 'static {
    /// A token that is valid now.
    async fn token(&self) -> Result<Secret<String>, TokenError>;

    /// Google refused `token` (a `401`): forget it, unless a newer one has replaced it already.
    async fn refused(&self, token: &Secret<String>);
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    expires_in: u64,
    token_type: String,
}

struct Cached {
    token: Secret<String>,
    refresh_at: Instant,
}

/// Tokens from the GCE metadata server.
pub struct WorkloadIdentityTokens {
    transport: Transport,
    origin: Origin,
    cache: Mutex<Option<Cached>>,
}

impl fmt::Debug for WorkloadIdentityTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkloadIdentityTokens").finish_non_exhaustive()
    }
}

impl WorkloadIdentityTokens {
    /// `origin` is the metadata server (`http://metadata.google.internal`).
    #[must_use]
    pub fn new(transport: Transport, origin: Origin) -> Self {
        Self {
            transport,
            origin,
            cache: Mutex::new(None),
        }
    }

    async fn fetch(&self) -> Result<Cached, TokenError> {
        let started = Instant::now();
        let metrics = self.transport.metrics();
        let request = GoogleRequest::get(TOKEN_PATH)
            .with_header("metadata-flavor", "Google")
            .map_err(|_| TokenError::Unavailable)?;
        let response = match self
            .transport
            .send(Operation::Token, &self.origin, &request)
            .await
        {
            Ok(response) => response,
            Err(error) => {
                metrics.record(
                    Operation::Token,
                    match error {
                        ApiError::Timeout => Outcome::Timeout,
                        ApiError::Rejected | ApiError::TooLarge => Outcome::BadResponse,
                        ApiError::Unavailable | ApiError::NoToken => Outcome::Unavailable,
                    },
                    started.elapsed(),
                );
                return Err(TokenError::Unavailable);
            }
        };
        let parsed = (response.status == 200)
            .then(|| serde_json::from_slice::<TokenAnswer>(&response.body).ok())
            .flatten();
        let status = response.status;
        response.wipe();
        let Some(answer) = parsed else {
            warn!(code = "metadata_token_refused", status);
            metrics.record(Operation::Token, Outcome::Refused, started.elapsed());
            return Err(if status == 200 {
                TokenError::Malformed
            } else {
                TokenError::Unavailable
            });
        };
        let valid = answer.token_type.eq_ignore_ascii_case("bearer")
            && (1..=MAX_LIFETIME_SECONDS).contains(&answer.expires_in)
            && !answer.access_token.is_empty()
            && answer.access_token.len() <= MAX_TOKEN_BYTES
            && answer.access_token.bytes().all(|b| (0x21..=0x7e).contains(&b));
        if !valid {
            warn!(code = "metadata_token_malformed");
            metrics.record(Operation::Token, Outcome::BadResponse, started.elapsed());
            return Err(TokenError::Malformed);
        }
        metrics.record(Operation::Token, Outcome::Ok, started.elapsed());
        let lifetime = Duration::from_secs(answer.expires_in);
        Ok(Cached {
            token: Secret::new(answer.access_token),
            refresh_at: started + lifetime.saturating_sub(REFRESH_MARGIN),
        })
    }
}

fn copy(token: &Secret<String>) -> Secret<String> {
    Secret::new(token.expose().clone())
}

#[async_trait]
impl AccessTokens for WorkloadIdentityTokens {
    async fn token(&self) -> Result<Secret<String>, TokenError> {
        // The lock is held across the fetch on purpose: one fetch at a time, and the fetch has its own deadline.
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            if Instant::now() < cached.refresh_at {
                return Ok(copy(&cached.token));
            }
        }
        let fresh = self.fetch().await?;
        let token = copy(&fresh.token);
        *cache = Some(fresh);
        Ok(token)
    }

    async fn refused(&self, token: &Secret<String>) {
        let mut cache = self.cache.lock().await;
        if cache.as_ref().is_some_and(|c| bool::from(c.token.ct_eq(token))) {
            *cache = None;
        }
    }
}
