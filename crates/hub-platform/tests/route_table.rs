//! S4 (T1): the platform's routes are all guarded and agree with `api/openapi.yaml`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

use std::collections::BTreeSet;
use std::path::Path;

use hub_platform::{RouteGuard, RouteSpec, ops_routes, platform_routes};
use ports::conformance::route_guard::{
    Allowances, Guard, RegisteredRoute, SpecOperation, Violation, assert_route_table, check_route_table,
    spec_operations_from_openapi,
};

fn spec() -> Vec<SpecOperation> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml");
    spec_operations_from_openapi(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn registered(routes: &[RouteSpec]) -> Vec<RegisteredRoute> {
    routes.iter().map(RouteSpec::registered).collect()
}

/// Everything in the spec that is not one of `mine` belongs to another crate; the operations listener's routes are the
/// only registered routes that are not in the spec.
fn allowances(spec: &[SpecOperation], mine: &[RouteSpec]) -> Allowances {
    let mine: BTreeSet<(&str, &str)> = mine
        .iter()
        .map(|r| (r.method.as_str(), r.path.as_str()))
        .collect();
    let mut allow = Allowances::default();
    for op in spec {
        if !mine.contains(&(op.method.as_str(), op.path.as_str())) {
            allow = allow.spec_only(
                &op.method,
                &op.path,
                "owned by another crate (03a, 05, 06, 07 or 14)",
            );
        }
    }
    for route in ops_routes() {
        allow = allow.route_only(
            &route.method,
            &route.path,
            "internal listener only; not an API operation",
        );
    }
    allow
}

#[test]
fn platform_route_table_is_guarded_and_matches_openapi() {
    let spec = spec();
    let mut routes = platform_routes();
    let mine = routes.clone();
    routes.extend(ops_routes());
    assert_route_table(&registered(&routes), &spec, &allowances(&spec, &mine));
}

#[test]
fn the_platform_declares_exactly_the_three_routes_it_owns() {
    let routes: Vec<(String, String)> = platform_routes()
        .into_iter()
        .map(|r| (r.method, r.path))
        .collect();
    assert_eq!(
        routes,
        vec![
            ("GET".to_owned(), "/api/v1/events".to_owned()),
            ("GET".to_owned(), "/api/v1/blobs/{hash}".to_owned()),
            ("POST".to_owned(), "/hooks/github".to_owned()),
        ]
    );
}

#[test]
fn the_harness_rejects_a_platform_route_that_is_unguarded_or_guarded_too_low() {
    let spec = spec();
    let mine = platform_routes();
    let allow = allowances(&spec, &mine);

    // A planted unguarded route.
    let mut routes = registered(&mine);
    routes.push(RegisteredRoute::new("GET", "/api/v1/planted", Guard::Unguarded));
    let violations = check_route_table(&routes, &spec, &allow);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::Unguarded { .. })),
        "{violations:?}"
    );

    // The blob route guarded below the spec's role.
    let mut weak = platform_routes();
    for route in &mut weak {
        if route.path.starts_with("/api/v1/blobs") {
            route.guard = RouteGuard::Public {
                reason: "oops".to_owned(),
            };
        }
    }
    let violations = check_route_table(&registered(&weak), &spec, &allow);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::PublicityMismatch { .. })),
        "{violations:?}"
    );

    // The webhook made to need a session while the spec says none.
    let mut strict = platform_routes();
    for route in &mut strict {
        if route.path == "/hooks/github" {
            route.guard = RouteGuard::Role(domain::Role::Admin);
        }
    }
    let violations = check_route_table(&registered(&strict), &spec, &allow);
    assert!(!violations.is_empty());
}
