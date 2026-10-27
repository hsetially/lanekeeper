//! T5: the SSE route in the composed hub: its guard, the headers and limits it passes through, and that a stream outlives the
//! request timeout (the timeout covers the response head, not the body of a stream).
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

use domain::{DomainEvent, Role, SwimlaneId, UserStatus};
use futures::StreamExt;
use http::{Method, StatusCode};
use hub::config::LimitSettings;
use hub::{Deps, EventsModule, Hub, HubSettings, SessionEdge};
use hub_platform::events::fake::FakeEventSource;
use hub_platform::events::metrics::EventMetrics;
use hub_platform::events::sse::{SseSettings, SseState};
use hub_platform::events::visibility::RoleVisibility;
use hub_platform::{FakeSessionAuth, RouteGuard, platform_routes};
use support::{get, header_of, request_as, send};
use tokio::sync::watch;

struct Rig {
    hub: Hub,
    source: Arc<FakeEventSource>,
    _closing: watch::Sender<bool>,
}

fn rig(settings: HubSettings) -> Rig {
    let source = Arc::new(FakeEventSource::new(64));
    let auth = Arc::new(
        FakeSessionAuth::new()
            .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending),
    );
    let (closing, closing_rx) = watch::channel(false);
    let state = SseState::new(
        Arc::clone(&source) as _,
        Arc::clone(&auth) as _,
        Arc::new(RoleVisibility),
        SseSettings::default(),
        closing_rx,
        EventMetrics::default(),
    );
    let hub = Hub::build(Deps {
        settings,
        session: SessionEdge::without_sessions(auth),
        mounted: Vec::new(),
        modules: vec![Arc::new(EventsModule::new(state))],
        static_files: None,
        checks: Vec::new(),
    })
    .unwrap();
    Rig {
        hub,
        source,
        _closing: closing,
    }
}

#[tokio::test]
async fn the_events_route_is_in_the_hub_route_table_with_the_platform_guard() {
    let rig = rig(HubSettings::default());
    let expected: Vec<_> = platform_routes()
        .into_iter()
        .filter(|r| r.path == "/api/v1/events")
        .collect();
    assert_eq!(expected.len(), 1);
    assert_eq!(rig.hub.routes, expected);
    assert_eq!(rig.hub.routes[0].guard, RouteGuard::Role(Role::Viewer));
}

#[tokio::test]
async fn a_user_who_is_not_active_gets_a_problem_not_a_stream() {
    let rig = rig(HubSettings::default());
    let response = send(
        &rig.hub.web,
        request_as(
            "pending",
            [198, 51, 100, 1],
            Method::GET,
            "/api/v1/events",
            axum::body::Body::empty(),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/problem+json")
    );
    let anonymous = send(
        &rig.hub.web,
        support::anonymous(Method::GET, "/api/v1/events", axum::body::Body::empty()),
    )
    .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(start_paused = true)]
async fn the_stream_carries_the_api_headers_and_outlives_the_request_timeout() {
    let settings = HubSettings {
        limits: LimitSettings {
            request_timeout: Duration::from_secs(5),
            ..LimitSettings::default()
        },
        ..HubSettings::default()
    };
    let rig = rig(settings);
    let response = send(&rig.hub.web, get("/api/v1/events")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "content-type"), Some("text/event-stream"));
    assert_eq!(header_of(&response, "cache-control"), Some("no-store"));
    assert_eq!(header_of(&response, "x-content-type-options"), Some("nosniff"));
    assert_eq!(
        header_of(&response, "strict-transport-security"),
        Some("max-age=63072000; includeSubDomains")
    );
    assert_eq!(
        header_of(&response, "content-security-policy"),
        Some("default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
    );
    assert!(header_of(&response, "x-request-id").is_some());

    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert_eq!(&first[..], b"retry: 3000\n\n");
    // Two minutes of virtual time pass: the 5 second request timeout does not cut the stream, and events still arrive.
    let mut seen = Vec::new();
    for _ in 0..8 {
        seen.push(body.next().await.unwrap().unwrap());
    }
    assert!(seen.iter().all(|f| &f[..] == b": heartbeat\n\n"));
    rig.source.publish(DomainEvent::FindingsChanged {
        swimlane: SwimlaneId::parse("sit1").unwrap(),
    });
    let event = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&event).contains("event: findings_changed"));
    drop(body);
}
