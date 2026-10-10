//! The config-server calls (T12, D83, D86, D88, Q37, S17): asking it to refresh, and asking it what it serves.
//!
//! The config-server has no authentication, so these two calls are the only way the agent touches it, and each is the
//! answer to one command from the hub (which has checked the user's role and written the audit event). Nothing in the agent
//! calls it on its own; `source_rules.rs` fails if another file names the endpoint or calls [`ConfigServerClient::notify`].
//!
//! # What goes out
//!
//! - **Refresh** ([`ConfigServerClient::notify`]): `POST /update-resources` with the header `backend: filesystem` and a form
//!   that repeats the field `path`, one per file, relative to the config root. Five seconds per attempt, three attempts, a
//!   full-jitter pause between them. Only a failure to get an answer is retried: whatever status the server answers is
//!   the answer, and goes back to the hub unchanged.
//! - **Served file** ([`ConfigServerClient::fetch_served`]): `GET /{application}/{tenant},default/master[/{channel}]/{file}`,
//!   asking for `application/octet-stream` when the file is binary. Five seconds, one attempt. At most
//!   [`FETCH_PER_SECOND`] a second for the whole agent; past that the answer is `429` with no body and the config-server
//!   is not asked. A response over [`MAX_RESPONSE_BYTES`] is refused whole, never cut.
//!
//! # Where it goes
//!
//! To the address the agent was configured with (`LK_CONFIG_SERVER_URL`) and nowhere else. The request target is built
//! from validated pieces and every byte that is not unreserved is percent-encoded, so a name cannot add a segment, a
//! query, a fragment or a second host. Redirects are never followed.
//!
//! # What does not come back
//!
//! - **A denied path.** `FetchServed` follows the same [`DenyList`] as every other path to a file (D79, T11): a name the
//!   list denies is refused before anything is asked of the config-server, and the answer is the bare code `DENIED`, with
//!   no name in it. The names checked are the ones the config-server could resolve the request to: the file, with the
//!   application's folder in front, with the channel's folder in the file's folder, and with the tenant's copy
//!   (`name-<tenant>.ext`, which the server prefers).
//! - **The text of an error.** Only a `2xx` carries a body. The body of a `404` or a `500` is the server's own page; it is
//!   not passed on, and neither is the text of an error from the transport. A reply carries a status or a code.
//!
//! The hub is told the config-server's status only when the server answered; a failure to connect, a timeout, a peer that
//! does not speak HTTP or a status outside 100 to 599 is `IO`.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use domain::{NfsPath, OpError, ServeRequest};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::backoff::Backoff;
use crate::clock::Clock;
use crate::config::BaseUrl;
use crate::deny::DenyList;
use crate::http::{HttpClient, HttpError, HttpRequest};
use crate::transport::dial::{Dialer, TcpDialer};

/// The config-server's refresh endpoint. Named here and nowhere else in `src/` (`source_rules.rs`).
pub const NOTIFY_PATH: &str = "/update-resources";
/// The largest response to a `FetchServed` the agent takes.
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// The most of a refresh's answer that is read. It is never used, only its status.
pub const NOTIFY_RESPONSE_BYTES: usize = 64 * 1024;
/// The most `FetchServed` requests the agent makes in any one second.
pub const FETCH_PER_SECOND: usize = 5;
/// The window of that limit.
pub const RATE_WINDOW: Duration = Duration::from_secs(1);

/// How long the agent waits, and how often it tries again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// The time one request may take, from connecting to the last byte of the answer.
    pub timeout: Duration,
    /// The attempts a refresh gets in all (the first and the retries).
    pub attempts: u32,
    /// The pause before the second attempt is drawn from `0..=backoff_base`, before the third from `0..=2 * backoff_base`,
    /// and so on up to `backoff_cap`.
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            attempts: 3,
            backoff_base: Duration::from_millis(200),
            backoff_cap: Duration::from_secs(2),
        }
    }
}

/// Why a call to the config-server gave no answer to pass on. None of these carries a path, a URL or the server's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigServerError {
    /// The deny list covers the path.
    #[error("the path is denied")]
    Denied,
    /// No answer: could not connect, timed out (retries used up), a peer that does not speak HTTP, or a status the wire
    /// cannot carry.
    #[error("the config-server could not be reached")]
    Unreachable,
    /// The response is over [`MAX_RESPONSE_BYTES`].
    #[error("the response is larger than the agent takes")]
    TooLarge,
    /// The request could not be made: it names no file.
    #[error("the request is not valid")]
    InvalidRequest,
}

impl ConfigServerError {
    /// The wire code the hub is told.
    pub fn code(self) -> OpError {
        match self {
            Self::Denied | Self::InvalidRequest => OpError::Denied,
            Self::Unreachable => OpError::Io,
            Self::TooLarge => OpError::Unsupported,
        }
    }
}

/// What the config-server served: its status and, for a `2xx`, the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    pub status: u16,
    pub body: Bytes,
}

// -------------------------------------------------------------------------------------------------- rate limit

/// At most `max` requests in any window: the instants of the last `max` requests, so that no window of `window` holds more.
///
/// Not a token bucket: a bucket that has filled up lets `max` through at once and then `max` a second, twice the limit over
/// a window.
#[derive(Debug)]
pub struct RateLimiter {
    clock: Arc<dyn Clock>,
    max: usize,
    window: Duration,
    recent: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    pub fn new(clock: Arc<dyn Clock>, max: usize, window: Duration) -> Self {
        Self {
            clock,
            max,
            window,
            recent: Mutex::new(VecDeque::with_capacity(max)),
        }
    }

    /// Take one request's place in the window, or say there is none.
    pub fn try_acquire(&self) -> bool {
        let now = self.clock.instant();
        let mut recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
        while recent
            .front()
            .is_some_and(|first| now.saturating_duration_since(*first) >= self.window)
        {
            recent.pop_front();
        }
        if recent.len() >= self.max {
            return false;
        }
        recent.push_back(now);
        true
    }

    /// The requests remembered, which is never more than `max`.
    pub fn tracked(&self) -> usize {
        self.recent.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

// -------------------------------------------------------------------------------------------------- the request

/// Percent-encode `text` into `out`: every byte except the unreserved ones (`A-Z a-z 0-9 - . _ ~`) becomes `%XX`. Used for
/// the segments of a path and for the values of a form, so that nothing a name holds can be taken for syntax.
fn encode_into(out: &mut String, text: &str) {
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

/// The form body of a refresh: `path=<p1>&path=<p2>...`, each path relative to the config root, form-encoded.
pub fn notify_body(paths: &[NfsPath]) -> String {
    let mut body = String::with_capacity(paths.iter().map(|p| p.as_str().len() + 8).sum());
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            body.push('&');
        }
        body.push_str("path=");
        encode_into(&mut body, path.as_str());
    }
    body
}

/// The request target for `request`: `/{application}/{tenant},default/master[/{channel}]/{file}`.
///
/// Every piece is encoded; the file is encoded component by component, so that a `/` in the target is always a separator
/// the agent wrote.
pub fn served_target(request: &ServeRequest) -> String {
    let mut target = String::with_capacity(64 + request.file.as_str().len() * 3);
    target.push('/');
    encode_into(&mut target, request.application.as_str());
    target.push('/');
    encode_into(&mut target, request.tenant.as_str());
    target.push_str(",default/master");
    if let Some(channel) = &request.channel {
        target.push('/');
        encode_into(&mut target, channel.as_str());
    }
    for component in request.file.components() {
        target.push('/');
        encode_into(&mut target, component);
    }
    target
}

/// Text files are asked for as text, which is rendered (`${KEY}` replaced, D82); every other file as bytes, which is not.
/// The classes are those of `docs/domain-model.md`: `.yml .yaml .json .properties .xsl .xml .txt` and files without an
/// extension are text, everything else is binary.
pub fn is_binary(file: &NfsPath) -> bool {
    const TEXT: [&str; 7] = ["yml", "yaml", "json", "properties", "xsl", "xml", "txt"];
    match extension(file.file_name()) {
        Some(ext) => !TEXT.iter().any(|t| ext.eq_ignore_ascii_case(t)),
        None => false,
    }
}

/// The text after the last dot of a file name, if the dot is neither the first nor the last character.
fn extension(name: &str) -> Option<&str> {
    let dot = name.rfind('.')?;
    (dot > 0 && dot + 1 < name.len()).then(|| &name[dot + 1..])
}

/// `name` with the tenant's suffix before the extension (`tx-infinity-core.yml` becomes `tx-infinity-core-sit1.yml`), or at
/// the end for a name without one (D13).
fn tenant_copy(name: &str, tenant: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => format!("{}-{tenant}{}", &name[..dot], &name[dot..]),
        _ => format!("{name}-{tenant}"),
    }
}

/// The paths a request could be answered from, as the deny list sees them: the file as named, and under the application's
/// folder, each with the channel's folder inside the file's folder and with the tenant's copy of the name.
fn candidates(request: &ServeRequest) -> Vec<String> {
    let file = request.file.as_str();
    let (dir, name) = file.rsplit_once('/').map_or(("", file), |(d, n)| (d, n));
    let tenant = request.tenant.as_str();
    let names = [name.to_owned(), tenant_copy(name, tenant)];
    let mut folders = vec![dir.to_owned()];
    if let Some(channel) = &request.channel {
        folders.push(if dir.is_empty() {
            channel.as_str().to_owned()
        } else {
            format!("{dir}/{}", channel.as_str())
        });
    }
    let mut out = Vec::with_capacity(folders.len() * names.len() * 2);
    for folder in &folders {
        for name in &names {
            let rel = if folder.is_empty() {
                name.clone()
            } else {
                format!("{folder}/{name}")
            };
            out.push(format!("{}/{rel}", request.application.as_str()));
            out.push(rel);
        }
    }
    out
}

/// The status of an answer, if the wire can carry it.
fn valid_status(status: u16) -> Result<u16, ConfigServerError> {
    if (100..=599).contains(&status) {
        Ok(status)
    } else {
        Err(ConfigServerError::Unreachable)
    }
}

// -------------------------------------------------------------------------------------------------- the client

/// Calls the config-server at one address. Cheap to share: it holds no connection (every request opens its own, which the
/// config-server closes after the answer).
#[derive(Debug)]
pub struct ConfigServerClient {
    base: BaseUrl,
    timing: Timing,
    dialer: Arc<dyn Dialer>,
    deny: DenyList,
    limiter: RateLimiter,
}

impl ConfigServerClient {
    /// A client for `base`. `deny` is the agent's one deny list, so a glob the hub adds applies at once.
    pub fn new(base: BaseUrl, deny: DenyList, clock: Arc<dyn Clock>) -> Self {
        Self {
            base,
            timing: Timing::default(),
            dialer: Arc::new(TcpDialer),
            deny,
            limiter: RateLimiter::new(clock, FETCH_PER_SECOND, RATE_WINDOW),
        }
    }

    /// Other timeouts and retries than the defaults (for a test).
    #[must_use]
    pub fn with_timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
    }

    /// Connect through `dialer` instead of TCP (for a test with an in-memory server).
    #[must_use]
    pub fn with_dialer(mut self, dialer: Arc<dyn Dialer>) -> Self {
        self.dialer = dialer;
        self
    }

    fn http(&self, max_body: usize) -> HttpClient {
        HttpClient::new(self.timing.timeout, max_body).with_dialer(Arc::clone(&self.dialer))
    }

    /// Ask the config-server to refresh `paths`. Returns the status it answered, whatever it is.
    ///
    /// A failure to get an answer is tried again, up to [`Timing::attempts`] attempts in all, with a pause drawn with full
    /// jitter. A refresh is safe to repeat, which is why a request that timed out may be sent again.
    pub async fn notify(&self, paths: &[NfsPath]) -> Result<u16, ConfigServerError> {
        let body = Bytes::from(notify_body(paths));
        let mut backoff = Backoff::new(self.timing.backoff_base, self.timing.backoff_cap);
        let attempts = self.timing.attempts.max(1);
        for attempt in 1..=attempts {
            let request = HttpRequest::post(NOTIFY_PATH, body.clone())
                .with_header("backend", "filesystem")
                .and_then(|r| r.with_header("content-type", "application/x-www-form-urlencoded"))
                .map_err(|_| ConfigServerError::InvalidRequest)?;
            match self.http(NOTIFY_RESPONSE_BYTES).send(&self.base, request).await {
                Ok(response) => return valid_status(response.status),
                Err(HttpError::InvalidRequest) => return Err(ConfigServerError::InvalidRequest),
                Err(error) => {
                    debug!(
                        attempt,
                        attempts,
                        ?error,
                        "the config-server gave no answer to a refresh"
                    );
                    if attempt < attempts {
                        tokio::time::sleep(backoff.next_delay()).await;
                    }
                }
            }
        }
        Err(ConfigServerError::Unreachable)
    }

    /// Ask the config-server for what it serves for `request`.
    ///
    /// Denied paths are refused first and cost nothing from the rate limit; then the limit is taken, and a request over it
    /// is answered here with status `429` and no body.
    pub async fn fetch_served(&self, request: &ServeRequest) -> Result<Served, ConfigServerError> {
        if request.file.is_root() {
            return Err(ConfigServerError::InvalidRequest);
        }
        let snapshot = self.deny.snapshot();
        if candidates(request).iter().any(|rel| snapshot.is_denied(rel)) {
            info!("a fetch of a denied path was refused");
            return Err(ConfigServerError::Denied);
        }
        if !self.limiter.try_acquire() {
            warn!(
                limit = FETCH_PER_SECOND,
                "too many fetches in a second; answered 429 here"
            );
            return Ok(Served {
                status: 429,
                body: Bytes::new(),
            });
        }
        let mut outgoing = HttpRequest::get(served_target(request));
        if is_binary(&request.file) {
            outgoing = outgoing
                .with_header("accept", "application/octet-stream")
                .map_err(|_| ConfigServerError::InvalidRequest)?;
        }
        match self.http(MAX_RESPONSE_BYTES).send(&self.base, outgoing).await {
            Ok(response) => {
                let status = valid_status(response.status)?;
                // Only a success carries a file; any other body is the server's own page.
                let body = if (200..300).contains(&status) {
                    response.body
                } else {
                    Bytes::new()
                };
                Ok(Served { status, body })
            }
            Err(HttpError::TooLarge) => Err(ConfigServerError::TooLarge),
            Err(HttpError::InvalidRequest) => Err(ConfigServerError::InvalidRequest),
            Err(error) => {
                debug!(?error, "the config-server gave no answer to a fetch");
                Err(ConfigServerError::Unreachable)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use domain::{AppName, ChannelName, TenantId};
    use proptest::prelude::*;

    use super::*;
    use crate::clock::SystemClock;

    fn req(app: &str, tenant: &str, channel: Option<&str>, file: &str) -> ServeRequest {
        ServeRequest {
            application: AppName::parse(app).unwrap(),
            tenant: TenantId::parse(tenant).unwrap(),
            channel: channel.map(|c| ChannelName::parse(c).unwrap()),
            file: NfsPath::parse(file).unwrap(),
        }
    }

    #[test]
    fn encoding_keeps_unreserved_bytes_and_escapes_the_rest() {
        let mut out = String::new();
        encode_into(&mut out, "aZ09-._~ /?#%&=+,;é");
        assert_eq!(out, "aZ09-._~%20%2F%3F%23%25%26%3D%2B%2C%3B%C3%A9");
    }

    #[test]
    fn the_target_has_the_documented_shape() {
        assert_eq!(
            served_target(&req("tx-infinity-api", "sit1", None, "tx-infinity-core.yml")),
            "/tx-infinity-api/sit1,default/master/tx-infinity-core.yml"
        );
        assert_eq!(
            served_target(&req("app", "sit1", Some("remote-itm-teller"), "xsl/a b.xsl")),
            "/app/sit1,default/master/remote-itm-teller/xsl/a%20b.xsl"
        );
    }

    #[test]
    fn extensions_and_tenant_copies() {
        assert_eq!(extension("a.yml"), Some("yml"));
        assert_eq!(extension("a.b.yml"), Some("yml"));
        assert_eq!(extension(".hidden"), None);
        assert_eq!(extension("trailing."), None);
        assert_eq!(extension("noext"), None);
        assert_eq!(
            tenant_copy("tx-infinity-core.yml", "sit1"),
            "tx-infinity-core-sit1.yml"
        );
        assert_eq!(tenant_copy("a.b.yml", "sit1"), "a.b-sit1.yml");
        assert_eq!(tenant_copy("noext", "sit1"), "noext-sit1");
        assert_eq!(tenant_copy(".hidden", "sit1"), ".hidden-sit1");
        assert_eq!(tenant_copy("trailing.", "sit1"), "trailing.-sit1");
    }

    #[test]
    fn candidates_cover_the_folders_and_the_tenant_copy() {
        let mut got = candidates(&req("app", "sit1", Some("ch"), "xsl/print.xsl"));
        got.sort();
        let mut want = vec![
            "xsl/print.xsl",
            "app/xsl/print.xsl",
            "xsl/print-sit1.xsl",
            "app/xsl/print-sit1.xsl",
            "xsl/ch/print.xsl",
            "app/xsl/ch/print.xsl",
            "xsl/ch/print-sit1.xsl",
            "app/xsl/ch/print-sit1.xsl",
        ];
        want.sort_unstable();
        assert_eq!(got, want);
        // A file in the root, with no channel.
        let mut got = candidates(&req("app", "sit1", None, "a.yml"));
        got.sort();
        assert_eq!(got, ["a-sit1.yml", "a.yml", "app/a-sit1.yml", "app/a.yml"]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_limiter_frees_a_place_when_the_oldest_request_is_a_window_old() {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let limiter = RateLimiter::new(clock, 2, Duration::from_secs(1));
        assert!(limiter.try_acquire());
        tokio::time::advance(Duration::from_millis(400)).await;
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());
        tokio::time::advance(Duration::from_millis(599)).await;
        assert!(!limiter.try_acquire(), "999 ms after the first");
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(limiter.try_acquire(), "a second after the first");
        assert!(
            !limiter.try_acquire(),
            "and the second request is still inside the window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_limiter_of_zero_lets_nothing_through() {
        let limiter = RateLimiter::new(Arc::new(SystemClock), 0, Duration::from_secs(1));
        assert!(!limiter.try_acquire());
        assert_eq!(limiter.tracked(), 0);
    }

    proptest! {
        /// Whatever the pieces, the target is origin-form ASCII with no query or fragment, and its structure is the
        /// agent's: the number of `/` is the number of pieces.
        #[test]
        fn a_target_never_changes_shape(
            file in "[^\\\\\u{0}-\u{1f}\u{7f}]{1,60}",
            channel in proptest::option::of("[A-Za-z0-9_-][A-Za-z0-9._-]{0,20}"),
        ) {
            let Ok(file) = NfsPath::parse(&file) else { return Ok(()); };
            let request = ServeRequest {
                application: AppName::parse("tx-infinity-api").unwrap(),
                tenant: TenantId::parse("sit1").unwrap(),
                channel: channel.as_deref().map(|c| ChannelName::parse(c).unwrap()),
                file,
            };
            let target = served_target(&request);
            prop_assert!(target.starts_with("/tx-infinity-api/sit1,default/master/"));
            prop_assert!(target.is_ascii());
            for bad in ['?', '#', ' ', '\\', '"', '<', '>', '\'', '`', '{', '}', '|', '^'] {
                prop_assert!(!target.contains(bad), "{bad:?} in {target}");
            }
            prop_assert!(!target.contains("//"));
            let pieces = 3 + usize::from(request.channel.is_some()) + request.file.components().count();
            prop_assert_eq!(target.matches('/').count(), pieces);
            prop_assert!(target.split('/').all(|s| s != ".." && s != "."));
            // The same target as the HTTP client accepts.
            prop_assert!(target.parse::<http::Uri>().is_ok());
        }

        #[test]
        fn a_form_body_has_one_field_per_path_and_nothing_else(
            paths in proptest::collection::vec("[^\\\\\u{0}-\u{1f}\u{7f}]{1,40}", 0..8),
        ) {
            let parsed: Vec<NfsPath> = paths.iter().filter_map(|p| NfsPath::parse(p).ok()).collect();
            let body = notify_body(&parsed);
            prop_assert!(body.is_ascii());
            if parsed.is_empty() {
                prop_assert_eq!(body, "");
            } else {
                prop_assert_eq!(body.matches('&').count(), parsed.len() - 1);
                prop_assert_eq!(body.matches("path=").count(), parsed.len());
                prop_assert_eq!(body.matches('=').count(), parsed.len());
            }
        }
    }
}
