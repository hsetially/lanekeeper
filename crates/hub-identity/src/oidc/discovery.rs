//! The discovery document (S1).
//!
//! The issuer in the document must equal the tenant's issuer, and every endpoint must be an `https` URL on the configured
//! authority host, with no credentials, no non-default port and no fragment. A document that names another host (a poisoned
//! cache, a hijacked proxy) is refused as a whole, so the hub never sends a code or fetches keys from somewhere else.
//! The document is cached for an hour; if a refresh fails, the last good copy is used for up to a day.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::time::Instant;
use url::Url;

use crate::clock::Clock;
use crate::config::EntraConfig;
use crate::error::{DiscoveryError, IdentityError};
use crate::http::{HttpsClient, HttpsRequest};
use crate::oidc::jwks::JwksLocation;

const REFRESH_AFTER: Duration = Duration::from_secs(60 * 60);
const USE_STALE_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// The endpoints the hub uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub jwks_uri: Url,
}

#[derive(Deserialize)]
struct Wire {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

impl Discovery {
    /// Parse and check a discovery document for `expected_issuer`, with every endpoint on `authority_host`.
    pub fn parse(body: &[u8], expected_issuer: &str, authority_host: &str) -> Result<Self, DiscoveryError> {
        let wire: Wire = serde_json::from_slice(body).map_err(|_| DiscoveryError::Malformed)?;
        if wire.issuer != expected_issuer {
            return Err(DiscoveryError::IssuerMismatch);
        }
        let endpoint = |raw: &str| -> Result<Url, DiscoveryError> {
            let url = Url::parse(raw).map_err(|_| DiscoveryError::EndpointRejected)?;
            let acceptable = url.scheme() == "https"
                && url.host_str() == Some(authority_host)
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none();
            if acceptable {
                Ok(url)
            } else {
                Err(DiscoveryError::EndpointRejected)
            }
        };
        Ok(Self {
            issuer: wire.issuer,
            authorization_endpoint: endpoint(&wire.authorization_endpoint)?,
            token_endpoint: endpoint(&wire.token_endpoint)?,
            jwks_uri: endpoint(&wire.jwks_uri)?,
        })
    }
}

struct Cached {
    doc: Arc<Discovery>,
    fetched_at: Instant,
}

/// Fetches and caches the tenant's discovery document. Concurrent callers share one fetch.
pub struct DiscoveryCache {
    http: Arc<dyn HttpsClient>,
    clock: Arc<dyn Clock>,
    url: Url,
    issuer: String,
    authority_host: String,
    cached: tokio::sync::Mutex<Option<Cached>>,
}

impl std::fmt::Debug for DiscoveryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryCache").finish_non_exhaustive()
    }
}

impl DiscoveryCache {
    pub fn new(
        cfg: &EntraConfig,
        http: Arc<dyn HttpsClient>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, IdentityError> {
        let url = Url::parse(&cfg.discovery_url()).map_err(|_| DiscoveryError::EndpointRejected)?;
        Ok(Self {
            http,
            clock,
            url,
            issuer: cfg.issuer(),
            authority_host: cfg.authority_host.clone(),
            cached: tokio::sync::Mutex::new(None),
        })
    }

    pub async fn get(&self) -> Result<Arc<Discovery>, IdentityError> {
        let mut slot = self.cached.lock().await;
        let now = self.clock.instant();
        if let Some(c) = slot.as_ref().filter(|c| now < c.fetched_at + REFRESH_AFTER) {
            return Ok(Arc::clone(&c.doc));
        }
        match self.fetch().await {
            Ok(doc) => {
                let doc = Arc::new(doc);
                *slot = Some(Cached {
                    doc: Arc::clone(&doc),
                    fetched_at: now,
                });
                Ok(doc)
            }
            Err(e) => match slot.as_ref().filter(|c| now < c.fetched_at + USE_STALE_FOR) {
                Some(c) => {
                    tracing::warn!("discovery refresh failed; using the last good copy");
                    Ok(Arc::clone(&c.doc))
                }
                None => Err(e),
            },
        }
    }

    async fn fetch(&self) -> Result<Discovery, IdentityError> {
        let resp = self.http.send(HttpsRequest::get(self.url.clone())).await?;
        if resp.status != 200 {
            tracing::warn!(status = resp.status, "discovery document was refused");
            return Err(DiscoveryError::Malformed.into());
        }
        Ok(Discovery::parse(&resp.body, &self.issuer, &self.authority_host)?)
    }
}

#[async_trait]
impl JwksLocation for DiscoveryCache {
    async fn jwks_url(&self) -> Result<Url, IdentityError> {
        Ok(self.get().await?.jwks_uri.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISS: &str = "https://login.microsoftonline.com/t/v2.0";

    fn doc(issuer: &str, host: &str) -> String {
        format!(
            r#"{{"issuer":"{issuer}","authorization_endpoint":"https://{host}/t/oauth2/v2.0/authorize",
            "token_endpoint":"https://{host}/t/oauth2/v2.0/token","jwks_uri":"https://{host}/t/discovery/v2.0/keys"}}"#
        )
    }

    #[test]
    fn a_matching_document_is_accepted() {
        let d = Discovery::parse(
            doc(ISS, "login.microsoftonline.com").as_bytes(),
            ISS,
            "login.microsoftonline.com",
        )
        .unwrap();
        assert_eq!(d.token_endpoint.path(), "/t/oauth2/v2.0/token");
    }

    #[test]
    fn discovery_issuer_or_host_mismatch_is_refused() {
        let host = "login.microsoftonline.com";
        assert_eq!(
            Discovery::parse(
                doc("https://login.microsoftonline.com/other/v2.0", host).as_bytes(),
                ISS,
                host
            ),
            Err(DiscoveryError::IssuerMismatch)
        );
        assert_eq!(
            Discovery::parse(doc(ISS, "evil.example.com").as_bytes(), ISS, host),
            Err(DiscoveryError::EndpointRejected)
        );
        let plain = doc(ISS, host).replace(
            "\"https://login.microsoftonline.com/t/oauth2",
            "\"http://login.microsoftonline.com/t/oauth2",
        );
        assert_eq!(
            Discovery::parse(plain.as_bytes(), ISS, host),
            Err(DiscoveryError::EndpointRejected)
        );
        let creds = doc(ISS, host).replace(
            "\"https://login.microsoftonline.com/t/oauth2",
            "\"https://u:p@login.microsoftonline.com/t/oauth2",
        );
        assert_eq!(
            Discovery::parse(creds.as_bytes(), ISS, host),
            Err(DiscoveryError::EndpointRejected)
        );
        let port = doc(ISS, "login.microsoftonline.com:8443");
        assert_eq!(
            Discovery::parse(port.as_bytes(), ISS, host),
            Err(DiscoveryError::EndpointRejected)
        );
        assert_eq!(Discovery::parse(b"{}", ISS, host), Err(DiscoveryError::Malformed));
    }
}
