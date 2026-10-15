//! The GitHub client of the write path (T5, plan A9, S6, S8, S18): [`GithubApi`], the calls the hub makes, and
//! [`GithubClient`], their implementation over octocrab.
//!
//! The stack names octocrab, and it is used: it builds the requests, adds the user's token to them and sets the base address.
//! What it does not do, this module does around it:
//!
//! - **TLS 1.3 only (S6).** octocrab's own connector speaks TLS 1.2 as well and also plain HTTP, so it is not used. The client is
//!   given a service of our own: hyper over rustls on aws-lc-rs, TLS 1.3 only, the platform's roots, HTTPS only.
//! - **Where it may go (S18).** The base address is checked at start: `https`, a host on the allowlist (`api.github.com`), no
//!   user, path or query. Redirects are not followed (octocrab's `follow-redirect` is off), so a token is never sent to a host
//!   the allowlist did not name.
//! - **Bounded (rule 5).** A timeout on each attempt and on the whole call, a cap on the body read, and at most three attempts,
//!   with full jitter between them. A rate limit that asks for a longer wait than this call allows is an error, not a sleep.
//! - **No leaks (S10, S21).** Every error is a [`GithubError`], a fixed word. Nothing octocrab says about a failure (its errors
//!   quote URIs and GitHub's messages) is kept, and the token never leaves the one request header octocrab sets, which is marked
//!   sensitive.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{Secret, Timestamp};
use http::{HeaderMap, HeaderName, HeaderValue, Uri, header};
use http_body_util::{BodyExt, Limited};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use octocrab::service::middleware::base_uri::BaseUriLayer;
use octocrab::service::middleware::extra_headers::ExtraHeadersLayer;
use octocrab::{AuthState, OctoBody, Octocrab, OctocrabBuilder};
use rustls::{ClientConfig, RootCertStore};

use super::api::{
    EXPIRY_HEADER, GithubError, MAX_BODY_BYTES, parse_push_permission, parse_user, status_error, token_expiry,
};
use super::types::{GithubLogin, RepoRef};

/// Who a token belongs to, and when it stops working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubUser {
    pub login: GithubLogin,
    /// From the expiry header; `None` when GitHub sent none or sent something unreadable.
    pub expires_at: Option<Timestamp>,
}

/// The calls the hub makes to GitHub for a user's token. Each takes the token as a [`Secret`] and keeps no copy of it.
#[async_trait]
pub trait GithubApi: Send + Sync + 'static {
    /// `GET /user`: the login the token belongs to, and its expiry.
    async fn user(&self, token: &Secret<String>) -> Result<GithubUser, GithubError>;

    /// Whether the token can push to `repo`. A repository the token cannot see is `Ok(false)`, the same answer as one it can see
    /// and not push to: the caller has to be told one thing, "this token cannot push there".
    async fn can_push(&self, token: &Secret<String>, repo: &RepoRef) -> Result<bool, GithubError>;
}

/// Settings of [`GithubClient`]. The defaults are the plan's.
#[derive(Debug, Clone)]
pub struct GithubClientConfig {
    /// `https://api.github.com`. The origin only.
    pub api_base: String,
    /// The hosts `api_base` may name.
    pub allowed_hosts: Vec<String>,
    /// One attempt: connect, handshake, request and the whole response.
    pub attempt_timeout: Duration,
    /// All attempts and the waits between them.
    pub total_timeout: Duration,
    pub max_attempts: u32,
    /// The wait before the second attempt is at most this, and doubles each time, up to `backoff_cap`; the wait itself is a
    /// random time up to that (full jitter).
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
    /// The longest `Retry-After` this call waits for. A longer one is [`GithubError::RateLimited`].
    pub max_rate_wait: Duration,
    pub max_body: usize,
}

impl Default for GithubClientConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.github.com".to_owned(),
            allowed_hosts: vec!["api.github.com".to_owned()],
            attempt_timeout: Duration::from_secs(8),
            total_timeout: Duration::from_secs(20),
            max_attempts: 3,
            backoff_base: Duration::from_millis(200),
            backoff_cap: Duration::from_secs(2),
            max_rate_wait: Duration::from_secs(2),
            max_body: MAX_BODY_BYTES,
        }
    }
}

/// The base address, if it is one the client may use: `https` (or, for the tests that ask, `http`), an allowed host, the default
/// port, and no user, path, query or fragment.
fn checked_base(cfg: &GithubClientConfig, plain_http: bool) -> Result<Uri, GithubError> {
    let uri: Uri = cfg.api_base.parse().map_err(|_| GithubError::Config)?;
    let scheme_ok = match uri.scheme_str() {
        Some("https") => true,
        Some("http") => plain_http,
        _ => false,
    };
    let authority = uri.authority().ok_or(GithubError::Config)?;
    let host = authority.host().to_ascii_lowercase();
    let host_ok = plain_http || cfg.allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host));
    let port_ok = plain_http || authority.port_u16().is_none_or(|p| p == 443);
    let bare = uri.path() == "/" && uri.query().is_none() && !authority.as_str().contains('@');
    if scheme_ok && host_ok && port_ok && bare && !cfg.api_base.contains(['?', '#']) {
        Ok(uri)
    } else {
        Err(GithubError::Config)
    }
}

/// A client configuration: TLS 1.3 and nothing older (S6), the given roots and no client certificate.
fn tls13_config(roots: RootCertStore) -> Result<ClientConfig, GithubError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    // No ALPN here: the connector builder sets `http/1.1` itself (`enable_http1`) and refuses a configuration that has any.
    Ok(ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| GithubError::Config)?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

fn native_roots() -> Result<RootCertStore, GithubError> {
    let loaded = rustls_native_certs::load_native_certs();
    let mut store = RootCertStore::empty();
    // One unreadable certificate in the system bundle must not stop the hub, but an empty store must.
    let (added, _ignored) = store.add_parsable_certificates(loaded.certs);
    if added == 0 {
        return Err(GithubError::Config);
    }
    Ok(store)
}

/// What one request got back.
struct Reply {
    status: u16,
    headers: HeaderMap,
    body: Bytes,
}

/// The GitHub client.
#[derive(Clone)]
pub struct GithubClient {
    octo: Octocrab,
    cfg: GithubClientConfig,
}

impl fmt::Debug for GithubClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GithubClient").finish_non_exhaustive()
    }
}

impl GithubClient {
    /// The client for GitHub: TLS 1.3 against the platform's roots, HTTPS only, to an allowed host.
    pub fn new(cfg: GithubClientConfig) -> Result<Self, GithubError> {
        let base = checked_base(&cfg, false)?;
        let tls = tls13_config(native_roots()?)?;
        Self::build(cfg, base, tls, true)
    }

    /// A client for the tests' local server: plain HTTP to any host. Not in any build of the hub.
    #[cfg(feature = "test-doubles")]
    pub fn for_tests(base: &str, cfg: GithubClientConfig) -> Result<Self, GithubError> {
        let cfg = GithubClientConfig {
            api_base: base.to_owned(),
            ..cfg
        };
        let base = checked_base(&cfg, true)?;
        let tls = tls13_config(RootCertStore::empty())?;
        Self::build(cfg, base, tls, false)
    }

    /// The same client with the roots and the address the test chooses, over TLS 1.3 as in service: for the test that checks a
    /// server which speaks TLS 1.2 only is refused. Not in any build of the hub.
    #[cfg(feature = "test-doubles")]
    pub fn for_tests_with_roots(
        base: &str,
        roots: RootCertStore,
        cfg: GithubClientConfig,
    ) -> Result<Self, GithubError> {
        let cfg = GithubClientConfig {
            api_base: base.to_owned(),
            ..cfg
        };
        let base = checked_base(&cfg, true)?;
        Self::build(cfg, base, tls13_config(roots)?, true)
    }

    fn build(
        cfg: GithubClientConfig,
        base: Uri,
        tls: ClientConfig,
        https_only: bool,
    ) -> Result<Self, GithubError> {
        let builder = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls);
        let connector = if https_only {
            builder.https_only().enable_http1().build()
        } else {
            builder.https_or_http().enable_http1().build()
        };
        let http: Client<_, OctoBody> = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(30))
            .build(connector);
        let headers = vec![
            (header::USER_AGENT, HeaderValue::from_static("lanekeeper-hub")),
            (
                header::ACCEPT,
                HeaderValue::from_static("application/vnd.github+json"),
            ),
            (
                HeaderName::from_static("x-github-api-version"),
                HeaderValue::from_static("2022-11-28"),
            ),
        ];
        let octo = OctocrabBuilder::new_empty()
            .with_service(http)
            .with_layer(&ExtraHeadersLayer::new(Arc::new(headers)))
            .with_layer(&BaseUriLayer::new(base))
            .with_auth(AuthState::None)
            .build()
            .map_err(|never| match never {})?;
        Ok(Self { octo, cfg })
    }

    /// One attempt: the request with the user's token, and the answer read up to the cap.
    async fn once(&self, token: &Secret<String>, path: &str) -> Result<Reply, GithubError> {
        // octocrab keeps the token as a `SecretString`, which zeroizes when dropped with the clone.
        let octo = self
            .octo
            .user_access_token(token.expose().clone())
            .map_err(|_| GithubError::Config)?;
        let response = octo._get(path.to_owned()).await.map_err(|_| {
            // Its error quotes URIs and GitHub's message: only the class is kept.
            tracing::warn!("github request failed");
            GithubError::Unavailable
        })?;
        let (parts, body) = response.into_parts();
        let status = parts.status.as_u16();
        // Only a 200 has a body this module reads. The rest is dropped unread.
        let body = if status == 200 {
            match Limited::new(body, self.cfg.max_body).collect().await {
                Ok(all) => all.to_bytes(),
                Err(e) => {
                    return Err(if e.is::<http_body_util::LengthLimitError>() {
                        GithubError::Unexpected
                    } else {
                        GithubError::Unavailable
                    });
                }
            }
        } else {
            Bytes::new()
        };
        Ok(Reply {
            status,
            headers: parts.headers,
            body,
        })
    }

    /// `GET path` with bounded retries. A reply is returned when its status is 200; every other outcome is an error.
    async fn get(&self, token: &Secret<String>, path: &str) -> Result<Reply, GithubError> {
        let run = async {
            let mut attempt = 0_u32;
            loop {
                attempt += 1;
                let last = attempt >= self.cfg.max_attempts;
                let once = tokio::time::timeout(self.cfg.attempt_timeout, self.once(token, path))
                    .await
                    .unwrap_or_else(|_| {
                        tracing::warn!("github request timed out");
                        Err(GithubError::Unavailable)
                    });
                let reply = match once {
                    Ok(reply) => reply,
                    Err(GithubError::Unavailable) if !last => {
                        self.pause(attempt, None).await;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let limited = reply
                    .headers
                    .get("x-ratelimit-remaining")
                    .is_some_and(|v| v == "0")
                    || reply.headers.contains_key(header::RETRY_AFTER);
                match status_error(reply.status, limited) {
                    None => return Ok(reply),
                    Some(GithubError::RateLimited) => {
                        let wait = retry_after(&reply.headers);
                        if !last && wait.is_some_and(|w| w <= self.cfg.max_rate_wait) {
                            self.pause(attempt, wait).await;
                            continue;
                        }
                        return Err(GithubError::RateLimited);
                    }
                    Some(GithubError::Unavailable) if !last => self.pause(attempt, None).await,
                    Some(e) => return Err(e),
                }
            }
        };
        tokio::time::timeout(self.cfg.total_timeout, run)
            .await
            .unwrap_or(Err(GithubError::Unavailable))
    }

    /// The wait between two attempts: what GitHub asked for, or a random time up to a doubling bound (full jitter).
    async fn pause(&self, attempt: u32, asked: Option<Duration>) {
        let wait = asked.unwrap_or_else(|| {
            let bound = self
                .cfg
                .backoff_base
                .saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1)))
                .min(self.cfg.backoff_cap);
            let ms = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX);
            Duration::from_millis(fastrand::u64(0..=ms))
        });
        tokio::time::sleep(wait).await;
    }
}

/// `Retry-After` in whole seconds. A date, or anything else, is not read (the call then does not wait).
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

#[async_trait]
impl GithubApi for GithubClient {
    async fn user(&self, token: &Secret<String>) -> Result<GithubUser, GithubError> {
        let reply = self.get(token, "/user").await?;
        let login = parse_user(&reply.body)?;
        let expires_at = token_expiry(reply.headers.get(EXPIRY_HEADER).map(HeaderValue::as_bytes));
        Ok(GithubUser { login, expires_at })
    }

    async fn can_push(&self, token: &Secret<String>, repo: &RepoRef) -> Result<bool, GithubError> {
        match self.get(token, &repo.api_path()).await {
            Ok(reply) => parse_push_permission(&reply.body),
            // A fine-grained token that was not given the repository gets 404 (or 403): it cannot push there.
            Err(GithubError::NotFound | GithubError::Forbidden) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(base: &str) -> GithubClientConfig {
        GithubClientConfig {
            api_base: base.to_owned(),
            ..GithubClientConfig::default()
        }
    }

    #[test]
    fn the_base_must_be_https_and_an_allowed_host() {
        assert!(checked_base(&cfg("https://api.github.com"), false).is_ok());
        assert!(checked_base(&cfg("https://api.github.com/"), false).is_ok());
        assert!(checked_base(&cfg("https://API.GitHub.com:443"), false).is_ok());
        for bad in [
            "http://api.github.com",
            "https://github.com",
            "https://api.github.com.evil.example",
            "https://evil.example/api.github.com",
            "https://api.github.com:8443",
            "https://user@api.github.com",
            "https://api.github.com/v3",
            "https://api.github.com/?x=1",
            "https://api.github.com/#f",
            "ftp://api.github.com",
            "api.github.com",
            "",
            "https://",
        ] {
            assert_eq!(
                checked_base(&cfg(bad), false).unwrap_err(),
                GithubError::Config,
                "{bad}"
            );
        }
        // A local test server is allowed only when the caller says it is a test.
        assert!(checked_base(&cfg("http://127.0.0.1:8080"), true).is_ok());
        assert!(checked_base(&cfg("http://127.0.0.1:8080"), false).is_err());
    }

    #[test]
    fn retry_after_is_whole_seconds() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("3"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(3)));
        h.insert(
            header::RETRY_AFTER,
            HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
        );
        assert_eq!(retry_after(&h), None);
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("-1"));
        assert_eq!(retry_after(&h), None);
    }
}
