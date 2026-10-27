//! The whole route table of the composed hub against `api/openapi.yaml` (plan 18, T8; S4, S12; AGENTS.md rule 9).
//!
//! The hub is started in the test's process by `hub::process::start`, the code the binary runs, over a real Postgres, so the table
//! is `Running::routes()`: every route of every module (identity, the registry, the write paths, the platform, the MCP server) with
//! the guard it was registered with. What is proved:
//!
//! - **Every one of the 99 operations is mounted with the guard the document names, or it is in the allowance list with a reason and
//!   an owner** (`hub::allowances::SPEC_ONLY`), and every mounted route is an operation or is in the other list with a reason.
//! - **A new route or a new operation that nobody listed fails**, and so does an allowance that no longer matches the table (an
//!   operation mounted since, a route that went away), so the lists shrink as owners mount their routes and cannot go stale.
//! - **The composed table is a golden**: a person reads the diff when a route, a role or an owner changes.
//! - **Every route answers with the security headers of S12**, even when it refuses the request.
//!
//! `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing Docker daemon a failure and not a skip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

#[path = "support/browser.rs"]
mod browser;
#[path = "support/child.rs"]
mod child;
#[path = "../examples/support/dev_edges.rs"]
mod dev_edges;
#[path = "support/full.rs"]
mod full;
#[path = "support/git_remote.rs"]
mod git_remote;
#[path = "support/net.rs"]
mod net;
#[path = "support/rig.rs"]
mod rig;
#[path = "../examples/support/soft_kms.rs"]
mod soft_kms;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use full::{Full, concrete};
use hub::allowances::{ROUTE_ONLY, SPEC_ONLY, UNASSIGNED};
use hub_platform::RouteSpec;
use ports::conformance::route_guard::{
    Allowances, Guard, RegisteredRoute, SpecOperation, SpecRole, Violation, assert_route_table,
    check_route_table, spec_operations_from_openapi,
};

fn spec() -> Vec<SpecOperation> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml");
    spec_operations_from_openapi(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn allowances() -> Allowances {
    let mut allow = Allowances::default();
    for entry in SPEC_ONLY {
        allow = allow.spec_only(entry.method, entry.path, entry.reason);
    }
    for entry in ROUTE_ONLY {
        allow = allow.route_only(entry.method, entry.path, entry.reason);
    }
    allow
}

fn registered(routes: &[RouteSpec]) -> Vec<RegisteredRoute> {
    routes.iter().map(RouteSpec::registered).collect()
}

fn key(method: &str, path: &str) -> (String, String) {
    (method.to_uppercase(), path.to_owned())
}

// ---------------------------------------------------------------------------------------------- the lists, without a hub

#[test]
fn the_document_has_ninety_nine_operations() {
    // The number the prompt and the plan are written against. A new operation is a contract change and changes it on purpose.
    assert_eq!(spec().len(), 99);
}

/// `06`, `14`, `03b`: two digits and, for a prompt that was split, a letter.
fn is_prompt_number(owner: &str) -> bool {
    let bytes = owner.as_bytes();
    matches!(bytes.len(), 2 | 3)
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes.get(2).is_none_or(u8::is_ascii_lowercase)
}

#[test]
fn every_allowance_names_a_reason_and_an_owner() {
    for entry in SPEC_ONLY {
        assert!(
            entry.reason.trim().len() >= 20,
            "{} {} has no real reason",
            entry.method,
            entry.path
        );
        assert!(
            !entry.owner.trim().is_empty(),
            "{} {} has no owner",
            entry.method,
            entry.path
        );
        // An owner is a prompt number or `unassigned`; nothing else is an owner.
        assert!(
            entry.owner == UNASSIGNED || is_prompt_number(entry.owner),
            "{} {}: owner {:?} is neither a prompt number nor `unassigned`",
            entry.method,
            entry.path,
            entry.owner
        );
    }
    for entry in ROUTE_ONLY {
        assert!(
            entry.reason.trim().len() >= 20,
            "{} {} has no real reason",
            entry.method,
            entry.path
        );
    }
}

#[test]
fn the_lists_name_real_operations_once_each() {
    let spec_keys: BTreeSet<_> = spec().iter().map(|o| key(&o.method, &o.path)).collect();
    let mut seen = BTreeSet::new();
    for entry in SPEC_ONLY {
        let k = key(entry.method, entry.path);
        assert!(
            spec_keys.contains(&k),
            "{} {} is allowed as spec-only and is not an operation of api/openapi.yaml",
            entry.method,
            entry.path
        );
        assert!(seen.insert(k), "{} {} is listed twice", entry.method, entry.path);
    }
    let mut routes = BTreeSet::new();
    for entry in ROUTE_ONLY {
        let k = key(entry.method, entry.path);
        assert!(
            !spec_keys.contains(&k),
            "{} {} is allowed as route-only and is an operation of api/openapi.yaml",
            entry.method,
            entry.path
        );
        assert!(
            routes.insert(k),
            "{} {} is listed twice",
            entry.method,
            entry.path
        );
    }
}

// --------------------------------------------------------------------------------------------------- the composed table

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_operation_is_mounted_or_allowed_with_a_reason_and_an_owner() {
    let Some(full) = Full::start().await else { return };
    let spec = spec();
    let table = registered(full.running.routes());

    // The check of S4: no unguarded route, the guard is the document's `x-required-role`, a public route has a reason, and the two
    // sides differ only by what the lists allow.
    assert_route_table(&table, &spec, &allowances());

    // The lists match the table exactly: nothing allowed is mounted, and every route-only entry is.
    let mounted: BTreeSet<_> = table.iter().map(|r| key(&r.method, &r.path)).collect();
    for entry in SPEC_ONLY {
        assert!(
            !mounted.contains(&key(entry.method, entry.path)),
            "{} {} is mounted now: delete its line from hub::allowances::SPEC_ONLY (owner {})",
            entry.method,
            entry.path,
            entry.owner
        );
    }
    for entry in ROUTE_ONLY {
        assert!(
            mounted.contains(&key(entry.method, entry.path)),
            "{} {} is allowed as route-only and is not mounted: delete its line from hub::allowances::ROUTE_ONLY",
            entry.method,
            entry.path
        );
    }

    // The arithmetic: every operation is mounted or allowed, and nothing is both.
    let mounted_operations = spec
        .iter()
        .filter(|o| mounted.contains(&key(&o.method, &o.path)))
        .count();
    assert_eq!(
        mounted_operations + SPEC_ONLY.len(),
        spec.len(),
        "{mounted_operations} operations are mounted and {} are allowed, of {}",
        SPEC_ONLY.len(),
        spec.len()
    );
    assert_eq!(table.len(), mounted_operations + ROUTE_ONLY.len());

    // The operations listener's three routes are not on the web listener: its table does not list them (above), and the web listener
    // does not answer them either (the web app's fallback answers the shell, which is not their output).
    for path in ["/healthz", "/readyz", "/metrics"] {
        let reply = full.web("GET", path, &[], None).await;
        assert!(
            !reply.body.starts_with("ok")
                && !reply.body.starts_with("ready")
                && !reply.body.starts_with("not ready")
                && !reply.body.contains("# TYPE")
                && !reply.body.contains("# HELP"),
            "the web listener answers {path} with something of the operations listener: {}",
            reply.body
        );
    }

    // No route is public unless the document says `none` for it: the table's public routes are the document's.
    for route in &table {
        if matches!(route.guard, Guard::Public { .. }) {
            let op = spec
                .iter()
                .find(|o| key(&o.method, &o.path) == key(&route.method, &route.path));
            if let Some(op) = op {
                assert_eq!(op.required, SpecRole::None, "{} {}", route.method, route.path);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_unlisted_route_or_operation_fails() {
    let Some(full) = Full::start().await else { return };
    let spec = spec();
    let table = registered(full.running.routes());
    let allow = allowances();
    assert!(
        check_route_table(&table, &spec, &allow).is_empty(),
        "the real table must be clean before it is spoiled"
    );

    // A route that nobody wrote in the document or in the list.
    let mut planted = table.clone();
    planted.push(RegisteredRoute::new(
        "POST",
        "/api/v1/planted",
        Guard::Role(domain::Role::Viewer),
    ));
    assert!(
        check_route_table(&planted, &spec, &allow).contains(&Violation::NotInSpec {
            method: "POST".to_owned(),
            path: "/api/v1/planted".to_owned()
        })
    );

    // A route with no guard.
    let mut unguarded = table.clone();
    unguarded.push(RegisteredRoute::new("GET", "/api/v1/me", Guard::Unguarded));
    assert!(
        check_route_table(&unguarded, &spec, &allow)
            .iter()
            .any(|v| matches!(v, Violation::Unguarded { .. }))
    );

    // A mounted route that is dropped, and a mounted route whose role was lowered.
    let (dropped, kept) = table.split_first().unwrap();
    assert!(
        check_route_table(kept, &spec, &allow)
            .iter()
            .any(|v| matches!(v, Violation::NotRegistered { .. } | Violation::NotInSpec { .. })),
        "dropping {} {} must be seen",
        dropped.method,
        dropped.path
    );
    let mut lowered = table.clone();
    if let Some(route) = lowered
        .iter_mut()
        .find(|r| matches!(r.guard, Guard::Role(domain::Role::Admin)))
    {
        route.guard = Guard::Role(domain::Role::Viewer);
        assert!(
            check_route_table(&lowered, &spec, &allow)
                .iter()
                .any(|v| matches!(v, Violation::RoleMismatch { .. }))
        );
    }

    // A new operation of the document that nobody mounted and nobody allowed.
    let mut grown = spec.clone();
    grown.push(SpecOperation {
        method: "GET".to_owned(),
        path: "/api/v1/new-operation".to_owned(),
        required: SpecRole::Role(domain::Role::Viewer),
    });
    assert!(
        check_route_table(&table, &grown, &allow).contains(&Violation::NotRegistered {
            method: "GET".to_owned(),
            path: "/api/v1/new-operation".to_owned()
        })
    );

    // An allowance taken away: the operation it covered is unmounted and unlisted.
    if let Some(first) = SPEC_ONLY.first() {
        let mut fewer = Allowances::default();
        for entry in SPEC_ONLY.iter().skip(1) {
            fewer = fewer.spec_only(entry.method, entry.path, entry.reason);
        }
        for entry in ROUTE_ONLY {
            fewer = fewer.route_only(entry.method, entry.path, entry.reason);
        }
        assert!(
            check_route_table(&table, &spec, &fewer).contains(&Violation::NotRegistered {
                method: first.method.to_owned(),
                path: first.path.to_owned()
            })
        );
    }

    // An allowance with no reason is itself a violation.
    let reasonless = allow.clone().spec_only("GET", "/api/v1/whatever", " ");
    assert!(
        check_route_table(&table, &spec, &reasonless)
            .iter()
            .any(|v| matches!(v, Violation::AllowanceWithoutReason { .. }))
    );
}

fn role_word(guard: &Guard) -> String {
    match guard {
        Guard::Role(role) => format!("role {role}"),
        Guard::Public { reason } => format!("public ({reason})"),
        Guard::Unguarded => "UNGUARDED".to_owned(),
    }
}

/// The golden: the table as a person reads it.
fn golden(table: &[RegisteredRoute]) -> String {
    let mut rows: Vec<String> = table
        .iter()
        .map(|r| format!("{:<7} {}  [{}]", r.method, r.path, role_word(&r.guard)))
        .collect();
    rows.sort();
    let mut text = format!("# mounted on the web listener: {} routes\n", rows.len());
    for row in &rows {
        let _ = writeln!(text, "{row}");
    }
    let mut allowed: Vec<String> = SPEC_ONLY
        .iter()
        .map(|e| format!("{:<7} {}  [owner {}] {}", e.method, e.path, e.owner, e.reason))
        .collect();
    allowed.sort();
    let _ = writeln!(
        text,
        "\n# operations of api/openapi.yaml that are not mounted: {}",
        allowed.len()
    );
    for row in &allowed {
        let _ = writeln!(text, "{row}");
    }
    let mut extra: Vec<String> = ROUTE_ONLY
        .iter()
        .map(|e| format!("{:<7} {}  {}", e.method, e.path, e.reason))
        .collect();
    extra.sort();
    let _ = writeln!(
        text,
        "\n# routes that are not operations of the document: {}",
        extra.len()
    );
    for row in &extra {
        let _ = writeln!(text, "{row}");
    }
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_composed_table_matches_the_golden() {
    let Some(full) = Full::start().await else { return };
    insta::assert_snapshot!("composed_route_table", golden(&registered(full.running.routes())));
}

/// Every response, a refusal included, carries these (S12; `tests/headers.rs` proves their values class by class).
const ALWAYS: [&str; 7] = [
    "strict-transport-security",
    "x-content-type-options",
    "referrer-policy",
    "cross-origin-opener-policy",
    "cross-origin-resource-policy",
    "content-security-policy",
    "permissions-policy",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_composed_route_carries_the_security_headers() {
    let Some(full) = Full::start().await else { return };
    let mut checked = 0;
    for route in full.running.routes() {
        let path = concrete(&route.path);
        let body: Option<&[u8]> = (route.method != "GET").then_some(b"{}");
        let headers: &[(&str, &str)] = if body.is_some() {
            &[("Content-Type", "application/json")]
        } else {
            &[]
        };
        let reply = full.web(&route.method, &path, headers, body).await;
        assert_ne!(reply.status, 0, "{} {}: no answer", route.method, route.path);
        for name in ALWAYS {
            assert!(
                reply.header(name).is_some(),
                "{} {} (status {}) has no {name}: {:?}",
                route.method,
                route.path,
                reply.status,
                reply.headers
            );
        }
        assert!(
            reply.header("x-request-id").is_some_and(|id| id.len() == 16),
            "{} {}: x-request-id",
            route.method,
            route.path
        );
        // Not one of them is reachable without a session or a credential of its own: nothing answers 2xx to an anonymous request
        // except the routes that are public by design (the webhook and discovery documents answer their own refusals).
        if matches!(route.guard, hub_platform::RouteGuard::Role(_)) {
            assert!(
                matches!(reply.status, 401 | 403),
                "{} {} answered {} to a request with no session",
                route.method,
                route.path,
                reply.status
            );
        }
        checked += 1;
    }
    assert_eq!(checked, full.running.routes().len());
    assert!(checked >= 49, "only {checked} routes were checked");
}

// ------------------------------------------------------------------------------------------------------------------ CSRF, the real routes

/// What kind of problem a `problem+json` body is: its `code`, or the last segment of its `type` (`urn:lanekeeper:problem:csrf`).
fn problem_code(reply: &net::Reply) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(&reply.body).ok()?;
    body["code"]
        .as_str()
        .or_else(|| body["type"].as_str().and_then(|t| t.rsplit(':').next()))
        .map(str::to_owned)
}

/// The CSRF walk of `csrf_all_routes.rs` over the table that this test file boots: `Running::routes()` is the identity module's
/// routes, the registry's reads, the write paths, the webhook, the blob endpoint and the MCP server, not the probe module of the
/// identity rig. Every state-changing route refuses a request that carries a live session and lacks, or gets wrong, the double-submit
/// token or the exact `Origin`, and it is `CsrfLayer` that refuses it, before the route's handler. The route list is read from the
/// table, so a route added tomorrow is walked the day it is added.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_real_state_changing_route_refuses_a_session_without_the_token_and_the_origin() {
    let Some(full) = Full::start().await else { return };
    let (_, admin) = full
        .signed_in(1, Some(domain::Role::Admin), domain::UserStatus::Active)
        .await;
    let (_, other) = full
        .signed_in(2, Some(domain::Role::Viewer), domain::UserStatus::Active)
        .await;
    let routes: Vec<&RouteSpec> = full
        .running
        .routes()
        .iter()
        .filter(|r| !matches!(r.method.as_str(), "GET" | "HEAD" | "OPTIONS"))
        .collect();
    // A walk over an empty or a probe-only table would pass, so the real routes of each module are named.
    assert!(routes.len() >= 24, "{} state-changing routes", routes.len());
    for (method, path) in [
        ("PATCH", "/api/v1/admin/users/{id}"),
        ("POST", "/auth/logout"),
        ("PUT", "/api/v1/swimlanes/{id}/files"),
        ("POST", "/api/v1/prs"),
        ("PUT", "/api/v1/me/github-token"),
        ("POST", "/api/v1/swimlanes/{id}/restart"),
        ("POST", "/api/v1/me/mcp-tokens"),
        ("POST", "/hooks/github"),
        ("POST", "/mcp"),
    ] {
        assert!(
            routes.iter().any(|r| r.method == method && r.path == path),
            "{method} {path} is not a state-changing route of the real table"
        );
    }

    let own = admin.header();
    let foreign_token = other.csrf.clone();
    for route in routes {
        let name = format!("{} {}", route.method, route.path);
        let path = concrete(&route.path);
        // Each variant lacks, or gets wrong, one thing the check needs. All of them carry a live admin session.
        let variants: Vec<(&str, Vec<(&str, String)>)> = vec![
            ("the cookies only", vec![("Cookie", own.clone())]),
            (
                "a token and no Origin",
                vec![("Cookie", own.clone()), ("X-CSRF-Token", admin.csrf.clone())],
            ),
            (
                "an Origin and no token",
                vec![("Cookie", own.clone()), ("Origin", browser::ORIGIN.to_owned())],
            ),
            (
                "a foreign Origin",
                vec![
                    ("Cookie", own.clone()),
                    ("X-CSRF-Token", admin.csrf.clone()),
                    ("Origin", "https://evil.example".to_owned()),
                ],
            ),
            (
                "a token that is not the cookie's",
                vec![
                    ("Cookie", own.clone()),
                    ("X-CSRF-Token", foreign_token.clone()),
                    ("Origin", browser::ORIGIN.to_owned()),
                ],
            ),
        ];
        for (what, headers) in variants {
            let mut all: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
            all.push(("Content-Type", "application/json"));
            let reply = full.web(&route.method, &path, &all, Some(b"{}")).await;
            assert_eq!(reply.status, 403, "{name} with {what}: {}", reply.body);
            assert_eq!(
                problem_code(&reply).as_deref(),
                Some("csrf"),
                "{name} with {what} was refused, but not by CSRF: {}",
                reply.body
            );
        }
    }

    // And the layer is not a blanket refusal: the same session with the token and the Origin gets past it, on the one route that
    // changes nothing but the session itself.
    let (_, signing_out) = full
        .signed_in(3, Some(domain::Role::Viewer), domain::UserStatus::Active)
        .await;
    let reply = full
        .web_as(&signing_out, "POST", "/auth/logout", Some(&serde_json::json!({})))
        .await;
    assert_ne!(problem_code(&reply).as_deref(), Some("csrf"), "{}", reply.body);
    assert!(reply.status < 400, "{}: {}", reply.status, reply.body);
}
