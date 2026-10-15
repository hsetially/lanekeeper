//! T3 (S4, S11, S21): the write routes. Every route is guarded and agrees with `api/openapi.yaml`; a request is read into typed
//! values before the service sees it; the service's answers become the responses the contract promises; and a body is limited
//! before it is read. The first tests use a scripted service, so no database is needed; the last drives the real hub.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use domain::{
    ChangePreview, ContentHash, DeleteRequest, Draft, DraftId, EditRequest, Expected, NotifyRequest,
    PrRequest, PreviewChange, PreviewId, PreviewRequest, PreviewScope, RejectReason, RestartRequest,
    RevertRequest, Role, ServiceRef, Timestamp, UploadRequest, UserStatus, ValidationIssue, WriteCtx,
    WriteOutcome,
};
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use hub_platform::{FakeSessionAuth, GuardedRouter, RouteSpec, TEST_USER_HEADER};
use hub_writes::routes::{MAX_EDIT_BODY, WRITE_ROUTES, WriteState, mount};
use ports::conformance::route_guard::{
    Allowances, SpecOperation, assert_route_table, spec_operations_from_openapi,
};
use ports::{WriteError, WriteService};
use serde_json::{Value, json};
use tower::ServiceExt;

// -------------------------------------------------------------------------------------------------- a scripted service

/// What the routes asked of the service.
#[derive(Debug, Clone)]
enum Call {
    Edit(EditRequest),
    Upload(UploadRequest),
    Delete(DeleteRequest),
    Revert(RevertRequest),
    Restart(RestartRequest),
    Preview(PreviewRequest),
}

/// A write service that answers what it is told and remembers what it was asked, with the context of the call.
#[derive(Default)]
struct Scripted {
    calls: Mutex<Vec<(Call, WriteCtx)>>,
    reply: Mutex<Option<Result<WriteOutcome, WriteError>>>,
    preview: Mutex<Option<Result<ChangePreview, WriteError>>>,
}

impl std::fmt::Debug for Scripted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scripted").finish_non_exhaustive()
    }
}

impl Scripted {
    fn says(&self, reply: Result<WriteOutcome, WriteError>) {
        *self.reply.lock().unwrap_or_else(PoisonError::into_inner) = Some(reply);
    }

    fn answer(&self, call: Call, ctx: &WriteCtx) -> Result<WriteOutcome, WriteError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((call, ctx.clone()));
        self.reply
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| {
                Ok(WriteOutcome::Applied {
                    hash: None,
                    audit_id: domain::AuditId::from(1),
                    warnings: Vec::new(),
                })
            })
    }

    fn calls(&self) -> Vec<(Call, WriteCtx)> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl WriteService for Scripted {
    async fn edit(&self, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError> {
        self.answer(Call::Edit(r), ctx)
    }
    async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError> {
        self.answer(Call::Upload(r), ctx)
    }
    async fn delete(&self, ctx: &WriteCtx, r: DeleteRequest) -> Result<WriteOutcome, WriteError> {
        self.answer(Call::Delete(r), ctx)
    }
    async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError> {
        self.answer(Call::Revert(r), ctx)
    }
    async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError> {
        self.answer(Call::Restart(r), ctx)
    }
    async fn raise_pr(&self, _: &WriteCtx, _: PrRequest) -> Result<WriteOutcome, WriteError> {
        Err(WriteError::Unavailable)
    }
    async fn propose_draft(&self, _: &WriteCtx, _: EditRequest) -> Result<Draft, WriteError> {
        Err(WriteError::Unavailable)
    }
    async fn apply_draft(&self, _: &WriteCtx, _: DraftId) -> Result<WriteOutcome, WriteError> {
        Err(WriteError::Unavailable)
    }
    async fn preview_effect(&self, ctx: &WriteCtx, r: PreviewRequest) -> Result<ChangePreview, WriteError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Call::Preview(r), ctx.clone()));
        self.preview
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Ok(sample_preview()))
    }
    async fn notify(&self, _: &WriteCtx, _: NotifyRequest) -> Result<WriteOutcome, WriteError> {
        Err(WriteError::Unavailable)
    }
}

fn sample_preview() -> ChangePreview {
    ChangePreview {
        preview_id: PreviewId::parse("preview-token-0123456789").unwrap(),
        expires_at: Timestamp::from_unix_millis(1_000),
        scope: PreviewScope::EveryService,
        warnings: Vec::new(),
        changes: Vec::new(),
        changes_total: 0,
        effective_changed: false,
    }
}

fn auth() -> Arc<FakeSessionAuth> {
    Arc::new(
        FakeSessionAuth::new()
            .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
            .with_user("editor", Some(Role::Editor), UserStatus::Active)
            .with_user("operator", Some(Role::Operator), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending)
            .with_user("disabled", Some(Role::Admin), UserStatus::Disabled),
    )
}

fn app_with(service: Arc<dyn WriteService>) -> (Router, Vec<RouteSpec>) {
    let state = WriteState::new(service, 2 * 1024 * 1024);
    let (router, specs) = mount(GuardedRouter::new(auth(), None), state)
        .into_parts()
        .unwrap();
    (router, specs)
}

fn app(service: &Arc<Scripted>) -> Router {
    app_with(service.clone()).0
}

// --------------------------------------------------------------------------------------------------------- the requests

const SWIMLANE: &str = "sit1";
const FILES: &str = "/api/v1/swimlanes/sit1/files";

fn hash_of(text: &str) -> ContentHash {
    ports::content_hash(text.as_bytes())
}

struct Req {
    method: Method,
    uri: String,
    user: Option<&'static str>,
    key: Option<&'static str>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Req {
    fn new(method: Method, uri: &str) -> Self {
        Self {
            method,
            uri: uri.to_owned(),
            user: Some("editor"),
            key: Some("key-1"),
            content_type: None,
            body: Vec::new(),
        }
    }

    fn json(mut self, value: &Value) -> Self {
        self.content_type = Some("application/json".to_owned());
        self.body = serde_json::to_vec(value).unwrap();
        self
    }

    fn raw(mut self, content_type: &str, body: Vec<u8>) -> Self {
        self.content_type = Some(content_type.to_owned());
        self.body = body;
        self
    }

    fn user(mut self, user: Option<&'static str>) -> Self {
        self.user = user;
        self
    }

    fn key(mut self, key: Option<&'static str>) -> Self {
        self.key = key;
        self
    }

    fn build(self) -> Request<Body> {
        let mut b = Request::builder().method(self.method).uri(self.uri);
        if let Some(u) = self.user {
            b = b.header(TEST_USER_HEADER, u);
        }
        if let Some(k) = self.key {
            b = b.header("idempotency-key", k);
        }
        if let Some(ct) = self.content_type {
            b = b.header(header::CONTENT_TYPE, ct);
        }
        b.body(Body::from(self.body)).unwrap()
    }
}

struct Reply {
    status: StatusCode,
    headers: http::HeaderMap,
    body: Bytes,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn send(router: &Router, req: Req) -> Reply {
    let response = router.clone().oneshot(req.build()).await.unwrap();
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes(),
    }
}

fn edit_body() -> Value {
    json!({ "path": "a.yml", "expected": { "state": "hash", "hash": hash_of("x: 1\n").to_string() }, "content": "x: 2\n" })
}

const BOUNDARY: &str = "----lkTestBoundary";

/// A request of the form tests: what it is, where it goes, and its fields.
type Case<'a> = (&'a str, String, Vec<(&'a str, &'a [u8])>);

fn multipart(fields: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if *name == "file" {
            body.extend_from_slice(b"; filename=\"upload.bin\"\r\nContent-Type: application/octet-stream");
        }
        body.extend_from_slice(b"\r\n\r\n");
        body.extend_from_slice(value);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

fn upload_fields<'a>(
    path: &'a str,
    file: &'a [u8],
    extra: &[(&'a str, &'a [u8])],
) -> Vec<(&'a str, &'a [u8])> {
    let mut fields: Vec<(&str, &[u8])> = vec![
        ("path", path.as_bytes()),
        ("expected_state", b"absent"),
        ("file", file),
    ];
    fields.extend_from_slice(extra);
    fields
}

// ------------------------------------------------------------------------------------------------------- the route table

fn openapi_text() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml")).unwrap()
}

fn spec() -> Vec<SpecOperation> {
    spec_operations_from_openapi(&openapi_text()).unwrap()
}

/// The operations of the spec tagged `writes`, as (method, path).
fn write_operations() -> BTreeSet<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Doc {
        paths: BTreeMap<String, BTreeMap<String, Value>>,
    }
    let doc: Doc = serde_saphyr::from_str(&openapi_text()).unwrap();
    let mut out = BTreeSet::new();
    for (path, item) in doc.paths {
        for (method, op) in item {
            let tagged = op
                .get("tags")
                .and_then(Value::as_array)
                .is_some_and(|tags| tags.iter().any(|t| t == "writes"));
            if tagged {
                out.insert((method.to_uppercase(), path.clone()));
            }
        }
    }
    out
}

/// The `writes` operations that later tasks of prompt 06 mount, with the task. When one is mounted its line goes, and
/// `every_write_operation_is_mounted_or_waits_for_its_task` fails until it does.
const LATER: [(&str, &str, &str); 9] = [
    ("POST", "/api/v1/drafts", "T4"),
    ("POST", "/api/v1/drafts/{id}/apply", "T4"),
    ("GET", "/api/v1/proposals", "T4"),
    ("POST", "/api/v1/proposals/{id}/approve", "T4"),
    ("POST", "/api/v1/proposals/{id}/reject", "T4"),
    ("POST", "/api/v1/prs", "T9"),
    ("GET", "/api/v1/prs", "T9"),
    ("GET", "/api/v1/pr-jobs/{id}", "T9"),
    ("POST", "/api/v1/swimlanes/{id}/notify", "T11"),
];

#[test]
fn route_table_matches_openapi_for_writes() {
    let spec = spec();
    let (_, specs) = app_with(Arc::new(Scripted::default()));
    let mine: BTreeSet<(String, String)> = specs.iter().map(|r| (r.method.clone(), r.path.clone())).collect();
    let mut allow = Allowances::default();
    for op in &spec {
        if !mine.contains(&(op.method.clone(), op.path.clone())) {
            allow = allow.spec_only(&op.method, &op.path, "owned by another task or crate");
        }
    }
    let registered: Vec<_> = specs.iter().map(RouteSpec::registered).collect();
    assert_route_table(&registered, &spec, &allow);

    // And the list the crate publishes is the table it registers, role for role.
    let declared: BTreeSet<(String, String, Role)> = WRITE_ROUTES
        .iter()
        .map(|(m, p, r)| ((*m).to_owned(), (*p).to_owned(), *r))
        .collect();
    let mounted: BTreeSet<(String, String, Role)> = specs
        .iter()
        .map(|r| match &r.guard {
            hub_platform::RouteGuard::Role(role) => (r.method.clone(), r.path.clone(), *role),
            hub_platform::RouteGuard::Public { .. } => panic!("a write route is never public"),
        })
        .collect();
    assert_eq!(declared, mounted);
}

#[test]
fn every_write_operation_is_mounted_or_waits_for_its_task() {
    let mounted: BTreeSet<(String, String)> = WRITE_ROUTES
        .iter()
        .map(|(m, p, _)| ((*m).to_owned(), (*p).to_owned()))
        .collect();
    let later: BTreeSet<(String, String)> = LATER
        .iter()
        .map(|(m, p, _)| ((*m).to_owned(), (*p).to_owned()))
        .collect();
    assert!(
        mounted.is_disjoint(&later),
        "a route that is mounted is not waiting"
    );
    let all = write_operations();
    let accounted: BTreeSet<(String, String)> = mounted.union(&later).cloned().collect();
    assert_eq!(
        all, accounted,
        "a `writes` operation of the spec is neither mounted nor waiting for a task"
    );
}

#[tokio::test]
async fn viewer_gets_403_on_every_write_route() {
    let service = Arc::new(Scripted::default());
    let router = app(&service);
    let (ct, body) = multipart(&upload_fields("n.yml", b"x", &[]));
    for (method, template, _) in WRITE_ROUTES {
        let uri = template.replace("{id}", SWIMLANE);
        for (user, expected) in [
            (Some("viewer"), StatusCode::FORBIDDEN),
            (Some("pending"), StatusCode::FORBIDDEN),
            (Some("disabled"), StatusCode::FORBIDDEN),
            (None, StatusCode::UNAUTHORIZED),
        ] {
            let req = Req::new(method.parse().unwrap(), &uri)
                .user(user)
                .raw(&ct, body.clone());
            let reply = send(&router, req).await;
            assert_eq!(reply.status, expected, "{method} {template} as {user:?}");
        }
    }
    assert!(
        service.calls().is_empty(),
        "the guard answers before the handler or the service"
    );

    // An editor is below the floor of a restart; an operator is above the floor of every route.
    let restart = format!("/api/v1/swimlanes/{SWIMLANE}/restart");
    let as_editor = send(
        &router,
        Req::new(Method::POST, &restart).json(&json!({ "namespace": "sit1", "name": "payments" })),
    )
    .await;
    assert_eq!(as_editor.status, StatusCode::FORBIDDEN);
    let as_operator = send(
        &router,
        Req::new(Method::POST, &restart)
            .user(Some("operator"))
            .json(&json!({ "namespace": "sit1", "name": "payments" })),
    )
    .await;
    assert_eq!(as_operator.status, StatusCode::OK);
}

// ----------------------------------------------------------------------------------------------- answers become responses

#[tokio::test]
async fn an_edit_is_read_into_typed_values_and_the_applied_outcome_is_200() {
    let service = Arc::new(Scripted::default());
    service.says(Ok(WriteOutcome::Applied {
        hash: Some(hash_of("x: 2\n")),
        audit_id: domain::AuditId::from(7),
        warnings: vec![ValidationIssue {
            kind: domain::IssueKind::DuplicateKey,
            line: std::num::NonZeroU32::new(3),
            rule: None,
        }],
    }));
    let reply = send(&app(&service), Req::new(Method::PUT, FILES).json(&edit_body())).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({
            "outcome": "applied",
            "hash": hash_of("x: 2\n").to_string(),
            "audit_id": 7,
            "warnings": [{ "kind": "duplicate_key", "line": 3 }],
        })
    );
    let calls = service.calls();
    let (Call::Edit(r), ctx) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(r.swimlane.as_str(), SWIMLANE);
    assert_eq!(r.path.as_str(), "a.yml");
    assert_eq!(
        r.expected,
        Expected::Hash {
            hash: hash_of("x: 1\n")
        }
    );
    assert_eq!(r.content.as_ref(), b"x: 2\n");
    assert_eq!(r.change_eol, None);
    assert_eq!(ctx.idempotency_key.as_str(), "key-1");
    assert_eq!(ctx.via, domain::Via::Ui);
    assert!(ctx.request_id.as_str().starts_with("w-"));
    assert_eq!(ctx.user.role, Some(Role::Editor));

    service.says(Ok(WriteOutcome::ProposalCreated {
        id: domain::ProposalId::from(5),
    }));
    let reply = send(&app(&service), Req::new(Method::PUT, FILES).json(&edit_body())).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({ "outcome": "proposal_created", "id": 5 }));
}

#[tokio::test]
async fn outcomes_that_are_not_successes_are_problems_with_the_fields_the_contract_names() {
    let service = Arc::new(Scripted::default());
    let router = app(&service);
    let call = || async { send(&router, Req::new(Method::PUT, FILES).json(&edit_body())).await };

    service.says(Ok(WriteOutcome::Conflict {
        current: Some(hash_of("x: 9\n")),
    }));
    let r = call().await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(r.json()["current"], json!(hash_of("x: 9\n").to_string()));
    assert_eq!(r.json()["code"], "conflict");

    service.says(Ok(WriteOutcome::Conflict { current: None }));
    assert_eq!(call().await.json()["current"], Value::Null);

    service.says(Ok(WriteOutcome::Locked { until: None }));
    let r = call().await;
    assert_eq!(r.status, StatusCode::LOCKED);
    assert_eq!(r.json()["until"], Value::Null);
    assert_eq!(r.json()["code"], "locked");

    service.says(Ok(WriteOutcome::Rejected {
        reason: RejectReason::SecretDetected,
        issues: vec![ValidationIssue {
            kind: domain::IssueKind::SecretMatch,
            line: std::num::NonZeroU32::new(2),
            rule: domain::ShortText::parse("aws_access_key").ok(),
        }],
    }));
    let r = call().await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.json()["reason"], "secret_detected");
    assert_eq!(r.json()["issues"][0]["line"], 2);
    assert_eq!(r.json()["issues"][0]["rule"], "aws_access_key");

    for (reason, status) in [
        (RejectReason::TooLarge, StatusCode::PAYLOAD_TOO_LARGE),
        (RejectReason::NotAllowed, StatusCode::UNPROCESSABLE_ENTITY),
        (RejectReason::Denied, StatusCode::UNPROCESSABLE_ENTITY),
        (RejectReason::AgentUnavailable, StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        service.says(Ok(WriteOutcome::Rejected {
            reason,
            issues: Vec::new(),
        }));
        let r = call().await;
        assert_eq!(r.status, status, "{reason:?}");
        assert!(r.json().get("issues").is_none(), "no empty list of places");
    }

    for (error, status) in [
        (WriteError::Forbidden, StatusCode::FORBIDDEN),
        (WriteError::NotFound, StatusCode::NOT_FOUND),
        (WriteError::BadRequest, StatusCode::BAD_REQUEST),
        (WriteError::IdempotencyConflict, StatusCode::UNPROCESSABLE_ENTITY),
        (WriteError::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        service.says(Err(error));
        let r = call().await;
        assert_eq!(r.status, status, "{error:?}");
        assert_eq!(r.headers[header::CONTENT_TYPE], "application/problem+json");
        let body = r.json();
        // A fixed set of fields, and none of them is text from the service or the request.
        let keys: BTreeSet<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            BTreeSet::from(["code", "status", "title", "type"]),
            "{error:?}"
        );
        if error == WriteError::Unavailable {
            assert_eq!(r.headers[header::RETRY_AFTER], "1");
        }
    }
}

// ------------------------------------------------------------------------------------------------- reading the request

#[tokio::test]
async fn a_request_that_is_not_well_formed_never_reaches_the_service() {
    let service = Arc::new(Scripted::default());
    let router = app(&service);
    let ok = edit_body();
    let with = |key: &str, value: Value| {
        let mut v = ok.clone();
        v[key] = value;
        v
    };
    let mut bad: Vec<(&str, Req, StatusCode)> = vec![
        (
            "no idempotency key",
            Req::new(Method::PUT, FILES).key(None).json(&ok),
            StatusCode::BAD_REQUEST,
        ),
        (
            "a key with a space",
            Req::new(Method::PUT, FILES).key(Some("a key")).json(&ok),
            StatusCode::BAD_REQUEST,
        ),
        (
            "not json",
            Req::new(Method::PUT, FILES).raw("application/json", b"{nope".to_vec()),
            StatusCode::BAD_REQUEST,
        ),
        (
            "wrong media type",
            Req::new(Method::PUT, FILES).raw("text/plain", b"{}".to_vec()),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "no media type",
            Req::new(Method::PUT, FILES),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "unknown field",
            Req::new(Method::PUT, FILES).json(&with("sneaky", json!(1))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "traversal",
            Req::new(Method::PUT, FILES).json(&with("path", json!("../etc/passwd"))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "absolute path",
            Req::new(Method::PUT, FILES).json(&with("path", json!("/etc/passwd"))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "empty path",
            Req::new(Method::PUT, FILES).json(&with("path", json!(""))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "bad hash",
            Req::new(Method::PUT, FILES).json(&with("expected", json!({ "state": "hash", "hash": "abc" }))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "bad state",
            Req::new(Method::PUT, FILES).json(&with("expected", json!({ "state": "maybe" }))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "bad eol",
            Req::new(Method::PUT, FILES).json(&with("change_eol", json!("cr"))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "content not text",
            Req::new(Method::PUT, FILES).json(&with("content", json!(5))),
            StatusCode::BAD_REQUEST,
        ),
        (
            "no content",
            Req::new(Method::PUT, FILES).json(&json!({ "path": "a.yml", "expected": { "state": "absent" } })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "bad swimlane",
            Req::new(Method::PUT, "/api/v1/swimlanes/Bad_Swimlane/files").json(&ok),
            StatusCode::BAD_REQUEST,
        ),
        (
            "revert without target",
            Req::new(Method::POST, &format!("{FILES}/revert"))
                .json(&json!({ "path": "a.yml", "expected": { "state": "absent" } })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "restart with a bad name",
            Req::new(Method::POST, "/api/v1/swimlanes/sit1/restart")
                .user(Some("operator"))
                .json(&json!({ "namespace": "sit1", "name": "Not A Name" })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "restart with an unknown field",
            Req::new(Method::POST, "/api/v1/swimlanes/sit1/restart")
                .user(Some("operator"))
                .json(&json!({ "namespace": "sit1", "name": "payments", "force": true })),
            StatusCode::BAD_REQUEST,
        ),
    ];
    // Delete takes its request in the query.
    let hash = hash_of("x: 1\n").to_string();
    for (what, query) in [
        ("delete without a path", format!("expected={hash}")),
        ("delete without a hash", "path=a.yml".to_owned()),
        ("delete with a bad hash", "path=a.yml&expected=zz".to_owned()),
        (
            "delete with a traversal",
            format!("path=..%2Fa.yml&expected={hash}"),
        ),
        (
            "delete with a bad preview id",
            format!("path=a.yml&expected={hash}&preview_id=not%20a%20token"),
        ),
        (
            "delete with a stray parameter",
            format!("path=a.yml&expected={hash}&force=1"),
        ),
    ] {
        bad.push((
            what,
            Req::new(Method::DELETE, &format!("{FILES}?{query}")),
            StatusCode::BAD_REQUEST,
        ));
    }
    for (what, req, status) in bad {
        let r = send(&router, req).await;
        assert_eq!(r.status, status, "{what}");
        assert_eq!(
            r.headers[header::CONTENT_TYPE],
            "application/problem+json",
            "{what}"
        );
    }
    assert!(
        service.calls().is_empty(),
        "nothing reached the service: {:?}",
        service.calls()
    );
}

#[tokio::test]
async fn bodies_are_limited_before_they_are_read() {
    let service = Arc::new(Scripted::default());
    let router = app(&service);
    // A body over the limit of the edit route.
    let big = vec![b'a'; MAX_EDIT_BODY + 1];
    let r = send(&router, Req::new(Method::PUT, FILES).raw("application/json", big)).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Small JSON routes have small limits.
    let r = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/revert")).raw("application/json", vec![b' '; 32 * 1024]),
    )
    .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // A file over the size limit, in a form that would otherwise be fine.
    let file = vec![b'f'; 2 * 1024 * 1024 + 1];
    let (ct, body) = multipart(&upload_fields("n.yml", &file, &[]));
    let r = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/upload")).raw(&ct, body),
    )
    .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Exactly at the limit is read.
    let file = vec![b'f'; 2 * 1024 * 1024];
    let (ct, body) = multipart(&upload_fields("n.yml", &file, &[]));
    let r = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/upload")).raw(&ct, body),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(service.calls().len(), 1);
    // The guard comes first: nobody who is not allowed gets a body read at all.
    let r = send(
        &router,
        Req::new(Method::PUT, FILES)
            .user(Some("viewer"))
            .raw("application/json", vec![b'a'; MAX_EDIT_BODY + 1]),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_edit_with_every_escape_in_it_still_fits() {
    // 2 MiB of control characters is the worst case for the size of the JSON: each is `\u00XX`.
    let service = Arc::new(Scripted::default());
    let text = "\u{1}".repeat(2 * 1024 * 1024);
    let body = json!({ "path": "a.bin.txt", "expected": { "state": "absent" }, "content": text });
    let r = send(&app(&service), Req::new(Method::PUT, FILES).json(&body)).await;
    assert_eq!(r.status, StatusCode::OK);
    let calls = service.calls();
    let (Call::Edit(edit), _) = &calls[0] else {
        panic!()
    };
    assert_eq!(edit.content.len(), 2 * 1024 * 1024);
}

#[tokio::test]
async fn delete_and_revert_and_restart_read_their_requests() {
    let service = Arc::new(Scripted::default());
    let router = app(&service);
    let hash = hash_of("x: 1\n");

    let r = send(
        &router,
        Req::new(
            Method::DELETE,
            &format!("{FILES}?path=tx-api%2Fa.yml&expected={hash}&preview_id=preview-token-0123456789"),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(
        &router,
        Req::new(Method::DELETE, &format!("{FILES}?path=a.yml&expected={hash}")),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    let r = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/revert")).json(&json!({
            "path": "a.yml",
            "expected": { "state": "hash", "hash": hash.to_string() },
            "to": hash_of("old\n").to_string(),
            "change_eol": "crlf",
        })),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    let r = send(
        &router,
        Req::new(Method::POST, "/api/v1/swimlanes/sit1/restart")
            .user(Some("operator"))
            .json(&json!({ "namespace": "sit1", "name": "payments" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    let calls = service.calls();
    let (Call::Delete(d), _) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(d.path.as_str(), "tx-api/a.yml");
    assert_eq!(d.expected, hash);
    assert_eq!(
        d.preview.as_ref().map(PreviewId::as_str),
        Some("preview-token-0123456789")
    );
    let (Call::Delete(d), _) = &calls[1] else { panic!() };
    assert!(
        d.preview.is_none(),
        "a delete without a token reaches the service, which refuses it"
    );
    let (Call::Revert(v), _) = &calls[2] else { panic!() };
    assert_eq!(v.to, hash_of("old\n"));
    assert_eq!(v.change_eol, Some(domain::ChangeEol::Crlf));
    let (Call::Restart(x), _) = &calls[3] else {
        panic!()
    };
    assert_eq!(x.service, ServiceRef::new("sit1", "payments").unwrap());
}

#[tokio::test]
async fn an_upload_form_is_read_and_the_preview_form_too() {
    let service = Arc::new(Scripted::default());
    let router = app(&service);
    let hash = hash_of("x: 1\n").to_string();

    let png: Vec<u8> = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0, 255, b'\n'];
    let (ct, body) = multipart(&[
        ("path", b"img/logo.png"),
        ("expected_state", b"hash"),
        ("expected_hash", hash.as_bytes()),
        ("preview_id", b"preview-token-0123456789"),
        ("file", &png),
    ]);
    let r = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/upload")).raw(&ct, body),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    let (ct, body) = multipart(&[
        ("action", b"upload"),
        ("path", b"img/logo.png"),
        ("expected_state", b"absent"),
        ("file", &png),
    ]);
    let r = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/preview")).raw(&ct, body),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(r.json()["preview_id"], "preview-token-0123456789");

    let (ct, body) = multipart(&[
        ("action", b"delete"),
        ("path", b"a.yml"),
        ("expected_state", b"hash"),
        ("expected_hash", hash.as_bytes()),
    ]);
    let r = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/preview")).raw(&ct, body),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    let calls = service.calls();
    let (Call::Upload(u), _) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(u.path.as_str(), "img/logo.png");
    assert_eq!(u.content.as_ref(), &png[..], "the bytes arrive as they were sent");
    assert_eq!(
        u.expected,
        Expected::Hash {
            hash: hash_of("x: 1\n")
        }
    );
    assert!(u.preview.is_some());
    let (Call::Preview(p), _) = &calls[1] else {
        panic!()
    };
    assert!(
        matches!(&p.change, PreviewChange::Upload { expected: Expected::Absent, content } if content.as_ref() == &png[..])
    );
    let (Call::Preview(p), _) = &calls[2] else {
        panic!()
    };
    assert!(matches!(&p.change, PreviewChange::Delete { expected } if *expected == hash_of("x: 1\n")));
}

#[tokio::test]
async fn a_form_that_is_not_exactly_the_contracts_is_refused() {
    let service = Arc::new(Scripted::default());
    let router = app(&service);
    let hash = hash_of("x: 1\n").to_string();
    let upload = format!("{FILES}/upload");
    let preview = format!("{FILES}/preview");
    let file: &[u8] = b"x";

    let cases: Vec<Case<'_>> = vec![
        (
            "no file",
            upload.clone(),
            vec![("path", b"n.yml"), ("expected_state", b"absent")],
        ),
        (
            "no path",
            upload.clone(),
            vec![("expected_state", b"absent"), ("file", file)],
        ),
        (
            "no state",
            upload.clone(),
            vec![("path", b"n.yml"), ("file", file)],
        ),
        (
            "state hash without a hash",
            upload.clone(),
            vec![("path", b"n.yml"), ("expected_state", b"hash"), ("file", file)],
        ),
        (
            "state absent with a hash",
            upload.clone(),
            vec![
                ("path", b"n.yml"),
                ("expected_state", b"absent"),
                ("expected_hash", hash.as_bytes()),
                ("file", file),
            ],
        ),
        (
            "unknown state",
            upload.clone(),
            vec![("path", b"n.yml"), ("expected_state", b"maybe"), ("file", file)],
        ),
        ("traversal", upload.clone(), upload_fields("../x.yml", file, &[])),
        (
            "bad preview id",
            upload.clone(),
            upload_fields("n.yml", file, &[("preview_id", b"not a token!")]),
        ),
        (
            "an action in the upload form",
            upload.clone(),
            upload_fields("n.yml", file, &[("action", b"upload")]),
        ),
        (
            "unknown field",
            upload.clone(),
            upload_fields("n.yml", file, &[("sneaky", b"1")]),
        ),
        (
            "path twice",
            upload.clone(),
            upload_fields("n.yml", file, &[("path", b"o.yml")]),
        ),
        (
            "preview: no action",
            preview.clone(),
            vec![("path", b"n.yml"), ("expected_state", b"absent"), ("file", file)],
        ),
        (
            "preview: unknown action",
            preview.clone(),
            vec![
                ("action", b"move"),
                ("path", b"n.yml"),
                ("expected_state", b"absent"),
                ("file", file),
            ],
        ),
        (
            "preview: a delete of an absent file",
            preview.clone(),
            vec![
                ("action", b"delete"),
                ("path", b"n.yml"),
                ("expected_state", b"absent"),
            ],
        ),
        (
            "preview: a delete with a file",
            preview.clone(),
            vec![
                ("action", b"delete"),
                ("path", b"n.yml"),
                ("expected_state", b"hash"),
                ("expected_hash", hash.as_bytes()),
                ("file", file),
            ],
        ),
        (
            "preview: an upload without a file",
            preview.clone(),
            vec![
                ("action", b"upload"),
                ("path", b"n.yml"),
                ("expected_state", b"absent"),
            ],
        ),
        (
            "preview: a token",
            preview.clone(),
            vec![
                ("action", b"upload"),
                ("path", b"n.yml"),
                ("expected_state", b"absent"),
                ("file", file),
                ("preview_id", b"preview-token-0123456789"),
            ],
        ),
    ];
    for (what, uri, fields) in cases {
        let (ct, body) = multipart(&fields);
        let r = send(&router, Req::new(Method::POST, &uri).raw(&ct, body)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{what}");
    }
    // The wrong media type, and a body that is not a form at all.
    let r = send(&router, Req::new(Method::POST, &upload).json(&json!({}))).await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    for body in [
        b"".to_vec(),
        b"garbage".to_vec(),
        format!("--{BOUNDARY}\r\n").into_bytes(),
    ] {
        let r = send(
            &router,
            Req::new(Method::POST, &upload).raw(&format!("multipart/form-data; boundary={BOUNDARY}"), body),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    }
    assert!(service.calls().is_empty(), "{:?}", service.calls());
}

// ------------------------------------------------------------------------------------------------------- the real hub

#[tokio::test]
async fn the_routes_drive_the_real_hub_end_to_end() {
    let Some(db) = support::pg::TestDb::ready().await else {
        return;
    };
    let rig = support::rig::Rig::new(db).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    // The users of the session store are the directory's users.
    let mut editor = ports::conformance::sample::active(1, Role::Editor);
    editor.id = hub_platform::session_auth::user_id("editor");
    rig.add_user(&editor).await;

    let state = WriteState::new(Arc::new(rig.hub.clone()), rig.hub.max_content_bytes());
    let (router, _) = mount(GuardedRouter::new(auth(), None), state)
        .into_parts()
        .unwrap();

    // Edit.
    let r = send(
        &router,
        Req::new(Method::PUT, FILES).key(Some("e-1")).json(&edit_body()),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["hash"], hash_of("x: 2\n").to_string());
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    // The same key replays; a stale hash is a 409 with the current hash.
    let again = send(
        &router,
        Req::new(Method::PUT, FILES).key(Some("e-1")).json(&edit_body()),
    )
    .await;
    assert_eq!(again.json(), r.json());
    let stale = send(
        &router,
        Req::new(Method::PUT, FILES).key(Some("e-2")).json(&edit_body()),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.json()["current"], hash_of("x: 2\n").to_string());
    // The key for another request is a 422.
    let reused = send(
        &router,
        Req::new(Method::PUT, FILES)
            .key(Some("e-1"))
            .json(&json!({ "path": "a.yml", "expected": { "state": "absent" }, "content": "z\n" })),
    )
    .await;
    assert_eq!(reused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reused.json()["code"], "idempotency_conflict");

    // Preview, then upload with the token it gave; without it the upload is a 422 `not_allowed`.
    let (ct, body) = multipart(&[
        ("action", b"upload"),
        ("path", b"new.yml"),
        ("expected_state", b"absent"),
        ("file", b"n: 1\n"),
    ]);
    let pv = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/preview"))
            .key(Some("p-1"))
            .raw(&ct, body),
    )
    .await;
    assert_eq!(pv.status, StatusCode::OK, "{}", String::from_utf8_lossy(&pv.body));
    let token = pv.json()["preview_id"].as_str().unwrap().to_owned();
    let (ct, body) = multipart(&upload_fields("new.yml", b"n: 1\n", &[]));
    let refused = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/upload"))
            .key(Some("u-1"))
            .raw(&ct, body),
    )
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.json()["reason"], "not_allowed");
    let (ct, body) = multipart(&upload_fields(
        "new.yml",
        b"n: 1\n",
        &[("preview_id", token.as_bytes())],
    ));
    let up = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/upload"))
            .key(Some("u-1"))
            .raw(&ct, body),
    )
    .await;
    assert_eq!(up.status, StatusCode::OK, "{}", String::from_utf8_lossy(&up.body));
    assert_eq!(rig.nfs_of(&s, "new.yml").as_deref(), Some(&b"n: 1\n"[..]));

    // Delete with a token, in the query.
    let (ct, body) = multipart(&[
        ("action", b"delete"),
        ("path", b"new.yml"),
        ("expected_state", b"hash"),
        ("expected_hash", hash_of("n: 1\n").to_string().as_bytes()),
    ]);
    let pv = send(
        &router,
        Req::new(Method::POST, &format!("{FILES}/preview"))
            .key(Some("p-2"))
            .raw(&ct, body),
    )
    .await;
    let token = pv.json()["preview_id"].as_str().unwrap().to_owned();
    let del = send(
        &router,
        Req::new(
            Method::DELETE,
            &format!(
                "{FILES}?path=new.yml&expected={}&preview_id={token}",
                hash_of("n: 1\n")
            ),
        )
        .key(Some("d-1")),
    )
    .await;
    assert_eq!(
        del.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&del.body)
    );
    assert!(rig.nfs_of(&s, "new.yml").is_none());
    assert_eq!(
        rig.chain().await.len(),
        3,
        "an edit, an upload and a delete, one event each"
    );
}
