//! The cached key set (JWKS) of an issuer (S1).
//!
//! - Only RSA signature keys are read, with a modulus of 2048 to 8192 bits. Keys with another `kty`, a `use` other than
//!   `sig`, or an `alg` other than RS256 are skipped. A repeated key id makes the whole set invalid, because it is ambiguous.
//! - The set is cached for the issuer's `Cache-Control: max-age`, clamped to 60 s to 6 h.
//! - A token with an unknown key id triggers one refresh, and at most one every 30 s however many tokens arrive. Concurrent
//!   callers share one fetch (single flight). The URL is configuration or the verified discovery document, never a token field.
//! - If a refresh fails, keys past their lifetime are not used: the answer is `Http`, and the next attempt waits 5 s.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use tokio::time::Instant;
use url::Url;

use crate::clock::Clock;
use crate::error::{IdentityError, TokenError};
use crate::http::{HttpsClient, HttpsError, HttpsRequest};

/// Shortest and longest time a fetched key set is trusted.
pub const MIN_TTL: Duration = Duration::from_secs(60);
pub const MAX_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// Used when the response has no usable `max-age`.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);
/// Minimum time between two fetches caused by an unknown key id.
pub const MIN_FORCED_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// After a failed fetch, wait this long before trying again.
pub const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(5);
/// Keys read from one document. Entra publishes a handful.
pub const MAX_KEYS: usize = 32;
const MIN_MODULUS_BYTES: usize = 256;
const MAX_MODULUS_BYTES: usize = 1024;
const MAX_EXPONENT_BYTES: usize = 8;
const MAX_KID_LEN: usize = 256;

/// An RSA public key as the big-endian bytes of the modulus and the exponent, without leading zeros.
#[derive(Clone, PartialEq, Eq)]
pub struct RsaPublicKey {
    n: Vec<u8>,
    e: Vec<u8>,
}

impl std::fmt::Debug for RsaPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RsaPublicKey")
            .field("bits", &(self.n.len() * 8))
            .finish_non_exhaustive()
    }
}

impl RsaPublicKey {
    pub fn from_components(n: Vec<u8>, e: Vec<u8>) -> Option<Self> {
        let ok = (MIN_MODULUS_BYTES..=MAX_MODULUS_BYTES).contains(&n.len())
            && n.first().is_some_and(|b| *b != 0)
            && (1..=MAX_EXPONENT_BYTES).contains(&e.len())
            && e.first().is_some_and(|b| *b != 0);
        ok.then_some(Self { n, e })
    }

    /// Verify an RSASSA-PKCS1-v1_5 SHA-256 signature (RS256). The provider also enforces the 2048-bit minimum.
    pub fn verify_rs256(&self, message: &[u8], signature: &[u8]) -> bool {
        aws_lc_rs::signature::RsaPublicKeyComponents {
            n: self.n.as_slice(),
            e: self.e.as_slice(),
        }
        .verify(
            &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
            message,
            signature,
        )
        .is_ok()
    }
}

/// The keys of one document, by key id.
#[derive(Debug, Clone, Default)]
pub struct KeySet {
    keys: HashMap<String, Arc<RsaPublicKey>>,
}

#[derive(Deserialize)]
struct JwkDoc {
    keys: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    #[serde(rename = "use")]
    use_: Option<String>,
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

impl KeySet {
    /// Parse a JWKS document. The body is already size-capped by the HTTP client; the key count is capped here.
    pub fn parse(body: &[u8]) -> Result<Self, IdentityError> {
        let doc: JwkDoc = serde_json::from_slice(body).map_err(|_| IdentityError::Jwks)?;
        if doc.keys.len() > MAX_KEYS {
            return Err(IdentityError::Jwks);
        }
        let mut keys = HashMap::new();
        for value in doc.keys {
            let Ok(jwk) = serde_json::from_value::<Jwk>(value) else {
                continue;
            };
            if jwk.kty != "RSA" || jwk.use_.as_deref().is_some_and(|u| u != "sig") {
                continue;
            }
            if jwk.alg.as_deref().is_some_and(|a| a != "RS256") {
                continue;
            }
            let (Some(kid), Some(n), Some(e)) = (jwk.kid, jwk.n, jwk.e) else {
                continue;
            };
            if kid.is_empty() || kid.len() > MAX_KID_LEN {
                continue;
            }
            let (Ok(n), Ok(e)) = (URL_SAFE_NO_PAD.decode(n), URL_SAFE_NO_PAD.decode(e)) else {
                continue;
            };
            let Some(key) = RsaPublicKey::from_components(n, e) else {
                continue;
            };
            if keys.insert(kid, Arc::new(key)).is_some() {
                return Err(IdentityError::Jwks);
            }
        }
        if keys.is_empty() {
            return Err(IdentityError::Jwks);
        }
        Ok(Self { keys })
    }

    pub fn get(&self, kid: &str) -> Option<&Arc<RsaPublicKey>> {
        self.keys.get(kid)
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// A set from keys the caller already holds. For tests and the fuzz targets.
    pub fn from_keys(keys: impl IntoIterator<Item = (String, RsaPublicKey)>) -> Self {
        Self {
            keys: keys.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(),
        }
    }
}

/// How long a response says it may be cached, clamped to [`MIN_TTL`, `MAX_TTL`].
pub fn ttl_from_cache_control(header: Option<&str>) -> Duration {
    let Some(header) = header else { return DEFAULT_TTL };
    let mut max_age = None;
    for directive in header.split(',').map(|d| d.trim().to_ascii_lowercase()) {
        if directive == "no-store" || directive == "no-cache" {
            return MIN_TTL;
        }
        if let Some(v) = directive.strip_prefix("max-age=") {
            max_age = v.trim_matches('"').parse::<u64>().ok();
        }
    }
    max_age.map_or(DEFAULT_TTL, |s| Duration::from_secs(s).clamp(MIN_TTL, MAX_TTL))
}

/// Where the key set lives. A fixed URL (Google) or the verified discovery document (Entra).
#[async_trait]
pub trait JwksLocation: Send + Sync + 'static {
    async fn jwks_url(&self) -> Result<Url, IdentityError>;
}

#[async_trait]
impl JwksLocation for Url {
    async fn jwks_url(&self) -> Result<Url, IdentityError> {
        Ok(self.clone())
    }
}

#[derive(Default)]
struct State {
    keys: Option<Arc<KeySet>>,
    fetched_at: Option<Instant>,
    ttl: Duration,
    last_attempt: Option<Instant>,
}

impl State {
    fn is_fresh(&self, now: Instant) -> bool {
        self.fetched_at.is_some_and(|at| now < at + self.ttl)
    }

    fn lookup(&self, kid: &str, now: Instant) -> Option<Arc<RsaPublicKey>> {
        if !self.is_fresh(now) {
            return None;
        }
        self.keys.as_ref()?.get(kid).cloned()
    }
}

/// The cache. One per issuer.
pub struct JwksCache {
    http: Arc<dyn HttpsClient>,
    location: Arc<dyn JwksLocation>,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    /// Held while fetching, so concurrent callers wait for one fetch instead of making their own.
    gate: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for JwksCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwksCache").finish_non_exhaustive()
    }
}

impl JwksCache {
    pub fn new(http: Arc<dyn HttpsClient>, location: Arc<dyn JwksLocation>, clock: Arc<dyn Clock>) -> Self {
        Self {
            http,
            location,
            clock,
            state: Mutex::new(State::default()),
            gate: tokio::sync::Mutex::new(()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // A poisoned lock only means another thread panicked while holding it; the state is plain data.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The key for `kid`. Unknown after the one allowed refresh is [`TokenError::UnknownKey`].
    pub async fn key(&self, kid: &str) -> Result<Arc<RsaPublicKey>, IdentityError> {
        if let Some(k) = self.state().lookup(kid, self.clock.instant()) {
            return Ok(k);
        }
        let _gate = self.gate.lock().await;
        let now = self.clock.instant();
        {
            let st = self.state();
            if let Some(k) = st.lookup(kid, now) {
                return Ok(k);
            }
            if st.is_fresh(now) {
                // The set is current but lacks this key: a rotation, or a forged key id. Refresh, but not more than
                // once per interval, however many tokens ask.
                if st
                    .last_attempt
                    .is_some_and(|at| now < at + MIN_FORCED_REFRESH_INTERVAL)
                {
                    return Err(TokenError::UnknownKey.into());
                }
            } else if st.last_attempt.is_some_and(|at| now < at + RETRY_AFTER_FAILURE) {
                return Err(HttpsError::Connect.into());
            }
        }
        self.state().last_attempt = Some(now);
        let fetched = self.fetch().await?;
        let key = fetched.0.get(kid).cloned();
        {
            let mut st = self.state();
            st.keys = Some(Arc::new(fetched.0));
            st.fetched_at = Some(now);
            st.ttl = fetched.1;
        }
        key.ok_or_else(|| TokenError::UnknownKey.into())
    }

    async fn fetch(&self) -> Result<(KeySet, Duration), IdentityError> {
        let url = self.location.jwks_url().await?;
        let resp = self.http.send(HttpsRequest::get(url)).await?;
        if resp.status != 200 {
            tracing::warn!(status = resp.status, "key set fetch was refused");
            return Err(IdentityError::Jwks);
        }
        let ttl = ttl_from_cache_control(resp.cache_control());
        Ok((KeySet::parse(&resp.body)?, ttl))
    }

    /// Number of keys currently cached. For tests and metrics.
    pub fn cached_keys(&self) -> usize {
        self.state().keys.as_ref().map_or(0, |k| k.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_is_clamped() {
        assert_eq!(ttl_from_cache_control(None), DEFAULT_TTL);
        assert_eq!(ttl_from_cache_control(Some("public, max-age=5")), MIN_TTL);
        assert_eq!(
            ttl_from_cache_control(Some("public, max-age=600")),
            Duration::from_secs(600)
        );
        assert_eq!(ttl_from_cache_control(Some("max-age=99999999")), MAX_TTL);
        assert_eq!(ttl_from_cache_control(Some("no-store")), MIN_TTL);
        assert_eq!(ttl_from_cache_control(Some("max-age=abc")), DEFAULT_TTL);
        assert_eq!(
            ttl_from_cache_control(Some("MAX-AGE=900")),
            Duration::from_secs(900)
        );
    }

    #[test]
    fn garbage_key_sets_are_refused() {
        for body in [
            &b""[..],
            b"{}",
            b"[]",
            b"{\"keys\":[]}",
            b"{\"keys\":[{\"kty\":\"oct\",\"kid\":\"a\"}]}",
        ] {
            assert!(matches!(KeySet::parse(body), Err(IdentityError::Jwks)));
        }
    }

    #[test]
    fn short_or_padded_moduli_are_not_keys() {
        assert!(RsaPublicKey::from_components(vec![1; 128], vec![1, 0, 1]).is_none());
        assert!(RsaPublicKey::from_components(vec![0; 256], vec![1, 0, 1]).is_none());
        assert!(RsaPublicKey::from_components(vec![1; 256], vec![]).is_none());
        assert!(RsaPublicKey::from_components(vec![1; 256], vec![1, 0, 1]).is_some());
    }
}
