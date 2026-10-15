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
        ("POST", "/api/v1/swimlanes/{id}/files/preview", Role::Editor),
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
    // (schema, property): a boolean about a secret, not the secret, with the reason: whether a token is stored.
    let flags = [("Me", "needs_github_token")];
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
            if flags.contains(&(name.as_str(), prop)) {
                assert_eq!(def["type"], "boolean", "{name}.{prop} is a flag, so a boolean");
                continue;
            }
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
            "pickup_changed",
        ])
    );
}

/// S2: the cookies carry the `__Host-` prefix (Secure, Path=/, no Domain), in the scheme and in every description.
#[test]
fn cookie_names_have_the_host_prefix() {
    let d = doc();
    assert_eq!(
        d["components"]["securitySchemes"]["sessionCookie"]["name"],
        "__Host-lk_session"
    );
    for bare in ["`lk_session`", "`lk_csrf`", "name: lk_session"] {
        assert!(
            !SPEC.contains(bare),
            "the bare cookie name {bare} is still in the spec"
        );
    }
    let csrf = d["components"]["parameters"]["CsrfToken"]["description"]
        .as_str()
        .unwrap();
    assert!(csrf.contains("`__Host-lk_csrf`"), "{csrf}");
    let callback =
        d["paths"]["/auth/callback"]["get"]["responses"]["302"]["headers"]["Set-Cookie"]["description"]
            .as_str()
            .unwrap();
    assert!(
        callback.contains("`__Host-lk_session`") && callback.contains("`__Host-lk_csrf`"),
        "{callback}"
    );
}

/// 03a: `Me.needs_github_token` is an additive boolean (the signed-in user has no stored GitHub token yet).
#[test]
fn me_has_the_additive_needs_github_token_flag() {
    let d = doc();
    let me = &d["components"]["schemas"]["Me"];
    let props = me["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|p| p.get("properties"))
        .expect("Me adds properties to User");
    assert_eq!(props["needs_github_token"]["type"], "boolean");
    for p in me["allOf"].as_array().unwrap() {
        let required = p.get("required").and_then(Value::as_array);
        assert!(
            required.is_none_or(|r| !r.iter().any(|v| v == "needs_github_token")),
            "a new response field starts optional, so old mocks and clients stay valid"
        );
    }
}

/// The SSE `EventKind` lists only public events: `user_sessions_revoked` is internal and never streamed.
#[test]
fn internal_events_are_not_in_the_sse_event_kinds() {
    let d = doc();
    let kinds: Vec<&str> = d["components"]["schemas"]["EventKind"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(!kinds.contains(&"user_sessions_revoked"));
    assert_eq!(kinds.len(), 16);
}

/// The `NotificationKind` of the admin API is the port's: the wire names match, in the same order, and a channel can
/// subscribe to every one of them (its `kinds` bound is at least the number of kinds).
#[test]
fn notification_kinds_are_the_ports_and_a_channel_can_name_them_all() {
    use ports::NotificationKind as K;
    /// Passes a variant through a match without a wildcard: a variant added to the port and not listed here fails to compile.
    fn listed(k: K) -> K {
        match k {
            K::AccessRequested
            | K::ProposalPending
            | K::ProposalDecided
            | K::ServiceRestarted
            | K::PrRaised
            | K::AgentDisconnected
            | K::CriticalFinding
            | K::DriftDetected
            | K::SyncWindowStale => k,
        }
    }
    let d = doc();
    let rust: Vec<String> = [
        K::AccessRequested,
        K::ProposalPending,
        K::ProposalDecided,
        K::ServiceRestarted,
        K::PrRaised,
        K::AgentDisconnected,
        K::CriticalFinding,
        K::DriftDetected,
        K::SyncWindowStale,
    ]
    .map(|k| {
        serde_json::to_value(listed(k))
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    })
    .to_vec();
    let spec: Vec<String> = d["components"]["schemas"]["NotificationKind"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        spec, rust,
        "OpenAPI NotificationKind and ports::NotificationKind differ"
    );
    for schema in ["NotificationChannelInput", "NotificationChannel"] {
        let max = d["components"]["schemas"][schema]["properties"]["kinds"]["maxItems"]
            .as_u64()
            .unwrap();
        assert!(
            max >= u64::try_from(rust.len()).unwrap(),
            "{schema}.kinds: maxItems {max} cannot hold every notification kind"
        );
    }
}

/// A client can name every kind: the `kinds` filters of the event stream and of the findings list are bounded by the number of
/// kinds they filter on (S11, `uniqueItems`), so a new kind raises the bound with it.
#[test]
fn kind_filters_allow_every_kind() {
    let d = doc();
    for (path, schema) in [
        ("/api/v1/events", "EventKind"),
        ("/api/v1/findings", "FindingKind"),
    ] {
        let kinds = u64::try_from(
            d["components"]["schemas"][schema]["enum"]
                .as_array()
                .unwrap()
                .len(),
        )
        .unwrap();
        let params = d["paths"][path]["get"]["parameters"].as_array().unwrap();
        let filter = params
            .iter()
            .find(|p| p["name"] == "kinds")
            .unwrap_or_else(|| panic!("{path} has no kinds filter"));
        assert_eq!(
            filter["schema"]["maxItems"].as_u64().unwrap(),
            kinds,
            "{path}: maxItems is the number of {schema} values"
        );
        assert_eq!(filter["schema"]["uniqueItems"], true, "{path}");
    }
}

fn required_of(schema: &Value) -> BTreeSet<String> {
    schema["required"]
        .as_array()
        .map(|r| r.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn assert_optional(d: &Value, schema: &str, prop: &str) {
    let s = &d["components"]["schemas"][schema];
    assert!(properties(d, s).contains_key(prop), "{schema}.{prop} is missing");
    assert!(
        !required_of(resolve(d, s)).contains(prop),
        "{schema}.{prop} must stay optional: an old client does not send it"
    );
}

fn wire_keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

/// Prompt 06's contract change (G2, G3, G11): the new request fields are optional, so a client built from the old
/// document keeps working, and their values are the ones the domain types serialise.
#[test]
fn write_request_additions_are_optional_and_match_the_domain() {
    use domain::{ChangeEol, PrTarget, TenantId};
    let d = doc();
    let schemas = &d["components"]["schemas"];
    assert_optional(&d, "EditFileRequest", "change_eol");
    assert_optional(&d, "RevertFileRequest", "change_eol");
    let eol: BTreeSet<String> = ChangeEol::ALL.iter().map(|e| e.as_str().to_owned()).collect();
    assert_eq!(strings(&schemas["ChangeEol"]["enum"]), eol);

    assert_optional(&d, "CreatePrRequest", "target");
    assert_optional(&d, "PrJob", "warnings");
    let tenant = serde_json::to_value(PrTarget::Tenant {
        branch: TenantId::parse("sit1").unwrap(),
    })
    .unwrap();
    let base = serde_json::to_value(PrTarget::Base).unwrap();
    assert_eq!(
        schemas["PrTargetTenant"]["properties"]["kind"]["const"],
        tenant["kind"]
    );
    assert_eq!(
        schemas["PrTargetBase"]["properties"]["kind"]["const"],
        base["kind"]
    );
    assert_eq!(required_of(&schemas["PrTargetTenant"]), wire_keys(&tenant));
    assert_eq!(required_of(&schemas["PrTargetBase"]), wire_keys(&base));

    assert_optional(&d, "RevealRequest", "reason");
}

/// Every [`domain::RejectReason`], walked as a chain: `next` has one arm per variant and names the variant after it, so
/// a new variant does not compile until it is placed in the chain, and the chain is the list the document is held to.
fn every_reject_reason() -> Vec<domain::RejectReason> {
    use domain::RejectReason as R;
    fn next(r: R) -> Option<R> {
        match r {
            R::Denied => Some(R::TooLarge),
            R::TooLarge => Some(R::InvalidContent),
            R::InvalidContent => Some(R::UnknownSwimlane),
            R::UnknownSwimlane => Some(R::AgentUnavailable),
            R::AgentUnavailable => Some(R::NotAllowed),
            R::NotAllowed => Some(R::SecretDetected),
            R::SecretDetected => Some(R::ParseError),
            R::ParseError => None,
        }
    }
    let mut all = vec![R::Denied];
    while let Some(n) = all.last().copied().and_then(next) {
        all.push(n);
    }
    all
}

/// Every [`domain::PreviewWarning`], walked the same way (see [`every_reject_reason`]).
fn every_preview_warning() -> Vec<domain::PreviewWarning> {
    use domain::{PreviewWarning as W, TenantId};
    fn next(w: &W) -> Option<W> {
        match w {
            W::FallsBackToBase => Some(W::NoFileLeft),
            W::NoFileLeft => Some(W::ForksFromBase),
            W::ForksFromBase => Some(W::NeverRead {
                suffix: TenantId::parse("sit9").ok()?,
            }),
            W::NeverRead { .. } => Some(W::NeedsNotifyOrRestart),
            W::NeedsNotifyOrRestart => Some(W::NewFolderNeedsConfigServerRestart),
            W::NewFolderNeedsConfigServerRestart => Some(W::ChannelsYmlNeedsConfigServerRestart),
            W::ChannelsYmlNeedsConfigServerRestart => None,
        }
    }
    let mut all = vec![W::FallsBackToBase];
    while let Some(n) = all.last().and_then(next) {
        all.push(n);
    }
    all
}

/// Prompt 06's contract change (G6): the rejection reasons, the issue shape and the warnings on a write.
#[test]
fn rejection_detail_is_optional_and_matches_the_domain() {
    use domain::{AuditId, IssueKind, ShortText, ValidationIssue, WriteOutcome};
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let all = every_reject_reason();
    let reasons: BTreeSet<String> = all
        .iter()
        .map(|r| serde_json::to_value(r).unwrap().as_str().unwrap().to_owned())
        .collect();
    assert_eq!(strings(&schemas["RejectReason"]["enum"]), reasons);

    let kinds: BTreeSet<String> = IssueKind::ALL.iter().map(|k| k.as_str().to_owned()).collect();
    assert_eq!(
        strings(&schemas["ValidationIssue"]["properties"]["kind"]["enum"]),
        kinds
    );
    assert_eq!(
        required_of(&schemas["ValidationIssue"]),
        BTreeSet::from(["kind".to_owned()])
    );
    let issue = ValidationIssue {
        kind: IssueKind::SecretMatch,
        line: std::num::NonZeroU32::new(3),
        rule: Some(ShortText::parse("jwt").unwrap()),
    };
    let declared: BTreeSet<String> = properties(&d, &schemas["ValidationIssue"])
        .into_keys()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        wire_keys(&serde_json::to_value(&issue).unwrap()),
        declared,
        "the issue's wire fields are the document's"
    );

    assert_optional(&d, "WriteApplied", "warnings");
    let applied = serde_json::to_value(WriteOutcome::Applied {
        hash: None,
        audit_id: AuditId::from(1),
        warnings: vec![issue],
    })
    .unwrap();
    for key in applied.as_object().unwrap().keys() {
        assert!(
            properties(&d, &schemas["WriteApplied"]).contains_key(key.as_str()),
            "WriteApplied lacks `{key}`"
        );
    }
    assert_optional(&d, "Problem", "issues");
}

/// Prompt 06's contract change (G1): the preview's wire fields, scopes and warnings are the domain's.
#[test]
fn change_preview_schema_matches_the_domain() {
    use domain::{ChangePreview, PreviewId, PreviewScope, PreviewWarning, Timestamp};
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let preview = ChangePreview {
        preview_id: PreviewId::parse("tok").unwrap(),
        expires_at: Timestamp::from_unix_millis(1),
        scope: PreviewScope::EveryService,
        warnings: vec![PreviewWarning::FallsBackToBase],
        changes: Vec::new(),
        changes_total: 0,
        effective_changed: false,
    };
    let declared: BTreeSet<String> = properties(&d, &schemas["ChangePreview"])
        .into_keys()
        .map(str::to_owned)
        .collect();
    assert_eq!(wire_keys(&serde_json::to_value(&preview).unwrap()), declared);
    assert_eq!(required_of(&schemas["ChangePreview"]), declared, "all required");

    // A flagged value is masked, and the entry says so: `changes` holds `PreviewedChange`, not a bare change.
    assert_eq!(
        schemas["ChangePreview"]["properties"]["changes"]["items"]["$ref"],
        "#/components/schemas/PreviewedChange"
    );
    let previewed = domain::PreviewedChange {
        change: domain::SettingChange {
            path: domain::SettingPath::parse("aws.key").unwrap(),
            kind: domain::ChangeKind::Added,
            left: None,
            right: None,
            left_location: None,
            right_location: None,
        },
        flagged: true,
    };
    let declared_entry: BTreeSet<String> = properties(&d, &schemas["PreviewedChange"])
        .into_keys()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        wire_keys(&serde_json::to_value(&previewed).unwrap()),
        declared_entry
    );
    assert_eq!(required_of(&schemas["PreviewedChange"]), declared_entry);
    assert_eq!(
        schemas["PreviewedChange"]["properties"]["flagged"]["type"],
        "boolean"
    );
    assert_eq!(
        schemas["PreviewedChange"]["properties"]["change"]["$ref"],
        "#/components/schemas/SettingChange"
    );

    let warning_kinds: BTreeSet<String> = every_preview_warning()
        .iter()
        .map(|w| {
            serde_json::to_value(w).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let mut documented = BTreeSet::new();
    for variant in schemas["PreviewWarning"]["oneOf"].as_array().unwrap() {
        let kind = &variant["properties"]["kind"];
        if let Some(c) = kind.get("const") {
            documented.insert(c.as_str().unwrap().to_owned());
        } else {
            documented.extend(strings(&kind["enum"]));
        }
    }
    assert_eq!(documented, warning_kinds);
    let scope_kinds: BTreeSet<String> = schemas["PreviewScope"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["properties"]["kind"]["const"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        scope_kinds,
        BTreeSet::from([
            "every_service".to_owned(),
            "application".to_owned(),
            "folder".to_owned()
        ])
    );
    assert_eq!(
        schemas["PreviewId"]["pattern"].as_str().unwrap(),
        "^[A-Za-z0-9._-]{1,128}$",
        "the same shape as the domain's token ids"
    );
}

/// Prompt 06's contract change (G1, A3): the preview operation exists, an upload and a delete may carry its id, and the
/// upload size is stated as the hub enforces it.
#[test]
fn preview_operation_and_upload_text() {
    let d = doc();
    let ops = operations(&d);
    let preview_op = find(&ops, "POST", "/api/v1/swimlanes/{id}/files/preview");
    assert!(preview_op.has_tag("writes"));
    assert_eq!(
        json_schema(preview_op.response(&d, "200").unwrap()).unwrap()["$ref"],
        "#/components/schemas/ChangePreview"
    );
    for code in ["403", "413", "422"] {
        assert!(
            preview_op.response(&d, code).is_some(),
            "preview documents {code}"
        );
    }
    let form = &preview_op.def["requestBody"]["content"]["multipart/form-data"]["schema"];
    assert_eq!(
        required_of(form),
        BTreeSet::from([
            "action".to_owned(),
            "path".to_owned(),
            "expected_state".to_owned()
        ])
    );
    assert_eq!(
        strings(&form["properties"]["action"]["enum"]),
        BTreeSet::from(["upload".to_owned(), "delete".to_owned()])
    );
    let upload = find(&ops, "POST", "/api/v1/swimlanes/{id}/files/upload");
    let upload_form = &upload.def["requestBody"]["content"]["multipart/form-data"]["schema"];
    assert!(upload_form["properties"].get("preview_id").is_some());
    assert!(!required_of(upload_form).contains("preview_id"));
    // The limit is the engine's `Limits::input_bytes` (docs/performance.md): 2 MiB, for the preview and the upload alike.
    let file_text = upload_form["properties"]["file"]["description"].as_str().unwrap();
    assert!(
        file_text.contains("2 MiB") && file_text.contains("413") && !file_text.contains("3 MiB"),
        "{file_text}"
    );
    let preview_text = form["properties"]["file"]["description"].as_str().unwrap();
    assert!(
        preview_text.contains("2 MiB") && !preview_text.contains("3 MiB"),
        "{preview_text}"
    );
    assert!(
        !SPEC.contains("3 MiB"),
        "no 3 MiB limit is stated for uploads or edits"
    );
    assert!(
        !SPEC.contains("4 MiB"),
        "no 4 MiB limit is stated for uploads or edits"
    );
    for schema in ["EditFileRequest", "DraftRequest"] {
        let content = &d["components"]["schemas"][schema]["properties"]["content"];
        // `maxLength` counts characters, and a UTF-8 character takes up to four bytes: it is a coarse bound of the schema
        // and is NOT the byte cap. The 2 MiB is a limit of bytes that the server enforces (413).
        assert_eq!(
            content["maxLength"],
            2 * 1024 * 1024,
            "{schema}.content: `maxLength` is a bound in characters and is not the 2 MiB byte cap (the server counts bytes)"
        );
        let text = content["description"].as_str().unwrap();
        assert!(
            text.contains("2 MiB") && text.contains("bytes") && text.contains("413") && text.contains("characters"),
            "{schema}.content says that the 2 MiB is counted in bytes by the server (413) and that `maxLength` counts characters: {text}"
        );
        assert!(
            !text.contains("never fewer than the bytes"),
            "{schema}.content: characters are never MORE than bytes, so `maxLength` cannot enforce a byte limit: {text}"
        );
    }
    assert!(!SPEC.contains("25 MiB"), "the 25 MiB claim is gone (A3)");
    assert!(upload.response(&d, "413").is_some());
    let delete = find(&ops, "DELETE", "/api/v1/swimlanes/{id}/files");
    let id_param = delete
        .query_param(&d, "preview_id")
        .expect("delete takes preview_id");
    assert_ne!(id_param["required"], true);
}

/// The MCP document names the same values as the REST document for the write additions: an assistant and the web app
/// must see one set of rejection reasons, issue kinds and line ending styles.
#[test]
fn mcp_write_definitions_match_the_rest_document() {
    const MCP: &str = include_str!("../../../docs/mcp-tools.md");
    let defs_text = MCP
        .split("## Shared definitions")
        .nth(1)
        .and_then(|rest| rest.split("```json\n").nth(1))
        .and_then(|rest| rest.split("\n```").next())
        .expect("the shared definitions block");
    let mcp: Value = serde_json::from_str(defs_text).expect("shared definitions are JSON");
    let defs = &mcp["$defs"];
    let d = doc();
    let schemas = &d["components"]["schemas"];

    assert_eq!(
        strings(&defs["ChangeEol"]["enum"]),
        strings(&schemas["ChangeEol"]["enum"])
    );
    assert_eq!(
        strings(&defs["RejectReason"]["enum"]),
        strings(&schemas["RejectReason"]["enum"])
    );
    assert_eq!(
        strings(&defs["ValidationIssue"]["properties"]["kind"]["enum"]),
        strings(&schemas["ValidationIssue"]["properties"]["kind"]["enum"])
    );
    assert_eq!(
        defs["ValidationIssue"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<BTreeSet<_>>(),
        schemas["ValidationIssue"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<BTreeSet<_>>()
    );
    let kinds = |v: &Value| -> BTreeSet<String> {
        v["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["properties"]["kind"]["const"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        kinds(&defs["PrTarget"]),
        BTreeSet::from(["tenant".to_owned(), "base".to_owned()])
    );
    assert_eq!(kinds(&defs["PrTarget"]), {
        // REST names its variants by reference.
        schemas["PrTarget"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                resolve(&d, r)["properties"]["kind"]["const"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<BTreeSet<_>>()
    });
    // One limit for the content of an edit, a draft and an upload: 2 MiB, as in the REST document.
    assert!(
        !MCP.contains("4 MiB"),
        "the MCP document states no 4 MiB content limit"
    );
    assert!(!MCP.contains("\"maxLength\": 4194304") && !MCP.contains("\"maxLength\": 4194304,"));
    assert_eq!(
        MCP.matches("\"maxLength\": 2097152").count(),
        2,
        "propose_change and upload"
    );
    // Additions are optional in the MCP outputs too.
    assert!(defs["WriteOutcome"]["properties"].get("warnings").is_some());
    assert!(!required_of(&defs["WriteOutcome"]).contains("warnings"));
    assert!(!required_of(&defs["ToolError"]).contains("issues"));
    assert!(!required_of(&defs["ToolError"]).contains("reason"));
}
