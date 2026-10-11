//! T11 (S9, S18, D64, D77, rule 8): the `pull_request` webhook. It passes the same HMAC and replay guard as a push, and then moves
//! `pr_links.state` through a state machine in which merged is terminal. The link update, the one audit event per changed link and
//! the `PrStateChanged` event are one transaction on a real Postgres 16: they all happen or none does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use domain::{DomainEvent, PrLinkId, PrState, RepoKind};
use futures::StreamExt;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use hub_platform::clock::Clock;
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::EventMetrics;
use hub_platform::git::deliveries::PgDeliveryLog;
use hub_platform::git::fetch_queue::FetchQueue;
use hub_platform::git::metrics::{GitMetrics, WebhookOutcome};
use hub_platform::git::payload::{PrAction, RepoFullName};
use hub_platform::git::pr_webhook::{PrLinks, next_state};
use hub_platform::git::webhook::{self, WebhookSettings, WebhookState};
use hub_platform::{FakeSessionAuth, GuardedRouter};
use ports::fakes::FakeSecretSource;
use ports::{BusError, EventBus, EventFilter, Tx};
use proptest::prelude::*;
use sqlx::{PgPool, Row};
use support::clock::ManualClock;
use support::fake_github::{Delivery, pull_request_body, push_body, signature};
use support::log_capture::LogCapture;
use support::pg::{PgTestAuditLog, TestDb};
use tokio::sync::watch;
use tower::ServiceExt;

const SECRET: &str = "whsec_PlantedWebhookSecretForTheTest42";
const BASE_REPO: &str = "acme/configuration-base-saas";
const TENANT_REPO: &str = "acme/csp-tenant-data";
const WITHIN: Duration = Duration::from_secs(5);

// ------------------------------------------------------------------------------------------------ the state machine

fn actions() -> impl Strategy<Value = (PrAction, bool)> {
    prop_oneof![
        Just((PrAction::Opened, false)),
        Just((PrAction::Closed, false)),
        Just((PrAction::Closed, true)),
        Just((PrAction::Reopened, false)),
        Just((PrAction::Other, false)),
        // `merged` is only set by GitHub on a closed event, but a payload can carry it on any action: it must not matter.
        Just((PrAction::Opened, true)),
        Just((PrAction::Reopened, true)),
        Just((PrAction::Other, true)),
    ]
}

fn start_states() -> impl Strategy<Value = PrState> {
    prop_oneof![Just(PrState::Open), Just(PrState::Closed), Just(PrState::Merged)]
}

proptest! {
    /// Merged is terminal: once a link is merged, no sequence of events moves it out of merged (D77).
    #[test]
    fn pull_request_state_machine_merged_is_terminal(
        start in start_states(),
        sequence in prop::collection::vec(actions(), 0..40),
    ) {
        let mut state = start;
        let mut was_merged = start == PrState::Merged;
        for (action, merged) in sequence {
            if let Some(next) = next_state(state, action, merged) {
                prop_assert_ne!(next, state, "next_state reports a change only when there is one");
                state = next;
            }
            was_merged |= state == PrState::Merged;
            prop_assert!(!was_merged || state == PrState::Merged, "left the terminal state");
        }
    }

    /// An action that is not opened, closed or reopened never changes anything.
    #[test]
    fn pull_request_state_machine_other_actions_change_nothing(start in start_states(), merged in any::<bool>()) {
        prop_assert_eq!(next_state(start, PrAction::Other, merged), None);
    }
}

#[test]
fn pull_request_state_machine_transitions() {
    use PrAction::{Closed, Opened, Other, Reopened};
    use PrState::{Closed as Shut, Merged, Open};
    let table = [
        // (from, action, merged flag, to)
        (Open, Closed, true, Some(Merged)),
        (Open, Closed, false, Some(Shut)),
        (Open, Opened, false, None),
        (Open, Reopened, false, None),
        (Shut, Reopened, false, Some(Open)),
        // A link is created open, so `opened` says nothing new; acting on it could undo a `closed` that arrived first.
        (Shut, Opened, false, None),
        (Shut, Closed, false, None),
        // A merge whose `reopened` we never heard of: closed -> merged is a fact GitHub states, so it is taken.
        (Shut, Closed, true, Some(Merged)),
        (Merged, Closed, true, None),
        (Merged, Closed, false, None),
        (Merged, Reopened, false, None),
        (Merged, Opened, false, None),
        (Open, Other, false, None),
    ];
    for (from, action, merged, to) in table {
        assert_eq!(
            next_state(from, action, merged),
            to,
            "{from:?} + {action:?} (merged {merged})"
        );
    }
}

// ----------------------------------------------------------------------------------------------------- the rig

/// A bus that refuses every event, to show that a failed publish takes the link update and the audit event with it.
#[derive(Debug)]
struct RefusingBus;

#[async_trait]
impl EventBus for RefusingBus {
    async fn publish(&self, _: DomainEvent) -> Result<(), BusError> {
        Err(BusError::Unavailable)
    }

    async fn publish_in_tx(&self, _: &mut Tx<'_>, _: DomainEvent) -> Result<(), BusError> {
        Err(BusError::Unavailable)
    }

    fn subscribe(&self, _: EventFilter) -> futures::stream::BoxStream<'static, DomainEvent> {
        futures::stream::empty().boxed()
    }
}

struct Rig {
    db: TestDb,
    pool: PgPool,
    router: Router,
    audit: Arc<PgTestAuditLog>,
    bus: PgEventBus,
    clock: Arc<ManualClock>,
    metrics: GitMetrics,
    queue: Arc<FetchQueue>,
    _stop: watch::Sender<bool>,
}

impl Rig {
    async fn new() -> Option<Self> {
        Self::build(false).await
    }

    /// A rig whose bus refuses every event.
    async fn with_refusing_bus() -> Option<Self> {
        Self::build(true).await
    }

    async fn build(refusing_bus: bool) -> Option<Self> {
        let db = TestDb::ready().await?;
        let pool = db.app_pool().await;
        let (stop, stop_rx) = watch::channel(false);
        let bus = PgEventBus::new(pool.clone(), BusSettings::default(), EventMetrics::default());
        bus.start(stop_rx).await;
        let audit = Arc::new(PgTestAuditLog::default());
        let clock = ManualClock::at_t0();
        let events: Arc<dyn EventBus> = if refusing_bus {
            Arc::new(RefusingBus)
        } else {
            Arc::new(bus.clone())
        };
        let links = PrLinks::new(
            pool.clone(),
            audit.clone(),
            events,
            Arc::clone(&clock) as Arc<dyn Clock>,
        );
        let queue = FetchQueue::new(64);
        let metrics = GitMetrics::default();
        let settings = WebhookSettings::new(
            "webhook-secret",
            vec![
                (RepoKind::Base, RepoFullName::parse(BASE_REPO).unwrap()),
                (RepoKind::Tenant, RepoFullName::parse(TENANT_REPO).unwrap()),
            ],
        );
        let state = WebhookState::new(
            settings,
            Arc::new(FakeSecretSource::new().with("webhook-secret", SECRET)),
            Arc::new(PgDeliveryLog::new(pool.clone())),
            Arc::clone(&queue),
            metrics.clone(),
        )
        .with_pull_requests(Arc::new(links));
        let router = webhook::mount(
            GuardedRouter::new(Arc::new(FakeSessionAuth::new()), None),
            Arc::new(state),
        );
        let (router, _) = router.into_parts().unwrap();
        db.seed_swimlane(&pool, "sit1", None).await;
        Some(Self {
            db,
            pool,
            router,
            audit,
            bus,
            clock,
            metrics,
            queue,
            _stop: stop,
        })
    }

    async fn send(&self, request: Request<Body>) -> StatusCode {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let _ = response.into_body().collect().await.unwrap();
        status
    }

    async fn deliver(
        &self,
        delivery_id: &str,
        repo: &str,
        number: i64,
        action: &str,
        merged: bool,
    ) -> StatusCode {
        let delivery = Delivery::signed(
            SECRET,
            "pull_request",
            delivery_id,
            pull_request_body(repo, number, action, merged),
        );
        self.send(delivery.request()).await
    }

    /// A link of PR `number` of `repo` for `path`, in `state`. Returns its id.
    async fn link(&self, repo: &str, number: i32, path: &str, hash: u8, state: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO pr_links (swimlane_id, path, nfs_hash, repo, branch, pr_number, state) \
             VALUES ('sit1', $1, $2::bytea, $3, 'sit1', $4, $5) RETURNING id",
        )
        .bind(path)
        .bind(vec![hash; 32])
        .bind(repo)
        .bind(number)
        .bind(state)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn state_of(&self, id: i64) -> String {
        sqlx::query_scalar("SELECT state FROM pr_links WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn count(&self, sql: &str) -> i64 {
        self.db.count(&self.pool, sql).await
    }

    async fn audit_rows(&self) -> Vec<(String, String, Option<String>, Option<String>)> {
        sqlx::query(
            // The test double's chain keeps the whole event in `event_json`; the path is read from there.
            "SELECT action, actor_kind, swimlane_id, event_json::jsonb ->> 'path' FROM audit_events \
             WHERE action = 'pr_state_changed' ORDER BY seq",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect()
    }
}

async fn next_event(events: &mut (impl futures::Stream<Item = DomainEvent> + Unpin)) -> DomainEvent {
    tokio::time::timeout(WITHIN, events.next())
        .await
        .expect("an event arrives")
        .expect("the stream is open")
}

async fn quiet(events: &mut (impl futures::Stream<Item = DomainEvent> + Unpin), why: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(500), events.next())
            .await
            .is_err(),
        "unexpected event: {why}"
    );
}

// ----------------------------------------------------------------------------------------- HMAC and replay guard

#[tokio::test]
async fn pull_request_webhook_uses_the_same_hmac_and_replay_guard() {
    let Some(rig) = Rig::new().await else { return };
    let id = rig.link("tenant", 7, "svc/a.yml", 1, "open").await;

    // A wrong signature: a bare 401, nothing changes, and the delivery id is not burned.
    let mut forged = Delivery::signed(
        SECRET,
        "pull_request",
        "pr-1",
        pull_request_body(TENANT_REPO, 7, "closed", true),
    );
    forged.signature = signature("not-the-secret", &forged.body);
    assert_eq!(rig.send(forged.request()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(rig.state_of(id).await, "open");
    assert_eq!(rig.count("SELECT count(*) FROM webhook_deliveries").await, 0);
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::BadSignature), 1);

    // The signature is over the exact bytes: a body changed after signing is refused as well.
    let mut tampered = Delivery::signed(
        SECRET,
        "pull_request",
        "pr-1",
        pull_request_body(TENANT_REPO, 7, "closed", false),
    );
    tampered.body = pull_request_body(TENANT_REPO, 7, "closed", true);
    assert_eq!(rig.send(tampered.request()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(rig.state_of(id).await, "open");

    // The real one is applied.
    assert_eq!(
        rig.deliver("pr-1", TENANT_REPO, 7, "closed", true).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(rig.state_of(id).await, "merged");
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::PrApplied), 1);

    // The same delivery again, as GitHub's redelivery sends it: acknowledged, and nothing happens.
    sqlx::query("UPDATE pr_links SET state = 'open' WHERE id = $1")
        .bind(id)
        .execute(&rig.pool)
        .await
        .unwrap();
    assert_eq!(
        rig.deliver("pr-1", TENANT_REPO, 7, "closed", true).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        rig.state_of(id).await,
        "open",
        "a replayed delivery is not applied a second time"
    );
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::Duplicate), 1);
    assert_eq!(rig.audit_rows().await.len(), 1, "one change, one audit event");

    // A delivery without an id cannot be checked against the guard, so it is refused (as for a push).
    let mut no_id = Delivery::signed(
        SECRET,
        "pull_request",
        "x",
        pull_request_body(TENANT_REPO, 7, "closed", true),
    )
    .request();
    no_id.headers_mut().remove("x-github-delivery");
    assert_eq!(rig.send(no_id).await, StatusCode::BAD_REQUEST);
    assert!(rig.queue.is_empty(), "a pull request never queues a fetch");
}

// ------------------------------------------------------------------------------- the update, audit and event

#[tokio::test]
async fn pull_request_updates_pr_link_audits_and_publishes_in_one_transaction() {
    let Some(rig) = Rig::new().await else { return };
    // Two links of one pull request (it changes two files), one of another pull request, one of the same number in the other repository.
    let a = rig.link("tenant", 7, "svc/a.yml", 1, "open").await;
    let b = rig.link("tenant", 7, "svc/b.yml", 2, "open").await;
    let other_pr = rig.link("tenant", 8, "svc/a.yml", 3, "open").await;
    let other_repo = rig.link("base", 7, "svc/a.yml", 4, "open").await;
    let created: String = sqlx::query_scalar("SELECT updated_at::text FROM pr_links WHERE id = $1")
        .bind(a)
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    let mut events = rig.bus.subscribe(EventFilter::all());
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert_eq!(
        rig.deliver("pr-merged", TENANT_REPO, 7, "closed", true).await,
        StatusCode::ACCEPTED
    );

    assert_eq!(rig.state_of(a).await, "merged");
    assert_eq!(rig.state_of(b).await, "merged");
    assert_eq!(rig.state_of(other_pr).await, "open", "another pull request");
    assert_eq!(
        rig.state_of(other_repo).await,
        "open",
        "the same number in the other repository"
    );
    let updated: String = sqlx::query_scalar("SELECT updated_at::text FROM pr_links WHERE id = $1")
        .bind(a)
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_ne!(created, updated, "updated_at moved with the state");

    // Exactly one audit event per changed link, by the system, with the swimlane and path.
    let audit = rig.audit_rows().await;
    assert_eq!(audit.len(), 2, "{audit:?}");
    for (action, actor, swimlane, _) in &audit {
        assert_eq!((action.as_str(), actor.as_str()), ("pr_state_changed", "system"));
        assert_eq!(swimlane.as_deref(), Some("sit1"));
    }
    let mut paths: Vec<_> = audit.iter().filter_map(|r| r.3.clone()).collect();
    paths.sort();
    assert_eq!(paths, ["svc/a.yml", "svc/b.yml"]);

    // And exactly one event per changed link, each subscriber hears it once, with the link id and the new state.
    let mut heard = Vec::new();
    for _ in 0..2 {
        match next_event(&mut events).await {
            DomainEvent::PrStateChanged { link_id, state } => heard.push((link_id, state)),
            other => panic!("expected PrStateChanged, got {other:?}"),
        }
    }
    heard.sort_by_key(|(id, _)| id.get());
    assert_eq!(
        heard,
        vec![
            (PrLinkId::from(a), PrState::Merged),
            (PrLinkId::from(b), PrState::Merged)
        ]
    );
    quiet(&mut events, "only the two changed links publish").await;
    assert_eq!(rig.count("SELECT count(*) FROM outbox").await, 2);

    // A second event that changes nothing (merged is terminal) writes no audit event and publishes nothing.
    assert_eq!(
        rig.deliver("pr-reopened", TENANT_REPO, 7, "reopened", false)
            .await,
        StatusCode::ACCEPTED
    );
    assert_eq!(rig.state_of(a).await, "merged");
    assert_eq!(rig.audit_rows().await.len(), 2);
    quiet(&mut events, "no change, no event").await;
}

#[tokio::test]
async fn pull_request_lifecycle_open_close_reopen_merge() {
    let Some(rig) = Rig::new().await else { return };
    let id = rig.link("base", 3, "svc/a.yml", 1, "open").await;
    let steps = [
        ("closed", false, "closed"),
        ("reopened", false, "open"),
        ("closed", true, "merged"),
        ("reopened", false, "merged"),
    ];
    for (n, (action, merged, expect)) in steps.into_iter().enumerate() {
        assert_eq!(
            rig.deliver(&format!("life-{n}"), BASE_REPO, 3, action, merged)
                .await,
            StatusCode::ACCEPTED
        );
        assert_eq!(rig.state_of(id).await, expect, "after {action}");
    }
    // closed, open, merged: three changes. The fourth event changed nothing.
    assert_eq!(rig.audit_rows().await.len(), 3);
}

#[tokio::test]
async fn pull_request_other_actions_change_nothing() {
    let Some(rig) = Rig::new().await else { return };
    let id = rig.link("tenant", 7, "svc/a.yml", 1, "open").await;
    for (n, action) in ["edited", "labeled", "synchronize", "assigned", "review_requested"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            rig.deliver(&format!("o-{n}"), TENANT_REPO, 7, action, false)
                .await,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(rig.state_of(id).await, "open");
    assert!(rig.audit_rows().await.is_empty());
    assert_eq!(rig.count("SELECT count(*) FROM outbox").await, 0);
}

#[tokio::test]
async fn pull_request_for_unknown_pr_is_ignored() {
    let Some(rig) = Rig::new().await else { return };
    let untouched = rig.link("tenant", 7, "svc/a.yml", 1, "open").await;
    let (capture, _guard) = LogCapture::install();

    // A pull request nobody raised through the hub, and one in a repository the hub does not mirror.
    assert_eq!(
        rig.deliver("u-1", TENANT_REPO, 999, "closed", true).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        rig.deliver("u-2", "someone/else", 7, "closed", true).await,
        StatusCode::ACCEPTED
    );

    assert_eq!(
        rig.state_of(untouched).await,
        "open",
        "the link of PR 7 is untouched"
    );
    assert!(
        rig.audit_rows().await.is_empty(),
        "no state changed, so nothing is audited"
    );
    assert_eq!(rig.count("SELECT count(*) FROM outbox").await, 0);
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::NotApplicable), 2);
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::PrApplied), 0);
    assert!(
        !capture.text().contains("someone/else"),
        "a repository name from the payload is not logged: {}",
        capture.text()
    );
}

#[tokio::test]
async fn pull_request_malformed_payload_is_400() {
    let Some(rig) = Rig::new().await else { return };
    for (n, body) in [
        br#"{"action":"closed"}"#.to_vec(),
        pull_request_body(TENANT_REPO, 0, "closed", true),
        pull_request_body(TENANT_REPO, -5, "closed", true),
        pull_request_body("not a repo", 7, "closed", true),
        // A push body under the pull_request header is not a pull request.
        push_body(
            TENANT_REPO,
            "refs/heads/sit1",
            "89abcdef0123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdef01234567",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let delivery = Delivery::signed(SECRET, "pull_request", &format!("bad-{n}"), body);
        assert_eq!(
            rig.send(delivery.request()).await,
            StatusCode::BAD_REQUEST,
            "case {n}"
        );
    }
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::BadRequest), 5);
    assert!(rig.audit_rows().await.is_empty());
}

// ------------------------------------------------------------------------------------------ the one-open rule

#[tokio::test]
async fn reopen_that_would_break_one_open_pr_is_kept_closed_and_logged() {
    let Some(rig) = Rig::new().await else { return };
    let (capture, _guard) = LogCapture::install();
    // PR 1 and PR 2 changed the same file to the same content (D77 allows one open link for that triple). PR 1 was closed, PR 2 is open.
    let blocked = rig.link("tenant", 1, "svc/a.yml", 9, "closed").await;
    let rival = rig.link("tenant", 2, "svc/a.yml", 9, "open").await;
    // PR 1 also changed another file, where nothing is in the way.
    let free = rig.link("tenant", 1, "svc/b.yml", 5, "closed").await;
    let mut events = rig.bus.subscribe(EventFilter::all());
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert_eq!(
        rig.deliver("re-1", TENANT_REPO, 1, "reopened", false).await,
        StatusCode::ACCEPTED,
        "acknowledged, so GitHub does not retry something that cannot succeed"
    );

    assert_eq!(rig.state_of(blocked).await, "closed", "kept closed");
    assert_eq!(
        rig.state_of(rival).await,
        "open",
        "the other pull request is untouched"
    );
    assert_eq!(rig.state_of(free).await, "open", "the link that could reopen did");
    let log = capture.text();
    assert!(
        log.contains("pr_reopen_blocked_by_open_link"),
        "the refusal is logged with a code: {log}"
    );
    // Only the link that changed is audited and published: a state that did not change is not a state change.
    let audit = rig.audit_rows().await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].3.as_deref(), Some("svc/b.yml"));
    match next_event(&mut events).await {
        DomainEvent::PrStateChanged { link_id, state } => {
            assert_eq!((link_id, state), (PrLinkId::from(free), PrState::Open));
        }
        other => panic!("expected PrStateChanged, got {other:?}"),
    }
    quiet(&mut events, "the blocked link published nothing").await;
}

// -------------------------------------------------------------------------------------------------- one transaction

#[tokio::test]
async fn pr_state_change_audit_is_in_the_same_transaction() {
    let Some(rig) = Rig::new().await else { return };
    let id = rig.link("tenant", 7, "svc/a.yml", 1, "open").await;
    rig.audit.set_failing(true);

    // The audit write fails, so the link keeps its state, no event is published, and GitHub is told to try again later.
    assert_eq!(
        rig.deliver("tx-1", TENANT_REPO, 7, "closed", true).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        rig.state_of(id).await,
        "open",
        "the update went with the failed audit write"
    );
    assert_eq!(rig.count("SELECT count(*) FROM outbox").await, 0);
    assert!(rig.audit_rows().await.is_empty());
    assert_eq!(
        rig.count("SELECT count(*) FROM webhook_deliveries").await,
        0,
        "the delivery id was not left behind: a redelivery of this id must be applied, not taken for a replay"
    );
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::Unavailable), 1);

    // The audit log recovers; GitHub redelivers the same delivery, and now it applies.
    rig.audit.set_failing(false);
    assert_eq!(
        rig.deliver("tx-1", TENANT_REPO, 7, "closed", true).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(rig.state_of(id).await, "merged");
    assert_eq!(rig.audit_rows().await.len(), 1);
    assert_eq!(rig.count("SELECT count(*) FROM outbox").await, 1);
}

#[tokio::test]
async fn pr_state_change_event_is_in_the_same_transaction() {
    let Some(rig) = Rig::with_refusing_bus().await else {
        return;
    };
    let id = rig.link("tenant", 7, "svc/a.yml", 1, "open").await;

    assert_eq!(
        rig.deliver("tx-2", TENANT_REPO, 7, "closed", true).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        rig.state_of(id).await,
        "open",
        "the update went with the failed publish"
    );
    assert!(
        rig.audit_rows().await.is_empty(),
        "so did the audit event, which was written before the publish"
    );
    assert_eq!(rig.count("SELECT count(*) FROM webhook_deliveries").await, 0);
}

#[tokio::test]
async fn the_audit_event_carries_the_time_of_the_hub_clock_and_no_secret() {
    let Some(rig) = Rig::new().await else { return };
    rig.link("tenant", 7, "svc/a.yml", 1, "open").await;
    rig.clock.advance_secs(3600);
    let at = rig.clock.now().unix_millis();
    assert_eq!(
        rig.deliver("t-1", TENANT_REPO, 7, "closed", true).await,
        StatusCode::ACCEPTED
    );
    let (json, recorded_at): (String, i64) =
        sqlx::query_as("SELECT event_json, (extract(epoch FROM at) * 1000)::bigint FROM audit_events WHERE action = 'pr_state_changed'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(recorded_at, at, "the event's time is the injected clock's");
    for secret in [SECRET, "signature", "whsec"] {
        assert!(!json.contains(secret), "{json}");
    }
}
