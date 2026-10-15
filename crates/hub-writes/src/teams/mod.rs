//! The Teams notifier (T7, D49, Q9, plan A10): [`TeamsNotifier`], the [`ports::Notifier`] that posts an Adaptive Card to a Teams
//! Workflows webhook.
//!
//! - **Where a notification goes.** `notification_settings` says, for each kind, whether it is on, the lowest severity worth a
//!   message and the *name* of a secret; the secret source returns the webhook URL (S8). Both are read at most once a minute per
//!   kind. A kind with no row, no destination, or one that is off sends nothing and is not an error: Q9's default is a notifier
//!   that does nothing until an admin configures it.
//! - **Never in the way.** [`ports::Notifier::notify`] only puts the notification on a bounded queue and returns; a worker sends.
//!   A slow or dead webhook therefore costs the caller nothing, and a full queue drops the notification (and says so) rather than
//!   wait. A failure to send is logged and never reaches the action that caused the notification (D49).
//! - **Bounded and polite.** Each delivery is at most three attempts with full-jitter backoff, each attempt and the whole delivery
//!   have a timeout, and a breaker per kind stops calling a webhook that keeps failing ([`breaker`]).
//! - **No leaks (S10).** The URL is a credential. It is a `Secret` from the secret source to the socket, it is checked against an
//!   allowlist of hosts ([`webhook`]), and nothing logs it, formats it into an error or keeps it where `Debug` would show it.

mod breaker;
mod cards;
mod webhook;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{Secret, Severity};
use ports::{Notification, NotificationKind, Notifier, NotifyError, SecretError, SecretSource};
use sqlx::{PgPool, Row};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::{JoinHandle, JoinSet};

use self::breaker::Breaker;
pub use self::breaker::{BreakerConfig, BreakerState};
use self::webhook::{Attempt, Hosts, Webhook};
use crate::clock::Clock;
use crate::error::ConfigError;

/// Settings of the notifier. The defaults are the plan's.
#[derive(Debug, Clone)]
pub struct TeamsConfig {
    /// The hosts a webhook URL may name: exact names and `*.suffix`. The defaults are the hosts Teams Workflows webhooks live on
    /// (Q9 has not named the tenant's, so an admin may need to add one).
    pub allowed_hosts: Vec<String>,
    /// The origin of the web app (`https://host`), for the button of a card. Without it a card has no button.
    pub web_base_url: Option<String>,
    /// One attempt: connect, handshake, request and the answer.
    pub attempt_timeout: Duration,
    /// All the attempts of a delivery and the waits between them.
    pub total_timeout: Duration,
    pub max_attempts: u32,
    /// The wait before the second attempt is at most this, and doubles each time up to `backoff_cap`; the wait itself is a random
    /// time up to that (full jitter).
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
    /// The longest `Retry-After` a delivery waits for. A longer one ends the delivery.
    pub max_rate_wait: Duration,
    /// The most of an answer that is read.
    pub max_response: usize,
    pub db_timeout: Duration,
    pub secret_timeout: Duration,
    /// How long the settings and the URL of a kind are used before they are read again.
    pub settings_ttl: Duration,
    /// The most notifications waiting to be sent. One more is dropped.
    pub queue: usize,
    /// The most deliveries at once.
    pub concurrency: usize,
    pub breaker: BreakerConfig,
}

impl Default for TeamsConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: vec![
                "*.logic.azure.com".to_owned(),
                "*.api.powerplatform.com".to_owned(),
                "*.webhook.office.com".to_owned(),
            ],
            web_base_url: None,
            attempt_timeout: Duration::from_secs(5),
            total_timeout: Duration::from_secs(15),
            max_attempts: 3,
            backoff_base: Duration::from_millis(200),
            backoff_cap: Duration::from_secs(2),
            max_rate_wait: Duration::from_secs(2),
            max_response: 64 * 1024,
            db_timeout: Duration::from_secs(5),
            secret_timeout: Duration::from_secs(5),
            settings_ttl: Duration::from_secs(60),
            queue: 256,
            concurrency: 4,
            breaker: BreakerConfig::default(),
        }
    }
}

impl TeamsConfig {
    /// Checks the bounds. Returns the hosts and the web origin, ready to use.
    fn checked(&self) -> Result<(Hosts, Option<String>), ConfigError> {
        let hosts = Hosts::parse(&self.allowed_hosts).ok_or(ConfigError::Invalid(
            "allowed_hosts must be a list of host names or *.suffix patterns",
        ))?;
        let origin = self.web_base_url.as_deref().map(web_origin).transpose()?;
        let nonzero = |d: Duration, what: &'static str| {
            if d.is_zero() {
                Err(ConfigError::Invalid(what))
            } else {
                Ok(())
            }
        };
        nonzero(self.attempt_timeout, "attempt_timeout must be more than zero")?;
        nonzero(self.total_timeout, "total_timeout must be more than zero")?;
        nonzero(self.db_timeout, "db_timeout must be more than zero")?;
        nonzero(self.secret_timeout, "secret_timeout must be more than zero")?;
        nonzero(self.settings_ttl, "settings_ttl must be more than zero")?;
        nonzero(self.breaker.window, "breaker window must be more than zero")?;
        nonzero(self.breaker.open_for, "breaker open time must be more than zero")?;
        if self.total_timeout < self.attempt_timeout {
            return Err(ConfigError::Invalid(
                "total_timeout must be at least attempt_timeout",
            ));
        }
        if !(1..=5).contains(&self.max_attempts) {
            return Err(ConfigError::Invalid("max_attempts must be 1 to 5"));
        }
        if self.max_response == 0 || self.max_response > 1024 * 1024 {
            return Err(ConfigError::Invalid("max_response must be 1 byte to 1 MiB"));
        }
        if !(1..=4_096).contains(&self.queue) {
            return Err(ConfigError::Invalid("queue must be 1 to 4096"));
        }
        if !(1..=32).contains(&self.concurrency) {
            return Err(ConfigError::Invalid("concurrency must be 1 to 32"));
        }
        if !(1..=100).contains(&self.breaker.failures) {
            return Err(ConfigError::Invalid("breaker failures must be 1 to 100"));
        }
        Ok((hosts, origin))
    }
}

/// The origin of the web app: `https://host[:port]` with nothing after it.
fn web_origin(url: &str) -> Result<String, ConfigError> {
    const BAD: ConfigError =
        ConfigError::Invalid("web_base_url must be an https origin such as https://host");
    let parsed: http::Uri = url.parse().map_err(|_| BAD)?;
    let authority = parsed.authority().ok_or(BAD)?;
    let bare = matches!(parsed.path(), "" | "/") && parsed.query().is_none() && !url.contains(['?', '#']);
    if parsed.scheme_str() != Some("https") || authority.as_str().contains('@') || !bare {
        return Err(BAD);
    }
    Ok(format!("https://{authority}"))
}

/// What the notifier reads and uses.
#[derive(Clone)]
pub struct TeamsDeps {
    pub pool: PgPool,
    pub secrets: Arc<dyn SecretSource>,
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for TeamsDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamsDeps").finish_non_exhaustive()
    }
}

/// The name of a kind in `notification_settings.event_kind` and in the log: its wire name.
fn kind_key(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::AccessRequested => "access_requested",
        NotificationKind::ProposalPending => "proposal_pending",
        NotificationKind::ProposalDecided => "proposal_decided",
        NotificationKind::ServiceRestarted => "service_restarted",
        NotificationKind::PrRaised => "pr_raised",
        NotificationKind::AgentDisconnected => "agent_disconnected",
        NotificationKind::CriticalFinding => "critical_finding",
        NotificationKind::DriftDetected => "drift_detected",
        NotificationKind::SyncWindowStale => "sync_window_stale",
    }
}

const SETTINGS: &str =
    "SELECT enabled, min_severity, destination_ref FROM notification_settings WHERE event_kind = $1";

/// Where a kind of notification goes, as read.
#[derive(Clone)]
enum Route {
    /// Nowhere: no row, switched off, no destination, or one that cannot be used.
    Off,
    On {
        min: Severity,
        /// The webhook URL, a secret from the secret source.
        url: Arc<Secret<String>>,
    },
}

struct Cached {
    at: i64,
    route: Route,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Inner {
    cfg: TeamsConfig,
    pool: PgPool,
    secrets: Arc<dyn SecretSource>,
    clock: Arc<dyn Clock>,
    webhook: Webhook,
    origin: Option<String>,
    /// At most one entry per kind: the set of kinds is closed.
    routes: Mutex<HashMap<NotificationKind, Cached>>,
    breakers: Mutex<HashMap<NotificationKind, Breaker>>,
}

/// The Teams notifier. See the module's documentation.
pub struct TeamsNotifier {
    inner: Arc<Inner>,
    tx: Mutex<Option<mpsc::Sender<Notification>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for TeamsNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamsNotifier").finish_non_exhaustive()
    }
}

impl TeamsNotifier {
    /// The notifier for Teams: TLS 1.3 to an allowed host, HTTPS only. It starts its worker, so it must be made inside a tokio
    /// runtime.
    pub fn new(deps: TeamsDeps, cfg: TeamsConfig) -> Result<Self, ConfigError> {
        let (hosts, _) = cfg.checked()?;
        let webhook = Webhook::service(hosts, cfg.max_response)?;
        Self::start(deps, cfg, webhook)
    }

    /// A notifier for the tests' local server: any port, and plain HTTP when asked. Not in any build of the hub.
    #[cfg(feature = "test-doubles")]
    pub fn for_tests(deps: TeamsDeps, cfg: TeamsConfig, plain_http: bool) -> Result<Self, ConfigError> {
        let (hosts, _) = cfg.checked()?;
        let webhook = Webhook::for_tests(hosts, cfg.max_response, plain_http, None)?;
        Self::start(deps, cfg, webhook)
    }

    /// A notifier that speaks TLS 1.3 to a test server with one of `roots`: for the test that a server which offers TLS 1.2
    /// only is refused. Not in any build of the hub.
    #[cfg(feature = "test-doubles")]
    pub fn for_tests_with_roots(
        deps: TeamsDeps,
        cfg: TeamsConfig,
        roots: rustls::RootCertStore,
    ) -> Result<Self, ConfigError> {
        let (hosts, _) = cfg.checked()?;
        let webhook = Webhook::for_tests(hosts, cfg.max_response, false, Some(roots))?;
        Self::start(deps, cfg, webhook)
    }

    fn start(deps: TeamsDeps, cfg: TeamsConfig, webhook: Webhook) -> Result<Self, ConfigError> {
        let (_, origin) = cfg.checked()?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| ConfigError::Invalid("the Teams notifier needs a running tokio runtime"))?;
        let (tx, rx) = mpsc::channel(cfg.queue);
        let inner = Arc::new(Inner {
            cfg,
            pool: deps.pool,
            secrets: deps.secrets,
            clock: deps.clock,
            webhook,
            origin,
            routes: Mutex::new(HashMap::new()),
            breakers: Mutex::new(HashMap::new()),
        });
        let worker = runtime.spawn(run(Arc::clone(&inner), rx));
        Ok(Self {
            inner,
            tx: Mutex::new(Some(tx)),
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Sends one notification now and says how it went, instead of queueing it. What the worker runs for each; public for the
    /// tests that look at the outcome.
    pub async fn deliver(&self, n: &Notification) -> Result<(), NotifyError> {
        self.inner.deliver(n).await
    }

    /// Where the breaker of a kind is.
    pub fn breaker_state(&self, kind: NotificationKind) -> BreakerState {
        lock(&self.inner.breakers)
            .get(&kind)
            .map_or(BreakerState::Closed, Breaker::state)
    }

    /// Takes no more notifications, sends the ones already queued and returns when they are done: for the shutdown of the hub.
    pub async fn close_and_drain(&self) {
        drop(lock(&self.tx).take());
        let worker = lock(&self.worker).take();
        if let Some(worker) = worker {
            // A worker that panicked has nothing left to wait for.
            let _ = worker.await;
        }
    }
}

#[async_trait]
impl Notifier for TeamsNotifier {
    /// Queues the notification. Returns at once; a full queue or a closed notifier is `Unavailable`, which the caller logs and
    /// carries on from (D49).
    async fn notify(&self, n: Notification) -> Result<(), NotifyError> {
        let tx = lock(&self.tx).clone().ok_or(NotifyError::Unavailable)?;
        match tx.try_send(n) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("the Teams queue is full: a notification was dropped");
                Err(NotifyError::Unavailable)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(NotifyError::Unavailable),
        }
    }
}

/// The worker: delivers what is queued, at most `concurrency` at a time, until the queue is closed and empty.
async fn run(inner: Arc<Inner>, mut rx: mpsc::Receiver<Notification>) {
    let permits = Arc::new(Semaphore::new(inner.cfg.concurrency));
    let mut tasks = JoinSet::new();
    while let Some(n) = rx.recv().await {
        let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
            break;
        };
        let inner = Arc::clone(&inner);
        tasks.spawn(async move {
            let _permit = permit;
            // The outcome is logged by `deliver`. Nothing waits for it.
            let _ = inner.deliver(&n).await;
        });
        while tasks.try_join_next().is_some() {}
    }
    while tasks.join_next().await.is_some() {}
}

impl Inner {
    async fn deliver(&self, n: &Notification) -> Result<(), NotifyError> {
        let (min, url) = match self.route(n.kind).await? {
            Route::Off => return Ok(()),
            Route::On { min, url } => (min, url),
        };
        if n.severity < min {
            return Ok(());
        }
        let body = serde_json::to_vec(&cards::message(n, self.origin.as_deref()))
            .map(Bytes::from)
            .map_err(|_| NotifyError::Rejected)?;
        if body.len() > cards::MAX_CARD_BYTES {
            return Err(NotifyError::Rejected);
        }

        if !self.breaker(n.kind, Breaker::allow) {
            tracing::debug!(
                kind = kind_key(n.kind),
                "the Teams breaker is open: a notification was not sent"
            );
            return Err(NotifyError::Unavailable);
        }
        let result = self.send(&url, body).await;
        match result {
            // A webhook that answers, even to say no, is up.
            Ok(()) => {
                self.breaker(n.kind, |b, _| b.success());
                Ok(())
            }
            Err(NotifyError::Rejected) => {
                self.breaker(n.kind, |b, _| b.success());
                tracing::warn!(
                    kind = kind_key(n.kind),
                    "the Teams webhook refused a notification"
                );
                Err(NotifyError::Rejected)
            }
            Err(NotifyError::Unavailable) => {
                if self.breaker(n.kind, Breaker::failure) {
                    tracing::warn!(
                        kind = kind_key(n.kind),
                        "the Teams webhook keeps failing: its breaker is open"
                    );
                }
                tracing::warn!(
                    kind = kind_key(n.kind),
                    "a notification could not be delivered to Teams"
                );
                Err(NotifyError::Unavailable)
            }
        }
    }

    /// Runs `f` on the breaker of `kind` with the time now.
    fn breaker<T>(&self, kind: NotificationKind, f: impl FnOnce(&mut Breaker, i64) -> T) -> T {
        let now = self.clock.now().unix_millis();
        let mut all = lock(&self.breakers);
        let breaker = all.entry(kind).or_insert_with(|| Breaker::new(self.cfg.breaker));
        f(breaker, now)
    }

    /// Where `kind` goes: from the cache for `settings_ttl`, else from the settings table and the secret source.
    async fn route(&self, kind: NotificationKind) -> Result<Route, NotifyError> {
        let now = self.clock.now().unix_millis();
        let ttl = i64::try_from(self.cfg.settings_ttl.as_millis()).unwrap_or(i64::MAX);
        if let Some(cached) = lock(&self.routes).get(&kind)
            && (0..ttl).contains(&now.saturating_sub(cached.at))
        {
            return Ok(cached.route.clone());
        }
        let route = self.read_route(kind).await?;
        lock(&self.routes).insert(
            kind,
            Cached {
                at: now,
                route: route.clone(),
            },
        );
        Ok(route)
    }

    /// Reads the settings of `kind` and the secret they name. A problem with the configuration is [`Route::Off`] (and said once
    /// a minute); a problem reaching the database or the secret source is an error and is not remembered.
    async fn read_route(&self, kind: NotificationKind) -> Result<Route, NotifyError> {
        let key = kind_key(kind);
        let read = tokio::time::timeout(
            self.cfg.db_timeout,
            sqlx::query(SETTINGS).bind(key).fetch_optional(&self.pool),
        )
        .await;
        let Ok(Ok(row)) = read else {
            tracing::warn!(kind = key, "the Teams settings could not be read");
            return Err(NotifyError::Unavailable);
        };
        let Some(row) = row else {
            return Ok(Route::Off);
        };
        let (Ok(enabled), Ok(min), Ok(destination)) = (
            row.try_get::<bool, _>("enabled"),
            row.try_get::<String, _>("min_severity"),
            row.try_get::<Option<String>, _>("destination_ref"),
        ) else {
            tracing::warn!(kind = key, "the Teams settings could not be understood");
            return Ok(Route::Off);
        };
        let (true, Some(destination), Ok(min)) = (
            enabled,
            destination.filter(|d| !d.is_empty()),
            min.parse::<Severity>(),
        ) else {
            return Ok(Route::Off);
        };
        let url = match tokio::time::timeout(self.cfg.secret_timeout, self.secrets.get(&destination)).await {
            Ok(Ok(url)) => url,
            Ok(Err(SecretError::NotFound | SecretError::InvalidName)) => {
                tracing::warn!(kind = key, "the Teams destination has no secret");
                return Ok(Route::Off);
            }
            Ok(Err(SecretError::Unavailable)) | Err(_) => {
                tracing::warn!(kind = key, "the Teams destination could not be read");
                return Err(NotifyError::Unavailable);
            }
        };
        if !self.webhook.accepts(url.expose()) {
            tracing::warn!(
                kind = key,
                "the Teams destination is not an address that may be called"
            );
            return Ok(Route::Off);
        }
        Ok(Route::On {
            min,
            url: Arc::new(url),
        })
    }

    /// One delivery: up to `max_attempts` attempts with jittered backoff, each and all of them bounded.
    async fn send(&self, url: &Secret<String>, body: Bytes) -> Result<(), NotifyError> {
        let run = async {
            let mut attempt = 0_u32;
            loop {
                attempt += 1;
                let asked = match tokio::time::timeout(
                    self.cfg.attempt_timeout,
                    self.webhook.post(url, body.clone()),
                )
                .await
                {
                    Ok(Attempt::Delivered) => return Ok(()),
                    Ok(Attempt::Refused) => return Err(NotifyError::Rejected),
                    Ok(Attempt::Retry(asked)) => asked,
                    Err(_) => {
                        tracing::debug!("a Teams attempt timed out");
                        None
                    }
                };
                // A webhook that asks for a longer wait than a worker may spend is given up on, not slept on.
                if attempt >= self.cfg.max_attempts || asked.is_some_and(|w| w > self.cfg.max_rate_wait) {
                    return Err(NotifyError::Unavailable);
                }
                self.pause(attempt, asked).await;
            }
        };
        tokio::time::timeout(self.cfg.total_timeout, run)
            .await
            .unwrap_or(Err(NotifyError::Unavailable))
    }

    /// The wait between two attempts: what the webhook asked for, or a random time up to a doubling bound (full jitter).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_valid_and_are_the_plans() {
        let c = TeamsConfig::default();
        assert!(c.checked().is_ok());
        assert_eq!(c.max_attempts, 3);
        assert_eq!(c.attempt_timeout, Duration::from_secs(5));
        assert_eq!(c.backoff_cap, Duration::from_secs(2));
        assert_eq!(c.max_response, 64 * 1024);
        assert_eq!(c.settings_ttl, Duration::from_secs(60));
        assert_eq!(c.breaker.failures, 5);
        assert_eq!(c.breaker.window, Duration::from_secs(60));
        assert_eq!(c.breaker.open_for, Duration::from_secs(30));
    }

    #[test]
    fn every_bound_is_checked() {
        let bad = |f: &dyn Fn(&mut TeamsConfig)| {
            let mut c = TeamsConfig::default();
            f(&mut c);
            assert!(c.checked().is_err());
        };
        bad(&|c| c.allowed_hosts = Vec::new());
        bad(&|c| c.allowed_hosts = vec!["https://x.example".to_owned()]);
        bad(&|c| c.attempt_timeout = Duration::ZERO);
        bad(&|c| c.total_timeout = Duration::ZERO);
        bad(&|c| c.total_timeout = Duration::from_secs(1));
        bad(&|c| c.db_timeout = Duration::ZERO);
        bad(&|c| c.secret_timeout = Duration::ZERO);
        bad(&|c| c.settings_ttl = Duration::ZERO);
        bad(&|c| c.max_attempts = 0);
        bad(&|c| c.max_attempts = 6);
        bad(&|c| c.max_response = 0);
        bad(&|c| c.max_response = 1024 * 1024 + 1);
        bad(&|c| c.queue = 0);
        bad(&|c| c.queue = 4_097);
        bad(&|c| c.concurrency = 0);
        bad(&|c| c.concurrency = 33);
        bad(&|c| c.breaker.failures = 0);
        bad(&|c| c.breaker.failures = 101);
        bad(&|c| c.breaker.window = Duration::ZERO);
        bad(&|c| c.breaker.open_for = Duration::ZERO);
    }

    #[test]
    fn the_web_origin_is_an_https_origin_and_nothing_more() {
        assert_eq!(
            web_origin("https://lanekeeper.example.test").unwrap(),
            "https://lanekeeper.example.test"
        );
        assert_eq!(
            web_origin("https://lanekeeper.example.test/").unwrap(),
            "https://lanekeeper.example.test"
        );
        assert_eq!(
            web_origin("https://lanekeeper.example.test:8443").unwrap(),
            "https://lanekeeper.example.test:8443"
        );
        for bad in [
            "http://lanekeeper.example.test",
            "https://lanekeeper.example.test/app",
            "https://lanekeeper.example.test/?x=1",
            "https://lanekeeper.example.test/#f",
            "https://user@lanekeeper.example.test",
            "lanekeeper.example.test",
            "",
        ] {
            assert!(web_origin(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_kind_has_a_distinct_settings_key_that_is_its_wire_name() {
        let kinds = [
            NotificationKind::AccessRequested,
            NotificationKind::ProposalPending,
            NotificationKind::ProposalDecided,
            NotificationKind::ServiceRestarted,
            NotificationKind::PrRaised,
            NotificationKind::AgentDisconnected,
            NotificationKind::CriticalFinding,
            NotificationKind::DriftDetected,
            NotificationKind::SyncWindowStale,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for kind in kinds {
            let wire = serde_json::to_value(kind).unwrap();
            assert_eq!(wire.as_str(), Some(kind_key(kind)));
            assert!(seen.insert(kind_key(kind)));
        }
    }
}
