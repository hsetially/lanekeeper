//! The HTTP leg of the Teams notifier (T7, plan A10, S6, S18): one POST of a card to a webhook URL, and what the answer means.
//!
//! - **TLS 1.3 only (S6).** The same connector as the GitHub client: hyper over rustls on aws-lc-rs, HTTPS only, the platform's
//!   roots.
//! - **Where it may go (S18).** The URL is a secret that an admin configured, and it is still checked before every call: `https`,
//!   the default port, no user, a host on the allowlist (an exact name or `*.suffix`). A redirect is never followed.
//! - **Bounded (rule 5).** The caller times each attempt; here the answer is read up to a cap and dropped.
//! - **No leaks (S10).** The URL carries the credential (its query holds a signature), so nothing here logs it, formats it into an
//!   error, or keeps it in a `Debug`. A failure is one of three fixed outcomes.

use std::time::Duration;

use bytes::Bytes;
use domain::Secret;
use http::{HeaderMap, Method, Request, Uri, header};
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use rustls::RootCertStore;

use crate::error::ConfigError;
use crate::github::client::{native_roots, retry_after, tls13_config};

/// The longest URL read from the secret source.
pub(super) const MAX_URL_BYTES: usize = 2048;

/// What one attempt came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Attempt {
    /// A 2xx: the webhook took the card.
    Delivered,
    /// Worth trying again: the network failed, the answer was a 5xx, 408 or 429. `Some` is the wait that was asked for.
    Retry(Option<Duration>),
    /// Not worth trying again: any other answer, or a destination that may not be called.
    Refused,
}

/// The hosts a webhook URL may name: exact names and `*.suffix` patterns. Each is kept in lower case, with whether it is a suffix
/// pattern.
#[derive(Debug, Clone)]
pub(super) struct Hosts(Vec<(bool, String)>);

/// A host name: dot-separated labels of letters, digits and hyphens, none empty, none starting or ending with a hyphen.
fn is_host_name(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

impl Hosts {
    /// `None` when the list is empty or a pattern is not a bare host name or a `*.` pattern.
    pub fn parse(patterns: &[String]) -> Option<Self> {
        if patterns.is_empty() {
            return None;
        }
        let mut all = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let lower = pattern.to_ascii_lowercase();
            let (suffix, name) = match lower.strip_prefix("*.") {
                Some(rest) => (true, rest.to_owned()),
                None => (false, lower),
            };
            if !is_host_name(&name) {
                return None;
            }
            all.push((suffix, name));
        }
        Some(Self(all))
    }

    /// Whether `host` is one of the names, or a name under one of the suffixes (whole labels only).
    pub fn allows(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.0.iter().any(|(suffix, name)| {
            if *suffix {
                host.len() > name.len() + 1
                    && host.ends_with(name.as_str())
                    && host.as_bytes().get(host.len() - name.len() - 1) == Some(&b'.')
            } else {
                host == *name
            }
        })
    }
}

/// How strict the address check is. Tests talk to a local server, so they may use plain HTTP and any port; the host list still
/// applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Service,
    #[cfg(feature = "test-doubles")]
    Test {
        plain_http: bool,
    },
}

/// The URL, if it is one the notifier may call.
pub(super) fn check_url(url: &str, hosts: &Hosts, mode: Mode) -> Option<Uri> {
    if url.len() > MAX_URL_BYTES
        || url
            .bytes()
            .any(|b| b.is_ascii_control() || b == b' ' || b == b'\\' || b == b'#')
    {
        return None;
    }
    let (plain_ok, any_port) = match mode {
        Mode::Service => (false, false),
        #[cfg(feature = "test-doubles")]
        Mode::Test { plain_http } => (plain_http, true),
    };
    let parsed: Uri = url.parse().ok()?;
    match parsed.scheme_str()? {
        "https" => {}
        "http" if plain_ok => {}
        _ => return None,
    }
    let authority = parsed.authority()?;
    if authority.as_str().contains('@') || !hosts.allows(authority.host()) {
        return None;
    }
    if !any_port && authority.port_u16().is_some_and(|p| p != 443) {
        return None;
    }
    // A webhook has a path; a bare host is a mistake, not a destination.
    (parsed.path().len() > 1).then_some(parsed)
}

type Connector = hyper_rustls::HttpsConnector<HttpConnector>;

/// The client. Holds no URL: each call is given one.
pub(super) struct Webhook {
    http: Client<Connector, Full<Bytes>>,
    hosts: Hosts,
    mode: Mode,
    max_response: usize,
}

impl std::fmt::Debug for Webhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Webhook").finish_non_exhaustive()
    }
}

impl Webhook {
    /// The client for Teams: TLS 1.3 against the platform's roots, HTTPS only, to an allowed host on the default port.
    pub fn service(hosts: Hosts, max_response: usize) -> Result<Self, ConfigError> {
        let roots = native_roots().map_err(|_| ConfigError::Invalid("no usable root certificates"))?;
        Self::build(hosts, Mode::Service, max_response, roots, true)
    }

    /// A client for the tests' local server: any port, and plain HTTP when asked. With roots, it speaks TLS 1.3 to a server
    /// that has one of them. Not in any build of the hub.
    #[cfg(feature = "test-doubles")]
    pub fn for_tests(
        hosts: Hosts,
        max_response: usize,
        plain_http: bool,
        roots: Option<RootCertStore>,
    ) -> Result<Self, ConfigError> {
        let https_only = !plain_http;
        Self::build(
            hosts,
            Mode::Test { plain_http },
            max_response,
            roots.unwrap_or_else(RootCertStore::empty),
            https_only,
        )
    }

    fn build(
        hosts: Hosts,
        mode: Mode,
        max_response: usize,
        roots: RootCertStore,
        https_only: bool,
    ) -> Result<Self, ConfigError> {
        let tls = tls13_config(roots).map_err(|_| ConfigError::Invalid("TLS 1.3 could not be set up"))?;
        let builder = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls);
        let connector = if https_only {
            builder.https_only().enable_http1().build()
        } else {
            builder.https_or_http().enable_http1().build()
        };
        let http = Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(2)
            .build(connector);
        Ok(Self {
            http,
            hosts,
            mode,
            max_response,
        })
    }

    /// Whether `url` is an address this client may call.
    pub fn accepts(&self, url: &str) -> bool {
        check_url(url, &self.hosts, self.mode).is_some()
    }

    /// POST `body` to `url`. One attempt, no timeout of its own: the caller bounds it.
    pub async fn post(&self, url: &Secret<String>, body: Bytes) -> Attempt {
        // Checked again here, so no path leads a card to a host the allowlist does not name.
        let Some(target) = check_url(url.expose(), &self.hosts, self.mode) else {
            tracing::warn!("a webhook address was refused");
            return Attempt::Refused;
        };
        let Ok(request) = Request::builder()
            .method(Method::POST)
            .uri(target)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::USER_AGENT, "lanekeeper-hub")
            .body(Full::new(body))
        else {
            return Attempt::Refused;
        };
        // What hyper says about a failure names the address; only that it failed is kept.
        let Ok(response) = self.http.request(request).await else {
            tracing::debug!("a webhook request failed");
            return Attempt::Retry(None);
        };
        let (parts, body) = response.into_parts();
        // The answer is not needed. It is read up to the cap, so the connection can be reused, and then dropped.
        let _ = Limited::new(body, self.max_response).collect().await;
        classify(parts.status.as_u16(), &parts.headers)
    }
}

/// What an answer means.
fn classify(status: u16, headers: &HeaderMap) -> Attempt {
    match status {
        200..=299 => Attempt::Delivered,
        429 => Attempt::Retry(retry_after(headers)),
        408 | 500..=599 => Attempt::Retry(None),
        // Any other answer, a redirect included, is a final no.
        _ => Attempt::Refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(p: &[&str]) -> Hosts {
        Hosts::parse(&p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn exact_and_suffix_patterns_match_whole_labels_only() {
        let h = hosts(&["hooks.example.test", "*.logic.example.test"]);
        assert!(h.allows("hooks.example.test"));
        assert!(h.allows("HOOKS.example.TEST"));
        assert!(h.allows("prod-12.westus.logic.example.test"));
        assert!(h.allows("a.logic.example.test"));
        for no in [
            "logic.example.test",
            "evil-logic.example.test",
            "a.logic.example.test.evil.example",
            "hooks.example.test.evil.example",
            "xhooks.example.test",
            "",
            ".logic.example.test",
            "example.test",
        ] {
            assert!(!h.allows(no), "{no:?} must not be allowed");
        }
    }

    #[test]
    fn bad_patterns_are_refused_at_start() {
        for bad in [
            "",
            "*",
            "*.",
            "**.example.test",
            "a.*.example.test",
            "https://hooks.example.test",
            "hooks.example.test/x",
            "hooks.example.test:443",
            "user@hooks.example.test",
            "hooks example",
            "*example.test",
        ] {
            assert!(Hosts::parse(&[bad.to_owned()]).is_none(), "{bad:?}");
        }
        assert!(
            Hosts::parse(&[]).is_none(),
            "an empty list would allow nothing: a mistake to say so"
        );
    }

    #[test]
    fn the_service_accepts_https_on_the_default_port_to_an_allowed_host_only() {
        let h = hosts(&["*.logic.example.test"]);
        let ok = |u: &str| check_url(u, &h, Mode::Service).is_some();
        assert!(ok("https://prod-1.logic.example.test/workflows/abc?sig=s3cr3t"));
        assert!(ok(
            "https://prod-1.logic.example.test:443/workflows/abc?sig=s3cr3t"
        ));
        for bad in [
            "http://prod-1.logic.example.test/w",
            "https://prod-1.logic.example.test:8443/w",
            "https://evil.example/w",
            "https://logic.example.test/w",
            "https://user@prod-1.logic.example.test/w",
            "https://user:pw@prod-1.logic.example.test/w",
            "https://prod-1.logic.example.test.evil.example/w",
            "https://evil.example/?x=prod-1.logic.example.test",
            "https://evil.example#@prod-1.logic.example.test/",
            "https://prod-1.logic.example.test",
            "ftp://prod-1.logic.example.test/w",
            "prod-1.logic.example.test/w",
            "",
            "https://",
            "https://prod-1.logic.example.test/w#frag",
            "https://prod-1.logic.example.test/w\r\nHost: evil",
            "https://prod-1.logic.example.test/w x",
        ] {
            assert!(!ok(bad), "{bad:?} must be refused");
        }
        let long = format!("https://prod-1.logic.example.test/{}", "a".repeat(MAX_URL_BYTES));
        assert!(!ok(&long));
    }

    #[cfg(feature = "test-doubles")]
    #[test]
    fn test_mode_relaxes_the_scheme_and_port_but_not_the_host() {
        let h = hosts(&["127.0.0.1"]);
        let plain = Mode::Test { plain_http: true };
        assert!(check_url("http://127.0.0.1:8080/hook?sig=x", &h, plain).is_some());
        assert!(check_url("http://localhost:8080/hook", &h, plain).is_none());
        assert!(check_url("http://127.0.0.1:8080/hook", &h, Mode::Test { plain_http: false }).is_none());
        assert!(
            check_url(
                "https://127.0.0.1:8080/hook",
                &h,
                Mode::Test { plain_http: false }
            )
            .is_some()
        );
    }
}
