//! Conventions of `api/openapi.yaml` that the linter cannot express: every operation names its role (S4), writes
//! carry the CSRF header and an idempotency key (S2, S11), lists page by keyset, content endpoints are cacheable
//! by hash, and no agent-facing join-token endpoint exists (S5).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use domain::Role;
use ports::conformance::route_guard::{
    Allowances, Guard, RegisteredRoute, SpecRole, check_route_table, spec_operations_from_openapi,
};
use serde_json::Value;

const SPEC: &str = include_str!("../../../api/openapi.yaml");

/// Operations outside `/api/v1`, each with the reason it is open. Every other operation needs a session.
const PUBLIC: &[(&str, &str, &str)] = &[
    (
        "GET",
        "/auth/login",
        "starts the Entra sign-in; there is no session yet",
    ),
    (
        "GET",
        "/auth/callback",
        "finishes the Entra sign-in; checks state and nonce itself",
    ),
    (
        "POST",
        "/auth/logout",
        "ends a session; with none it does nothing",
    ),
    (
        "POST",
        "/hooks/github",
        "no session; verified by HMAC and delivery id",
    ),
];

/// Writes that cannot carry a CSRF token (no browser session), with the reason.
const NO_CSRF: &[(&str, &str)] = &[("POST", "/hooks/github")];

/// Writes that need no idempotency key, with the reason: the hook has GitHub's delivery id, logout repeats safely.
const NO_IDEMPOTENCY_KEY: &[(&str, &str)] = &[("POST", "/hooks/github"), ("POST", "/auth/logout")];

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

/// Follow `$ref` ("#/a/b/c") until a non-reference value.
fn resolve<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..16 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            return cur;
        };
        let pointer = r.strip_prefix('#').unwrap_or_else(|| panic!("external $ref {r}"));
        cur = doc
            .pointer(pointer)
            .unwrap_or_else(|| panic!("dangling $ref {r}"));
    }
    panic!("$ref chain too deep");
}

struct Op<'a> {
    method: String,
    path: String,
    item: &'a Value,
    def: &'a Value,
}

impl Op<'_> {
    fn label(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    fn is_write(&self) -> bool {
        !matches!(self.method.as_str(), "GET" | "HEAD" | "OPTIONS")
    }

    /// Parameters of the path item and the operation, references resolved.
    fn params<'a>(&'a self, doc: &'a Value) -> Vec<&'a Value> {
        let mut out = Vec::new();
        for holder in [self.item, self.def] {
            if let Some(list) = holder.get("parameters").and_then(Value::as_array) {
                out.extend(list.iter().map(|p| resolve(doc, p)));
            }
        }
        out
    }

    fn header_param<'a>(&'a self, doc: &'a Value, name: &str) -> Option<&'a Value> {
        self.params(doc)
            .into_iter()
            .find(|p| p["in"] == "header" && p["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }

    fn query_param<'a>(&'a self, doc: &'a Value, name: &str) -> Option<&'a Value> {
        self.params(doc)
            .into_iter()
            .find(|p| p["in"] == "query" && p["name"] == name)
    }

    fn response<'a>(&'a self, doc: &'a Value, code: &str) -> Option<&'a Value> {
        self.def["responses"].get(code).map(|r| resolve(doc, r))
    }

    fn has_tag(&self, tag: &str) -> bool {
        self.def["tags"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x == tag))
    }
}

const METHODS: [&str; 5] = ["get", "put", "post", "delete", "patch"];

fn operations(doc: &Value) -> Vec<Op<'_>> {
    let mut out = Vec::new();
    for (path, item) in doc["paths"].as_object().expect("paths") {
        for m in METHODS {
            if let Some(op) = item.get(m) {
                out.push(Op {
                    method: m.to_uppercase(),
                    path: path.clone(),
                    item,
                    def: op,
                });
            }
        }
    }
    out
}

fn find<'a>(ops: &'a [Op<'a>], method: &str, path: &str) -> &'a Op<'a> {
    ops.iter()
        .find(|o| o.method == method && o.path == path)
        .unwrap_or_else(|| panic!("{method} {path} is not in the spec"))
}

/// The properties of an object schema, references resolved one level.
fn properties<'a>(doc: &'a Value, schema: &'a Value) -> BTreeMap<&'a str, &'a Value> {
    let s = resolve(doc, schema);
    let mut out = BTreeMap::new();
    if let Some(p) = s.get("properties").and_then(Value::as_object) {
        out.extend(p.iter().map(|(k, v)| (k.as_str(), v)));
    }
    for part in s.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        out.extend(properties(doc, part));
    }
    out
}

fn json_schema(response: &Value) -> Option<&Value> {
    response["content"]["application/json"].get("schema")
}

#[test]
fn every_operation_has_valid_x_required_role() {
    // Reads every operation; fails on a missing or unknown `x-required-role`.
    let ops = spec_operations_from_openapi(SPEC).unwrap();
    assert!(
        ops.len() >= 80,
        "the whole surface is in the spec, found {}",
        ops.len()
    );

    let d = doc();
    let all = operations(&d);
    assert_eq!(
        ops.len(),
        all.len(),
        "the reader and this test see the same operations"
    );

    let mut ids = BTreeSet::new();
    for o in &all {
        let id = o.def["operationId"]
            .as_str()
            .unwrap_or_else(|| panic!("{} has no operationId", o.label()));
        assert!(ids.insert(id), "operationId {id} is used twice");
        assert!(
            o.def["summary"].as_str().is_some_and(|s| !s.is_empty()),
            "{} has no summary",
            o.label()
        );
    }

    let public: BTreeSet<(String, String)> = PUBLIC
        .iter()
        .map(|(m, p, reason)| {
            assert!(!reason.is_empty());
            ((*m).to_owned(), (*p).to_owned())
        })
        .collect();
    for o in &ops {
        let is_public = public.contains(&(o.method.clone(), o.path.clone()));
        assert_eq!(
            o.required == SpecRole::None,
            is_public,
            "{} {}: x-required-role none is allowed exactly for the listed public operations",
            o.method,
            o.path
        );
        let outside_api = !o.path.starts_with("/api/v1/");
        assert_eq!(
            outside_api, is_public,
            "{} {}: only the public operations live outside /api/v1",
            o.method, o.path
        );
    }
    // Public operations opt out of the cookie scheme explicitly; all others inherit it.
    for o in &all {
        let is_public = public.contains(&(o.method.clone(), o.path.clone()));
        let own = o.def.get("security");
        if is_public {
            assert_eq!(
                own,
                Some(&Value::Array(vec![])),
                "{} must set security: []",
                o.label()
            );
        } else {
            assert!(own.is_none(), "{} must inherit the session security", o.label());
        }
    }
    assert_eq!(d["security"][0]["sessionCookie"], Value::Array(vec![]));
    assert_eq!(
        d["components"]["securitySchemes"]["sessionCookie"]["in"],
        "cookie"
    );
}

/// The roles Appendix A of plan 01 gives, which the route-guard harness compares with the hub's routes.
#[test]
fn roles_follow_the_approved_inventory() {
    let ops = spec_operations_from_openapi(SPEC).unwrap();
    let role = |m: &str, p: &str| {
        ops.iter()
            .find(|o| o.method == m && o.path == p)
            .unwrap_or_else(|| panic!("{m} {p} is not in the spec"))
            .required
    };
    let expect = [
        ("GET", "/api/v1/me", Role::Viewer),
        ("PUT", "/api/v1/me/github-token", Role::Editor),
        ("DELETE", "/api/v1/me/github-token", Role::Editor),
        ("GET", "/api/v1/swimlanes/{id}/served", Role::Viewer),
        ("POST", "/api/v1/swimlanes/{id}/files/reveal", Role::Editor),
        ("PUT", "/api/v1/swimlanes/{id}/files", Role::Editor),
        ("POST", "/api/v1/swimlanes/{id}/files/upload", Role::Editor),
        ("DELETE", "/api/v1/swimlanes/{id}/files", Role::Editor),
        ("POST", "/api/v1/swimlanes/{id}/files/revert", Role::Editor),
        ("POST", "/api/v1/swimlanes/{id}/restart", Role::Operator),
        ("POST", "/api/v1/swimlanes/{id}/notify", Role::Operator),
        ("POST", "/api/v1/prs", Role::Editor),
        ("POST", "/api/v1/proposals/{id}/approve", Role::Admin),
        ("POST", "/api/v1/proposals/{id}/reject", Role::Admin),
        ("POST", "/api/v1/docs", Role::Editor),
        ("GET", "/api/v1/events", Role::Viewer),
        ("GET", "/api/v1/blobs/{hash}", Role::Viewer),
        ("GET", "/api/v1/admin/swimlanes/{id}/auto-notify", Role::Admin),
        ("PUT", "/api/v1/admin/swimlanes/{id}/auto-notify", Role::Admin),
        ("GET", "/api/v1/admin/sentinels", Role::Admin),
        ("GET", "/api/v1/admin/audit", Role::Admin),
    ];
    for (m, p, r) in expect {
        assert_eq!(role(m, p), SpecRole::Role(r), "{m} {p}");
    }
    // Everything under /admin is Admin only.
    for o in &ops {
        if o.path.starts_with("/api/v1/admin/") {
            assert_eq!(o.required, SpecRole::Role(Role::Admin), "{} {}", o.method, o.path);
        }
    }
    // A route table built from the spec itself passes the S4 harness: keys are full paths (`/api/v1/...`).
    let routes: Vec<RegisteredRoute> = ops
        .iter()
        .map(|o| {
            let guard = match o.required {
                SpecRole::Role(r) => Guard::Role(r),
                SpecRole::None => Guard::Public {
                    reason: "listed as public in the spec".into(),
                },
            };
            RegisteredRoute::new(&o.method, &o.path, guard)
        })
        .collect();
    let violations = check_route_table(&routes, &ops, &Allowances::default());
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn every_non_get_requires_csrf_and_idempotency_key() {
    let d = doc();
    let mut writes = 0;
    for o in operations(&d).iter().filter(|o| o.is_write()) {
        writes += 1;
        let key = (o.method.as_str(), o.path.as_str());
        if !NO_CSRF.contains(&key) {
            let csrf = o
                .header_param(&d, "X-CSRF-Token")
                .unwrap_or_else(|| panic!("{} has no X-CSRF-Token header", o.label()));
            assert_eq!(csrf["required"], true, "{} must require X-CSRF-Token", o.label());
        }
        if !NO_IDEMPOTENCY_KEY.contains(&key) {
            let idem = o
                .header_param(&d, "Idempotency-Key")
                .unwrap_or_else(|| panic!("{} has no Idempotency-Key header", o.label()));
            assert_eq!(
                idem["required"],
                true,
                "{} must require Idempotency-Key",
                o.label()
            );
        }
        // A write that can be refused for a stale hash says so.
        assert!(
            o.response(&d, "403").is_some(),
            "{} must document 403 (role, CSRF or pending user)",
            o.label()
        );
    }
    assert!(writes >= 40, "found only {writes} writes");

    // The exemptions are real operations, not leftovers.
    let all = operations(&d);
    for (m, p) in NO_CSRF.iter().chain(NO_IDEMPOTENCY_KEY) {
        find(&all, m, p);
    }
    // Reads never ask for the write headers.
    for o in all.iter().filter(|o| !o.is_write()) {
        assert!(
            o.header_param(&d, "X-CSRF-Token").is_none(),
            "{} is a read",
            o.label()
        );
    }
}

#[test]
fn every_list_uses_keyset_with_limit_max_500() {
    let d = doc();
    let all = operations(&d);
    let mut lists = BTreeSet::new();
    for o in &all {
        // No offset paging anywhere.
        for p in o.params(&d) {
            let name = p["name"].as_str().unwrap_or_default();
            assert!(
                !matches!(name, "offset" | "page" | "page_size" | "skip"),
                "{} pages with {name}",
                o.label()
            );
        }
        let Some(ok) = o.response(&d, "200") else {
            continue;
        };
        let Some(schema) = json_schema(ok) else {
            continue;
        };
        let top = properties(&d, schema);
        let paged_in_body = top.contains_key("next")
            || top
                .values()
                .any(|v| properties(&d, v).contains_key("next") && properties(&d, v).contains_key("items"));
        // A response with an `items` array is a list, whatever its parameters say.
        let looks_like_list = paged_in_body || top.contains_key("items");
        if o.method != "GET" || !(looks_like_list || o.query_param(&d, "cursor").is_some()) {
            continue;
        }
        lists.insert(o.label());
        let cursor = o
            .query_param(&d, "cursor")
            .unwrap_or_else(|| panic!("{} is a list without a cursor parameter", o.label()));
        assert!(!cursor["required"].as_bool().unwrap_or(false), "{}", o.label());
        let limit = o
            .query_param(&d, "limit")
            .unwrap_or_else(|| panic!("{} is a list without a limit parameter", o.label()));
        let schema = resolve(&d, &limit["schema"]);
        assert_eq!(schema["type"], "integer", "{}", o.label());
        assert_eq!(schema["minimum"], 1, "{}", o.label());
        let max = schema["maximum"]
            .as_u64()
            .unwrap_or_else(|| panic!("{} has no maximum on limit", o.label()));
        assert!(max <= 500, "{} allows limit {max}", o.label());
        // The response carries the next cursor.
        assert!(
            paged_in_body,
            "{} takes a cursor but its response has no `next`",
            o.label()
        );
    }
    // Guard against a vacuous pass: these are the lists the UI and MCP rely on.
    for expected in [
        "GET /api/v1/swimlanes",
        "GET /api/v1/swimlanes/{id}/tree",
        "GET /api/v1/swimlanes/{id}/files/history",
        "GET /api/v1/swimlanes/{id}/drift",
        "GET /api/v1/swimlanes/{id}/pending-restarts",
        "GET /api/v1/compare/tree",
        "GET /api/v1/grid",
        "GET /api/v1/search/settings",
        "GET /api/v1/findings",
        "GET /api/v1/prs",
        "GET /api/v1/proposals",
        "GET /api/v1/docs",
        "GET /api/v1/admin/users",
        "GET /api/v1/admin/access-requests",
        "GET /api/v1/admin/audit",
        "GET /api/v1/admin/agents",
        "GET /api/v1/admin/sentinels",
    ] {
        assert!(
            lists.contains(expected),
            "{expected} is not detected as a list: {lists:?}"
        );
    }
    // Free-text results are bounded by a maximum instead of a cursor.
    for (path, param) in [
        ("/api/v1/search/text", "max_total"),
        ("/api/v1/docs/search", "limit"),
        ("/api/v1/docs/fs/grep", "limit"),
    ] {
        let op = find(&all, "GET", path);
        let p = op
            .query_param(&d, param)
            .unwrap_or_else(|| panic!("{path} has no {param}"));
        assert!(
            resolve(&d, &p["schema"])["maximum"]
                .as_u64()
                .is_some_and(|m| m <= 500),
            "{path} {param} needs a maximum of at most 500"
        );
    }
}

#[test]
fn content_endpoints_have_etag() {
    let d = doc();
    let all = operations(&d);
    let tagged: Vec<&Op<'_>> = all.iter().filter(|o| o.has_tag("content")).collect();
    let tagged_labels: BTreeSet<String> = tagged.iter().map(|o| o.label()).collect();
    for expected in [
        "GET /api/v1/swimlanes/{id}/files/content",
        "GET /api/v1/swimlanes/{id}/effective",
        "GET /api/v1/swimlanes/{id}/served",
        "GET /api/v1/blobs/{hash}",
        "GET /api/v1/compare",
        "GET /api/v1/docs/fs/read",
    ] {
        assert!(
            tagged_labels.contains(expected),
            "{expected} must carry the `content` tag"
        );
    }
    for o in tagged {
        assert_eq!(o.method, "GET", "{}", o.label());
        let inm = o
            .header_param(&d, "If-None-Match")
            .unwrap_or_else(|| panic!("{} does not accept If-None-Match", o.label()));
        assert!(!inm["required"].as_bool().unwrap_or(false));
        let ok = o
            .response(&d, "200")
            .unwrap_or_else(|| panic!("{} has no 200", o.label()));
        assert!(
            ok["headers"].get("ETag").is_some(),
            "{} 200 does not send an ETag",
            o.label()
        );
        let nm = o
            .response(&d, "304")
            .unwrap_or_else(|| panic!("{} has no 304", o.label()));
        assert!(nm.get("content").is_none(), "{} 304 has no body", o.label());
        assert!(
            nm["headers"].get("ETag").is_some(),
            "{} 304 repeats the ETag",
            o.label()
        );
    }
}

#[test]
fn blobs_cache_control_private_immutable() {
    let d = doc();
    let all = operations(&d);
    let blobs = find(&all, "GET", "/api/v1/blobs/{hash}");
    let ok = blobs.response(&d, "200").expect("200");
    let cc = resolve(&d, &ok["headers"]["Cache-Control"]);
    let value = cc["schema"]["enum"][0]
        .as_str()
        .or_else(|| cc["schema"]["example"].as_str())
        .or_else(|| cc["example"].as_str())
        .expect("Cache-Control gives its exact value");
    let directives: BTreeSet<&str> = value.split(',').map(str::trim).collect();
    assert!(directives.contains("private"), "{value}");
    assert!(directives.contains("immutable"), "{value}");
    assert!(directives.contains("max-age=31536000"), "{value}");
    assert!(
        !directives.contains("public"),
        "blobs are for signed-in users only: {value}"
    );
    // The hash is the ETag, and the path takes only a content hash.
    let hash = blobs
        .params(&d)
        .into_iter()
        .find(|p| p["in"] == "path" && p["name"] == "hash")
        .expect("hash path parameter");
    assert_eq!(resolve(&d, &hash["schema"])["pattern"], "^[0-9a-fA-F]{64}$");
}

#[test]
fn no_agent_join_token_endpoint() {
    let d = doc();
    let all = operations(&d);
    for o in &all {
        let lower = o.path.to_lowercase();
        let mentions_join = lower.contains("join")
            || o.def["operationId"]
                .as_str()
                .is_some_and(|i| i.to_lowercase().contains("join"));
        if mentions_join {
            // The one allowed place: an Admin issues a token for a swimlane (fallback only, S5).
            assert!(
                o.path.starts_with("/api/v1/admin/agents/") && o.method == "POST",
                "{} looks like an agent-facing join endpoint",
                o.label()
            );
            assert_eq!(o.def["x-required-role"], "admin", "{}", o.label());
        }
        assert!(
            !lower.starts_with("/api/v1/agent") && !lower.starts_with("/agent"),
            "{} is an agent-facing endpoint; agents use gRPC",
            o.label()
        );
    }
    find(&all, "POST", "/api/v1/admin/agents/{swimlane}/join-token");
}

#[test]
fn errors_are_problem_json_and_every_operation_has_a_default() {
    let d = doc();
    for o in operations(&d) {
        let default = o
            .response(&d, "default")
            .unwrap_or_else(|| panic!("{} has no default error response", o.label()));
        assert!(
            default["content"].get("application/problem+json").is_some(),
            "{}",
            o.label()
        );
        for (code, r) in o.def["responses"].as_object().expect("responses") {
            let is_error = code.starts_with('4') || code.starts_with('5') || code == "default";
            if !is_error {
                continue;
            }
            let r = resolve(&d, r);
            let content = r["content"]
                .as_object()
                .unwrap_or_else(|| panic!("{} {code} has no body", o.label()));
            assert_eq!(
                content.keys().collect::<Vec<_>>(),
                vec!["application/problem+json"],
                "{} {code}",
                o.label()
            );
        }
    }
}

/// S7, S21: a secret is accepted but never returned. Properties named like secrets must be `writeOnly`.
#[test]
fn secrets_are_write_only() {
    let d = doc();
    // (schema, property): a value that is shown exactly once, with the reason.
    let shown_once = [("JoinToken", "token")];
    let mut checked = 0;
    for (name, schema) in d["components"]["schemas"].as_object().expect("schemas") {
        for (prop, def) in properties(&d, schema) {
            let p = prop.to_lowercase();
            let secretish = ["token", "secret", "password", "webhook_url", "private_key"]
                .iter()
                .any(|s| p.contains(s));
            if !secretish || p.ends_with("_configured") || p == "token_type" {
                continue;
            }
            checked += 1;
            if shown_once.contains(&(name.as_str(), prop)) {
                assert_eq!(def["readOnly"], true, "{name}.{prop} is shown once, so readOnly");
                continue;
            }
            assert_eq!(
                def["writeOnly"], true,
                "{name}.{prop} looks like a secret and must be writeOnly"
            );
        }
    }
    assert!(checked >= 3, "found only {checked} secret-like properties");
}

/// The attribution, severity, pickup state, tenant set and withheld-content additions of prompt 01.
#[test]
fn schemas_carry_the_required_fields() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let has = |schema: &str, prop: &str| {
        let s = schemas
            .get(schema)
            .unwrap_or_else(|| panic!("schema {schema} is missing"));
        assert!(
            properties(&d, s).contains_key(prop),
            "schema {schema} has no property {prop}"
        );
    };
    for (schema, prop) in [
        ("FileEntry", "attribution"),
        ("FileEntry", "pickup_state"),
        ("FileEntry", "severity"),
        ("FileEntry", "content_withheld"),
        ("Version", "attribution"),
        ("Version", "severity"),
        ("AuditEvent", "attribution"),
        ("Finding", "severity"),
        ("SettingChange", "severity"),
        ("ServiceState", "pickup"),
        ("SwimlaneSummary", "tenants"),
        ("FileContent", "content_withheld"),
        ("Attribution", "source"),
        ("Attribution", "confidence"),
        ("Attribution", "actor"),
        ("Attribution", "evidence"),
        ("SeverityRule", "severity"),
        ("RetentionSettings", "dry_run_preview"),
        ("Problem", "status"),
    ] {
        has(schema, prop);
    }
    // Wire names of the closed enums are the domain ones.
    let enum_values = |name: &str| -> BTreeSet<String> {
        schemas[name]["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} is not an enum"))
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    let domain_set = |all: &[&str]| all.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
    assert_eq!(enum_values("Role"), domain_set(&Role::ALL.map(Role::as_str)));
    assert_eq!(
        enum_values("DriftState"),
        domain_set(&domain::DriftState::ALL.map(domain::DriftState::as_str))
    );
    assert_eq!(
        enum_values("PickupState"),
        domain_set(&domain::PickupState::ALL.map(domain::PickupState::as_str))
    );
    assert_eq!(
        enum_values("Severity"),
        domain_set(&domain::Severity::ALL.map(domain::Severity::as_str))
    );
    assert_eq!(
        enum_values("FindingKind"),
        domain_set(&domain::FindingKind::ALL.map(domain::FindingKind::as_str))
    );
    assert_eq!(
        enum_values("EventKind"),
        domain_set(&[
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
        ])
    );
}
