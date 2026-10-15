//! T7 (D49, Q9, S6, S10, S18, rule 5): the Teams notifier against a local webhook server and a real Postgres. Where a notification
//! goes comes from `notification_settings` and the secret source; what is sent is one Adaptive Card; a failing webhook is retried
//! a bounded number of times, then given up on, and a breaker stops calling one that is down. Whatever happens, the action that
//! caused the notification is never failed or slowed by it.
#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use domain::{Role, Secret, Severity, ShortText, SwimlaneId, Timestamp, WriteOutcome};
use hub_writes::ManualClock;
use hub_writes::teams::{BreakerConfig, BreakerState, TeamsConfig, TeamsDeps, TeamsNotifier};
use ports::{
    AuditAction, Notification, NotificationKind, Notifier, NotifyError, SecretError, SecretSource,
    WriteService,
};
use sqlx::PgPool;

use crate::support::fake_github::{Reply, Tls};
use crate::support::fake_teams::{FakeTeams, accepted, status};
use crate::support::log_capture;
use crate::support::pg::TestDb;
use crate::support::rig::{Rig, START, at, ctx, edit};

const ORIGIN: &str = "https://lanekeeper.example.test";
/// The part of every webhook URL that is the credential. It must appear nowhere but in the request line.
const SIGNATURE: &str = "SIG-7f3a91c2e4b8d605";

// ------------------------------------------------------------------------------------------------------------ fixtures

/// A secret source that tests can change, that counts its reads and that can be made to fail.
#[derive(Default)]
struct Secrets {
    map: Mutex<HashMap<String, String>>,
    reads: AtomicUsize,
    down: AtomicBool,
}

impl Secrets {
    fn set(&self, name: &str, value: &str) {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), value.to_owned());
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SecretSource for Secrets {
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err(SecretError::Unavailable);
        }
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .map(|v| Secret::new(v.clone()))
            .ok_or(SecretError::NotFound)
    }
}

/// Settings with the waits cut to what a test can afford and the bounds of service.
fn fast() -> TeamsConfig {
    TeamsConfig {
        allowed_hosts: vec!["127.0.0.1".to_owned()],
        web_base_url: Some(ORIGIN.to_owned()),
        attempt_timeout: Duration::from_millis(500),
        total_timeout: Duration::from_secs(5),
        backoff_base: Duration::from_millis(1),
        backoff_cap: Duration::from_millis(5),
        max_rate_wait: Duration::from_millis(50),
        ..TeamsConfig::default()
    }
}

struct Teams {
    _db: TestDb,
    pool: PgPool,
    secrets: Arc<Secrets>,
    clock: Arc<ManualClock>,
    notifier: Arc<TeamsNotifier>,
}

async fn fixture_with(cfg: TeamsConfig, plain_http: bool) -> Option<Teams> {
    let db = TestDb::ready().await?;
    let pool = db.app_pool(4).await;
    Some(build(db, pool, cfg, plain_http))
}

fn build(db: TestDb, pool: PgPool, cfg: TeamsConfig, plain_http: bool) -> Teams {
    let secrets = Arc::new(Secrets::default());
    let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(START)));
    let notifier = Arc::new(
        TeamsNotifier::for_tests(
            TeamsDeps {
                pool: pool.clone(),
                secrets: secrets.clone(),
                clock: clock.clone(),
            },
            cfg,
            plain_http,
        )
        .unwrap(),
    );
    Teams {
        _db: db,
        pool,
        secrets,
        clock,
        notifier,
    }
}

async fn fixture() -> Option<Teams> {
    fixture_with(fast(), true).await
}

/// The key `notification_settings.event_kind` holds for a kind: its wire name.
fn key(kind: NotificationKind) -> String {
    serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()
}

impl Teams {
    /// Send `kind` to the secret `dest`, which holds `url`.
    async fn route(&self, kind: NotificationKind, dest: &str, url: &str) {
        self.secrets.set(dest, url);
        self.settings(kind, true, "low", Some(dest)).await;
    }

    async fn settings(&self, kind: NotificationKind, enabled: bool, min: &str, dest: Option<&str>) {
        sqlx::query(
            "INSERT INTO notification_settings (event_kind, enabled, min_severity, destination_ref) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (event_kind) DO UPDATE \
             SET enabled = $2, min_severity = $3, destination_ref = $4",
        )
        .bind(key(kind))
        .bind(enabled)
        .bind(min)
        .bind(dest)
        .execute(&self.pool)
        .await
        .unwrap();
        // What was read before the change is allowed to be used for a minute (A10); a test that changes settings wants them now.
        self.clock.advance_secs(61);
    }

    async fn deliver(&self, kind: NotificationKind) -> Result<(), NotifyError> {
        self.notifier.deliver(&note(kind)).await
    }
}

fn note(kind: NotificationKind) -> Notification {
    Notification {
        kind,
        severity: Severity::High,
        swimlane: Some(SwimlaneId::parse("sit1").unwrap()),
        title: ShortText::parse("Change awaiting approval").unwrap(),
        summary: ShortText::parse("Ada proposed a change in sit1").unwrap(),
        link: Some(ShortText::parse("/proposals/7").unwrap()),
    }
}

const KIND: NotificationKind = NotificationKind::ProposalPending;

// -------------------------------------------------------------------------------------------------------- where it goes

#[tokio::test]
async fn one_card_is_posted_to_the_destination_of_its_kind() {
    let Some(t) = fixture().await else { return };
    let (pending, restarted) = (FakeTeams::start().await, FakeTeams::start().await);
    t.route(KIND, "teams-approvals", &pending.url("a", SIGNATURE))
        .await;
    t.route(
        NotificationKind::ServiceRestarted,
        "teams-ops",
        &restarted.url("b", SIGNATURE),
    )
    .await;

    t.deliver(KIND).await.unwrap();

    assert_eq!(
        restarted.requests(),
        0,
        "another kind's destination is not called"
    );
    let seen = pending.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(
        seen[0].path,
        format!("/workflows/a/triggers/manual/paths/invoke?sig={SIGNATURE}")
    );
    assert_eq!(seen[0].header("content-type"), Some("application/json"));
    let body = seen[0].json();
    assert_eq!(body["type"], "message");
    let card = &body["attachments"][0]["content"];
    assert_eq!(card["type"], "AdaptiveCard");
    let text = card.to_string();
    assert!(text.contains("Change awaiting approval") && text.contains("Ada proposed a change in sit1"));
    assert_eq!(card["actions"][0]["url"], format!("{ORIGIN}/proposals/7"));
}

#[tokio::test]
async fn no_op_when_no_destination_is_configured() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;

    // Nothing is configured at all: Q9's default is a notifier that does nothing.
    assert_eq!(t.deliver(KIND).await, Ok(()));

    // A row with no destination.
    t.settings(KIND, true, "low", None).await;
    assert_eq!(t.deliver(KIND).await, Ok(()));

    // A destination that is switched off.
    t.secrets.set("teams-approvals", &server.url("a", SIGNATURE));
    t.settings(KIND, false, "low", Some("teams-approvals")).await;
    assert_eq!(t.deliver(KIND).await, Ok(()));

    // A notification below the minimum severity.
    t.settings(KIND, true, "critical", Some("teams-approvals")).await;
    assert_eq!(t.deliver(KIND).await, Ok(()));

    // A destination whose secret does not exist.
    t.settings(KIND, true, "low", Some("no-such-secret")).await;
    assert_eq!(t.deliver(KIND).await, Ok(()));

    assert_eq!(server.requests(), 0, "no request was made for any of these");

    // The same settings with the severity met do send.
    t.settings(KIND, true, "high", Some("teams-approvals")).await;
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn settings_and_the_url_are_cached_for_a_minute() {
    let Some(t) = fixture().await else { return };
    let (first, second) = (FakeTeams::start().await, FakeTeams::start().await);
    t.route(KIND, "teams-approvals", &first.url("a", SIGNATURE)).await;

    t.deliver(KIND).await.unwrap();
    t.deliver(KIND).await.unwrap();
    assert_eq!(t.secrets.reads(), 1, "the URL is read once for the minute");

    // The admin moves the destination. For the rest of the minute the old one is used.
    t.secrets.set("teams-approvals", &second.url("b", SIGNATURE));
    t.clock.advance_secs(59);
    t.deliver(KIND).await.unwrap();
    assert_eq!((first.requests(), second.requests()), (3, 0));

    // After the minute the new one is.
    t.clock.advance_secs(2);
    t.deliver(KIND).await.unwrap();
    assert_eq!((first.requests(), second.requests()), (3, 1));
    assert_eq!(t.secrets.reads(), 2);
}

#[tokio::test]
async fn a_secret_manager_outage_is_unavailable_and_is_not_remembered() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    t.secrets.down.store(true, Ordering::SeqCst);
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    t.secrets.down.store(false, Ordering::SeqCst);
    // No minute has to pass: an outage is not cached.
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn a_url_off_the_allowlist_is_never_called() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    // The server is on 127.0.0.1, which is allowed; `localhost` is not.
    let url = server.url("a", SIGNATURE).replace("127.0.0.1", "localhost");
    t.route(KIND, "teams-approvals", &url).await;
    assert_eq!(
        t.deliver(KIND).await,
        Ok(()),
        "a destination that may not be called is a no-op"
    );
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn http_to_a_service_client_is_never_spoken() {
    // The client a test builds for plain HTTP is the only one that speaks it: the one in service refuses.
    let Some(t) = fixture_with(fast(), false).await else {
        return;
    };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(server.requests(), 0);
}

// ------------------------------------------------------------------------------------------------------ retries, bounds

#[tokio::test]
async fn webhook_retries_with_backoff_then_gives_up() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;

    // A server error is retried, and three attempts are all there are.
    server.always(status(503));
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    assert_eq!(server.requests(), 3);

    // A webhook that recovers part way is delivered to.
    server.next(status(502));
    server.next(status(500));
    server.always(accepted());
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(server.requests(), 6, "two failures and the delivery");
}

#[tokio::test]
async fn the_wait_between_attempts_is_jittered_and_bounded_by_the_cap() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.always(status(503));
    let started = Instant::now();
    assert!(t.deliver(KIND).await.is_err());
    // Two waits of at most 5 ms each (the cap of `fast`): the whole call is far below the attempt timeout.
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_refusal_is_final_and_is_not_retried() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    for code in [400_u16, 401, 403, 404, 413, 302] {
        let before = server.requests();
        server.always(status(code));
        assert_eq!(t.deliver(KIND).await, Err(NotifyError::Rejected), "{code}");
        assert_eq!(server.requests(), before + 1, "{code} is tried once");
    }
}

#[tokio::test]
async fn a_short_rate_limit_is_waited_for_and_a_long_one_is_not() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;

    server.next(status(429).with_header("retry-after", "0"));
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(server.requests(), 2);

    // Ten seconds is more than this call is willing to hold a worker for (`max_rate_wait` is 50 ms here).
    server.always(status(429).with_header("retry-after", "10"));
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    assert_eq!(server.requests(), 3, "no second attempt");
}

#[tokio::test]
async fn a_slow_webhook_times_out_and_the_whole_call_is_bounded() {
    let Some(t) = fixture_with(
        TeamsConfig {
            attempt_timeout: Duration::from_millis(100),
            ..fast()
        },
        true,
    )
    .await
    else {
        return;
    };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.slow(Duration::from_secs(3));
    let started = Instant::now();
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    assert!(
        started.elapsed() < Duration::from_millis(2_000),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(server.requests(), 3);

    // And the sum of the attempts is bounded too.
    let Some(t) = fixture_with(
        TeamsConfig {
            attempt_timeout: Duration::from_millis(150),
            total_timeout: Duration::from_millis(200),
            ..fast()
        },
        true,
    )
    .await
    else {
        return;
    };
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    let started = Instant::now();
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    assert!(
        started.elapsed() < Duration::from_millis(1_000),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn nothing_listening_is_unavailable() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    let url = server.url("a", SIGNATURE);
    drop(server);
    t.route(KIND, "teams-approvals", &url).await;
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let Some(t) = fixture().await else { return };
    let (first, other) = (FakeTeams::start().await, FakeTeams::start().await);
    t.route(KIND, "teams-approvals", &first.url("a", SIGNATURE)).await;
    first.always(status(307).with_header("location", &other.url("x", "stolen")));
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Rejected));
    assert_eq!(
        other.requests(),
        0,
        "the card, and the signature, went nowhere else"
    );
}

#[tokio::test]
async fn a_huge_answer_is_not_read_whole_and_does_not_matter() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.always(Reply::json(202, &"x".repeat(512 * 1024)));
    assert_eq!(t.deliver(KIND).await, Ok(()));
}

// --------------------------------------------------------------------------------------------------------------- TLS

fn roots_of(server: &FakeTeams) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(server.cert().unwrap()).unwrap();
    roots
}

async fn tls_fixture(server: &FakeTeams) -> Option<Teams> {
    let db = TestDb::ready().await?;
    let pool = db.app_pool(4).await;
    let secrets = Arc::new(Secrets::default());
    let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(START)));
    let notifier = Arc::new(
        TeamsNotifier::for_tests_with_roots(
            TeamsDeps {
                pool: pool.clone(),
                secrets: secrets.clone(),
                clock: clock.clone(),
            },
            fast(),
            roots_of(server),
        )
        .unwrap(),
    );
    Some(Teams {
        _db: db,
        pool,
        secrets,
        clock,
        notifier,
    })
}

#[tokio::test]
async fn tls_13_is_spoken_and_a_tls_12_only_webhook_is_refused() {
    let good = FakeTeams::start_with(Tls::V13).await;
    let Some(t) = tls_fixture(&good).await else { return };
    t.route(KIND, "teams-approvals", &good.url("a", SIGNATURE)).await;
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(good.requests(), 1);

    // S6: the same client, a server that offers TLS 1.2 and nothing newer. No card is sent.
    let old = FakeTeams::start_with(Tls::V12).await;
    let Some(t) = tls_fixture(&old).await else { return };
    t.route(KIND, "teams-approvals", &old.url("a", SIGNATURE)).await;
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    assert_eq!(old.requests(), 0);
}

// ----------------------------------------------------------------------------------------------------------- the breaker

fn trip(cfg: BreakerConfig) -> TeamsConfig {
    TeamsConfig {
        max_attempts: 1,
        breaker: cfg,
        ..fast()
    }
}

#[tokio::test]
async fn breaker_opens_after_repeated_failures_and_half_opens() {
    let Some(t) = fixture_with(trip(BreakerConfig::default()), true).await else {
        return;
    };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.always(status(503));

    for _ in 0..5 {
        assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
        t.clock.advance_secs(1);
    }
    assert_eq!(server.requests(), 5);
    assert_eq!(t.notifier.breaker_state(KIND), BreakerState::Open);

    // Open: the calls are refused without touching the webhook.
    for _ in 0..3 {
        assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    }
    assert_eq!(server.requests(), 5, "an open breaker makes no request");

    // After the pause one probe goes through. It fails: open again, for the full time.
    t.clock.advance_secs(30);
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    assert_eq!(server.requests(), 6, "the probe");
    assert_eq!(t.notifier.breaker_state(KIND), BreakerState::Open);
    t.clock.advance_secs(29);
    assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
    assert_eq!(server.requests(), 6, "still open");

    // Next pause: the webhook is back, the probe succeeds and the breaker closes.
    t.clock.advance_secs(1);
    server.always(accepted());
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(t.notifier.breaker_state(KIND), BreakerState::Closed);
    assert_eq!(t.deliver(KIND).await, Ok(()));
    assert_eq!(server.requests(), 8);
}

#[tokio::test]
async fn failures_that_are_not_close_together_do_not_open_the_breaker() {
    let Some(t) = fixture_with(trip(BreakerConfig::default()), true).await else {
        return;
    };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.always(status(500));
    for _ in 0..10 {
        assert_eq!(t.deliver(KIND).await, Err(NotifyError::Unavailable));
        t.clock.advance_secs(20);
    }
    assert_eq!(server.requests(), 10);
    assert_eq!(t.notifier.breaker_state(KIND), BreakerState::Closed);
}

#[tokio::test]
async fn only_one_probe_goes_through_at_a_time() {
    let Some(t) = fixture_with(trip(BreakerConfig::default()), true).await else {
        return;
    };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.always(status(503));
    for _ in 0..5 {
        let _ = t.deliver(KIND).await;
    }
    t.clock.advance_secs(30);
    // The probe is slow; a second delivery arrives while it is out.
    server.slow(Duration::from_millis(200));
    let before = server.requests();
    let (a, b) = tokio::join!(t.deliver(KIND), t.deliver(KIND));
    assert_eq!(server.requests(), before + 1, "one probe, one request");
    assert_eq!([a, b].iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(t.notifier.breaker_state(KIND), BreakerState::Closed);
}

#[tokio::test]
async fn a_refusal_by_the_webhook_does_not_trip_the_breaker() {
    let Some(t) = fixture_with(trip(BreakerConfig::default()), true).await else {
        return;
    };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.always(status(400));
    for _ in 0..10 {
        assert_eq!(t.deliver(KIND).await, Err(NotifyError::Rejected));
    }
    assert_eq!(server.requests(), 10, "a webhook that answers is not down");
    assert_eq!(t.notifier.breaker_state(KIND), BreakerState::Closed);
}

#[tokio::test]
async fn one_kinds_breaker_does_not_silence_another() {
    let Some(t) = fixture_with(trip(BreakerConfig::default()), true).await else {
        return;
    };
    let (down, up) = (FakeTeams::start().await, FakeTeams::start().await);
    t.route(KIND, "teams-approvals", &down.url("a", SIGNATURE)).await;
    t.route(NotificationKind::PrRaised, "teams-prs", &up.url("b", SIGNATURE))
        .await;
    down.always(status(503));
    for _ in 0..5 {
        let _ = t.deliver(KIND).await;
    }
    assert_eq!(t.notifier.breaker_state(KIND), BreakerState::Open);
    assert_eq!(t.deliver(NotificationKind::PrRaised).await, Ok(()));
    assert_eq!(up.requests(), 1);
    assert_eq!(
        t.notifier.breaker_state(NotificationKind::PrRaised),
        BreakerState::Closed
    );
}

// ------------------------------------------------------------------------------------------------- never fails the action

#[tokio::test]
async fn notifications_are_queued_and_sent_by_a_worker() {
    let Some(t) = fixture().await else { return };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;

    t.notifier.notify(note(KIND)).await.unwrap();
    t.notifier.notify(note(KIND)).await.unwrap();
    t.notifier.close_and_drain().await;
    assert_eq!(server.requests(), 2, "both were sent before the drain returned");

    // Once closed the notifier takes nothing more.
    assert_eq!(t.notifier.notify(note(KIND)).await, Err(NotifyError::Unavailable));
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn a_full_queue_drops_the_notification_and_never_waits() {
    let Some(t) = fixture_with(TeamsConfig { queue: 1, ..fast() }, true).await else {
        return;
    };
    let server = FakeTeams::start().await;
    t.route(KIND, "teams-approvals", &server.url("a", SIGNATURE))
        .await;
    server.slow(Duration::from_secs(3));

    // Nothing awaits the network here, so the worker has taken nothing: one fits and the rest are refused at once.
    let started = Instant::now();
    let mut taken = 0;
    for _ in 0..5 {
        if t.notifier.notify(note(KIND)).await.is_ok() {
            taken += 1;
        }
    }
    assert_eq!(taken, 1);
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[tokio::test]
async fn a_failing_webhook_never_fails_the_action() {
    let Some(db) = TestDb::ready().await else { return };
    let rig = Rig::new(db).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    let careful = rig.approval_user(1, Role::Editor).await;

    let server = FakeTeams::start().await;
    server.always(status(503));
    let secrets = Arc::new(Secrets::default());
    secrets.set("teams-approvals", &server.url("a", SIGNATURE));
    let notifier = Arc::new(
        TeamsNotifier::for_tests(
            TeamsDeps {
                pool: rig.pool.clone(),
                secrets,
                clock: rig.clock.clone(),
            },
            fast(),
            true,
        )
        .unwrap(),
    );
    sqlx::query("INSERT INTO notification_settings (event_kind, destination_ref) VALUES ('proposal_pending', 'teams-approvals')")
        .execute(&rig.pool)
        .await
        .unwrap();
    let hub = rig.hub_with_notifier(notifier.clone());

    // A change that needs approval tells the admins. The webhook is down, and the proposal is made all the same.
    let outcome = hub
        .edit(&ctx(&careful, "k-1"), edit(&s, "a.yml", at("x: 1\n"), "x: 2\n"))
        .await
        .unwrap();
    assert!(
        matches!(outcome, WriteOutcome::ProposalCreated { .. }),
        "{outcome:?}"
    );
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].event.action, AuditAction::ProposalCreated);

    // The card was tried, and given up on, without anyone being asked to wait for it.
    notifier.close_and_drain().await;
    assert_eq!(server.requests(), 3);
}

// ------------------------------------------------------------------------------------------------------------------ S10

#[tokio::test]
async fn teams_webhook_url_never_logged() {
    let (capture, _guard) = log_capture::install();
    let Some(t) = fixture_with(trip(BreakerConfig::default()), true).await else {
        return;
    };
    let server = FakeTeams::start().await;
    let url = server.url("flow-9c1d", SIGNATURE);
    t.route(KIND, "teams-approvals", &url).await;
    t.route(
        NotificationKind::PrRaised,
        "teams-prs",
        &url.replace("127.0.0.1", "localhost"),
    )
    .await;
    t.settings(
        NotificationKind::AccessRequested,
        true,
        "low",
        Some("missing-secret"),
    )
    .await;

    let mut errors = Vec::new();
    // A webhook that fails, until its breaker opens, then one that refuses, one that is slow and one that is off the allowlist.
    server.always(status(503));
    for _ in 0..7 {
        errors.push(format!("{:?}", t.deliver(KIND).await));
    }
    server.always(status(400));
    t.clock.advance_secs(31);
    errors.push(format!("{:?}", t.deliver(KIND).await));
    errors.push(format!("{:?}", t.deliver(NotificationKind::PrRaised).await));
    errors.push(format!(
        "{:?}",
        t.deliver(NotificationKind::AccessRequested).await
    ));
    // Through the queue as well.
    t.notifier.notify(note(KIND)).await.unwrap();
    t.notifier.close_and_drain().await;
    errors.push(format!("{:?} {:?}", t.notifier, t.notifier.breaker_state(KIND)));

    let logs = capture.text();
    assert!(
        !logs.is_empty(),
        "the failures are logged: an empty capture proves nothing"
    );
    for secret in [SIGNATURE, "flow-9c1d", "/workflows/", "teams-approvals", "sig="] {
        assert!(!logs.contains(secret), "{secret:?} is in the logs:\n{logs}");
        for e in &errors {
            assert!(!e.contains(secret), "{secret:?} is in {e}");
        }
    }
    for e in &errors {
        assert!(!e.contains("127.0.0.1"), "{e}");
    }
}
