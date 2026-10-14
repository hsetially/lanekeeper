//! S4 (T6): authorization on the route *and* on the port. A request with no session is 401; a pending, disabled or role-less user is
//! 403 on every one of the fifteen routes, before the handler reads anything; every role that can sign in reads them all; and the
//! service itself refuses such a user, so a route that forgot its guard would still not leak.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use domain::{CompareRef, ContentSource, Role, ServeRequest, UserStatus};
use hub_registry::RegistryApi;
use ports::ReadError;
use ports::conformance::sample::user;
use support::http::Http;
use support::reads::{lane, logical, nfs, page};

/// One valid request for each of the fifteen routes, over the seeded world.
fn requests() -> Vec<(&'static str, String)> {
    let served = "application=core&tenant=sit1&file=core.yml";
    vec![
        ("listSwimlanes", "/api/v1/swimlanes".to_owned()),
        ("getSwimlane", "/api/v1/swimlanes/sit1".to_owned()),
        ("listSwimlaneTree", "/api/v1/swimlanes/sit1/tree".to_owned()),
        ("listDrift", "/api/v1/swimlanes/sit1/drift".to_owned()),
        (
            "getFileContent",
            "/api/v1/swimlanes/sit1/files/content?path=core.yml".to_owned(),
        ),
        (
            "listFileHistory",
            "/api/v1/swimlanes/sit1/files/history?path=core.yml".to_owned(),
        ),
        (
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml".to_owned(),
        ),
        (
            "compareTree",
            "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2".to_owned(),
        ),
        (
            "getGrid",
            "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2".to_owned(),
        ),
        ("searchSettings", "/api/v1/search/settings?q=port".to_owned()),
        ("searchText", "/api/v1/search/text?pattern=timeout".to_owned()),
        ("listFindings", "/api/v1/findings".to_owned()),
        (
            "getEffectiveConfig",
            "/api/v1/swimlanes/sit1/effective?file=core.yml&tenant=sit1".to_owned(),
        ),
        (
            "listPendingRestarts",
            "/api/v1/swimlanes/sit1/pending-restarts".to_owned(),
        ),
        ("getServedFile", format!("/api/v1/swimlanes/sit1/served?{served}")),
    ]
}

#[tokio::test]
async fn there_is_a_request_for_every_route_of_the_table() {
    let Some(h) = Http::seeded().await else { return };
    let mut paths: Vec<String> = requests()
        .iter()
        .map(|(_, uri)| {
            let path = uri.split('?').next().unwrap();
            // `sit1` stands for `{id}`.
            path.replace("/swimlanes/sit1", "/swimlanes/{id}")
        })
        .collect();
    paths.sort();
    let mut table: Vec<String> = h.specs.iter().map(|s| s.path.clone()).collect();
    table.sort();
    assert_eq!(
        paths, table,
        "a route without a request here would escape the tests below"
    );
}

#[tokio::test]
async fn no_session_is_401_on_every_route() {
    let Some(h) = Http::seeded().await else { return };
    for (name, uri) in requests() {
        let reply = h.get(None, &uri).await;
        assert_eq!(reply.status, 401, "{name}: {}", reply.text());
        assert_eq!(
            reply.header("content-type"),
            Some("application/problem+json"),
            "{name}"
        );
        assert_eq!(reply.code(), "unauthorized", "{name}");
        let unknown = h.get(Some("nobody-knows-me"), &uri).await;
        assert_eq!(unknown.status, 401, "{name}: an unknown session is no session");
    }
}

#[tokio::test]
async fn a_pending_disabled_or_role_less_user_is_403_on_every_route_and_reads_nothing() {
    let Some(h) = Http::seeded().await else { return };
    for (name, uri) in requests() {
        for key in ["pending", "disabled", "norole"] {
            let reply = h.get(Some(key), &uri).await;
            assert_eq!(reply.status, 403, "{name} as {key}: {}", reply.text());
            assert_eq!(
                reply.header("content-type"),
                Some("application/problem+json"),
                "{name} as {key}"
            );
            assert_eq!(reply.code(), "forbidden", "{name} as {key}");
            let text = reply.text();
            assert!(
                !text.contains("sit1") && !text.contains("core.yml"),
                "{name} as {key}: the refusal says nothing about the data: {text}"
            );
        }
    }
    // None of the refusals reached a handler: the service counted no read.
    let m = &h.reads.world.rig.metrics;
    for op in [
        hub_registry::metrics::ReadOp::Swimlanes,
        hub_registry::metrics::ReadOp::Grid,
        hub_registry::metrics::ReadOp::Content,
        hub_registry::metrics::ReadOp::SearchText,
    ] {
        for outcome in [
            hub_registry::metrics::ReadOutcome::Ok,
            hub_registry::metrics::ReadOutcome::Forbidden,
        ] {
            assert_eq!(m.read_count(op, outcome), 0, "{op:?} {outcome:?}");
        }
    }
}

#[tokio::test]
async fn every_role_that_can_sign_in_reads_every_route_and_no_route_asks_for_more() {
    let Some(h) = Http::seeded().await else { return };
    for (name, uri) in requests() {
        for key in ["viewer", "editor", "admin"] {
            let reply = h.get(Some(key), &uri).await;
            assert!(
                !matches!(reply.status.as_u16(), 401 | 403),
                "{name} as {key}: {} {}",
                reply.status,
                reply.text()
            );
        }
    }
    // The reads that need nothing else answer 200 for a viewer.
    for (name, uri) in requests() {
        if name == "getServedFile" {
            continue; // needs an agent, tested with its own script
        }
        let reply = h.view(&uri).await;
        assert_eq!(reply.status, 200, "{name}: {}", reply.text());
    }
}

#[tokio::test]
async fn the_service_itself_refuses_a_user_who_is_not_an_active_viewer() {
    let Some(h) = Http::seeded().await else { return };
    let api = &h.reads.service;
    let refused = [
        user(7, None, UserStatus::Pending),
        user(8, Some(Role::Viewer), UserStatus::Disabled),
        user(9, None, UserStatus::Active),
        user(10, Some(Role::Admin), UserStatus::Pending),
    ];
    let s = lane("sit1");
    let f = logical("core.yml");
    let p = nfs("core.yml");
    for u in &refused {
        let forbidden = ReadError::Forbidden;
        assert_eq!(api.swimlanes_page(u, page(10)).await.unwrap_err(), forbidden);
        assert_eq!(api.swimlane(u, &s).await.unwrap_err(), forbidden);
        assert_eq!(
            api.tree_filtered(u, &s, &domain::NfsPath::root(), None, page(10))
                .await
                .unwrap_err(),
            forbidden
        );
        assert_eq!(
            api.drift(u, &s, None, &domain::NfsPath::root(), page(10))
                .await
                .unwrap_err(),
            forbidden
        );
        assert_eq!(
            api.content_tagged(u, &s, &p, ContentSource::Nfs, None)
                .await
                .unwrap_err(),
            forbidden
        );
        let (left, right) = (CompareRef::Nfs(lane("sit1")), CompareRef::Nfs(lane("sit2")));
        assert_eq!(
            api.compare_tagged(u, left.clone(), right.clone(), &f, None, None)
                .await
                .unwrap_err(),
            forbidden
        );
        assert_eq!(
            api.compare_tree_under(u, left, right, &domain::NfsPath::root(), page(10))
                .await
                .unwrap_err(),
            forbidden
        );
        let grid = domain::GridQuery {
            file: f.clone(),
            swimlanes: vec![lane("sit1")],
            channel: None,
            only_differing: false,
            page: page(10),
        };
        assert_eq!(api.grid_named(u, grid).await.unwrap_err(), forbidden);
        assert_eq!(
            api.effective_tagged(u, &s, &f, None, None, None)
                .await
                .unwrap_err(),
            forbidden
        );
        assert_eq!(
            api.pending_restarts_page(u, &s, page(10)).await.unwrap_err(),
            forbidden
        );
        let request = ServeRequest {
            application: domain::AppName::parse("core").unwrap(),
            tenant: domain::TenantId::parse("sit1").unwrap(),
            channel: None,
            file: p.clone(),
        };
        assert_eq!(api.served(u, &s, request, None).await.unwrap_err(), forbidden);
    }
    // And the port's own methods are covered by the conformance suite (`read_conformance`).
}
