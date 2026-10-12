//! Access tokens for Cloud Storage from the GKE metadata server (Workload Identity).
//!
//! The metadata server is a link-local address inside the node (`169.254.169.254`) that speaks plain HTTP; there is no TLS to
//! add and no name to resolve. This is the one plain-HTTP call in the crate, and it can reach only that address: the target is
//! fixed in the type, and the response must carry `Metadata-Flavor: Google`, which a server that is not the metadata server does
//! not send. A call has a timeout and a size cap. The token is cached until a minute before it expires, and never printed.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::Secret;
use http::Request;
use http_body_util::{BodyExt, Empty, Limited};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::bucket::{AccessTokenSource, BucketError};

const METADATA_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)), 80);
const METADATA_HOST: &str = "metadata.google.internal";
const TOKEN_PATH: &str = "/computeMetadata/v1/instance/service-accounts/default/token";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: usize = 16 * 1024;
/// A token is renewed this long before it expires.
const EARLY: Duration = Duration::from_secs(60);
/// A token with a lifetime outside this range is not believed: the shortest is renewed at once, the longest is cut.
const MAX_LIFETIME: Duration = Duration::from_secs(3600 * 12);

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Secret<String>,
    expires_in: u64,
}

struct Cached {
    token: Secret<String>,
    renew_at: Instant,
}

/// Tokens for the pod's Google service account.
pub struct MetadataTokenSource {
    addr: SocketAddr,
    cached: Mutex<Option<Cached>>,
}

impl std::fmt::Debug for MetadataTokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataTokenSource").finish_non_exhaustive()
    }
}

impl Default for MetadataTokenSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataTokenSource {
    pub fn new() -> Self {
        Self {
            addr: METADATA_ADDR,
            cached: Mutex::new(None),
        }
    }

    /// Test hook: ask this address instead of the real metadata server.
    #[doc(hidden)]
    #[must_use]
    pub fn at(addr: SocketAddr) -> Self {
        Self {
            addr,
            cached: Mutex::new(None),
        }
    }

    async fn fetch(&self) -> Result<TokenResponse, BucketError> {
        let tcp = TcpStream::connect(self.addr)
            .await
            .map_err(|_| BucketError::Unavailable)?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(tcp))
                .await
                .map_err(|_| BucketError::Unavailable)?;
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        let result = async {
            let request = Request::builder()
                .method("GET")
                .uri(TOKEN_PATH)
                .header(http::header::HOST, METADATA_HOST)
                .header("metadata-flavor", "Google")
                .header(http::header::CONNECTION, "close")
                .body(Empty::<Bytes>::new())
                .map_err(|_| BucketError::Unavailable)?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| BucketError::Unavailable)?;
            let (parts, body) = response.into_parts();
            if parts.status.as_u16() != 200 {
                tracing::warn!(
                    status = parts.status.as_u16(),
                    "metadata server refused the token request"
                );
                return Err(BucketError::Rejected);
            }
            if parts.headers.get("metadata-flavor").and_then(|v| v.to_str().ok()) != Some("Google") {
                tracing::warn!("answer did not come from a metadata server");
                return Err(BucketError::Rejected);
            }
            let body = Limited::new(body, MAX_BODY_BYTES)
                .collect()
                .await
                .map_err(|_| BucketError::Unavailable)?
                .to_bytes();
            serde_json::from_slice::<TokenResponse>(&body).map_err(|_| BucketError::Rejected)
        }
        .await;
        driver.abort();
        result
    }
}

#[async_trait]
impl AccessTokenSource for MetadataTokenSource {
    async fn token(&self) -> Result<Secret<String>, BucketError> {
        // One caller refreshes; the others wait for it and use its answer.
        let mut cached = self.cached.lock().await;
        if let Some(c) = cached.as_ref().filter(|c| Instant::now() < c.renew_at) {
            return Ok(Secret::new(c.token.expose().clone()));
        }
        let fetched = tokio::time::timeout(TIMEOUT, self.fetch())
            .await
            .map_err(|_| BucketError::Unavailable)??;
        if fetched.access_token.expose().is_empty() || fetched.access_token.expose().len() > MAX_BODY_BYTES {
            return Err(BucketError::Rejected);
        }
        let lifetime = Duration::from_secs(fetched.expires_in).min(MAX_LIFETIME);
        let renew_at = Instant::now() + lifetime.saturating_sub(EARLY);
        let handed_out = Secret::new(fetched.access_token.expose().clone());
        *cached = Some(Cached {
            token: fetched.access_token,
            renew_at,
        });
        Ok(handed_out)
    }
}
