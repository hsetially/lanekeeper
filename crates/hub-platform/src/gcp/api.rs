//! Calling Google: retries with jitter, a deadline, the access token and metrics (T2, rule 5).
//!
//! [`Transport`] adds to one HTTP exchange what every Google call needs: bounded retries on transport failures and on
//! `408`, `429` and `5xx`, with full jitter between them, and a deadline over the whole call. [`GoogleApi`] adds the
//! bearer token, which it renews once when Google answers `401`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tracing::warn;

use super::http::{GoogleHttp, GoogleRequest, GoogleResponse, HttpError, Origin};
use super::metrics::{GoogleMetrics, Operation, Outcome};
use super::retry::RetryPolicy;
use super::token::AccessTokens;

/// Why a call produced no answer. The cause goes to the log as a code; callers map this to their port's error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    /// The deadline ran out.
    #[error("the call timed out")]
    Timeout,
    /// The network or Google failed and the retries ran out.
    #[error("the service is unavailable")]
    Unavailable,
    /// The request never left: the destination is not allowed, or the request is malformed.
    #[error("the request was rejected before it was sent")]
    Rejected,
    /// The answer is bigger than the cap.
    #[error("the response is too large")]
    TooLarge,
    /// No access token could be had.
    #[error("no access token")]
    NoToken,
}

impl ApiError {
    fn outcome(self) -> Outcome {
        match self {
            Self::Timeout => Outcome::Timeout,
            Self::Unavailable | Self::NoToken => Outcome::Unavailable,
            Self::Rejected => Outcome::Rejected,
            Self::TooLarge => Outcome::BadResponse,
        }
    }
}

/// Whether Google's answer is worth another try.
fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500..=599)
}

/// Limits of one call: tries, waits and the total time.
#[derive(Debug, Clone, Copy)]
pub struct CallLimits {
    pub retry: RetryPolicy,
    /// From the first try to the answer, waits included.
    pub deadline: Duration,
}

impl Default for CallLimits {
    fn default() -> Self {
        Self {
            retry: RetryPolicy::default(),
            deadline: Duration::from_secs(30),
        }
    }
}

/// One HTTP exchange with retries and a deadline, and no token.
#[derive(Clone)]
pub struct Transport {
    http: GoogleHttp,
    limits: CallLimits,
    metrics: GoogleMetrics,
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Transport {
    #[must_use]
    pub fn new(http: GoogleHttp, limits: CallLimits, metrics: GoogleMetrics) -> Self {
        Self {
            http,
            limits,
            metrics,
        }
    }

    #[must_use]
    pub fn metrics(&self) -> &GoogleMetrics {
        &self.metrics
    }

    /// Send `request`, retrying as the policy says, within the deadline. Any answer that is not worth retrying is
    /// returned, whatever its status.
    pub async fn send(
        &self,
        operation: Operation,
        origin: &Origin,
        request: &GoogleRequest,
    ) -> Result<GoogleResponse, ApiError> {
        tokio::time::timeout(self.limits.deadline, self.attempts(operation, origin, request))
            .await
            .map_err(|_| ApiError::Timeout)?
    }

    async fn attempts(
        &self,
        operation: Operation,
        origin: &Origin,
        request: &GoogleRequest,
    ) -> Result<GoogleResponse, ApiError> {
        let max = self.limits.retry.max_attempts.max(1);
        let mut attempt = 1;
        loop {
            let failure = match self.http.exchange(origin, request.clone()).await {
                Ok(response) if is_retryable_status(response.status) => {
                    let status = response.status;
                    response.wipe();
                    warn!(code = "google_status_retryable", ?operation, status, attempt);
                    ApiError::Unavailable
                }
                Ok(response) => return Ok(response),
                Err(HttpError::HostNotAllowed | HttpError::InvalidRequest) => return Err(ApiError::Rejected),
                Err(HttpError::TooLarge) => return Err(ApiError::TooLarge),
                Err(error) => {
                    warn!(code = "google_transport_failed", ?operation, ?error, attempt);
                    if error == HttpError::Timeout {
                        ApiError::Timeout
                    } else {
                        ApiError::Unavailable
                    }
                }
            };
            if attempt >= max {
                return Err(failure);
            }
            self.metrics.retry(operation);
            tokio::time::sleep(self.limits.retry.jittered(attempt)).await;
            attempt += 1;
        }
    }
}

/// A [`Transport`] that authenticates every request with the workload's access token.
#[derive(Clone)]
pub struct GoogleApi {
    transport: Transport,
    tokens: Arc<dyn AccessTokens>,
}

impl fmt::Debug for GoogleApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GoogleApi")
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl GoogleApi {
    #[must_use]
    pub fn new(transport: Transport, tokens: Arc<dyn AccessTokens>) -> Self {
        Self { transport, tokens }
    }

    /// Start a call. It is measured from now; [`Call::finish`] records it.
    pub fn begin(&self, operation: Operation) -> Call<'_> {
        Call {
            api: self,
            operation,
            started: Instant::now(),
        }
    }
}

/// One logical Google call, for its metrics. `send` records a failure itself; the caller records every other end with
/// [`Call::finish`], because only the caller knows whether the answer made sense.
#[derive(Debug)]
#[must_use = "a call is recorded by `finish`"]
pub struct Call<'a> {
    api: &'a GoogleApi,
    operation: Operation,
    started: Instant,
}

impl Call<'_> {
    /// Send `request` with a bearer token. A `401` renews the token once and sends again.
    pub async fn send(&self, origin: &Origin, request: GoogleRequest) -> Result<GoogleResponse, ApiError> {
        let result = tokio::time::timeout(
            self.api.transport.limits.deadline,
            self.authenticated(origin, request),
        )
        .await
        .unwrap_or(Err(ApiError::Timeout));
        if let Err(error) = result {
            self.api
                .transport
                .metrics
                .record(self.operation, error.outcome(), self.started.elapsed());
        }
        result
    }

    async fn authenticated(
        &self,
        origin: &Origin,
        request: GoogleRequest,
    ) -> Result<GoogleResponse, ApiError> {
        let transport = &self.api.transport;
        let mut renewed = false;
        loop {
            let token = self.api.tokens.token().await.map_err(|_| ApiError::NoToken)?;
            let signed = request
                .clone()
                .with_bearer(&token)
                .map_err(|_| ApiError::Rejected)?;
            let response = transport.send(self.operation, origin, &signed).await?;
            if response.status == 401 && !renewed {
                renewed = true;
                self.api.tokens.refused(&token).await;
                continue;
            }
            return Ok(response);
        }
    }

    /// The call ended with `outcome`.
    pub fn finish(self, outcome: Outcome) {
        self.api
            .transport
            .metrics
            .record(self.operation, outcome, self.started.elapsed());
    }

    /// The outcome for an HTTP status that is not a success the caller handles itself.
    #[must_use]
    pub fn outcome_for_status(status: u16) -> Outcome {
        match status {
            200..=299 => Outcome::Ok,
            404 => Outcome::NotFound,
            _ => Outcome::Refused,
        }
    }
}
