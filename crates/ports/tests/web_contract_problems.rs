//! contract/08-web, `CG13`: the problem vocabulary and the answers the generated client must be able to type.
//!
//! The document is held to what the servers send. Each body below is built the way the server that sends it builds it
//! (`hub-writes/src/routes/problem.rs` and `me.rs`, `hub-platform/src/error.rs`, `hub-identity/src/problem.rs`), as JSON,
//! and checked against the schema the document gives that operation and status, with this file's own small walker for
//! `$ref`, `allOf`, `oneOf`, `anyOf`, `properties`, `required` and `additionalProperties`. Before this commit the walker
//! rejects the bodies of every `ConflictProblem` and `LockedProblem` response (a closed second `allOf` branch) and of a
//! duplicate-PR conflict (`duplicate_pr` is not a `RejectReason`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use domain::{ContentHash, NfsPath, PrState, SwimlaneId, Timestamp};
use ports::conformance::sample::user_id;
use serde_json::{Value, json};

const SPEC: &str = include_str!("../../../api/openapi.yaml");
const SCHEMA_REF: &str = "#/components/schemas/";
const PROBLEM: &str = "application/problem+json";

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

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

fn ref_name(v: &Value) -> &str {
    let r = v["$ref"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a $ref, got {v}"));
    r.strip_prefix(SCHEMA_REF)
        .unwrap_or_else(|| panic!("not a schema $ref: {r}"))
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn required_of(schema: &Value) -> BTreeSet<String> {
    schema.get("required").map(strings).unwrap_or_default()
}

fn declared_of(schema: &Value) -> BTreeSet<String> {
    schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no properties: {schema}"))
        .keys()
        .cloned()
        .collect()
}

/// The operation with this id: its method, path and definition.
fn operation<'a>(d: &'a Value, id: &str) -> (String, String, &'a Value) {
    for (path, item) in d["paths"].as_object().expect("paths") {
        for m in ["get", "put", "post", "delete", "patch"] {
            if let Some(op) = item.get(m)
                && op["operationId"] == id
            {
                return (m.to_uppercase(), path.clone(), op);
            }
        }
    }
    panic!("operation {id} is not in the spec");
}

/// The response an operation documents for `code`, references resolved.
fn response<'a>(d: &'a Value, id: &str, code: &str) -> &'a Value {
    let (_, _, op) = operation(d, id);
    let r = op["responses"]
        .get(code)
        .unwrap_or_else(|| panic!("{id} documents no {code}"));
    resolve(d, r)
}

/// The schema of the body of `code` of `id`, in the given media type.
fn body_schema<'a>(d: &'a Value, id: &str, code: &str, media: &str) -> &'a Value {
    let r = response(d, id, code);
    r["content"]
        .get(media)
        .unwrap_or_else(|| panic!("{id} {code} has no {media} body"))
        .get("schema")
        .unwrap_or_else(|| panic!("{id} {code} {media} has no schema"))
}

// ---------------------------------------------------------------------------------------------------------------------
// A small JSON Schema walker: only what the problem schemas use. `additionalProperties: false` looks at the `properties` of
// the schema that carries it and nothing else, which is exactly why a closed branch of an `allOf` rejects the keys of its
// siblings.
// ---------------------------------------------------------------------------------------------------------------------

fn check(d: &Value, schema: &Value, v: &Value, at: &str) -> Result<(), String> {
    let s = resolve(d, schema);
    if let Some(parts) = s.get("allOf").and_then(Value::as_array) {
        for p in parts {
            check(d, p, v, at)?;
        }
    }
    if let Some(arms) = s.get("oneOf").and_then(Value::as_array) {
        let ok = arms.iter().filter(|a| check(d, a, v, at).is_ok()).count();
        if ok != 1 {
            return Err(format!("{at}: matches {ok} arms of a oneOf, not exactly one"));
        }
    }
    if let Some(arms) = s.get("anyOf").and_then(Value::as_array)
        && !arms.iter().any(|a| check(d, a, v, at).is_ok())
    {
        return Err(format!("{at}: matches no arm of an anyOf"));
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        return Err(format!("{at}: {v} is not the constant {c}"));
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        return Err(format!("{at}: {v} is not one of {e:?}"));
    }
    if let Some(t) = s.get("type").and_then(Value::as_str) {
        let fits = match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            other => panic!("the walker does not know the type {other}"),
        };
        if !fits {
            return Err(format!("{at}: {v} is not of type {t}"));
        }
    }
    if let Some(obj) = v.as_object() {
        for r in required_of(s) {
            if !obj.contains_key(&r) {
                return Err(format!("{at}: `{r}` is required and absent"));
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        for (k, val) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(p) => check(d, p, val, &format!("{at}.{k}"))?,
                None if s.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!(
                        "{at}: `{k}` is not allowed (additionalProperties: false)"
                    ));
                }
                None => {}
            }
        }
    }
    if let Some(arr) = v.as_array() {
        if let Some(max) = s.get("maxItems").and_then(Value::as_u64)
            && arr.len() as u64 > max
        {
            return Err(format!("{at}: {} items, at most {max}", arr.len()));
        }
        if let Some(items) = s.get("items") {
            for (i, x) in arr.iter().enumerate() {
                check(d, items, x, &format!("{at}[{i}]"))?;
            }
        }
    }
    if let Some(text) = v.as_str() {
        let n = text.chars().count() as u64;
        if let Some(max) = s.get("maxLength").and_then(Value::as_u64)
            && n > max
        {
            return Err(format!("{at}: {n} characters, at most {max}"));
        }
    }
    if let Some(n) = v.as_i64() {
        if let Some(min) = s.get("minimum").and_then(Value::as_i64)
            && n < min
        {
            return Err(format!("{at}: {n} is below {min}"));
        }
        if let Some(max) = s.get("maximum").and_then(Value::as_i64)
            && n > max
        {
            return Err(format!("{at}: {n} is above {max}"));
        }
    }
    Ok(())
}

fn accepts(d: &Value, schema: &Value, body: &Value) -> Result<(), String> {
    check(d, schema, body, "$")
}

// ---------------------------------------------------------------------------------------------------------------------
// The bodies, as the servers build them.
// ---------------------------------------------------------------------------------------------------------------------

fn hash() -> ContentHash {
    ContentHash::from_bytes([0xab; 32])
}

/// `hub-platform`'s `problem_body` and `hub-writes`' `problem`: `type`, `title`, `status`, `code`, then the extras.
fn write_problem(status: u16, code: &str, title: &str, extra: Value) -> Value {
    let mut body = json!({
        "type": format!("urn:lanekeeper:problem:{code}"),
        "title": title,
        "status": status,
        "code": code,
    });
    let Value::Object(extra) = extra else {
        panic!("the extra fields of a problem are an object");
    };
    for (k, v) in extra {
        body[k] = v;
    }
    body
}

/// `hub-identity`'s `problem`: `type`, `title`, `status`, and the request id when there is one. No `code`.
fn identity_problem(status: u16, kind: &str, title: &str) -> Value {
    json!({
        "type": format!("urn:lanekeeper:problem:{kind}"),
        "title": title,
        "status": status,
        "request_id": "req-7f3a",
    })
}

/// The `PrLinkView` the duplicate-PR conflict carries, built from the domain's own types.
fn pr_link() -> Value {
    json!({
        "id": 11,
        "swimlane": SwimlaneId::parse("sit1").unwrap(),
        "path": NfsPath::parse("tx-infinity-api/tx-infinity-core-sit1.yml").unwrap(),
        "nfs_hash": hash(),
        "state": PrState::Open,
        "pr_url": "https://github.com/example/csp-tenant-data/pull/42",
        "pr_number": 42,
        "author": user_id(1),
        "created_at": Timestamp::from_unix_millis(1_700_000_000_000),
        "closed_at": Value::Null,
    })
}

/// One real answer: the operation, the status, the media type and the body the server sends.
struct Answer {
    label: &'static str,
    operation: &'static str,
    code: &'static str,
    media: &'static str,
    body: Value,
}

impl Answer {
    /// A problem body (`application/problem+json`).
    fn problem(label: &'static str, operation: &'static str, code: &'static str, body: Value) -> Self {
        Self {
            label,
            operation,
            code,
            media: PROBLEM,
            body,
        }
    }

    /// A success body (`application/json`).
    fn ok(label: &'static str, operation: &'static str, code: &'static str, body: Value) -> Self {
        Self {
            label,
            operation,
            code,
            media: "application/json",
            body,
        }
    }
}

fn conflict(current: &Value) -> Value {
    write_problem(409, "conflict", "Conflict", json!({ "current": current }))
}

fn locked(until: &Value) -> Value {
    write_problem(423, "locked", "Locked", json!({ "until": until }))
}

fn rejected(status: u16, code: &str, title: &str, reason: &str) -> Value {
    write_problem(status, code, title, json!({ "reason": reason }))
}

/// The stale-hash conflict, the lock and the rejections of the content writes (F11: the first two were rejected).
fn file_write_answers() -> Vec<Answer> {
    let later = Timestamp::from_unix_millis(1_700_000_060_000);
    vec![
        Answer::problem(
            "editFile 409 with the hash found",
            "editFile",
            "409",
            conflict(&json!(hash())),
        ),
        Answer::problem(
            "editFile 409 for an absent file",
            "editFile",
            "409",
            conflict(&Value::Null),
        ),
        Answer::problem(
            "approveProposal 409 for a stale proposal",
            "approveProposal",
            "409",
            conflict(&json!(hash())),
        ),
        Answer::problem("applyDraft 409", "applyDraft", "409", conflict(&json!(hash()))),
        Answer::problem(
            "editFile 423 with no known end",
            "editFile",
            "423",
            locked(&Value::Null),
        ),
        Answer::problem(
            "restartService 423 until a time",
            "restartService",
            "423",
            locked(&json!(later)),
        ),
        Answer::problem(
            "editFile 422 with a reason and issues",
            "editFile",
            "422",
            write_problem(
                422,
                "unprocessable",
                "Unprocessable request",
                json!({
                    "reason": "secret_detected",
                    "issues": [{ "kind": "secret_match", "line": 12, "rule": "github_token" }],
                }),
            ),
        ),
        Answer::problem(
            "editFile 413",
            "editFile",
            "413",
            rejected(413, "payload_too_large", "Payload too large", "too_large"),
        ),
        Answer::problem(
            "editFile 403",
            "editFile",
            "403",
            write_problem(403, "forbidden", "Not allowed", json!({})),
        ),
    ]
}

/// What `createPr` and `notifyServices` send: a duplicate, the refusals of a job, and the proposal of a user who needs
/// approval (`routes/prs.rs:50`, `routes/notify.rs:54`).
fn pr_and_notify_answers() -> Vec<Answer> {
    let proposal = || json!({ "outcome": "proposal_created", "id": 7 });
    vec![
        Answer::problem(
            "createPr 409 duplicate_pr with the link of the open PR",
            "createPr",
            "409",
            write_problem(
                409,
                "conflict",
                "Conflict",
                json!({ "reason": "duplicate_pr", "link": pr_link() }),
            ),
        ),
        Answer::problem(
            "createPr 422 with a reason",
            "createPr",
            "422",
            rejected(422, "unprocessable", "Unprocessable request", "invalid_content"),
        ),
        Answer::problem(
            "createPr 413",
            "createPr",
            "413",
            rejected(413, "payload_too_large", "Payload too large", "too_large"),
        ),
        Answer::problem(
            "createPr 423 while another PR job covers the paths",
            "createPr",
            "423",
            locked(&Value::Null),
        ),
        Answer::ok("createPr 200 proposal_created", "createPr", "200", proposal()),
        Answer::ok(
            "notifyServices 200 applied",
            "notifyServices",
            "200",
            json!({ "accepted": 2, "audit_id": 41 }),
        ),
        Answer::ok(
            "notifyServices 200 proposal_created",
            "notifyServices",
            "200",
            proposal(),
        ),
    ]
}

/// The refusals of the GitHub token (`routes/me.rs`).
fn token_answers() -> Vec<Answer> {
    vec![
        Answer::problem(
            "putGithubToken 422 token_missing_access",
            "putGithubToken",
            "422",
            write_problem(
                422,
                "token_missing_access",
                "The token cannot push to both repositories",
                json!({
                    "detail": "The token must be able to push to both the base and the tenant repository.",
                    "base": true,
                    "tenant": false,
                }),
            ),
        ),
        Answer::problem(
            "putGithubToken 422 classic_token",
            "putGithubToken",
            "422",
            write_problem(
                422,
                "classic_token",
                "Classic tokens are not accepted",
                json!({ "detail": "Only fine-grained personal access tokens are accepted." }),
            ),
        ),
        Answer::problem(
            "putGithubToken 422 token_expired",
            "putGithubToken",
            "422",
            write_problem(
                422,
                "token_expired",
                "The stored token has expired",
                json!({ "detail": "The stored token has expired. Save a new one." }),
            ),
        ),
        Answer::problem(
            "putGithubToken 503 with a request id",
            "putGithubToken",
            "503",
            write_problem(
                503,
                "unavailable",
                "Service unavailable",
                json!({ "request_id": "req-9" }),
            ),
        ),
    ]
}

/// The three identity conflicts (they never carry `current`, F13, nor a `code`), the cooldown and the throttles.
fn identity_and_throttle_answers() -> Vec<Answer> {
    let state = "The request conflicts with the current state";
    let mut with_rule = identity_problem(409, "conflict", state);
    with_rule["detail"] = json!("The last active admin cannot be changed.");
    vec![
        Answer::problem(
            "adminApproveAccessRequest 409",
            "adminApproveAccessRequest",
            "409",
            identity_problem(409, "conflict", state),
        ),
        Answer::problem(
            "adminRejectAccessRequest 409",
            "adminRejectAccessRequest",
            "409",
            identity_problem(409, "conflict", state),
        ),
        Answer::problem(
            "adminUpdateUser 409 with the rule's sentence",
            "adminUpdateUser",
            "409",
            with_rule,
        ),
        Answer::problem(
            "createAccessRequest 429, the cooldown",
            "createAccessRequest",
            "429",
            identity_problem(429, "too-many-requests", "Too many requests, try again later"),
        ),
        Answer::problem(
            "revealValue 429",
            "revealValue",
            "429",
            write_problem(429, "too_many_requests", "Too many requests", json!({})),
        ),
        Answer::problem(
            "streamEvents 429, the sixth stream",
            "streamEvents",
            "429",
            write_problem(
                429,
                "too_many_requests",
                "Too many requests",
                json!({ "request_id": "req-3" }),
            ),
        ),
    ]
}

fn answers() -> Vec<Answer> {
    [
        file_write_answers(),
        pr_and_notify_answers(),
        token_answers(),
        identity_and_throttle_answers(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Every body a server sends is accepted by the schema of that operation and status.
#[test]
fn problem_extensions_validate_real_bodies() {
    let d = doc();
    let mut failures = Vec::new();
    for a in answers() {
        let schema = body_schema(&d, a.operation, a.code, a.media);
        if let Err(why) = accepts(&d, schema, &a.body) {
            failures.push(format!("{}: {why}\n  body: {}", a.label, a.body));
        }
    }
    assert!(
        failures.is_empty(),
        "the document rejects bodies that the servers send:\n{}",
        failures.join("\n")
    );
}

/// The walker has teeth: the shapes this commit corrects are rejected when they come back.
#[test]
fn a_closed_extension_branch_would_reject_the_servers_bodies() {
    let d = doc();

    // F11: `additionalProperties: false` in the extension branch of an `allOf` rejects the keys of its sibling `Problem`.
    let mut closed = d.clone();
    for name in ["ConflictProblem", "LockedProblem"] {
        closed["components"]["schemas"][name]["allOf"][1]["additionalProperties"] = json!(false);
    }
    let conflict = write_problem(409, "conflict", "Conflict", json!({ "current": hash() }));
    let locked = write_problem(423, "locked", "Locked", json!({ "until": Value::Null }));
    for (name, body) in [("ConflictProblem", &conflict), ("LockedProblem", &locked)] {
        let schema = json!({ "$ref": format!("{SCHEMA_REF}{name}") });
        assert!(
            accepts(&closed, &schema, body).is_err(),
            "the walker must reject a {name} body when the extension branch is closed"
        );
        assert!(
            accepts(&d, &schema, body).is_ok(),
            "{name} must accept the body the server builds"
        );
    }

    // `duplicate_pr` is not a `RejectReason`, so `Problem` alone rejects the body that createPr sends.
    let duplicate = write_problem(
        409,
        "conflict",
        "Conflict",
        json!({ "reason": "duplicate_pr", "link": pr_link() }),
    );
    let plain = json!({ "$ref": format!("{SCHEMA_REF}Problem") });
    assert!(
        accepts(&d, &plain, &duplicate).is_err(),
        "a `reason` that is not a RejectReason must not pass as a plain Problem"
    );
    let reasons = strings(&d["components"]["schemas"]["RejectReason"]["enum"]);
    assert!(
        !reasons.contains("duplicate_pr"),
        "RejectReason is the domain's enum and has no duplicate_pr"
    );

    // And a body with a key the document does not list is refused by a closed schema, so the walker is not vacuous.
    let mut extra = write_problem(429, "too_many_requests", "Too many requests", json!({}));
    extra["unlisted"] = json!(1);
    let link = json!({ "$ref": format!("{SCHEMA_REF}PrLink") });
    let mut bad_link = pr_link();
    bad_link["unlisted"] = json!(1);
    assert!(accepts(&d, &link, &bad_link).is_err());
    assert!(
        accepts(&d, &plain, &extra).is_ok(),
        "Problem itself stays open (additionalProperties: true)"
    );
}

/// `code` mirrors the suffix of `type`; `type` is the discriminator and the known suffixes are written down.
#[test]
fn problem_code_and_type_are_documented() {
    let d = doc();
    let problem = &d["components"]["schemas"]["Problem"];
    let code = &problem["properties"]["code"];
    assert_eq!(code["type"], "string", "Problem.code");
    assert!(
        !required_of(problem).contains("code"),
        "an identity problem has no `code`, so it cannot be required"
    );
    assert_eq!(
        required_of(problem),
        ["type", "title", "status"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        "the required keys of a Problem are unchanged"
    );
    assert!(
        code["maxLength"].is_u64() || code["pattern"].is_string(),
        "Problem.code is bounded (S11)"
    );
    let said = code["description"].as_str().unwrap_or("").to_lowercase();
    assert!(
        said.contains("suffix") && said.contains("`type`"),
        "Problem.code must say it mirrors the suffix of `type`"
    );

    // Every suffix a server sends is named in the description of `type`. kebab-case comes from hub-identity, snake_case from
    // hub-platform and hub-writes; both appear and nothing is renamed.
    let types = problem["properties"]["type"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(types.contains("urn:lanekeeper:problem:"), "the URN prefix");
    let lower = types.to_lowercase();
    assert!(
        lower.contains("discriminator") && lower.contains("status"),
        "`type` is the discriminator, and an unknown value is handled by `status`"
    );
    for suffix in [
        // hub-platform's `ApiError` and hub-writes
        "bad_request",
        "unauthorized",
        "forbidden",
        "not_found",
        "method_not_allowed",
        "request_timeout",
        "conflict",
        "payload_too_large",
        "unsupported_media_type",
        "unprocessable",
        "too_many_requests",
        "internal",
        "unavailable",
        "shutting_down",
        "gateway_timeout",
        "idempotency_conflict",
        "locked",
        // the GitHub token
        "classic_token",
        "not_fine_grained_token",
        "token_rejected",
        "token_missing_access",
        "github_account_mismatch",
        "token_expired",
        "token_unreadable",
        // hub-identity
        "unauthenticated",
        "access-pending",
        "access-disabled",
        "insufficient-role",
        "csrf",
        "not-found",
        "bad-request",
        "unsupported-media-type",
        "payload-too-large",
        "too-many-requests",
        "sign-in-invalid",
        "sign-in-refused",
        "identity-provider",
    ] {
        assert!(
            types.contains(&format!("`{suffix}`")),
            "the description of Problem.type must list the suffix `{suffix}`"
        );
    }
}

/// `createPr` answers 200 (a proposal), 202 (a job), 409 (a stale or duplicate), 413, 422 and 423, each with the schema
/// the server sends.
#[test]
fn create_pr_documents_every_answer_it_gives() {
    let d = doc();
    let (_, path, op) = operation(&d, "createPr");
    assert_eq!(path, "/api/v1/prs");
    for code in [
        "200", "202", "400", "401", "403", "409", "413", "422", "423", "default",
    ] {
        assert!(
            op["responses"].get(code).is_some(),
            "createPr must document {code}"
        );
    }

    assert_eq!(
        body_schema(&d, "createPr", "200", "application/json")["$ref"],
        format!("{SCHEMA_REF}WriteProposalCreated"),
        "a user who needs approval gets the proposal's own answer"
    );
    assert_eq!(
        body_schema(&d, "createPr", "202", "application/json")["$ref"],
        format!("{SCHEMA_REF}PrJob")
    );
    assert_eq!(
        op["responses"]["413"]["$ref"],
        "#/components/responses/PayloadTooLarge"
    );
    assert_eq!(op["responses"]["423"]["$ref"], "#/components/responses/Locked");

    // 409 is a conflict of two kinds, told apart by `reason`.
    let conflict = body_schema(&d, "createPr", "409", PROBLEM);
    let arms: Vec<&str> = conflict["oneOf"]
        .as_array()
        .expect("createPr 409 is a oneOf")
        .iter()
        .map(ref_name)
        .collect();
    assert_eq!(arms, ["ConflictProblem", "DuplicatePrProblem"]);
    assert_eq!(
        response(&d, "createPr", "409")["content"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        [PROBLEM]
    );
}

/// `DuplicatePrProblem` is a standalone closed schema of the keys of a `Problem`, with `reason` fixed and the open PR's link,
/// and it tells itself apart from a stale-hash conflict.
#[test]
fn duplicate_pr_problem_is_a_standalone_conflict_with_the_link() {
    let d = doc();
    let (_, _, op) = operation(&d, "createPr");
    let dup = &d["components"]["schemas"]["DuplicatePrProblem"];
    assert!(
        dup.get("allOf").is_none(),
        "standalone: `Problem.reason` is a RejectReason and cannot hold duplicate_pr"
    );
    assert_eq!(dup["type"], "object");
    assert_eq!(dup["additionalProperties"], false);
    assert_eq!(dup["properties"]["reason"]["const"], "duplicate_pr");
    assert_eq!(dup["properties"]["link"]["$ref"], format!("{SCHEMA_REF}PrLink"));
    assert_eq!(
        required_of(dup),
        ["type", "title", "status", "reason", "link"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        "the optional keys are detail, instance, request_id and code"
    );
    assert_eq!(
        declared_of(dup),
        [
            "type",
            "title",
            "status",
            "detail",
            "instance",
            "request_id",
            "code",
            "reason",
            "link"
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect::<BTreeSet<_>>(),
        "the keys of a Problem, with `reason` fixed and a `link`"
    );
    // The same kinds as `Problem` for the keys they share, so a client reads both alike (`status` is the constant 409 here).
    let problem = &d["components"]["schemas"]["Problem"];
    for key in ["type", "title", "detail", "instance", "request_id", "code"] {
        assert_eq!(
            dup["properties"][key]["type"], problem["properties"][key]["type"],
            "DuplicatePrProblem.{key} must be of the kind of Problem.{key}"
        );
    }
    assert_eq!(
        dup["properties"]["type"]["format"],
        problem["properties"]["type"]["format"]
    );
    assert_eq!(dup["properties"]["status"]["const"], 409);
    assert_eq!(
        dup["properties"]["code"]["pattern"],
        problem["properties"]["code"]["pattern"]
    );

    // Both arms are told apart: a duplicate body fits only the new arm, a stale-hash body only the old one.
    let duplicate = write_problem(
        409,
        "conflict",
        "Conflict",
        json!({ "reason": "duplicate_pr", "link": pr_link() }),
    );
    let stale = write_problem(409, "conflict", "Conflict", json!({ "current": hash() }));
    let dup_ref = json!({ "$ref": format!("{SCHEMA_REF}DuplicatePrProblem") });
    let conflict_ref = json!({ "$ref": format!("{SCHEMA_REF}ConflictProblem") });
    assert!(accepts(&d, &dup_ref, &duplicate).is_ok());
    assert!(accepts(&d, &conflict_ref, &duplicate).is_err());
    assert!(accepts(&d, &dup_ref, &stale).is_err());
    assert!(accepts(&d, &conflict_ref, &stale).is_ok());

    let said = op["description"].as_str().unwrap_or("").to_lowercase();
    assert!(
        said.contains("duplicatepr"),
        "the description names the duplicate-PR body"
    );
    assert!(
        said.contains("proposal"),
        "the description says a user who needs approval gets a proposal"
    );
}

/// `notifyServices` answers a user who needs approval with a proposal, as every other write does.
#[test]
fn notify_documents_the_proposal_answer() {
    let d = doc();
    let ok = body_schema(&d, "notifyServices", "200", "application/json");
    let arms: Vec<&str> = ok["oneOf"]
        .as_array()
        .expect("notifyServices 200 is a oneOf")
        .iter()
        .map(ref_name)
        .collect();
    assert_eq!(arms, ["NotifyResult", "WriteProposalCreated"]);

    // The arms are told apart by their keys (`accepted` against `outcome`), both closed.
    for name in ["NotifyResult", "WriteProposalCreated"] {
        assert_eq!(
            d["components"]["schemas"][name]["additionalProperties"], false,
            "{name}"
        );
    }
    let said = response(&d, "notifyServices", "200")["description"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    assert!(
        said.contains("approval") && said.contains("`outcome`"),
        "the 200 says when a proposal comes back and how to tell it (`outcome`)"
    );
    // The roles and the other answers are unchanged.
    let (_, _, op) = operation(&d, "notifyServices");
    assert_eq!(op["x-required-role"], "operator");
    for code in ["400", "401", "403", "404", "422", "default"] {
        assert!(op["responses"].get(code).is_some(), "notifyServices keeps {code}");
    }
}

/// The token problems: `token_missing_access` says which repository fell short, and the other codes are listed.
#[test]
fn token_problems_are_typed_and_documented() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let t = &schemas["TokenMissingAccessProblem"];
    let parts = t["allOf"]
        .as_array()
        .expect("TokenMissingAccessProblem extends Problem");
    assert_eq!(parts[0]["$ref"], format!("{SCHEMA_REF}Problem"));
    let ext = &parts[1];
    assert_eq!(
        declared_of(ext),
        ["base", "tenant"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(required_of(ext), declared_of(ext), "both flags are always sent");
    for flag in ["base", "tenant"] {
        assert_eq!(ext["properties"][flag]["type"], "boolean", "{flag}");
    }
    assert_ne!(
        ext["additionalProperties"],
        json!(false),
        "a closed extension branch rejects the keys of `Problem` (F11)"
    );

    // The 422 of the PUT accepts this body and every other refusal of the token, so it is a union, not the one schema.
    let r422 = body_schema(&d, "putGithubToken", "422", PROBLEM);
    let arms: Vec<&str> = r422["anyOf"]
        .as_array()
        .expect("putGithubToken 422 is an anyOf: the other token problems carry no base and tenant")
        .iter()
        .map(ref_name)
        .collect();
    assert_eq!(arms, ["TokenMissingAccessProblem", "Problem"]);
    let said = response(&d, "putGithubToken", "422")["description"]
        .as_str()
        .unwrap_or("");
    for code in [
        "classic_token",
        "not_fine_grained_token",
        "token_rejected",
        "token_missing_access",
        "github_account_mismatch",
        "token_expired",
        "token_unreadable",
    ] {
        assert!(said.contains(&format!("`{code}`")), "the 422 must name `{code}`");
    }
    assert!(
        said.contains("`base`") && said.contains("`tenant`"),
        "the 422 says what base and tenant mean"
    );

    // A body without the flags is not a TokenMissingAccessProblem, a body with them is.
    let ref_t = json!({ "$ref": format!("{SCHEMA_REF}TokenMissingAccessProblem") });
    let missing = write_problem(
        422,
        "token_missing_access",
        "t",
        json!({ "base": true, "tenant": true }),
    );
    let classic = write_problem(422, "classic_token", "t", json!({}));
    assert!(accepts(&d, &ref_t, &missing).is_ok());
    assert!(accepts(&d, &ref_t, &classic).is_err());
}

/// 429 and 503 are documented where the server sends them, with the header the client needs to wait.
#[test]
fn too_many_requests_and_service_unavailable_are_documented_where_the_server_sends_them() {
    let d = doc();
    let responses = &d["components"]["responses"];
    let many = &responses["TooManyRequests"];
    assert_eq!(
        many["content"][PROBLEM]["schema"]["$ref"],
        format!("{SCHEMA_REF}Problem")
    );
    let retry = &many["headers"]["Retry-After"];
    assert!(retry.is_object(), "TooManyRequests documents Retry-After");
    assert_ne!(
        resolve(&d, retry)["required"],
        json!(true),
        "the stream limit and the access-request cooldown send no Retry-After, so it is optional on a 429"
    );

    let unavailable = &responses["ServiceUnavailable"];
    assert_eq!(
        unavailable["content"][PROBLEM]["schema"]["$ref"],
        format!("{SCHEMA_REF}Problem")
    );
    let retry = resolve(&d, &unavailable["headers"]["Retry-After"]);
    assert_eq!(
        retry["required"], true,
        "write_error(Unavailable) always sends Retry-After"
    );
    assert_eq!(resolve(&d, &retry["schema"])["type"], "integer");

    for (id, code, name) in [
        ("streamEvents", "429", "TooManyRequests"),
        ("revealValue", "429", "TooManyRequests"),
        ("createAccessRequest", "429", "TooManyRequests"),
        ("putGithubToken", "503", "ServiceUnavailable"),
    ] {
        let (_, _, op) = operation(&d, id);
        assert_eq!(
            op["responses"][code]["$ref"],
            format!("#/components/responses/{name}"),
            "{id} {code}"
        );
        // Only the answers are added: the role and the other responses stay.
        assert!(op["responses"].get("default").is_some(), "{id}");
    }
    // Everything else keeps `default` (the contract does not list 429 and 503 on every operation).
    let users: BTreeSet<String> = d["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(p, item)| {
            ["get", "put", "post", "delete", "patch"]
                .into_iter()
                .filter_map(move |m| item.get(m).map(|op| (p.clone(), m, op)))
        })
        .filter(|(_, _, op)| {
            op["responses"]
                .as_object()
                .unwrap()
                .iter()
                .any(|(code, r)| (code == "429" || code == "503") && r["$ref"].as_str().is_some())
        })
        .map(|(_, _, op)| op["operationId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        users,
        [
            "streamEvents",
            "revealValue",
            "createAccessRequest",
            "putGithubToken"
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect::<BTreeSet<_>>()
    );
}

/// The three identity conflicts never carry `current`: they point at a plain conflict, and no other operation moved.
#[test]
fn access_request_and_user_conflicts_are_plain_problems() {
    let d = doc();
    let state = &d["components"]["responses"]["ConflictState"];
    assert_eq!(
        state["content"][PROBLEM]["schema"]["$ref"],
        format!("{SCHEMA_REF}Problem"),
        "ConflictState is a plain Problem: no `current`"
    );
    assert_eq!(
        state["content"].as_object().unwrap().keys().collect::<Vec<_>>(),
        [PROBLEM]
    );
    for id in [
        "adminApproveAccessRequest",
        "adminRejectAccessRequest",
        "adminUpdateUser",
    ] {
        let (_, _, op) = operation(&d, id);
        assert_eq!(
            op["responses"]["409"]["$ref"], "#/components/responses/ConflictState",
            "{id} never sends `current`"
        );
    }

    // Who still uses `Conflict` (a hash was expected and a different one was found): eight operations, all of them
    // content writes or the approval of one. `createPr` left it for its own union; the three above for `ConflictState`.
    let mut users = BTreeSet::new();
    for (path, item) in d["paths"].as_object().unwrap() {
        for m in ["get", "put", "post", "delete", "patch"] {
            let Some(op) = item.get(m) else { continue };
            if op["responses"]["409"]["$ref"] == "#/components/responses/Conflict" {
                users.insert(format!("{} {path}", op["operationId"].as_str().unwrap()));
            }
        }
    }
    let ids: BTreeSet<&str> = users.iter().map(|s| s.split(' ').next().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "applyDraft",
            "approveProposal",
            "createDoc",
            "deleteFile",
            "editFile",
            "revertFile",
            "updateDoc",
            "uploadFile"
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
}

/// Every schema this commit adds is closed or open on purpose and bounded (S11).
#[test]
fn new_schemas_are_bounded() {
    let d = doc();
    let schemas = &d["components"]["schemas"];

    let dup = &schemas["DuplicatePrProblem"];
    let tp = &schemas["TokenMissingAccessProblem"]["allOf"][1];
    let code = &schemas["Problem"]["properties"]["code"];

    for (what, s) in [
        ("DuplicatePrProblem.code", &dup["properties"]["code"]),
        ("Problem.code", code),
    ] {
        assert!(
            s["maxLength"].as_u64().is_some_and(|m| m <= 64) || s["pattern"].is_string(),
            "{what} needs a bound"
        );
    }
    assert_eq!(dup["properties"]["reason"]["const"], "duplicate_pr");
    for (key, s) in dup["properties"].as_object().unwrap() {
        if s["type"] == "string" {
            assert!(
                s["maxLength"].is_u64() || s["pattern"].is_string() || s["const"].is_string(),
                "DuplicatePrProblem.{key} is an unbounded string"
            );
        }
    }
    for flag in ["base", "tenant"] {
        assert_eq!(
            tp["properties"][flag]["type"], "boolean",
            "TokenMissingAccessProblem.{flag}"
        );
    }
    // The headers of a throttle are bounded numbers.
    for (name, header) in [
        (
            "TooManyRequests",
            &d["components"]["responses"]["TooManyRequests"]["headers"]["Retry-After"],
        ),
        (
            "ServiceUnavailable",
            &d["components"]["responses"]["ServiceUnavailable"]["headers"]["Retry-After"],
        ),
    ] {
        let s = resolve(&d, &resolve(&d, header)["schema"]);
        assert_eq!(s["type"], "integer", "{name} Retry-After");
        assert!(s["maximum"].is_u64(), "{name} Retry-After needs a maximum");
    }
}
