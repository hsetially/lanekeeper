//! S4 (T6): the read routes are all guarded, at Viewer, and agree with `api/openapi.yaml`. The route-table harness of
//! `ports::conformance` is the one the platform's own routes pass; here it checks the fifteen operations of this crate, and a planted
//! violation proves the harness would say so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use domain::Role;
use hub_platform::{RouteGuard, RouteSpec};
use hub_registry::{RegistryApi, registry_route_specs, registry_routes};
use ports::conformance::route_guard::{
    Allowances, Guard, RegisteredRoute, SpecOperation, Violation, assert_route_table, check_route_table,
    spec_operations_from_openapi,
};

/// The fifteen read operations of the contract, by `operationId`, and the path each has.
const OPERATIONS: [(&str, &str); 15] = [
    ("listSwimlanes", "/api/v1/swimlanes"),
    ("getSwimlane", "/api/v1/swimlanes/{id}"),
    ("listSwimlaneTree", "/api/v1/swimlanes/{id}/tree"),
    ("listDrift", "/api/v1/swimlanes/{id}/drift"),
    ("getFileContent", "/api/v1/swimlanes/{id}/files/content"),
    ("listFileHistory", "/api/v1/swimlanes/{id}/files/history"),
    ("compareFile", "/api/v1/compare"),
    ("compareTree", "/api/v1/compare/tree"),
    ("getGrid", "/api/v1/grid"),
    ("searchSettings", "/api/v1/search/settings"),
    ("searchText", "/api/v1/search/text"),
    ("listFindings", "/api/v1/findings"),
    ("getEffectiveConfig", "/api/v1/swimlanes/{id}/effective"),
    ("listPendingRestarts", "/api/v1/swimlanes/{id}/pending-restarts"),
    ("getServedFile", "/api/v1/swimlanes/{id}/served"),
];

fn yaml() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml")).unwrap()
}

fn spec() -> Vec<SpecOperation> {
    spec_operations_from_openapi(&yaml()).unwrap()
}

fn registered(routes: &[RouteSpec]) -> Vec<RegisteredRoute> {
    routes.iter().map(RouteSpec::registered).collect()
}

/// Everything in the spec that is not one of `mine` belongs to another crate.
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
                "owned by another crate (03a, 05 writes, 06, 07 or 14)",
            );
        }
    }
    allow
}

#[test]
fn registry_route_table_is_guarded_and_matches_openapi() {
    let spec = spec();
    let mine = registry_route_specs();
    assert_route_table(&registered(&mine), &spec, &allowances(&spec, &mine));
}

#[test]
fn the_routes_are_exactly_the_fifteen_read_operations_of_the_contract() {
    let doc: serde_json::Value = serde_saphyr::from_str(&yaml()).unwrap();
    let mut by_id: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if let Some(id) = op["operationId"].as_str() {
                by_id.insert(id.to_owned(), (method.to_uppercase(), path.clone()));
            }
        }
    }
    let mut want: Vec<(String, String)> = Vec::new();
    for (id, path) in OPERATIONS {
        let found = by_id.get(id).unwrap_or_else(|| panic!("{id} is not in the spec"));
        assert_eq!(found.1, path, "{id}");
        assert_eq!(found.0, "GET", "{id} is a read");
        want.push(found.clone());
    }
    want.sort();
    let mut have: Vec<(String, String)> = registry_route_specs()
        .into_iter()
        .map(|r| (r.method, r.path))
        .collect();
    have.sort();
    assert_eq!(
        have, want,
        "the table is the fifteen operations, no more and no fewer"
    );
    // No read operation of the contract has any route other than viewer: the spec says so too.
    let spec = spec();
    for (_, path) in OPERATIONS {
        let op = spec.iter().find(|o| o.path == path && o.method == "GET").unwrap();
        assert_eq!(
            op.required,
            ports::conformance::route_guard::SpecRole::Role(Role::Viewer),
            "{path}"
        );
    }
}

#[tokio::test]
async fn what_is_mounted_is_what_the_table_declares() {
    let api: Arc<dyn RegistryApi> = Arc::new(support::http::service_without_a_database());
    let (_router, mounted) = registry_routes(support::http::guarded(), api)
        .into_parts()
        .unwrap();
    let mut mounted: Vec<_> = mounted;
    let mut declared = registry_route_specs();
    mounted.sort_by(|a, b| a.path.cmp(&b.path));
    declared.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(mounted, declared);
    assert!(
        mounted.iter().all(|r| r.guard == RouteGuard::Role(Role::Viewer)),
        "every mounted route is guarded at viewer; there is no way to mount one without a guard"
    );
    // The registry adds nothing next to another module's routes: the two tables merge without a duplicate.
    let platform = hub_platform::platform_routes();
    for p in &platform {
        assert!(
            !mounted.iter().any(|m| m.method == p.method && m.path == p.path),
            "{}",
            p.path
        );
    }
}

#[test]
fn the_harness_rejects_an_unguarded_or_wrongly_guarded_registry_route() {
    let spec = spec();
    let mine = registry_route_specs();
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

    // A read route guarded above the spec's role is a mismatch, and so is one that is public.
    let mut strict = registry_route_specs();
    strict[0].guard = RouteGuard::Role(Role::Admin);
    let violations = check_route_table(&registered(&strict), &spec, &allow);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::RoleMismatch { .. })),
        "{violations:?}"
    );
    let mut open = registry_route_specs();
    open[1].guard = RouteGuard::Public {
        reason: "oops".to_owned(),
    };
    let violations = check_route_table(&registered(&open), &spec, &allow);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::PublicityMismatch { .. })),
        "{violations:?}"
    );

    // A dropped route is found as one the spec has and nothing registers.
    let mut short = registry_route_specs();
    short.pop();
    let violations = check_route_table(&registered(&short), &spec, &Allowances::default());
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::NotRegistered { .. })),
        "{violations:?}"
    );

    // A route the spec does not have.
    let mut extra = registry_route_specs();
    extra.push(RouteSpec::new(
        &http::Method::GET,
        "/api/v1/not-in-the-spec",
        RouteGuard::Role(Role::Viewer),
    ));
    let violations = check_route_table(&registered(&extra), &spec, &allow);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::NotInSpec { .. })),
        "{violations:?}"
    );
}

#[test]
fn a_route_cannot_be_mounted_without_a_guard_by_construction() {
    // `GuardedRouter` has no method that takes a handler without a role or a reason, so the only way to register a route is a guarded one;
    // this module registers its routes with nothing else.
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http/routes.rs")).unwrap();
    let production = source.split("#[cfg(test)]").next().unwrap();
    assert!(
        !production.contains("axum::routing"),
        "routes go through GuardedRouter only"
    );
    assert!(
        !production.contains("Router::new"),
        "routes go through GuardedRouter only"
    );
    assert!(!production.contains(".public("), "no read route is public");
    assert_eq!(
        production.matches("mount!(router").count(),
        15,
        "one mount per operation"
    );
}
