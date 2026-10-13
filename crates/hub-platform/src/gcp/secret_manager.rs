//! `SecretSource` on the Secret Manager REST API (T2, S7, S8).
//!
//! `GET /v1/projects/P/secrets/NAME/versions/latest:access`, authenticated with the workload's access token. A value is
//! cached for at most five minutes, so a rotated secret is picked up within that time and a busy path does not call Google
//! per request; the cache is bounded. A secret that is not there is never cached, so a secret created later shows up at
//! once. Names are validated by `ports::is_valid_secret_name` before they reach the URL; values are `Secret`s, and the
//! decoded copy of the payload is wiped when the function that made it returns.

use std::time::Duration;

use async_trait::async_trait;
use domain::Secret;
use ports::{SecretError, SecretSource, is_valid_secret_name};
use serde::Deserialize;
use tracing::warn;
use zeroize::Zeroizing;

use super::api::{Call, GoogleApi};
use super::crc32c::crc32c;
use super::http::{GoogleRequest, Origin};
use super::metrics::{Operation, Outcome};
use super::ttl_cache::TtlCache;
use super::wire::{Int64, ProjectId, from_b64};

/// Secrets are read at most this stale (S7).
pub const MAX_CACHE_TTL: Duration = Duration::from_secs(300);
/// A hub reads a few dozen secrets; this is far above that and still bounded.
pub const DEFAULT_CACHE_ENTRIES: usize = 256;

#[derive(Deserialize)]
struct AccessResponse {
    payload: Payload,
}

#[derive(Deserialize)]
struct Payload {
    data: String,
    #[serde(rename = "dataCrc32c")]
    data_crc32c: Option<Int64>,
}

#[derive(Debug)]
pub struct SecretManagerSource {
    api: GoogleApi,
    origin: Origin,
    project: ProjectId,
    cache: TtlCache<Secret<String>>,
}

impl SecretManagerSource {
    /// `ttl` is capped at [`MAX_CACHE_TTL`]; `max_entries` bounds the cache.
    #[must_use]
    pub fn new(
        api: GoogleApi,
        origin: Origin,
        project: ProjectId,
        ttl: Duration,
        max_entries: usize,
    ) -> Self {
        Self {
            api,
            origin,
            project,
            cache: TtlCache::new(ttl.min(MAX_CACHE_TTL), max_entries),
        }
    }

    async fn fetch(&self, name: &str) -> Result<Secret<String>, SecretError> {
        let call = self.api.begin(Operation::SecretAccess);
        let target = format!(
            "/v1/projects/{}/secrets/{name}/versions/latest:access",
            self.project.as_str()
        );
        let response = call
            .send(&self.origin, GoogleRequest::get(target))
            .await
            .map_err(|error| {
                warn!(code = "secret_manager_unreachable", ?error);
                SecretError::Unavailable
            })?;
        let status = response.status;
        match status {
            200 => {}
            404 => {
                response.wipe();
                call.finish(Outcome::NotFound);
                return Err(SecretError::NotFound);
            }
            _ => {
                response.wipe();
                warn!(code = "secret_manager_refused", status);
                call.finish(Call::outcome_for_status(status));
                return Err(SecretError::Unavailable);
            }
        }
        let parsed = serde_json::from_slice::<AccessResponse>(&response.body);
        response.wipe();
        let value = parsed.ok().and_then(|answer| {
            let data = Zeroizing::new(from_b64(&answer.payload.data)?);
            // Google always sends the checksum; an answer without one, or with a wrong one, is not trusted.
            let intact = answer
                .payload
                .data_crc32c
                .is_some_and(|c| c.is_crc32c(crc32c(&data)));
            let text = intact
                .then(|| std::str::from_utf8(&data).ok().map(str::to_owned))
                .flatten();
            text.map(Secret::new)
        });
        if let Some(value) = value {
            call.finish(Outcome::Ok);
            Ok(value)
        } else {
            warn!(code = "secret_manager_bad_payload");
            call.finish(Outcome::BadResponse);
            Err(SecretError::Unavailable)
        }
    }
}

#[async_trait]
impl SecretSource for SecretManagerSource {
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError> {
        if !is_valid_secret_name(name) {
            return Err(SecretError::InvalidName);
        }
        if let Some(hit) = self.cache.get(name, |value| Secret::new(value.expose().clone())) {
            return Ok(hit);
        }
        let value = self.fetch(name).await?;
        self.cache.insert(name, Secret::new(value.expose().clone()));
        Ok(value)
    }
}
