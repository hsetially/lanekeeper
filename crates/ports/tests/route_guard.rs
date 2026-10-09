//! The S4 route-table harness that 03a, 05 and 07 call against `api/openapi.yaml`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{Role, UserStatus};
use ports::conformance::route_guard::{
    Allowances, Guard, RegisteredRoute, SpecOperation, SpecRole, Violation, assert_route_table,
    assert_user_gate, check_route_table, spec_operations_from_openapi,
};

const SPEC: &str = r"
openapi: 3.1.0
info: {title: t, version: '1'}
paths:
  /api/v1/swimlanes/{id}:
    parameters:
      - {name: id, in: path, required: true, schema: {type: string}}
    get:
      operationId: getSwimlane
      x-required-role: viewer
    delete:
      operationId: deleteSwimlane
      x-required-role: admin
  /api/v1/swimlanes/{id}/restart:
    post:
      operationId: restart
      x-required-role: operator
  /auth/logout:
    post:
      operationId: logout
      x-required-role: none
";

fn spec() -> Vec<SpecOperation> {
    spec_operations_from_openapi(SPEC).unwrap()
}

fn good_routes() -> Vec<RegisteredRoute> {
    vec![
        RegisteredRoute::new("GET", "/api/v1/swimlanes/:id", Guard::Role(Role::Viewer)),
        RegisteredRoute::new("DELETE", "/api/v1/swimlanes/{sid}", Guard::Role(Role::Admin)),
        RegisteredRoute::new(
            "POST",
            "/api/v1/swimlanes/{id}/restart",
            Guard::Role(Role::Operator),
        ),
        RegisteredRoute::new(
            "POST",
            "/auth/logout",
            Guard::Public {
                reason: "ends a session; needs none".into(),
            },
        ),
    ]
}

#[test]
fn spec_reader_finds_every_operation_and_its_role() {
    let ops = spec();
    assert_eq!(ops.len(), 4, "parameters blocks are not operations");
    let find = |m: &str, p: &str| {
        ops.iter()
            .find(|o| o.method == m && o.path == p)
            .unwrap()
            .required
    };
    assert_eq!(
        find("GET", "/api/v1/swimlanes/{id}"),
        SpecRole::Role(Role::Viewer)
    );
    assert_eq!(
        find("DELETE", "/api/v1/swimlanes/{id}"),
        SpecRole::Role(Role::Admin)
    );
    assert_eq!(find("POST", "/auth/logout"), SpecRole::None);
}

#[test]
fn operation_without_x_required_role_is_an_error() {
    let yaml = "paths:\n  /x:\n    get:\n      operationId: x\n";
    let e = spec_operations_from_openapi(yaml).unwrap_err();
    assert!(e.0.contains("GET /x"), "{e}");
    let yaml = "paths:\n  /x:\n    get:\n      x-required-role: superuser\n";
    assert!(spec_operations_from_openapi(yaml).is_err(), "unknown role name");
    assert!(spec_operations_from_openapi("paths: [").is_err(), "not YAML");
}

#[test]
fn all_guarded_and_matching_passes() {
    assert_route_table(&good_routes(), &spec(), &Allowances::default());
}

#[test]
fn unguarded_route_fails() {
    let mut routes = good_routes();
    routes[2].guard = Guard::Unguarded;
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert_eq!(
        v,
        [Violation::Unguarded {
            method: "POST".into(),
            path: "/api/v1/swimlanes/{id}/restart".into()
        }]
    );
}

#[test]
#[should_panic(expected = "has no guard")]
fn assert_route_table_panics_with_the_violations() {
    let routes = vec![RegisteredRoute::new(
        "GET",
        "/api/v1/swimlanes/{id}",
        Guard::Unguarded,
    )];
    assert_route_table(&routes, &spec(), &Allowances::default());
}

#[test]
fn role_mismatch_with_openapi_fails() {
    let mut routes = good_routes();
    routes[1].guard = Guard::Role(Role::Viewer);
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert_eq!(
        v,
        [Violation::RoleMismatch {
            method: "DELETE".into(),
            path: "/api/v1/swimlanes/{sid}".into(),
            registered: Role::Viewer,
            spec: Role::Admin,
        }]
    );
}

#[test]
fn public_guard_and_none_role_must_agree() {
    // A route that drops its guard but the spec still names a role.
    let mut routes = good_routes();
    routes[0].guard = Guard::Public {
        reason: "oops".into(),
    };
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert!(
        matches!(v.as_slice(), [Violation::PublicityMismatch { .. }]),
        "{v:?}"
    );
    // A guarded route whose spec says none.
    let mut routes = good_routes();
    routes[3].guard = Guard::Role(Role::Viewer);
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert!(
        matches!(v.as_slice(), [Violation::PublicityMismatch { .. }]),
        "{v:?}"
    );
    // Public needs a reason.
    let mut routes = good_routes();
    routes[3].guard = Guard::Public { reason: "  ".into() };
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert!(
        matches!(v.as_slice(), [Violation::PublicWithoutReason { .. }]),
        "{v:?}"
    );
}

#[test]
fn spec_operation_without_route_fails_unless_allowed() {
    let routes: Vec<_> = good_routes().into_iter().take(3).collect();
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert_eq!(
        v,
        [Violation::NotRegistered {
            method: "POST".into(),
            path: "/auth/logout".into()
        }]
    );

    let allow =
        Allowances::default().spec_only("post", "/auth/logout", "served by hub-identity, composed in 03b");
    assert!(check_route_table(&routes, &spec(), &allow).is_empty());

    let no_reason = Allowances::default().spec_only("POST", "/auth/logout", " ");
    let v = check_route_table(&routes, &spec(), &no_reason);
    assert!(
        matches!(v.as_slice(), [Violation::AllowanceWithoutReason { .. }]),
        "{v:?}"
    );
}

#[test]
fn route_missing_from_spec_fails_unless_allowed() {
    let mut routes = good_routes();
    routes.push(RegisteredRoute::new("GET", "/healthz", Guard::Role(Role::Viewer)));
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert_eq!(
        v,
        [Violation::NotInSpec {
            method: "GET".into(),
            path: "/healthz".into()
        }]
    );

    let allow = Allowances::default().route_only("GET", "/healthz", "liveness probe");
    assert!(check_route_table(&routes, &spec(), &allow).is_empty());
}

#[test]
fn registering_a_route_twice_is_reported() {
    let mut routes = good_routes();
    routes.push(routes[0].clone());
    let v = check_route_table(&routes, &spec(), &Allowances::default());
    assert!(
        matches!(v.as_slice(), [Violation::DuplicateRoute { .. }]),
        "{v:?}"
    );
}

#[test]
fn pending_and_disabled_users_are_denied_before_handler() {
    // The gate the extractors use: a real one passes the harness...
    assert_user_gate(domain::User::allows);
}

#[test]
#[should_panic(expected = "Pending")]
fn a_gate_that_lets_pending_users_through_fails_the_harness() {
    assert_user_gate(|u, min| u.role.is_some_and(|r| r >= min));
}

#[test]
#[should_panic(expected = "must be denied")]
fn a_gate_that_ignores_disabled_status_fails_the_harness() {
    assert_user_gate(|u, min| u.status != UserStatus::Pending && u.role.is_some_and(|r| r >= min));
}

#[test]
#[should_panic(expected = "active")]
fn a_gate_that_ignores_the_role_floor_fails_the_harness() {
    assert_user_gate(|u, _| u.status == UserStatus::Active && u.role.is_some());
}
