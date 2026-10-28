//! T5: the SSE endpoint `GET /api/v1/events` (D66, S4, S11, S12, P13), without a database.
//!
//! The source of events is the in-memory fake, so these tests cover everything the edge does: who may connect, what the query
//! may ask for, what a user may see, heartbeats, replay from `Last-Event-ID`, connection caps, and how a stream ends. Time is
//! paused, so a 15 second heartbeat costs nothing and the tests assert on virtual time.
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

use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use domain::{DomainEvent, ProposalId, ProposalStatus, Role, SwimlaneId, UserStatus};
use futures::Stream;
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use hub_platform::events::fake::FakeEventSource;
use hub_platform::events::metrics::{EventMetrics, SseEnd, SseRefusal};
use hub_platform::events::sse::{self, SseSettings, SseState};
use hub_platform::events::visibility::{EventVisibility, RoleVisibility};
use hub_platform::session_auth::{SessionAuth, StreamBinding, user_id};
use hub_platform::{FakeSessionAuth, GuardedRouter, TEST_USER_HEADER};
use support::sse_client::{Frames, kind_of, parse};
use tokio::sync::watch;
use tokio::time::Instant;
use tower::ServiceExt;

const RETRY: &str = "retry: 3000\n\n";
const HEARTBEAT: &str = ": heartbeat\n\n";

struct Rig {
    app: Router,
    source: Arc<FakeEventSource>,
    auth: Arc<FakeSessionAuth>,
    state: Arc<SseState>,
    closing: watch::Sender<bool>,
    metrics: EventMetrics,
}

fn settings() -> SseSettings {
    SseSettings::default()
}

fn rig(settings: SseSettings) -> Rig {
    let source = Arc::new(FakeEventSource::new(64));
    let auth = Arc::new(
        FakeSessionAuth::new()
            .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
            .with_user("viewer2", Some(Role::Viewer), UserStatus::Active)
            .with_user("admin", Some(Role::Admin), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending)
            .with_user("disabled", Some(Role::Admin), UserStatus::Disabled)
            .with_user("norole", None, UserStatus::Active),
    );
    let (closing, closing_rx) = watch::channel(false);
    let metrics = EventMetrics::default();
    let state = SseState::new(
        Arc::clone(&source) as _,
        Arc::clone(&auth) as _,
        Arc::new(RoleVisibility),
        settings,
        closing_rx,
        metrics.clone(),
    );
    let router = sse::mount(
        GuardedRouter::new(Arc::clone(&auth) as _, None),
        Arc::clone(&state),
    );
    let (app, specs) = router.into_parts().unwrap();
    assert_eq!(specs.len(), 1);
    Rig {
        app,
        source,
        auth,
        state,
        closing,
        metrics,
    }
}

async fn open(rig: &Rig, user: Option<&str>, uri: &str, extra: &[(&str, &str)]) -> Response<Body> {
    let mut request = Request::builder().method(Method::GET).uri(uri);
    if let Some(user) = user {
        request = request.header(TEST_USER_HEADER, user);
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    rig.app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn frames(response: Response<Body>) -> Frames<impl Stream<Item = Result<Bytes, axum::Error>> + Unpin> {
    assert_eq!(response.status(), StatusCode::OK);
    Frames::new(response.into_body().into_data_stream())
}

async fn problem_code(response: Response<Body>) -> String {
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    value["code"].as_str().unwrap().to_owned()
}

fn swimlane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

fn findings(s: &str) -> DomainEvent {
    DomainEvent::FindingsChanged {
        swimlane: swimlane(s),
    }
}

fn proposal(n: i64) -> DomainEvent {
    DomainEvent::ProposalChanged {
        id: ProposalId::from(n),
        status: ProposalStatus::Pending,
    }
}

/// The internal event of the contract (`DomainEvent::is_internal`): hub replicas tell each other that a user's sessions are gone.
/// It carries a user id and is a global event, so a per-user filter alone would hand it to every Viewer.
fn revoked(user: &str) -> DomainEvent {
    DomainEvent::UserSessionsRevoked { user: user_id(user) }
}

// ------------------------------------------------------------------------------------------------------ S4

#[tokio::test(start_paused = true)]
async fn sse_requires_viewer_and_active_user() {
    let rig = rig(settings());
    let status = |r: &Response<Body>| r.status();
    assert_eq!(
        status(&open(&rig, None, "/api/v1/events", &[]).await),
        StatusCode::UNAUTHORIZED,
        "no session"
    );
    assert_eq!(
        status(&open(&rig, Some("nobody"), "/api/v1/events", &[]).await),
        StatusCode::UNAUTHORIZED,
        "unknown user"
    );
    assert_eq!(
        status(&open(&rig, Some("pending"), "/api/v1/events", &[]).await),
        StatusCode::FORBIDDEN,
        "pending"
    );
    assert_eq!(
        status(&open(&rig, Some("disabled"), "/api/v1/events", &[]).await),
        StatusCode::FORBIDDEN,
        "disabled, even as an admin"
    );
    assert_eq!(
        status(&open(&rig, Some("norole"), "/api/v1/events", &[]).await),
        StatusCode::FORBIDDEN,
        "signed in but no role"
    );
    let ok = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(
        ok.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    assert_eq!(ok.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(
        rig.source.subscribers(),
        1,
        "only the request that was allowed subscribed"
    );
}

// ----------------------------------------------------------------------------------------------------- S11

#[tokio::test(start_paused = true)]
async fn sse_query_bounds_are_enforced() {
    let rig = rig(settings());
    let kinds = [
        "file_observed",
        "drift_changed",
        "findings_changed",
        "pending_restart_changed",
        "proposal_changed",
        "agent_status_changed",
        "git_head_moved",
        "access_request_created",
        "doc_indexed",
        "resync",
        "attribution_updated",
        "pr_state_changed",
        "pr_job_progress",
        "sync_window_opened",
        "sync_window_closed",
        "pickup_changed",
        "swimlane_changed",
    ];
    let query = |name: &str, values: Vec<String>| -> String {
        let pairs: Vec<String> = values.iter().map(|v| format!("{name}={v}")).collect();
        format!("/api/v1/events?{}", pairs.join("&"))
    };
    let lanes = |n: usize| -> Vec<String> { (0..n).map(|i| format!("sit{i}")).collect() };

    let ok = |uri: String| {
        let rig = &rig;
        async move { open(rig, Some("viewer"), &uri, &[]).await.status() }
    };
    assert_eq!(
        ok(query("swimlane", lanes(64))).await,
        StatusCode::OK,
        "64 swimlanes"
    );
    assert_eq!(
        ok(query("swimlane", lanes(65))).await,
        StatusCode::BAD_REQUEST,
        "65 swimlanes"
    );
    assert_eq!(
        ok(query("kinds", kinds.iter().map(ToString::to_string).collect())).await,
        StatusCode::OK,
        "all 17 kinds"
    );
    let mut eighteen: Vec<String> = kinds.iter().map(ToString::to_string).collect();
    eighteen.push("resync".to_owned());
    assert_eq!(
        ok(query("kinds", eighteen)).await,
        StatusCode::BAD_REQUEST,
        "18 kinds"
    );

    let long = "a".repeat(64);
    for bad in [
        "/api/v1/events?swimlane=UPPER".to_owned(),
        "/api/v1/events?swimlane=".to_owned(),
        "/api/v1/events?swimlane".to_owned(),
        "/api/v1/events?swimlane=a%2Fb".to_owned(),
        "/api/v1/events?swimlane=a+b".to_owned(),
        "/api/v1/events?swimlane=-leading-dash".to_owned(),
        format!("/api/v1/events?swimlane={long}"),
        "/api/v1/events?kinds=no_such_kind".to_owned(),
        // The kind exists in the contract, but it is internal: the OpenAPI list of kinds does not have it, and a request
        // for it is not a way to get it.
        "/api/v1/events?kinds=user_sessions_revoked".to_owned(),
        "/api/v1/events?kinds=findings_changed&kinds=user_sessions_revoked".to_owned(),
        "/api/v1/events?kinds=File_Observed".to_owned(),
        "/api/v1/events?kinds=".to_owned(),
        "/api/v1/events?unknown=1".to_owned(),
        "/api/v1/events?swimlane=sit1&&kinds=resync".to_owned(),
        format!("/api/v1/events?swimlane=sit1&pad={}", "x".repeat(9000)),
    ] {
        let response = open(&rig, Some("viewer"), &bad, &[]).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{}",
            &bad[..bad.len().min(80)]
        );
        assert_eq!(problem_code(response).await, "bad_request");
    }
    assert_eq!(
        open(
            &rig,
            Some("viewer"),
            "/api/v1/events",
            &[("last-event-id", &"1".repeat(65))]
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
        "a Last-Event-ID over 64 characters"
    );
    assert!(rig.metrics.sse_refused_total(SseRefusal::BadRequest) >= 16);
}

#[tokio::test(start_paused = true)]
async fn a_duplicate_in_the_query_is_one_filter_entry() {
    let rig = rig(settings());
    let mut stream = frames(
        open(
            &rig,
            Some("viewer"),
            "/api/v1/events?swimlane=sit1&swimlane=sit1&kinds=findings_changed&kinds=findings_changed",
            &[],
        )
        .await,
    );
    assert_eq!(stream.next().await.unwrap(), RETRY);
    rig.source.publish(findings("sit1"));
    assert_eq!(kind_of(&stream.next().await.unwrap()), "findings_changed");
}

// ------------------------------------------------------------------------------------------------ frames

#[tokio::test(start_paused = true)]
async fn the_stream_opens_with_a_retry_hint_and_beats_every_15_seconds() {
    let rig = rig(settings());
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    let start = Instant::now();
    assert_eq!(stream.next().await.unwrap(), RETRY);
    assert_eq!(start.elapsed(), Duration::ZERO, "the hint is sent at once");
    assert_eq!(stream.next().await.unwrap(), HEARTBEAT);
    assert_eq!(start.elapsed(), Duration::from_secs(15));
    assert_eq!(stream.next().await.unwrap(), HEARTBEAT);
    assert_eq!(start.elapsed(), Duration::from_secs(30));
}

#[tokio::test(start_paused = true)]
async fn sse_frame_format_is_golden() {
    let rig = rig(settings());
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    let mut seen = vec![stream.next().await.unwrap()];
    rig.source.publish(findings("sit1"));
    rig.source.publish(proposal(7));
    rig.source.publish_unstored(DomainEvent::Resync);
    rig.source.publish(DomainEvent::DriftChanged {
        swimlane: swimlane("sit2"),
        paths: vec![domain::NfsPath::parse("svc/app.yml").unwrap()],
    });
    for _ in 0..4 {
        seen.push(stream.next().await.unwrap());
    }
    seen.push(stream.next().await.unwrap());
    insta::assert_snapshot!("sse_frames", seen.join("-----\n"));
}

#[tokio::test(start_paused = true)]
async fn a_frame_holds_one_data_line_whatever_the_event_contains() {
    let rig = rig(settings());
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    rig.source.publish(DomainEvent::DriftChanged {
        swimlane: swimlane("sit1"),
        paths: vec![domain::NfsPath::parse("a b/c.yml").unwrap()],
    });
    let frame = stream.next_event().await.unwrap();
    assert_eq!(
        frame.matches('\n').count(),
        4,
        "id, event, data, blank line: {frame:?}"
    );
    assert!(frame.ends_with("\n\n"));
}

// -------------------------------------------------------------------------------------- what is delivered

#[tokio::test(start_paused = true)]
async fn only_events_that_match_the_query_are_delivered() {
    let rig = rig(settings());
    let mut stream = frames(
        open(
            &rig,
            Some("viewer"),
            "/api/v1/events?swimlane=sit1&kinds=findings_changed&kinds=proposal_changed",
            &[],
        )
        .await,
    );
    rig.source.publish(findings("sit2"));
    rig.source.publish(findings("sit1"));
    rig.source.publish(DomainEvent::DriftChanged {
        swimlane: swimlane("sit1"),
        paths: vec![],
    });
    rig.source.publish(proposal(9));
    let first = stream.next_event().await.unwrap();
    let second = stream.next_event().await.unwrap();
    assert_eq!(parse(&first).event.as_deref(), Some("findings_changed"));
    assert_eq!(parse(&first).id.as_deref(), Some("2"));
    assert_eq!(
        parse(&second).event.as_deref(),
        Some("proposal_changed"),
        "an event that belongs to no swimlane passes a swimlane filter"
    );
}

#[tokio::test(start_paused = true)]
async fn an_access_request_is_for_admins_and_a_resync_is_for_everyone() {
    let rig = rig(settings());
    let mut viewer = frames(
        open(
            &rig,
            Some("viewer"),
            "/api/v1/events?kinds=access_request_created&kinds=findings_changed",
            &[],
        )
        .await,
    );
    let mut admin = frames(open(&rig, Some("admin"), "/api/v1/events", &[]).await);
    rig.source.publish(DomainEvent::AccessRequestCreated {
        user: user_id("newcomer"),
    });
    rig.source.publish_unstored(DomainEvent::Resync);
    rig.source.publish(findings("sit1"));

    let kinds_of = |frames: Vec<String>| frames.iter().map(|f| kind_of(f)).collect::<Vec<_>>();
    let mut for_viewer = Vec::new();
    for _ in 0..2 {
        for_viewer.push(viewer.next_event().await.unwrap());
    }
    assert_eq!(kinds_of(for_viewer), ["resync", "findings_changed"]);
    let mut for_admin = Vec::new();
    for _ in 0..3 {
        for_admin.push(admin.next_event().await.unwrap());
    }
    assert_eq!(
        kinds_of(for_admin),
        ["access_request_created", "resync", "findings_changed"]
    );
}

// ---------------------------------------------------------------------------------- internal events (S4, S12)

/// The kinds a stream carries up to and including the first `findings_changed` (the probe each test publishes after the
/// internal events, so that an internal event that leaks arrives before it).
async fn kinds_up_to_the_probe<S, E>(stream: &mut Frames<S>) -> Vec<String>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    let mut kinds = Vec::new();
    loop {
        let frame = stream.next_event().await.unwrap();
        let kind = kind_of(&frame);
        let done = kind == "findings_changed";
        kinds.push(kind);
        if done {
            return kinds;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn an_internal_event_is_never_streamed_live_to_a_viewer_or_an_admin() {
    let rig = rig(settings());
    let mut viewer = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    let mut admin = frames(open(&rig, Some("admin"), "/api/v1/events", &[]).await);
    // A viewer who narrows the stream by swimlane and one who narrows it by kind get nothing extra either.
    let mut by_lane = frames(open(&rig, Some("viewer2"), "/api/v1/events?swimlane=sit1", &[]).await);
    let mut by_kind = frames(
        open(
            &rig,
            Some("viewer"),
            "/api/v1/events?kinds=findings_changed&kinds=resync",
            &[],
        )
        .await,
    );
    rig.source.publish(revoked("viewer"));
    rig.source.publish(revoked("someone-else"));
    rig.source.publish(findings("sit1"));
    for (who, stream) in [
        ("viewer", &mut viewer),
        ("admin", &mut admin),
        ("by swimlane", &mut by_lane),
        ("by kind", &mut by_kind),
    ] {
        assert_eq!(
            kinds_up_to_the_probe(stream).await,
            ["findings_changed"],
            "{who}: the internal events are dropped at the SSE edge"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn an_internal_event_is_never_replayed_to_a_viewer_or_an_admin() {
    let rig = rig(settings());
    rig.source.publish(proposal(1)); // 1: the event the client last saw
    rig.source.publish(revoked("viewer")); // 2
    rig.source.publish(proposal(3)); // 3
    rig.source.publish(revoked("someone-else")); // 4
    rig.source.publish(findings("sit1")); // 5
    for who in ["viewer", "admin"] {
        let mut stream = frames(open(&rig, Some(who), "/api/v1/events", &[("last-event-id", "1")]).await);
        assert_eq!(stream.next().await.unwrap(), RETRY, "{who}");
        let mut ids = Vec::new();
        let mut kinds = Vec::new();
        loop {
            let frame = stream.next_event().await.unwrap();
            ids.push(parse(&frame).id.unwrap());
            kinds.push(kind_of(&frame));
            if kinds.last().map(String::as_str) == Some("findings_changed") {
                break;
            }
        }
        assert_eq!(
            kinds,
            ["proposal_changed", "findings_changed"],
            "{who}: the replay holds no internal event"
        );
        assert_eq!(ids, ["3", "5"], "{who}: and keeps the ids of the others");
    }
}

#[tokio::test(start_paused = true)]
async fn an_internal_event_that_is_both_replayed_and_live_is_still_dropped() {
    let rig = rig(settings());
    rig.source.publish(proposal(1));
    let source = Arc::clone(&rig.source);
    // Committed between the subscription and the replay's query, so it is in both paths of the edge. The probe follows it
    // in the same hook, so that it is the later of the two in the log and a leak of the first would come before it.
    rig.source.before_next_replay(move || {
        source.publish(revoked("viewer"));
        source.publish(findings("sit1"));
    });
    let mut stream = frames(open(&rig, Some("admin"), "/api/v1/events", &[("last-event-id", "1")]).await);
    assert_eq!(kinds_up_to_the_probe(&mut stream).await, ["findings_changed"]);
}

#[tokio::test(start_paused = true)]
async fn a_resync_and_the_other_kinds_still_reach_the_users_they_reached_before() {
    let rig = rig(settings());
    let mut viewer = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    let mut admin = frames(open(&rig, Some("admin"), "/api/v1/events", &[]).await);
    rig.source.publish(revoked("viewer"));
    rig.source.publish_unstored(DomainEvent::Resync);
    rig.source.publish(DomainEvent::AccessRequestCreated {
        user: user_id("newcomer"),
    });
    rig.source.publish(findings("sit1"));
    assert_eq!(
        kinds_up_to_the_probe(&mut viewer).await,
        ["resync", "findings_changed"]
    );
    assert_eq!(
        kinds_up_to_the_probe(&mut admin).await,
        ["resync", "access_request_created", "findings_changed"]
    );
}

/// One value of every variant of `DomainEvent`. The match has no wildcard: a variant added to the contract does not compile
/// here until someone has decided who sees it, and said so in the `OpenAPI` (`the_openapi_names_every_event_...` below).
fn one_of_each() -> Vec<DomainEvent> {
    let lane = || SwimlaneId::parse("sit1").unwrap();
    let path = || domain::NfsPath::parse("svc/app.yml").unwrap();
    let job = || domain::JobRef::new("sync-job-1", "uid-1").unwrap();
    let services = || vec![domain::ServiceRef::new("csp", "tx-api").unwrap()];
    let events = vec![
        DomainEvent::FileObserved {
            swimlane: lane(),
            path: path(),
            hash: domain::ContentHash::from_bytes([1; 32]),
            source: domain::ObservationSource::Scan,
        },
        DomainEvent::DriftChanged {
            swimlane: lane(),
            paths: vec![path()],
        },
        DomainEvent::FindingsChanged { swimlane: lane() },
        DomainEvent::PendingRestartChanged {
            swimlane: lane(),
            services: services(),
        },
        DomainEvent::PickupChanged {
            swimlane: lane(),
            services: services(),
        },
        DomainEvent::ProposalChanged {
            id: ProposalId::from(7),
            status: ProposalStatus::Pending,
        },
        DomainEvent::AgentStatusChanged {
            swimlane: lane(),
            status: domain::AgentStatus::Connected,
        },
        DomainEvent::GitHeadMoved {
            repo: domain::RepoKind::Base,
            branch: domain::GitRef::parse("master").unwrap(),
            from: None,
            to: domain::CommitId::parse(&"a".repeat(40)).unwrap(),
        },
        DomainEvent::AccessRequestCreated {
            user: user_id("newcomer"),
        },
        DomainEvent::DocIndexed { doc: 3.into() },
        DomainEvent::Resync,
        DomainEvent::AttributionUpdated {
            swimlane: lane(),
            path: path(),
            hash: domain::ContentHash::from_bytes([2; 32]),
        },
        DomainEvent::PrStateChanged {
            link_id: 3.into(),
            state: domain::PrState::Merged,
        },
        DomainEvent::PrJobProgress {
            job_id: 4.into(),
            state: domain::PrJobState::Committing,
        },
        DomainEvent::SyncWindowOpened {
            swimlane: lane(),
            job: job(),
        },
        DomainEvent::SyncWindowClosed {
            swimlane: lane(),
            job: job(),
        },
        DomainEvent::SwimlaneChanged { swimlane: lane() },
        revoked("someone"),
    ];
    for event in &events {
        match event {
            DomainEvent::FileObserved { .. }
            | DomainEvent::DriftChanged { .. }
            | DomainEvent::FindingsChanged { .. }
            | DomainEvent::PendingRestartChanged { .. }
            | DomainEvent::PickupChanged { .. }
            | DomainEvent::ProposalChanged { .. }
            | DomainEvent::AgentStatusChanged { .. }
            | DomainEvent::GitHeadMoved { .. }
            | DomainEvent::AccessRequestCreated { .. }
            | DomainEvent::DocIndexed { .. }
            | DomainEvent::Resync
            | DomainEvent::AttributionUpdated { .. }
            | DomainEvent::PrStateChanged { .. }
            | DomainEvent::PrJobProgress { .. }
            | DomainEvent::SyncWindowOpened { .. }
            | DomainEvent::SyncWindowClosed { .. }
            | DomainEvent::SwimlaneChanged { .. }
            | DomainEvent::UserSessionsRevoked { .. } => {}
        }
    }
    let kinds: std::collections::BTreeSet<String> = events.iter().map(wire_type).collect();
    assert_eq!(
        kinds.len(),
        18,
        "one value of each of the 18 variants, none twice: {kinds:?}"
    );
    events
}

/// The `type` of an event on the wire: the `event:` of its frame and the kind of the `OpenAPI`.
fn wire_type(event: &DomainEvent) -> String {
    serde_json::to_value(event).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn user_of(role: Role) -> domain::User {
    let text = |s: &str| domain::ShortText::parse(s).unwrap();
    domain::User {
        id: user_id("reader"),
        email: text("reader@example.test"),
        display_name: text("reader"),
        role: Some(role),
        status: UserStatus::Active,
        requires_approval: false,
        github_login: None,
        last_seen_at: None,
    }
}

/// The document says who does not receive an event, and it is the code's list: the events that `RoleVisibility` keeps from a
/// Viewer, an Operator and an Admin, over one value of every variant, are the ones the `Visibility.` paragraph of
/// `GET /api/v1/events` names, so a client of a lower role never waits for one. A variant that is restricted without the
/// document saying so, or a document that names an event the rule does not restrict, fails here.
#[test]
fn the_openapi_names_every_event_the_default_rule_keeps_from_a_viewer() {
    const SPEC: &str = include_str!("../../../api/openapi.yaml");
    let events = one_of_each();
    let withheld_from = |role: Role| -> std::collections::BTreeSet<String> {
        let user = user_of(role);
        events
            .iter()
            .filter(|e| !e.is_internal() && !RoleVisibility.visible(&user, e))
            .map(wire_type)
            .collect()
    };
    let restricted: std::collections::BTreeSet<String> = ["access_request_created".to_owned()].into();
    assert_eq!(withheld_from(Role::Viewer), restricted, "a Viewer");
    assert_eq!(withheld_from(Role::Operator), restricted, "an Operator");
    assert!(
        withheld_from(Role::Admin).is_empty(),
        "an Admin sees every event that is streamed"
    );

    let start = SPEC
        .find("Visibility. Roles are global")
        .expect("the description of GET /api/v1/events has a `Visibility.` paragraph");
    let paragraph = SPEC[start..].split("\n\n").next().unwrap();
    for kind in &restricted {
        assert!(
            paragraph.contains(&format!("`{kind}`")),
            "the Visibility paragraph must name `{kind}`: only an Admin receives it"
        );
    }
    for event in &events {
        let kind = wire_type(event);
        if restricted.contains(&kind) || kind == "resync" || kind == "user_sessions_revoked" {
            continue;
        }
        assert!(
            !paragraph.contains(&format!("`{kind}`")),
            "the Visibility paragraph names `{kind}`, which the default rule shows to every Viewer"
        );
    }
}

#[test]
fn the_default_visibility_rule_refuses_an_internal_event_to_every_role() {
    let internal = revoked("viewer");
    for (name, role) in [
        ("viewer", Role::Viewer),
        ("operator", Role::Operator),
        ("admin", Role::Admin),
    ] {
        let text = |s: &str| domain::ShortText::parse(s).unwrap();
        let user = domain::User {
            id: user_id(name),
            email: text(&format!("{name}@example.test")),
            display_name: text(name),
            role: Some(role),
            status: UserStatus::Active,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        };
        assert!(
            RoleVisibility.visible(&user, &findings("sit1")),
            "{name}: the rule still shows everything else"
        );
        assert!(
            !RoleVisibility.visible(&user, &internal),
            "{name}: the edge drops internal events first, and the rule must not be the hole behind it"
        );
    }
}

// ----------------------------------------------------------------------------------------------- replay

#[tokio::test(start_paused = true)]
async fn last_event_id_replays_what_was_missed_and_then_goes_live() {
    let rig = rig(settings());
    for n in 1..=4 {
        rig.source.publish(proposal(n));
    }
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[("last-event-id", "2")]).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    let a = parse(&stream.next().await.unwrap());
    let b = parse(&stream.next().await.unwrap());
    assert_eq!((a.id.as_deref(), b.id.as_deref()), (Some("3"), Some("4")));
    rig.source.publish(proposal(5));
    assert_eq!(
        parse(&stream.next_event().await.unwrap()).id.as_deref(),
        Some("5")
    );
}

#[tokio::test(start_paused = true)]
async fn a_replay_follows_the_query_and_the_visibility_rules() {
    let rig = rig(settings());
    rig.source.publish(findings("sit1")); // 1
    rig.source.publish(DomainEvent::AccessRequestCreated {
        user: user_id("newcomer"),
    }); // 2
    rig.source.publish(findings("sit2")); // 3
    rig.source.publish(findings("sit1")); // 4
    let mut stream = frames(
        open(
            &rig,
            Some("viewer"),
            "/api/v1/events?swimlane=sit1",
            &[("last-event-id", "1")],
        )
        .await,
    );
    let replayed = parse(&stream.next_event().await.unwrap());
    assert_eq!(
        replayed.id.as_deref(),
        Some("4"),
        "2 is not for a viewer and 3 is another swimlane"
    );
}

#[tokio::test(start_paused = true)]
async fn a_last_event_id_that_is_too_old_unknown_or_not_a_number_gets_a_resync_first() {
    let rig = rig(settings());
    for n in 1..=3 {
        rig.source.publish(proposal(n));
    }
    rig.source.expire_through(domain_id(1));
    for id in ["1", "500", "abc", "-4", "0"] {
        let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[("last-event-id", id)]).await);
        assert_eq!(stream.next().await.unwrap(), RETRY, "{id}");
        let frame = stream.next().await.unwrap();
        assert_eq!(
            frame, "event: resync\ndata: {\"type\":\"resync\"}\n\n",
            "{id}: a resync has no id, so nothing can resume from it"
        );
    }
}

fn domain_id(n: i64) -> hub_platform::events::wire::EventId {
    hub_platform::events::wire::EventId::new(n).unwrap()
}

#[tokio::test(start_paused = true)]
async fn a_replay_larger_than_the_limit_is_a_resync() {
    let rig = rig(SseSettings {
        replay_limit: 2,
        ..settings()
    });
    for n in 1..=4 {
        rig.source.publish(proposal(n));
    }
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[("last-event-id", "1")]).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    assert_eq!(kind_of(&stream.next().await.unwrap()), "resync");
    rig.source.publish(proposal(5));
    let live = parse(&stream.next_event().await.unwrap());
    assert_eq!(live.id.as_deref(), Some("5"), "and then the stream is live");
}

#[tokio::test(start_paused = true)]
async fn an_event_that_is_both_in_the_replay_and_live_is_delivered_once() {
    let rig = rig(settings());
    rig.source.publish(proposal(1));
    rig.source.publish(proposal(2));
    let source = Arc::clone(&rig.source);
    // Event 3 commits after the subscription and before the replay's query: it is in both.
    rig.source.before_next_replay(move || {
        source.publish(proposal(3));
    });
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[("last-event-id", "1")]).await);
    let mut ids = Vec::new();
    ids.push(parse(&stream.next_event().await.unwrap()).id.unwrap());
    ids.push(parse(&stream.next_event().await.unwrap()).id.unwrap());
    rig.source.publish(proposal(4));
    ids.push(parse(&stream.next_event().await.unwrap()).id.unwrap());
    assert_eq!(ids, ["2", "3", "4"]);
}

// -------------------------------------------------------------------------------- how a stream ends

#[tokio::test(start_paused = true)]
async fn the_stream_ends_within_one_heartbeat_when_the_session_ends() {
    let rig = rig(settings());
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    rig.auth.remove("viewer");
    let start = Instant::now();
    assert_eq!(stream.next().await, None, "the stream is closed");
    assert!(
        start.elapsed() <= Duration::from_secs(15),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(rig.metrics.sse_ended_total(SseEnd::Revoked), 1);
    assert_eq!(rig.state.active_streams(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_stream_ends_when_the_role_drops_below_viewer() {
    let rig = rig(settings());
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    rig.auth.set("viewer", None, UserStatus::Active);
    assert_eq!(stream.next().await, None);
    assert_eq!(rig.metrics.sse_ended_total(SseEnd::Revoked), 1);
}

// ------------------------------------------------------------- plan 18, T4 (G2): a stream is bound to its own session

/// The header that carries a test session's name, which stands in for the session cookie.
const SESSION_HEADER: &str = "x-test-session";

/// `FakeSessionAuth` for the users, and a session binding of the test's own: the hash of the session header. A session is ended by
/// naming it in `end`, as a logout ends one cookie and not the user's other browsers.
struct BoundAuth {
    users: FakeSessionAuth,
    ended: std::sync::Mutex<std::collections::HashSet<[u8; 32]>>,
    asked: std::sync::atomic::AtomicUsize,
}

fn binding_of(session: &str) -> StreamBinding {
    use sha2::{Digest, Sha256};
    StreamBinding::new(Sha256::digest(session.as_bytes()).into())
}

impl BoundAuth {
    fn end(&self, session: &str) {
        self.ended.lock().unwrap().insert(*binding_of(session).as_bytes());
    }
}

#[async_trait::async_trait]
impl SessionAuth for BoundAuth {
    async fn authenticate(&self, parts: &http::request::Parts) -> Result<domain::User, ports::AuthError> {
        self.users.authenticate(parts).await
    }

    async fn still_valid(&self, user: &domain::UserId) -> bool {
        self.users.still_valid(user).await
    }

    fn stream_binding(&self, headers: &http::HeaderMap) -> Option<StreamBinding> {
        let session = headers.get(SESSION_HEADER)?.to_str().ok()?;
        Some(binding_of(session))
    }

    async fn binding_valid(&self, binding: &StreamBinding) -> bool {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        !self.ended.lock().unwrap().contains(binding.as_bytes())
    }
}

/// The router, the auth it asks, the state, the metrics, and the sender that keeps the hub "open".
type BoundRig = (
    Router,
    Arc<BoundAuth>,
    Arc<SseState>,
    EventMetrics,
    watch::Sender<bool>,
);

fn bound_rig() -> BoundRig {
    let auth = Arc::new(BoundAuth {
        users: FakeSessionAuth::new().with_user("viewer", Some(Role::Viewer), UserStatus::Active),
        ended: std::sync::Mutex::default(),
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let (closing, closing_rx) = watch::channel(false);
    let metrics = EventMetrics::default();
    let state = SseState::new(
        Arc::new(FakeEventSource::new(64)) as _,
        Arc::clone(&auth) as _,
        Arc::new(RoleVisibility),
        settings(),
        closing_rx,
        metrics.clone(),
    );
    let (app, _) = sse::mount(
        GuardedRouter::new(Arc::clone(&auth) as _, None),
        Arc::clone(&state),
    )
    .into_parts()
    .unwrap();
    (app, auth, state, metrics, closing)
}

async fn open_session(app: &Router, session: Option<&str>) -> Response<Body> {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/events")
        .header(TEST_USER_HEADER, "viewer");
    if let Some(session) = session {
        request = request.header(SESSION_HEADER, session);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn a_stream_ends_within_one_heartbeat_when_its_session_ends_though_the_user_is_still_valid() {
    let (app, auth, state, metrics, _closing) = bound_rig();
    let mut stream = frames(open_session(&app, Some("laptop")).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    assert_eq!(stream.next().await.unwrap(), HEARTBEAT, "while the session lives");

    auth.end("laptop");
    let start = Instant::now();
    assert_eq!(stream.next().await, None, "the stream is closed");
    assert!(
        start.elapsed() <= Duration::from_secs(15),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(metrics.sse_ended_total(SseEnd::Revoked), 1);
    assert_eq!(state.active_streams(), 0);
}

#[tokio::test(start_paused = true)]
async fn ending_one_session_leaves_the_same_users_other_session_streaming() {
    let (app, auth, state, metrics, _closing) = bound_rig();
    let mut laptop = frames(open_session(&app, Some("laptop")).await);
    let mut phone = frames(open_session(&app, Some("phone")).await);
    assert_eq!(laptop.next().await.unwrap(), RETRY);
    assert_eq!(phone.next().await.unwrap(), RETRY);

    auth.end("laptop");
    assert_eq!(
        laptop.next().await,
        None,
        "the signed-out browser's stream is closed"
    );
    assert_eq!(
        phone.next().await.unwrap(),
        HEARTBEAT,
        "the user's other browser keeps its stream"
    );
    assert_eq!(state.active_streams(), 1);
    assert_eq!(metrics.sse_ended_total(SseEnd::Revoked), 1);
}

#[tokio::test(start_paused = true)]
async fn an_implementation_that_names_no_session_is_asked_nothing_more_than_before() {
    let (app, auth, _state, metrics, _closing) = bound_rig();
    // No session header: `stream_binding` answers `None`, and the stream is judged by the user alone, as it always was.
    let mut stream = frames(open_session(&app, None).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    assert_eq!(stream.next().await.unwrap(), HEARTBEAT);
    assert_eq!(stream.next().await.unwrap(), HEARTBEAT);
    assert_eq!(
        auth.asked.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "`binding_valid` is not consulted for a stream without a binding"
    );
    assert_eq!(metrics.sse_ended_total(SseEnd::Revoked), 0);
}

/// An implementation that names the session that opened a stream and forgets to say how to ask whether it still lives: it
/// overrides `stream_binding` and not `binding_valid`.
struct NamesASessionOnly {
    users: FakeSessionAuth,
}

#[async_trait::async_trait]
impl SessionAuth for NamesASessionOnly {
    async fn authenticate(&self, parts: &http::request::Parts) -> Result<domain::User, ports::AuthError> {
        self.users.authenticate(parts).await
    }

    async fn still_valid(&self, user: &domain::UserId) -> bool {
        self.users.still_valid(user).await
    }

    fn stream_binding(&self, headers: &http::HeaderMap) -> Option<StreamBinding> {
        let session = headers.get(SESSION_HEADER)?.to_str().ok()?;
        Some(binding_of(session))
    }
}

#[tokio::test(start_paused = true)]
async fn an_implementation_that_names_a_session_and_cannot_answer_for_it_fails_closed() {
    // A provided `binding_valid` that said yes would keep every stream of such an implementation open for ever, after a sign-out as
    // well: a missing method is not an answer. It says no, so the stream ends at its first heartbeat and the mistake is seen at once.
    let auth = Arc::new(NamesASessionOnly {
        users: FakeSessionAuth::new().with_user("viewer", Some(Role::Viewer), UserStatus::Active),
    });
    let (_closing, closing_rx) = watch::channel(false);
    let metrics = EventMetrics::default();
    let state = SseState::new(
        Arc::new(FakeEventSource::new(64)) as _,
        Arc::clone(&auth) as _,
        Arc::new(RoleVisibility),
        settings(),
        closing_rx,
        metrics.clone(),
    );
    let (app, _) = sse::mount(
        GuardedRouter::new(Arc::clone(&auth) as _, None),
        Arc::clone(&state),
    )
    .into_parts()
    .unwrap();
    let mut stream = frames(open_session(&app, Some("laptop")).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    let start = Instant::now();
    assert_eq!(stream.next().await, None, "the stream is closed, not kept open");
    assert!(
        start.elapsed() <= Duration::from_secs(15),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(metrics.sse_ended_total(SseEnd::Revoked), 1);
    assert_eq!(state.active_streams(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_stream_ends_with_a_retry_hint_when_the_hub_shuts_down() {
    let rig = rig(settings());
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    rig.closing.send_replace(true);
    assert_eq!(
        stream.next().await.unwrap(),
        RETRY,
        "the client is told when to come back"
    );
    assert_eq!(stream.next().await, None);
    assert_eq!(rig.metrics.sse_ended_total(SseEnd::Shutdown), 1);
    assert_eq!(rig.state.active_streams(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_has_lived_its_maximum_age_is_closed_for_the_client_to_resume() {
    let rig = rig(SseSettings {
        max_age: Duration::from_secs(40),
        ..settings()
    });
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    let start = Instant::now();
    assert_eq!(stream.next().await.unwrap(), RETRY);
    assert_eq!(stream.next().await.unwrap(), HEARTBEAT);
    assert_eq!(stream.next().await.unwrap(), HEARTBEAT);
    assert_eq!(stream.next().await.unwrap(), RETRY, "the final hint");
    assert_eq!(stream.next().await, None);
    assert_eq!(start.elapsed(), Duration::from_secs(40));
    assert_eq!(rig.metrics.sse_ended_total(SseEnd::MaxAge), 1);
}

#[tokio::test(start_paused = true)]
async fn the_stream_ends_when_the_bus_does() {
    let rig = rig(settings());
    let mut stream = frames(open(&rig, Some("viewer"), "/api/v1/events", &[]).await);
    assert_eq!(stream.next().await.unwrap(), RETRY);
    rig.source.close();
    assert_eq!(stream.next().await, None);
    assert_eq!(rig.metrics.sse_ended_total(SseEnd::BusClosed), 1);
}

#[tokio::test(start_paused = true)]
async fn a_client_that_goes_away_releases_its_place() {
    let rig = rig(settings());
    let response = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    assert_eq!(rig.state.active_streams(), 1);
    drop(response);
    wait_until(|| rig.state.active_streams() == 0).await;
    assert_eq!(rig.metrics.sse_ended_total(SseEnd::ClientGone), 1);
    assert_eq!(rig.source.subscribers(), 0, "and its subscription");
}

#[tokio::test(start_paused = true)]
async fn a_client_that_stops_reading_is_cut_off_after_the_write_timeout() {
    let rig = rig(SseSettings {
        outbound_queue: 1,
        write_timeout: Duration::from_secs(30),
        ..settings()
    });
    // Opened and never read: the first frame fills the queue, the next write waits.
    let response = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    let start = Instant::now();
    wait_until(|| rig.state.active_streams() == 0).await;
    assert!(
        start.elapsed() >= Duration::from_secs(30) && start.elapsed() < Duration::from_secs(60),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(rig.metrics.sse_ended_total(SseEnd::SlowClient), 1);
    drop(response);
}

// ------------------------------------------------------------------------------------------------ caps

#[tokio::test(start_paused = true)]
async fn connections_are_capped_per_user_and_in_total() {
    let rig = rig(SseSettings {
        per_user: 2,
        global: 3,
        ..settings()
    });
    let a = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    let b = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    assert_eq!(a.status(), StatusCode::OK);
    assert_eq!(b.status(), StatusCode::OK);
    let third = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(problem_code(third).await, "too_many_requests");

    let c = open(&rig, Some("viewer2"), "/api/v1/events", &[]).await;
    assert_eq!(c.status(), StatusCode::OK, "another user has their own allowance");
    let over = open(&rig, Some("admin"), "/api/v1/events", &[]).await;
    assert_eq!(
        over.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the global cap is 3"
    );
    assert_eq!(rig.metrics.sse_refused_total(SseRefusal::TooManyForUser), 1);
    assert_eq!(rig.metrics.sse_refused_total(SseRefusal::TooManyTotal), 1);

    drop(a);
    wait_until(|| rig.state.active_streams() == 2).await;
    let again = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    assert_eq!(
        again.status(),
        StatusCode::OK,
        "a closed stream gives its place back"
    );
    drop((b, c, again));
}

#[tokio::test(start_paused = true)]
async fn a_refused_request_does_not_subscribe() {
    let rig = rig(SseSettings {
        per_user: 1,
        ..settings()
    });
    let first = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    let second = open(&rig, Some("viewer"), "/api/v1/events", &[]).await;
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(rig.source.subscribers(), 1);
    drop(first);
}

/// Let the stream tasks run until `condition` holds. Time is paused, so this costs no real time.
async fn wait_until(condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(condition(), "the condition did not become true");
}

#[test]
fn the_default_settings_are_the_ones_in_the_plan() {
    let s = SseSettings::default();
    assert_eq!(s.heartbeat, Duration::from_secs(15));
    assert_eq!(s.per_user, 5);
    assert_eq!(s.global, 2000);
    assert_eq!(s.replay_limit, 1000);
}
