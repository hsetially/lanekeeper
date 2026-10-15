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
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use domain::{
    ChangePreview, ContentHash, DeleteRequest, Draft, DraftId, EditRequest, Expected, NotifyRequest, Paged,
    PrJobId, PrJobState, PrRequest, PrState, PreviewChange, PreviewId, PreviewRequest, PreviewScope,
    ProposalId, ProposalStatus, RejectReason, RestartRequest, RevertRequest, Role, ServiceRef, ShortText,
    Timestamp, UploadRequest, User, UserStatus, ValidationIssue, WriteCtx, WriteOutcome,
};
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use hub_platform::{FakeSessionAuth, GuardedRouter, RouteSpec, TEST_USER_HEADER};
use hub_writes::github::TokenShapeError;
use hub_writes::prs::{LinkQuery, PrDesk, PrJobView, PrLinkView, Raised};
use hub_writes::routes::{MAX_EDIT_BODY, WRITE_ROUTES, WriteState, mount};
use hub_writes::vault::{PolicyRepos, StoredToken, TokenDesk, TokenError, VaultDeps};
use hub_writes::{ProposalDesk, ProposalQuery, ProposalView, RejectResult};
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
    Draft(EditRequest),
    ApplyDraft(DraftId),
}

/// A write service that answers what it is told and remembers what it was asked, with the context of the call.
#[derive(Default)]
struct Scripted {
    calls: Mutex<Vec<(Call, WriteCtx)>>,
    reply: Mutex<Option<Result<WriteOutcome, WriteError>>>,
    preview: Mutex<Option<Result<ChangePreview, WriteError>>>,
    draft: Mutex<Option<Result<Draft, WriteError>>>,
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
    async fn propose_draft(&self, ctx: &WriteCtx, r: EditRequest) -> Result<Draft, WriteError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Call::Draft(r), ctx.clone()));
        self.draft
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or(Err(WriteError::Unavailable))
    }
    async fn apply_draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<WriteOutcome, WriteError> {
        self.answer(Call::ApplyDraft(id), ctx)
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

/// What the routes asked of the proposal desk.
#[derive(Debug, Clone)]
enum DeskCall {
    List(User, ProposalQuery),
    Approve(ProposalId, Option<ShortText>, WriteCtx),
    Reject(ProposalId, ShortText, WriteCtx),
}

/// A proposal desk that answers what it is told and remembers what it was asked.
#[derive(Default)]
struct ScriptedDesk {
    calls: Mutex<Vec<DeskCall>>,
    approve: Mutex<Option<Result<WriteOutcome, WriteError>>>,
    reject: Mutex<Option<Result<RejectResult, WriteError>>>,
}

impl std::fmt::Debug for ScriptedDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedDesk").finish_non_exhaustive()
    }
}

impl ScriptedDesk {
    fn calls(&self) -> Vec<DeskCall> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn push(&self, call: DeskCall) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }
}

/// What the routes asked of the token desk.
#[derive(Debug, Clone)]
enum TokenCall {
    Put(String, WriteCtx),
    Delete(WriteCtx),
}

/// A token desk that answers what it is told and remembers what it was asked.
#[derive(Default)]
struct ScriptedTokens {
    calls: Mutex<Vec<TokenCall>>,
    put: Mutex<Option<Result<StoredToken, TokenError>>>,
    delete: Mutex<Option<Result<(), TokenError>>>,
}

impl std::fmt::Debug for ScriptedTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedTokens").finish_non_exhaustive()
    }
}

impl ScriptedTokens {
    fn calls(&self) -> Vec<TokenCall> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn says_put(&self, reply: Result<StoredToken, TokenError>) {
        *self.put.lock().unwrap_or_else(PoisonError::into_inner) = Some(reply);
    }

    fn says_delete(&self, reply: Result<(), TokenError>) {
        *self.delete.lock().unwrap_or_else(PoisonError::into_inner) = Some(reply);
    }
}

/// What the routes asked of the pull-request desk.
#[derive(Debug, Clone)]
enum PrCall {
    Raise(PrRequest, WriteCtx),
    Job(User, PrJobId),
    Links(User, LinkQuery),
}

/// A pull-request desk that answers what it is told and remembers what it was asked.
#[derive(Default)]
struct ScriptedPrs {
    calls: Mutex<Vec<PrCall>>,
    started: Mutex<Vec<PrJobId>>,
    raise: Mutex<Option<Result<Raised, WriteError>>>,
    job: Mutex<Option<Result<Option<PrJobView>, WriteError>>>,
}

impl std::fmt::Debug for ScriptedPrs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedPrs").finish_non_exhaustive()
    }
}

impl ScriptedPrs {
    fn calls(&self) -> Vec<PrCall> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn started(&self) -> Vec<PrJobId> {
        self.started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn says_raise(&self, reply: Result<Raised, WriteError>) {
        *self.raise.lock().unwrap_or_else(PoisonError::into_inner) = Some(reply);
    }

    fn says_job(&self, reply: Result<Option<PrJobView>, WriteError>) {
        *self.job.lock().unwrap_or_else(PoisonError::into_inner) = Some(reply);
    }

    fn push(&self, call: PrCall) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }
}

fn sample_job(state: PrJobState) -> PrJobView {
    PrJobView {
        id: 7,
        swimlane: domain::SwimlaneId::parse("sit1").unwrap(),
        state,
        paths: vec![ports::conformance::sample::nfs("a.yml")],
        pr_url: (state == PrJobState::Completed)
            .then(|| "https://github.com/acme/csp-tenant-data/pull/3".to_owned()),
        error: None,
        warnings: Vec::new(),
        created_at: Timestamp::from_unix_millis(1_000),
        updated_at: Timestamp::from_unix_millis(2_000),
    }
}

fn sample_link() -> PrLinkView {
    PrLinkView {
        id: 11,
        swimlane: domain::SwimlaneId::parse("sit1").unwrap(),
        path: ports::conformance::sample::nfs("a.yml"),
        nfs_hash: hash_of("x: 1\n"),
        state: PrState::Open,
        pr_url: "https://github.com/acme/csp-tenant-data/pull/3".to_owned(),
        pr_number: 3,
        author: ports::conformance::sample::user_id(2),
        created_at: Timestamp::from_unix_millis(1_000),
        closed_at: None,
    }
}

#[async_trait]
impl PrDesk for ScriptedPrs {
    async fn raise(&self, ctx: &WriteCtx, r: PrRequest) -> Result<Raised, WriteError> {
        self.push(PrCall::Raise(r, ctx.clone()));
        self.raise
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Ok(Raised::Job(sample_job(PrJobState::Pending))))
    }

    async fn job(&self, user: &User, id: PrJobId) -> Result<Option<PrJobView>, WriteError> {
        self.push(PrCall::Job(user.clone(), id));
        self.job
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Ok(Some(sample_job(PrJobState::Opening))))
    }

    async fn links(&self, user: &User, q: LinkQuery) -> Result<Paged<PrLinkView>, WriteError> {
        self.push(PrCall::Links(user.clone(), q));
        Ok(Paged {
            items: vec![sample_link()],
            next: domain::Cursor::parse("11").ok(),
        })
    }

    fn start(&self, id: PrJobId) -> bool {
        self.started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(id);
        true
    }
}

fn sample_stored() -> StoredToken {
    StoredToken {
        github_login: "octocat".to_owned(),
        verified_at: Timestamp::from_unix_millis(1_791_936_000_000),
        scopes: vec![
            "push:acme/configuration-base-saas".to_owned(),
            "push:acme/csp-tenant-data".to_owned(),
        ],
        expires_at: Some(Timestamp::from_unix_millis(1_793_000_000_000)),
        expires_soon: true,
    }
}

#[async_trait]
impl TokenDesk for ScriptedTokens {
    async fn put_token(
        &self,
        ctx: &WriteCtx,
        token: domain::Secret<String>,
    ) -> Result<StoredToken, TokenError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(TokenCall::Put(token.expose().clone(), ctx.clone()));
        self.put
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Ok(sample_stored()))
    }

    async fn delete_token(&self, ctx: &WriteCtx) -> Result<(), TokenError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(TokenCall::Delete(ctx.clone()));
        (*self.delete.lock().unwrap_or_else(PoisonError::into_inner)).unwrap_or(Ok(()))
    }
}

fn sample_view(id: i64) -> ProposalView {
    ProposalView {
        id,
        author: ports::conformance::sample::user_id(2),
        swimlane: domain::SwimlaneId::parse("sit1").unwrap(),
        path: ports::conformance::sample::nfs("a.yml"),
        expected: Expected::Absent,
        new_hash: hash_of("x: 2\n"),
        status: ProposalStatus::Rejected,
        hunks: Vec::new(),
        setting_changes: Vec::new(),
        severity: None,
        created_at: Timestamp::from_unix_millis(1_000),
        expires_at: Timestamp::from_unix_millis(2_000),
        decided_by: Some(ports::conformance::sample::user_id(1)),
        decision_comment: ShortText::parse("not now").ok(),
    }
}

#[async_trait]
impl ProposalDesk for ScriptedDesk {
    async fn list_proposals(&self, user: &User, q: ProposalQuery) -> Result<Paged<ProposalView>, WriteError> {
        self.push(DeskCall::List(user.clone(), q));
        Ok(Paged {
            items: vec![sample_view(7)],
            next: domain::Cursor::parse("7").ok(),
        })
    }

    async fn approve_proposal(
        &self,
        ctx: &WriteCtx,
        id: ProposalId,
        comment: Option<ShortText>,
    ) -> Result<WriteOutcome, WriteError> {
        self.push(DeskCall::Approve(id, comment, ctx.clone()));
        self.approve
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or(Ok(WriteOutcome::Applied {
                hash: None,
                audit_id: domain::AuditId::from(1),
                warnings: Vec::new(),
            }))
    }

    async fn reject_proposal(
        &self,
        ctx: &WriteCtx,
        id: ProposalId,
        reason: ShortText,
    ) -> Result<RejectResult, WriteError> {
        self.push(DeskCall::Reject(id, reason, ctx.clone()));
        self.reject
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Ok(RejectResult::Rejected(Box::new(sample_view(7)))))
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
            .with_user("admin", Some(Role::Admin), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending)
            .with_user("disabled", Some(Role::Admin), UserStatus::Disabled),
    )
}

fn app_with(service: Arc<dyn WriteService>) -> (Router, Vec<RouteSpec>) {
    app_with_desk(service, Arc::new(ScriptedDesk::default()))
}

fn app_with_desk(service: Arc<dyn WriteService>, desk: Arc<dyn ProposalDesk>) -> (Router, Vec<RouteSpec>) {
    app_with_all(service, desk, Arc::new(ScriptedTokens::default()))
}

fn app_with_all(
    service: Arc<dyn WriteService>,
    desk: Arc<dyn ProposalDesk>,
    tokens: Arc<dyn TokenDesk>,
) -> (Router, Vec<RouteSpec>) {
    app_with_prs(service, desk, tokens, Arc::new(ScriptedPrs::default()))
}

fn app_with_prs(
    service: Arc<dyn WriteService>,
    desk: Arc<dyn ProposalDesk>,
    tokens: Arc<dyn TokenDesk>,
    prs: Arc<dyn PrDesk>,
) -> (Router, Vec<RouteSpec>) {
    let state = WriteState::new(service, desk, tokens, prs, 2 * 1024 * 1024);
    let (router, specs) = mount(GuardedRouter::new(auth(), None), state)
        .into_parts()
        .unwrap();
    (router, specs)
}

fn pr_app(prs: &Arc<ScriptedPrs>) -> Router {
    app_with_prs(
        Arc::new(Scripted::default()),
        Arc::new(ScriptedDesk::default()),
        Arc::new(ScriptedTokens::default()),
        prs.clone(),
    )
    .0
}

fn token_app(tokens: &Arc<ScriptedTokens>) -> Router {
    app_with_all(
        Arc::new(Scripted::default()),
        Arc::new(ScriptedDesk::default()),
        tokens.clone(),
    )
    .0
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
            // The token routes are tagged `me` in the spec (they are the signed-in user's), and are this crate's.
            let tagged = path == "/api/v1/me/github-token"
                || op
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
const LATER: [(&str, &str, &str); 1] = [("POST", "/api/v1/swimlanes/{id}/notify", "T11")];

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
    for (method, template, floor) in WRITE_ROUTES {
        // The id of a pull-request job is a number; every other `{id}` here is the swimlane.
        let uri = template.replace(
            "{id}",
            if template.contains("pr-jobs") {
                "7"
            } else {
                SWIMLANE
            },
        );
        for (user, expected) in [
            // The list of proposals is for a viewer too; every other route is above that.
            (
                Some("viewer"),
                if floor == Role::Viewer {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
            ),
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

/// serde reads a struct from a JSON array too (`["a.yml", ...]` is the fields in order), so a reader that only asks serde accepts a
/// body that is not the object the contract describes. The readers say so with the same word as for any other malformed body
/// (S11); a body that arrives as an array is refused by every JSON reader, whatever it holds.
#[test]
fn a_json_array_is_not_an_object_for_any_reader() {
    use hub_writes::routes::wire::{self, Refusal};

    let swimlane = || domain::SwimlaneId::parse("sit1").unwrap();
    let hash = hash_of("x: 1\n").to_string();
    let absent = json!({ "state": "absent" });
    let at_hash = json!({ "state": "hash", "hash": hash });
    let bytes = |v: &Value| serde_json::to_vec(v).unwrap();

    // Each reader, with the object it reads and the same values as an array in the order of the struct's fields. The object is
    // the control: it is read, so the refusal of the array is the shape's and not a bad value's.
    let edit = json!({ "path": "a.yml", "expected": at_hash, "content": "x: 2\n" });
    assert!(wire::edit_request(swimlane(), &bytes(&edit)).is_ok());
    assert_eq!(
        wire::edit_request(swimlane(), &bytes(&json!(["a.yml", at_hash, "x: 2\n", null]))).map(|_| ()),
        Err(Refusal::BadRequest),
        "edit"
    );

    let revert = json!({ "path": "a.yml", "expected": at_hash, "to": hash });
    assert!(wire::revert_request(swimlane(), &bytes(&revert)).is_ok());
    assert_eq!(
        wire::revert_request(swimlane(), &bytes(&json!(["a.yml", at_hash, hash, null]))).map(|_| ()),
        Err(Refusal::BadRequest),
        "revert"
    );

    let restart = json!({ "namespace": "sit1", "name": "payments" });
    assert!(wire::restart_request(swimlane(), &bytes(&restart)).is_ok());
    assert_eq!(
        wire::restart_request(swimlane(), &bytes(&json!(["sit1", "payments"]))).map(|_| ()),
        Err(Refusal::BadRequest),
        "restart"
    );

    let draft = json!({ "swimlane": "sit1", "path": "a.yml", "expected": absent, "content": "x: 2\n" });
    assert!(wire::draft_request(&bytes(&draft)).is_ok());
    assert_eq!(
        wire::draft_request(&bytes(&json!(["sit1", "a.yml", absent, "x: 2\n"]))).map(|_| ()),
        Err(Refusal::BadRequest),
        "draft"
    );

    assert!(wire::approve_comment(&bytes(&json!({ "comment": "ok" }))).is_ok());
    assert_eq!(
        wire::approve_comment(&bytes(&json!(["ok"]))).map(|_| ()),
        Err(Refusal::BadRequest),
        "approve"
    );

    assert!(wire::reject_reason(&bytes(&json!({ "reason": "no" }))).is_ok());
    assert_eq!(
        wire::reject_reason(&bytes(&json!(["no"]))).map(|_| ()),
        Err(Refusal::BadRequest),
        "reject"
    );

    // An array is no object with leading white space either, and an empty approval body is still "nothing at all".
    assert_eq!(
        wire::reject_reason(b" \r\n\t[\"no\"]").map(|_| ()),
        Err(Refusal::BadRequest)
    );
    assert!(wire::approve_comment(b"").is_ok());
}

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

    let state = WriteState::new(
        Arc::new(rig.hub.clone()),
        Arc::new(rig.hub.clone()),
        Arc::new(ScriptedTokens::default()),
        Arc::new(ScriptedPrs::default()),
        rig.hub.max_content_bytes(),
    );
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

// ---------------------------------------------------------------------------------------- approvals and drafts (T4)

const PROPOSALS: &str = "/api/v1/proposals";

fn desk_app() -> (Router, Arc<Scripted>, Arc<ScriptedDesk>) {
    let service = Arc::new(Scripted::default());
    let desk = Arc::new(ScriptedDesk::default());
    let (router, _) = app_with_desk(service.clone(), desk.clone());
    (router, service, desk)
}

#[tokio::test]
async fn approve_and_reject_are_for_admins_and_nobody_else() {
    let (router, _, desk) = desk_app();
    for template in [format!("{PROPOSALS}/7/approve"), format!("{PROPOSALS}/7/reject")] {
        for (user, expected) in [
            (Some("viewer"), StatusCode::FORBIDDEN),
            (Some("editor"), StatusCode::FORBIDDEN),
            (Some("operator"), StatusCode::FORBIDDEN),
            (Some("pending"), StatusCode::FORBIDDEN),
            (Some("disabled"), StatusCode::FORBIDDEN),
            (None, StatusCode::UNAUTHORIZED),
        ] {
            let r = send(
                &router,
                Req::new(Method::POST, &template)
                    .user(user)
                    .json(&json!({ "reason": "no" })),
            )
            .await;
            assert_eq!(r.status, expected, "{template} as {user:?}");
        }
    }
    assert!(
        desk.calls().is_empty(),
        "the guard answers before the handler or the desk"
    );
    let ok = send(
        &router,
        Req::new(Method::POST, &format!("{PROPOSALS}/7/approve")).user(Some("admin")),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
}

#[tokio::test]
async fn approve_reads_an_optional_comment_and_a_proposal_id() {
    let (router, _, desk) = desk_app();
    let uri = format!("{PROPOSALS}/7/approve");
    let admin = |req: Req| req.user(Some("admin"));

    // No body, a comment, and an explicit null comment.
    for (n, req) in [
        Req::new(Method::POST, &uri),
        Req::new(Method::POST, &uri).json(&json!({ "comment": "looks right" })),
        Req::new(Method::POST, &uri).json(&json!({ "comment": null })),
    ]
    .into_iter()
    .enumerate()
    {
        let r = send(&router, admin(req).key(Some(["k-1", "k-2", "k-3"][n]))).await;
        assert_eq!(r.status, StatusCode::OK, "{n}");
    }
    let calls = desk.calls();
    let comments: Vec<_> = calls
        .iter()
        .map(|c| match c {
            DeskCall::Approve(id, comment, ctx) => {
                assert_eq!(*id, ProposalId::from(7));
                assert_eq!(ctx.user.role, Some(Role::Admin));
                comment.as_ref().map(|c| c.as_str().to_owned())
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(comments, vec![None, Some("looks right".to_owned()), None]);

    // Refused before the desk: an unknown field, a body that is not JSON said to be, a body with no media type, no key.
    desk.calls.lock().unwrap().clear();
    for (what, req, expected) in [
        (
            "unknown field",
            Req::new(Method::POST, &uri).json(&json!({ "comment": "a", "extra": 1 })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "not json",
            Req::new(Method::POST, &uri).raw("application/json", b"{oops".to_vec()),
            StatusCode::BAD_REQUEST,
        ),
        (
            "no media type",
            Req::new(Method::POST, &uri).raw("text/plain", b"{}".to_vec()),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "comment with a control character",
            Req::new(Method::POST, &uri).json(&json!({ "comment": "a\u{0007}b" })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "no key",
            Req::new(Method::POST, &uri).key(None),
            StatusCode::BAD_REQUEST,
        ),
        (
            "not a number",
            Req::new(Method::POST, &format!("{PROPOSALS}/abc/approve")),
            StatusCode::BAD_REQUEST,
        ),
        (
            "zero",
            Req::new(Method::POST, &format!("{PROPOSALS}/0/approve")),
            StatusCode::BAD_REQUEST,
        ),
        (
            "negative",
            Req::new(Method::POST, &format!("{PROPOSALS}/-3/approve")),
            StatusCode::BAD_REQUEST,
        ),
        (
            "signed",
            Req::new(Method::POST, &format!("{PROPOSALS}/+3/approve")),
            StatusCode::BAD_REQUEST,
        ),
        (
            "too big",
            Req::new(Method::POST, &format!("{PROPOSALS}/99999999999999999999/approve")),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let r = send(&router, admin(req)).await;
        assert_eq!(r.status, expected, "{what}");
        // The problem says what failed and nothing of what was sent.
        assert!(!String::from_utf8_lossy(&r.body).contains("oops"), "{what}");
    }
    assert!(desk.calls().is_empty(), "{:?}", desk.calls());

    // A body over its limit is 413 without being read.
    let big = vec![b' '; hub_writes::routes::MAX_SMALL_BODY + 1];
    let r = send(
        &router,
        admin(Req::new(Method::POST, &uri).raw("application/json", big)),
    )
    .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn reject_needs_a_reason_and_answers_with_the_proposal() {
    let (router, _, desk) = desk_app();
    let uri = format!("{PROPOSALS}/7/reject");
    for (what, req, expected) in [
        (
            "no body",
            Req::new(Method::POST, &uri),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "no reason",
            Req::new(Method::POST, &uri).json(&json!({})),
            StatusCode::BAD_REQUEST,
        ),
        (
            "empty reason",
            Req::new(Method::POST, &uri).json(&json!({ "reason": "" })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "blank reason",
            Req::new(Method::POST, &uri).json(&json!({ "reason": "   " })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "unknown field",
            Req::new(Method::POST, &uri).json(&json!({ "reason": "a", "x": 1 })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "a reason of the wrong type",
            Req::new(Method::POST, &uri).json(&json!({ "reason": 5 })),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let r = send(&router, req.user(Some("admin"))).await;
        assert_eq!(r.status, expected, "{what}");
    }
    assert!(desk.calls().is_empty());

    let r = send(
        &router,
        Req::new(Method::POST, &uri)
            .user(Some("admin"))
            .json(&json!({ "reason": "not now" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let body = r.json();
    assert_eq!(body["id"], 7);
    assert_eq!(body["status"], "rejected");
    assert_eq!(body["decision_comment"], "not now");
    assert_eq!(body["path"], "a.yml");
    assert!(body["hunks"].is_array() && body["setting_changes"].is_array());
    assert!(body["severity"].is_null());
    match &desk.calls()[0] {
        DeskCall::Reject(id, reason, ctx) => {
            assert_eq!(*id, ProposalId::from(7));
            assert_eq!(reason.as_str(), "not now");
            assert_eq!(ctx.user.role, Some(Role::Admin));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_answers_of_a_decision_become_the_responses_the_contract_promises() {
    let (router, _, desk) = desk_app();
    let uri = format!("{PROPOSALS}/7/approve");
    let rej = format!("{PROPOSALS}/7/reject");
    let hash = hash_of("x: 9\n");
    for (answer, status) in [
        (
            Ok(WriteOutcome::Conflict { current: Some(hash) }),
            StatusCode::CONFLICT,
        ),
        (Ok(WriteOutcome::Locked { until: None }), StatusCode::LOCKED),
        (
            Ok(WriteOutcome::Rejected {
                reason: RejectReason::NotAllowed,
                issues: Vec::new(),
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (Err(WriteError::Forbidden), StatusCode::FORBIDDEN),
        (Err(WriteError::NotFound), StatusCode::NOT_FOUND),
        (
            Err(WriteError::IdempotencyConflict),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (Err(WriteError::Unavailable), StatusCode::SERVICE_UNAVAILABLE),
    ] {
        *desk.approve.lock().unwrap() = Some(answer.clone());
        let r = send(&router, Req::new(Method::POST, &uri).user(Some("admin"))).await;
        assert_eq!(r.status, status, "{answer:?}");
        if status == StatusCode::CONFLICT {
            assert_eq!(r.json()["current"], hash.to_string());
        }
        // A problem holds no internal detail.
        let text = String::from_utf8_lossy(&r.body).to_lowercase();
        assert!(!text.contains("sql") && !text.contains("postgres"), "{text}");
    }
    *desk.reject.lock().unwrap() = Some(Ok(RejectResult::Refused(WriteOutcome::Rejected {
        reason: RejectReason::NotAllowed,
        issues: Vec::new(),
    })));
    let r = send(
        &router,
        Req::new(Method::POST, &rej)
            .user(Some("admin"))
            .json(&json!({ "reason": "no" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    *desk.reject.lock().unwrap() = Some(Err(WriteError::Forbidden));
    let r = send(
        &router,
        Req::new(Method::POST, &rej)
            .user(Some("admin"))
            .json(&json!({ "reason": "no" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_list_of_proposals_reads_its_query_into_typed_values() {
    let (router, _, desk) = desk_app();
    let r = send(
        &router,
        Req::new(
            Method::GET,
            &format!("{PROPOSALS}?status=pending&swimlane=sit1&limit=10&cursor=42"),
        )
        .user(Some("viewer")),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["items"][0]["id"], 7);
    assert_eq!(r.json()["next"], "7");
    assert_eq!(
        r.headers.get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    match &desk.calls()[0] {
        DeskCall::List(user, q) => {
            assert_eq!(user.role, Some(Role::Viewer));
            assert_eq!(q.status, Some(ProposalStatus::Pending));
            assert_eq!(q.swimlane.as_ref().map(domain::SwimlaneId::as_str), Some("sit1"));
            assert_eq!(q.page.limit(), 10);
            assert_eq!(q.page.cursor().map(domain::Cursor::as_str), Some("42"));
        }
        other => panic!("{other:?}"),
    }
    // No query at all is the first page of the default size.
    let bare = send(&router, Req::new(Method::GET, PROPOSALS).user(Some("viewer"))).await;
    assert_eq!(bare.status, StatusCode::OK);
    match &desk.calls()[1] {
        DeskCall::List(_, q) => {
            assert_eq!(q.page.limit(), domain::Page::DEFAULT_LIMIT);
            assert!(q.status.is_none() && q.swimlane.is_none() && q.page.cursor().is_none());
        }
        other => panic!("{other:?}"),
    }
    desk.calls.lock().unwrap().clear();
    for query in [
        "limit=0",
        "limit=501",
        "limit=abc",
        "status=bogus",
        "swimlane=NOT%20A%20SLUG",
        "cursor=bad%20cursor",
        "page=2",
        "status=pending&status=applied",
    ] {
        let r = send(
            &router,
            Req::new(Method::GET, &format!("{PROPOSALS}?{query}")).user(Some("viewer")),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{query}");
    }
    assert!(desk.calls().is_empty());
}

#[tokio::test]
async fn draft_routes_read_a_draft_and_return_the_draft_and_the_outcome() {
    let (router, service, _) = desk_app();
    let draft_body = json!({
        "swimlane": "sit1",
        "path": "a.yml",
        "expected": { "state": "hash", "hash": hash_of("x: 1\n").to_string() },
        "content": "x: 2\n",
    });
    // The service has no draft to give: 503 (the scripted one answers Unavailable).
    let r = send(
        &router,
        Req::new(Method::POST, "/api/v1/drafts").json(&draft_body),
    )
    .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);

    let draft = Draft {
        id: DraftId::parse("d-1").unwrap(),
        author: ports::conformance::sample::user_id(1),
        swimlane: domain::SwimlaneId::parse("sit1").unwrap(),
        path: ports::conformance::sample::nfs("a.yml"),
        expected: Expected::Hash {
            hash: hash_of("x: 1\n"),
        },
        new_hash: hash_of("x: 2\n"),
        hunks: Vec::new(),
        setting_changes: Vec::new(),
        status: ProposalStatus::Draft,
        expires_at: Timestamp::from_unix_millis(5),
    };
    *service.draft.lock().unwrap() = Some(Ok(draft.clone()));
    let r = send(
        &router,
        Req::new(Method::POST, "/api/v1/drafts").json(&draft_body),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["id"], "d-1");
    assert_eq!(r.json()["status"], "draft");
    match &service.calls().last().unwrap().0 {
        Call::Draft(req) => {
            assert_eq!(req.swimlane.as_str(), "sit1");
            assert_eq!(req.path.as_str(), "a.yml");
            assert_eq!(req.content, Bytes::from_static(b"x: 2\n"));
            assert!(req.change_eol.is_none());
        }
        other => panic!("{other:?}"),
    }

    for (what, req, expected) in [
        ("unknown field", Req::new(Method::POST, "/api/v1/drafts").json(&json!({ "swimlane": "sit1", "path": "a.yml", "expected": { "state": "absent" }, "content": "x", "extra": 1 })), StatusCode::BAD_REQUEST),
        ("no swimlane", Req::new(Method::POST, "/api/v1/drafts").json(&json!({ "path": "a.yml", "expected": { "state": "absent" }, "content": "x" })), StatusCode::BAD_REQUEST),
        ("path out of the root", Req::new(Method::POST, "/api/v1/drafts").json(&json!({ "swimlane": "sit1", "path": "../etc/passwd", "expected": { "state": "absent" }, "content": "x" })), StatusCode::BAD_REQUEST),
        ("wrong media type", Req::new(Method::POST, "/api/v1/drafts").raw("text/plain", b"{}".to_vec()), StatusCode::UNSUPPORTED_MEDIA_TYPE),
        ("viewer", Req::new(Method::POST, "/api/v1/drafts").user(Some("viewer")).json(&draft_body), StatusCode::FORBIDDEN),
    ] {
        let before = service.calls().len();
        let r = send(&router, req).await;
        assert_eq!(r.status, expected, "{what}");
        assert_eq!(service.calls().len(), before, "{what}");
    }

    // Applying a draft: the id from the path, the key from the header, the service's outcome.
    let r = send(&router, Req::new(Method::POST, "/api/v1/drafts/d-1/apply")).await;
    assert_eq!(r.status, StatusCode::OK);
    match &service.calls().last().unwrap().0 {
        Call::ApplyDraft(id) => assert_eq!(id.as_str(), "d-1"),
        other => panic!("{other:?}"),
    }
    service.says(Ok(WriteOutcome::Conflict {
        current: Some(hash_of("x: 9\n")),
    }));
    let r = send(&router, Req::new(Method::POST, "/api/v1/drafts/d-1/apply")).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    for bad in ["d%201", "d%2F1", &"d".repeat(129)] {
        let r = send(
            &router,
            Req::new(Method::POST, &format!("/api/v1/drafts/{bad}/apply")),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let r = send(
        &router,
        Req::new(Method::POST, "/api/v1/drafts/d-1/apply").key(None),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(
        &router,
        Req::new(Method::POST, "/api/v1/drafts/d-1/apply").user(Some("viewer")),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_proposal_routes_drive_the_real_hub_end_to_end() {
    let Some(db) = support::pg::TestDb::ready().await else {
        return;
    };
    let rig = support::rig::Rig::new(db).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n")]);
    // The users of the session store are the directory's users: an editor who needs approval, and an admin.
    let mut careful = ports::conformance::sample::active(1, Role::Editor);
    careful.id = hub_platform::session_auth::user_id("editor");
    careful.requires_approval = true;
    rig.add_user(&careful).await;
    let mut admin = ports::conformance::sample::active(2, Role::Admin);
    admin.id = hub_platform::session_auth::user_id("admin");
    rig.add_user(&admin).await;
    for (n, name, role) in [(3, "viewer", Role::Viewer), (4, "operator", Role::Operator)] {
        let mut u = ports::conformance::sample::active(n, role);
        u.id = hub_platform::session_auth::user_id(name);
        rig.add_user(&u).await;
    }

    let state = WriteState::new(
        Arc::new(rig.hub.clone()),
        Arc::new(rig.hub.clone()),
        Arc::new(ScriptedTokens::default()),
        Arc::new(ScriptedPrs::default()),
        rig.hub.max_content_bytes(),
    );
    let (router, _) = mount(GuardedRouter::new(auth(), None), state)
        .into_parts()
        .unwrap();

    // The careful editor's edit becomes a proposal (200 with the outcome).
    let r = send(
        &router,
        Req::new(Method::PUT, FILES).key(Some("e-1")).json(&edit_body()),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["outcome"], "proposal_created");
    let id = r.json()["id"].as_i64().unwrap();
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 1\n"[..]));

    // Anyone signed in can look at the queue; it shows the proposal.
    let list = send(
        &router,
        Req::new(Method::GET, &format!("{PROPOSALS}?status=pending")).user(Some("viewer")),
    )
    .await;
    assert_eq!(
        list.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&list.body)
    );
    assert_eq!(list.json()["items"][0]["id"], id);
    assert_eq!(list.json()["items"][0]["path"], "a.yml");

    // The author cannot approve it; the editor role cannot either.
    let own = send(
        &router,
        Req::new(Method::POST, &format!("{PROPOSALS}/{id}/approve")).user(Some("operator")),
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN);

    // An admin does, and the file changes. The same key replays; another key finds it decided (422).
    let ok = send(
        &router,
        Req::new(Method::POST, &format!("{PROPOSALS}/{id}/approve"))
            .user(Some("admin"))
            .key(Some("a-1"))
            .json(&json!({ "comment": "ok" })),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", String::from_utf8_lossy(&ok.body));
    assert_eq!(ok.json()["outcome"], "applied");
    assert_eq!(ok.json()["hash"], hash_of("x: 2\n").to_string());
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
    let again = send(
        &router,
        Req::new(Method::POST, &format!("{PROPOSALS}/{id}/approve"))
            .user(Some("admin"))
            .key(Some("a-1"))
            .json(&json!({ "comment": "ok" })),
    )
    .await;
    assert_eq!(again.json(), ok.json());
    let decided = send(
        &router,
        Req::new(Method::POST, &format!("{PROPOSALS}/{id}/approve"))
            .user(Some("admin"))
            .key(Some("a-2")),
    )
    .await;
    assert_eq!(decided.status, StatusCode::UNPROCESSABLE_ENTITY);
    let missing = send(
        &router,
        Req::new(Method::POST, &format!("{PROPOSALS}/99999/approve"))
            .user(Some("admin"))
            .key(Some("a-3")),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let done = send(
        &router,
        Req::new(Method::GET, &format!("{PROPOSALS}?status=applied")).user(Some("viewer")),
    )
    .await;
    assert_eq!(done.json()["items"][0]["status"], "applied");
    assert_eq!(done.json()["items"][0]["decision_comment"], "ok");

    // A draft by the same route, then rejection of a second proposal.
    let draft = send(
        &router,
        Req::new(Method::POST, "/api/v1/drafts")
            .key(Some("d-1"))
            .json(&json!({
                "swimlane": "sit1",
                "path": "a.yml",
                "expected": { "state": "hash", "hash": hash_of("x: 2\n").to_string() },
                "content": "x: 3\n",
            })),
    )
    .await;
    assert_eq!(
        draft.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&draft.body)
    );
    let did = draft.json()["id"].as_str().unwrap().to_owned();
    let applied_draft = send(
        &router,
        Req::new(Method::POST, &format!("/api/v1/drafts/{did}/apply")).key(Some("ad-1")),
    )
    .await;
    assert_eq!(applied_draft.json()["outcome"], "proposal_created");
    let pid = applied_draft.json()["id"].as_i64().unwrap();
    let rejected = send(
        &router,
        Req::new(Method::POST, &format!("{PROPOSALS}/{pid}/reject"))
            .user(Some("admin"))
            .key(Some("r-1"))
            .json(&json!({ "reason": "later" })),
    )
    .await;
    assert_eq!(
        rejected.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&rejected.body)
    );
    assert_eq!(rejected.json()["status"], "rejected");
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
}

// ------------------------------------------------------------------------------------------------------- the GitHub token

const ME_TOKEN: &str = "/api/v1/me/github-token";

/// A well-formed fine-grained token. Not a real one.
const TOKEN: &str = "github_pat_11ROUTETEST0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHIJKLMNOP";

fn put_token(token: &str) -> Req {
    Req::new(Method::PUT, ME_TOKEN).json(&json!({ "token": token }))
}

#[tokio::test]
async fn putting_a_token_hands_it_to_the_vault_and_answers_without_it() {
    let tokens = Arc::new(ScriptedTokens::default());
    let reply = send(&token_app(&tokens), put_token(TOKEN)).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    // Exactly the fields the contract names: the expiry stays inside (the spec closes the object).
    assert_eq!(
        reply.json(),
        json!({
            "github_login": "octocat",
            "verified_at": Timestamp::from_unix_millis(1_791_936_000_000),
            "scopes": ["push:acme/configuration-base-saas", "push:acme/csp-tenant-data"],
        })
    );
    assert_eq!(reply.headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert!(!String::from_utf8_lossy(&reply.body).contains("github_pat_"));
    let calls = tokens.calls();
    assert_eq!(calls.len(), 1);
    let TokenCall::Put(token, ctx) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(token, TOKEN);
    assert_eq!(ctx.idempotency_key.as_str(), "key-1");
    assert_eq!(ctx.user.role, Some(Role::Editor));
    assert_eq!(ctx.via, domain::Via::Ui);
}

#[tokio::test]
async fn deleting_a_token_is_204_with_no_body() {
    let tokens = Arc::new(ScriptedTokens::default());
    let reply = send(&token_app(&tokens), Req::new(Method::DELETE, ME_TOKEN)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.body.is_empty());
    assert!(
        matches!(tokens.calls().as_slice(), [TokenCall::Delete(c)] if c.idempotency_key.as_str() == "key-1")
    );
}

#[tokio::test]
async fn classic_token_is_rejected_with_the_policy_message() {
    let tokens = Arc::new(ScriptedTokens::default());
    tokens.says_put(Err(TokenError::Shape(TokenShapeError::Classic)));
    let classic = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";
    let reply = send(&token_app(&tokens), put_token(classic)).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    let body = reply.json();
    assert_eq!(body["code"], "classic_token");
    let detail = body["detail"].as_str().unwrap();
    assert!(
        detail.contains("fine-grained") && detail.contains("policy"),
        "{detail}"
    );
    assert!(!String::from_utf8_lossy(&reply.body).contains("ghp_"));
}

#[tokio::test]
async fn every_token_error_is_a_fixed_problem() {
    let cases: [(TokenError, StatusCode, &str); 13] = [
        (TokenError::Forbidden, StatusCode::FORBIDDEN, "forbidden"),
        (
            TokenError::IdempotencyConflict,
            StatusCode::UNPROCESSABLE_ENTITY,
            "idempotency_conflict",
        ),
        (
            TokenError::Shape(TokenShapeError::Empty),
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            TokenError::Shape(TokenShapeError::TooLong),
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            TokenError::Shape(TokenShapeError::Classic),
            StatusCode::UNPROCESSABLE_ENTITY,
            "classic_token",
        ),
        (
            TokenError::Shape(TokenShapeError::NotFineGrained),
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_fine_grained_token",
        ),
        (
            TokenError::Shape(TokenShapeError::BadChars),
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_fine_grained_token",
        ),
        (
            TokenError::Rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "token_rejected",
        ),
        (
            TokenError::NoPushAccess {
                base: true,
                tenant: false,
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "token_missing_access",
        ),
        (
            TokenError::LoginMismatch,
            StatusCode::UNPROCESSABLE_ENTITY,
            "github_account_mismatch",
        ),
        (
            TokenError::GithubUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
        ),
        (
            TokenError::Unavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
        ),
        (TokenError::NoToken, StatusCode::NOT_FOUND, "not_found"),
    ];
    for (error, status, code) in cases {
        let tokens = Arc::new(ScriptedTokens::default());
        tokens.says_put(Err(error));
        let reply = send(&token_app(&tokens), put_token(TOKEN)).await;
        assert_eq!(reply.status, status, "{error:?}");
        assert_eq!(reply.json()["code"], code, "{error:?}");
        assert_eq!(
            reply.headers.get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        // The policy text names the prefix; it must not carry the token that was sent.
        assert!(
            !String::from_utf8_lossy(&reply.body).contains(&TOKEN["github_pat_".len()..]),
            "{error:?}"
        );
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert!(reply.headers.contains_key(header::RETRY_AFTER), "{error:?}");
        }
    }
    // The missing access says which repository is the problem, and nothing else.
    let tokens = Arc::new(ScriptedTokens::default());
    tokens.says_put(Err(TokenError::NoPushAccess {
        base: true,
        tenant: false,
    }));
    let body = send(&token_app(&tokens), put_token(TOKEN)).await.json();
    assert_eq!(
        (body["base"].clone(), body["tenant"].clone()),
        (json!(true), json!(false))
    );
    // And deleting maps the same way.
    let tokens = Arc::new(ScriptedTokens::default());
    tokens.says_delete(Err(TokenError::Forbidden));
    assert_eq!(
        send(&token_app(&tokens), Req::new(Method::DELETE, ME_TOKEN))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

/// A request that is not a token request is refused before the vault sees it, and the refusal does not repeat what was sent.
#[tokio::test]
async fn bad_token_requests_never_reach_the_vault_and_are_not_echoed() {
    let marker = "echo-marker-3c9e";
    let long = "a".repeat(513);
    let cases: Vec<(&str, Req, StatusCode)> = vec![
        ("no key", put_token(TOKEN).key(None), StatusCode::BAD_REQUEST),
        (
            "bad key",
            put_token(TOKEN).key(Some("a b")),
            StatusCode::BAD_REQUEST,
        ),
        (
            "not json",
            Req::new(Method::PUT, ME_TOKEN).raw("text/plain", TOKEN.as_bytes().to_vec()),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "broken json",
            Req::new(Method::PUT, ME_TOKEN).raw(
                "application/json",
                format!("{{\"token\": \"{marker}").into_bytes(),
            ),
            StatusCode::BAD_REQUEST,
        ),
        (
            "missing field",
            Req::new(Method::PUT, ME_TOKEN).json(&json!({})),
            StatusCode::BAD_REQUEST,
        ),
        (
            "unknown field",
            Req::new(Method::PUT, ME_TOKEN).json(&json!({ "token": TOKEN, "extra": marker })),
            StatusCode::BAD_REQUEST,
        ),
        (
            "token is a number",
            Req::new(Method::PUT, ME_TOKEN).json(&json!({ "token": 1 })),
            StatusCode::BAD_REQUEST,
        ),
        ("empty token", put_token(""), StatusCode::BAD_REQUEST),
        ("token over 512", put_token(&long), StatusCode::BAD_REQUEST),
        (
            "huge body",
            Req::new(Method::PUT, ME_TOKEN).raw("application/json", vec![b' '; 64 * 1024]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            "delete without a key",
            Req::new(Method::DELETE, ME_TOKEN).key(None),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (what, req, status) in cases {
        let tokens = Arc::new(ScriptedTokens::default());
        let reply = send(&token_app(&tokens), req).await;
        assert_eq!(
            reply.status,
            status,
            "{what}: {}",
            String::from_utf8_lossy(&reply.body)
        );
        assert!(tokens.calls().is_empty(), "{what}: the vault was called");
        let text = String::from_utf8_lossy(&reply.body);
        assert!(
            !text.contains(marker) && !text.contains("github_pat_"),
            "{what}: the request came back: {text}"
        );
    }
}

/// The token routes are the signed-in user's own: they need an editor, and a refusal comes before the body is read.
#[tokio::test]
async fn token_routes_need_an_editor_and_a_session() {
    let tokens = Arc::new(ScriptedTokens::default());
    let router = token_app(&tokens);
    for method in [Method::PUT, Method::DELETE] {
        for (user, status) in [
            (Some("viewer"), StatusCode::FORBIDDEN),
            (Some("pending"), StatusCode::FORBIDDEN),
            (Some("disabled"), StatusCode::FORBIDDEN),
            (None, StatusCode::UNAUTHORIZED),
            (Some("editor"), StatusCode::OK),
            (Some("operator"), StatusCode::OK),
            (Some("admin"), StatusCode::OK),
        ] {
            let req = if method == Method::PUT {
                put_token(TOKEN)
            } else {
                Req::new(Method::DELETE, ME_TOKEN)
            }
            .user(user);
            let reply = send(&router, req).await;
            let expected = if status == StatusCode::OK && method == Method::DELETE {
                StatusCode::NO_CONTENT
            } else {
                status
            };
            assert_eq!(reply.status, expected, "{method} as {user:?}");
        }
    }
    // Only the three allowed users per method reached the vault.
    assert_eq!(tokens.calls().len(), 6);
}

// ------------------------------------------------------------------------------------------- the token leaves no trace

/// A distinctive run inside a token, searched for in everything the system writes. The prefix is left out: the policy text names it.
fn body_of_token(token: &str) -> &str {
    &token["github_pat_".len()..]
}

/// A GitHub that answers by the token it is shown, and whose every error repeats the token back (a hostile or careless server).
fn echoing_github(tokens: &'static [(&'static str, &'static str, bool)]) -> support::fake_github::Handler {
    use support::fake_github::Reply;
    std::sync::Arc::new(move |seen, _| {
        let bearer = seen
            .header("authorization")
            .unwrap_or_default()
            .trim_start_matches("Bearer ")
            .to_owned();
        let echo = format!(r#"{{"message":"Bad credentials {bearer} {}"}}"#, seen.path);
        let Some((_, login, pushes_tenant)) = tokens.iter().find(|(t, _, _)| *t == bearer) else {
            return Reply::json(401, &echo);
        };
        if *login == "down" {
            return Reply::json(503, &echo);
        }
        if seen.path == "/user" {
            Reply::json(200, &format!(r#"{{"login":"{login}"}}"#)).with_header(
                "github-authentication-token-expiration",
                "2026-10-20 00:00:00 UTC",
            )
        } else if seen.path.ends_with("csp-tenant-data") && !pushes_tenant {
            Reply::json(404, &echo)
        } else {
            Reply::json(200, r#"{"permissions":{"push":true}}"#)
        }
    })
}

/// S8, S10, S21: whatever happens to a token, success or any refusal, it is in no log line, no error, no response, no audit event
/// and no stored row except as the ciphertext. The GitHub side is the real client against a server that echoes tokens back.
#[tokio::test]
async fn token_never_appears_in_logs_errors_audit_or_responses() {
    const GOOD: &str = "github_pat_11LEAKGOOD0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHIJKLMN";
    const GOOD2: &str = "github_pat_11LEAKGOOD2_9876543210zyxwvutsrqponmlkjihgfedcba_ABCDEFGHIJKLM";
    const UNKNOWN: &str = "github_pat_11LEAKUNKNOWN0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHIJK";
    const NO_TENANT: &str = "github_pat_11LEAKNOTENANT0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHIJ";
    const OTHER: &str = "github_pat_11LEAKOTHERLOGIN0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHI";
    const DOWN: &str = "github_pat_11LEAKGITHUBDOWN0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHI";
    const CLASSIC: &str = "ghp_LEAKCLASSIC0123456789abcdefghijklmnopqr";
    const BROKEN: &str = "github_pat_11LEAKBROKENJSON0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGH";
    const AUDITFAIL: &str = "github_pat_11LEAKAUDITFAIL0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHI";
    static ACCOUNTS: [(&str, &str, bool); 6] = [
        (GOOD, "octocat", true),
        (GOOD2, "octocat", true),
        (NO_TENANT, "octocat", false),
        (OTHER, "someone-else", true),
        (DOWN, "down", true),
        (AUDITFAIL, "octocat", true),
    ];

    let (capture, _guard) = support::log_capture::install();
    let db = db!();
    let rig = support::rig::Rig::new(db).await;
    let mut editor = ports::conformance::sample::active(1, Role::Editor);
    editor.id = hub_platform::session_auth::user_id("editor");
    rig.add_user(&editor).await;

    let server = support::fake_github::LocalGithub::start(echoing_github(&ACCOUNTS)).await;
    let client = hub_writes::github::GithubClient::for_tests(
        &server.base(),
        hub_writes::github::GithubClientConfig {
            attempt_timeout: std::time::Duration::from_millis(500),
            total_timeout: std::time::Duration::from_secs(5),
            backoff_base: std::time::Duration::from_millis(1),
            backoff_cap: std::time::Duration::from_millis(5),
            ..hub_writes::github::GithubClientConfig::default()
        },
    )
    .unwrap();
    let vault = Arc::new(rig.hub.token_vault(VaultDeps {
        kms: Arc::new(ports::fakes::FakeKmsEnvelope::new()),
        github: Arc::new(client),
        repos: PolicyRepos {
            base: hub_writes::github::RepoRef::parse("acme/configuration-base-saas").unwrap(),
            tenant: hub_writes::github::RepoRef::parse("acme/csp-tenant-data").unwrap(),
        },
    }));
    let state = WriteState::new(
        Arc::new(Scripted::default()),
        Arc::new(ScriptedDesk::default()),
        vault.clone(),
        Arc::new(ScriptedPrs::default()),
        2 * 1024 * 1024,
    );
    let (router, _) = mount(GuardedRouter::new(auth(), None), state)
        .into_parts()
        .unwrap();

    let mut seen_bodies: Vec<String> = Vec::new();
    let mut statuses = Vec::new();
    let mut go = |reply: Reply| {
        let mut text = String::from_utf8_lossy(&reply.body).into_owned();
        for (n, v) in &reply.headers {
            let _ = write!(text, "\n{n}: {}", v.to_str().unwrap_or_default());
        }
        seen_bodies.push(text);
        statuses.push(reply.status.as_u16());
    };

    // Each way a token can go, through the real routes, the real vault and the real client.
    go(send(&router, put_token(GOOD).key(Some("t-1"))).await); // stored
    go(send(&router, put_token(GOOD).key(Some("t-1"))).await); // replayed
    go(send(&router, put_token(GOOD2).key(Some("t-1"))).await); // same key, another token
    go(send(&router, put_token(CLASSIC).key(Some("t-2"))).await); // policy
    go(send(&router, put_token(UNKNOWN).key(Some("t-3"))).await); // GitHub says 401 and repeats the token
    go(send(&router, put_token(NO_TENANT).key(Some("t-4"))).await); // no push to the tenant repository
    go(send(&router, put_token(OTHER).key(Some("t-5"))).await); // another account than the linked one
    go(send(&router, put_token(DOWN).key(Some("t-6"))).await); // GitHub 503 repeating the token
    go(send(
        &router,
        Req::new(Method::PUT, ME_TOKEN)
            .key(Some("t-7"))
            .raw("application/json", format!(r#"{{"token":"{BROKEN}"#).into_bytes()),
    )
    .await); // broken JSON
    go(send(
        &router,
        Req::new(Method::PUT, ME_TOKEN)
            .key(Some("t-8"))
            .json(&json!({"token": BROKEN, "x": 1})),
    )
    .await); // unknown field
    rig.audit.fail_next();
    go(send(&router, put_token(AUDITFAIL).key(Some("t-9"))).await); // the audit log fails after the token was checked
    go(send(&router, put_token(GOOD2).key(Some("t-10"))).await); // replaced
    go(send(&router, Req::new(Method::DELETE, ME_TOKEN).key(Some("t-11"))).await);
    go(send(&router, put_token(GOOD).key(Some("t-12")).user(Some("viewer"))).await); // not allowed
    assert_eq!(
        statuses,
        [
            200, 200, 422, 422, 422, 422, 422, 503, 400, 400, 503, 200, 204, 403
        ],
        "the paths this test means to drive are not the ones it drove: {seen_bodies:?}"
    );

    // Use it once, inside a request, and drop it.
    let ctx = {
        let mut u = editor.clone();
        u.role = Some(Role::Editor);
        support::rig::ctx(&u, "use-1")
    };
    let _ = send(&router, put_token(GOOD).key(Some("t-13"))).await;
    let used = vault
        .with_token(&ctx, |t| -> hub_writes::vault::TokenFuture<'_, usize> {
            Box::pin(async move { t.expose().len() })
        })
        .await
        .unwrap();
    assert_eq!(used, GOOD.len());

    let everything = {
        let mut all = seen_bodies.join("\n");
        all.push_str(&capture.text());
        all.push_str(
            &serde_json::to_string(&rig.chain().await.iter().map(|e| &e.event).collect::<Vec<_>>()).unwrap(),
        );
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT encode(request_hash, 'hex'), coalesce(convert_from(response_body, 'UTF8'), '') FROM idempotency_keys",
        )
        .fetch_all(&rig.pool)
        .await
        .unwrap();
        for (a, b) in rows {
            all.push_str(&a);
            all.push_str(&b);
        }
        let users: Vec<Option<String>> = sqlx::query_scalar("SELECT github_login FROM users")
            .fetch_all(&rig.pool)
            .await
            .unwrap();
        let _ = write!(all, "{users:?}");
        let outbox: Vec<String> = sqlx::query_scalar("SELECT to_jsonb(o)::text FROM outbox o")
            .fetch_all(&rig.pool)
            .await
            .unwrap();
        all.push_str(&outbox.join("\n"));
        all
    };
    assert!(
        !everything.is_empty() && capture.text().len() > 100,
        "nothing was captured: the test would pass for any code"
    );
    for token in [
        GOOD, GOOD2, UNKNOWN, NO_TENANT, OTHER, DOWN, CLASSIC, BROKEN, AUDITFAIL,
    ] {
        let needle = body_of_token(token);
        assert!(
            !everything.contains(needle),
            "a token ({}...) reached a log, a response, the audit log or a stored row",
            &token[..16]
        );
        assert!(!everything.contains(token));
    }
    // Only the ciphertext is in the table.
    let sealed: Vec<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("SELECT wrapped_dek, ciphertext FROM github_credentials")
            .fetch_all(&rig.pool)
            .await
            .unwrap();
    for (wrapped, ciphertext) in sealed {
        for needle in [body_of_token(GOOD), body_of_token(GOOD2)] {
            assert!(
                !ciphertext
                    .windows(12)
                    .any(|w| needle.as_bytes().windows(12).any(|n| n == w))
            );
            assert!(
                !wrapped
                    .windows(12)
                    .any(|w| needle.as_bytes().windows(12).any(|n| n == w))
            );
        }
    }
    assert!(
        rig.chain().await.len() >= 3,
        "the audit events this test depends on were not written"
    );
}

// ------------------------------------------------------------------------------------------------ pull requests (T9)

const PRS: &str = "/api/v1/prs";

fn pr_body() -> Value {
    json!({
        "swimlane": "sit1",
        "paths": ["tx-api/a-sit1.yml", "tx-api/b.yml"],
        "title": "Sync the NFS changes",
        "body": "Please review.",
        "target": { "kind": "tenant", "branch": "sit1" },
    })
}

#[tokio::test]
async fn a_pull_request_is_accepted_as_a_job_with_202_and_started() {
    let prs = Arc::new(ScriptedPrs::default());
    let router = pr_app(&prs);
    let r = send(&router, Req::new(Method::POST, PRS).json(&pr_body())).await;
    assert_eq!(
        r.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(r.headers[header::CONTENT_TYPE], "application/json");
    let body = r.json();
    assert_eq!(body["id"], 7);
    assert_eq!(body["state"], "pending");
    assert_eq!(body["swimlane"], "sit1");
    assert_eq!(body["paths"], json!(["a.yml"]));
    assert_eq!(body["pr_url"], Value::Null);
    assert_eq!(body["error"], Value::Null);
    assert!(
        body.get("warnings").is_none(),
        "absent when there is nothing to say: {body}"
    );
    assert_eq!(
        prs.started(),
        vec![PrJobId::from(7)],
        "the job runs in the background"
    );

    // What the desk was asked: typed values, the key of the request, and the door.
    let calls = prs.calls();
    let [PrCall::Raise(request, ctx)] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(request.swimlane.as_str(), "sit1");
    assert_eq!(
        request
            .paths
            .iter()
            .map(domain::NfsPath::as_str)
            .collect::<Vec<_>>(),
        vec!["tx-api/a-sit1.yml", "tx-api/b.yml"]
    );
    assert_eq!(request.title.as_str(), "Sync the NFS changes");
    assert_eq!(request.body.as_deref(), Some("Please review."));
    assert_eq!(
        request.target,
        Some(domain::PrTarget::Tenant {
            branch: domain::TenantId::parse("sit1").unwrap()
        })
    );
    assert_eq!(ctx.idempotency_key.as_str(), "key-1");
    assert_eq!(ctx.via, domain::Via::Ui);

    // The target and the text are optional.
    let r = send(
        &router,
        Req::new(Method::POST, PRS).json(&json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let target_base = send(
        &router,
        Req::new(Method::POST, PRS).json(&json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "target": { "kind": "base" }, "body": null })),
    )
    .await;
    assert_eq!(target_base.status, StatusCode::ACCEPTED);

    // A retry that finds the job finished starts nothing.
    prs.says_raise(Ok(Raised::Job(sample_job(PrJobState::Completed))));
    let before = prs.started().len();
    let r = send(&router, Req::new(Method::POST, PRS).json(&pr_body())).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(
        r.json()["pr_url"],
        "https://github.com/acme/csp-tenant-data/pull/3"
    );
    assert_eq!(prs.started().len(), before);
}

#[tokio::test]
async fn a_second_pull_request_for_a_change_is_409_with_the_link_of_the_first() {
    let prs = Arc::new(ScriptedPrs::default());
    let router = pr_app(&prs);
    prs.says_raise(Ok(Raised::Duplicate(sample_link())));
    let r = send(&router, Req::new(Method::POST, PRS).json(&pr_body())).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.headers[header::CONTENT_TYPE], "application/problem+json");
    let body = r.json();
    assert_eq!(body["code"], "conflict");
    assert_eq!(body["reason"], "duplicate_pr");
    assert_eq!(body["link"]["id"], 11);
    assert_eq!(body["link"]["pr_number"], 3);
    assert_eq!(body["link"]["state"], "open");
    assert_eq!(
        body["link"]["pr_url"],
        "https://github.com/acme/csp-tenant-data/pull/3"
    );
    assert_eq!(body["link"]["path"], "a.yml");
    assert_eq!(body["link"]["closed_at"], Value::Null);
    assert!(prs.started().is_empty(), "nothing was accepted");
}

#[tokio::test]
async fn the_other_answers_to_a_pull_request_request_are_the_contracts() {
    let prs = Arc::new(ScriptedPrs::default());
    let router = pr_app(&prs);
    let go = || send(&router, Req::new(Method::POST, PRS).json(&pr_body()));

    // A user who requires approval: a proposal, as for any write.
    prs.says_raise(Ok(Raised::Proposal(ProposalId::from(9))));
    let r = go().await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({ "outcome": "proposal_created", "id": 9 }));

    // A request that cannot become a pull request.
    prs.says_raise(Ok(Raised::Refused(WriteOutcome::Rejected {
        reason: RejectReason::UnknownSwimlane,
        issues: Vec::new(),
    })));
    let r = go().await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.json()["reason"], "unknown_swimlane");

    for (error, status) in [
        (WriteError::Forbidden, StatusCode::FORBIDDEN),
        (WriteError::IdempotencyConflict, StatusCode::UNPROCESSABLE_ENTITY),
        (WriteError::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        prs.says_raise(Err(error));
        let r = go().await;
        assert_eq!(r.status, status, "{error:?}");
        let keys: BTreeSet<String> = r.json().as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            BTreeSet::from(["code", "status", "title", "type"].map(str::to_owned)),
            "{error:?}: a fixed problem"
        );
    }
}

#[tokio::test]
async fn a_pull_request_request_that_is_not_well_formed_never_reaches_the_desk() {
    let prs = Arc::new(ScriptedPrs::default());
    let router = pr_app(&prs);
    let many: Vec<String> = (0..=100).map(|i| format!("tx-api/f{i}.yml")).collect();
    let bad: Vec<(&str, Value)> = vec![
        (
            "no paths",
            json!({ "swimlane": "sit1", "paths": [], "title": "t" }),
        ),
        (
            "101 paths",
            json!({ "swimlane": "sit1", "paths": many, "title": "t" }),
        ),
        (
            "a path twice",
            json!({ "swimlane": "sit1", "paths": ["a.yml", "a.yml"], "title": "t" }),
        ),
        (
            "a path that climbs",
            json!({ "swimlane": "sit1", "paths": ["../etc/passwd"], "title": "t" }),
        ),
        (
            "an absolute path",
            json!({ "swimlane": "sit1", "paths": ["/etc/passwd"], "title": "t" }),
        ),
        (
            "a blank title",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "   " }),
        ),
        (
            "a title with a control character",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "a\nb" }),
        ),
        (
            "a long title",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "x".repeat(257) }),
        ),
        (
            "a text with a NUL",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "body": "a\u{0}b" }),
        ),
        (
            "a text over the limit",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "body": "x".repeat(65_537) }),
        ),
        (
            "an unknown field",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "draft": true }),
        ),
        (
            "an unknown target",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "target": { "kind": "fork" } }),
        ),
        (
            "a target with a field too many",
            json!({ "swimlane": "sit1", "paths": ["a.yml"], "title": "t", "target": { "kind": "base", "branch": "x" } }),
        ),
        (
            "a bad swimlane",
            json!({ "swimlane": "Sit 1", "paths": ["a.yml"], "title": "t" }),
        ),
        ("no swimlane", json!({ "paths": ["a.yml"], "title": "t" })),
        ("an array", json!(["sit1", ["a.yml"], "t"])),
    ];
    for (what, body) in &bad {
        let r = send(&router, Req::new(Method::POST, PRS).json(body)).await;
        assert_eq!(
            r.status,
            StatusCode::BAD_REQUEST,
            "{what}: {}",
            String::from_utf8_lossy(&r.body)
        );
    }
    // Not JSON at all, the wrong media type, no key, and a body over its limit.
    let r = send(
        &router,
        Req::new(Method::POST, PRS).raw("application/json", b"{".to_vec()),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(
        &router,
        Req::new(Method::POST, PRS).raw("text/plain", b"{}".to_vec()),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let r = send(&router, Req::new(Method::POST, PRS).key(None).json(&pr_body())).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(
        &router,
        Req::new(Method::POST, PRS).raw(
            "application/json",
            vec![b' '; hub_writes::routes::MAX_PR_BODY + 1],
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(prs.calls().is_empty(), "{:?}", prs.calls());

    // The largest request the contract allows fits: 100 paths and 65,536 characters of text, all of them escaped.
    let paths: Vec<String> = (0..100)
        .map(|i| format!("{}/f{i}.yml", "d".repeat(200)))
        .collect();
    let r = send(
        &router,
        Req::new(Method::POST, PRS).json(
            &json!({ "swimlane": "sit1", "paths": paths, "title": "t", "body": "\u{1}".repeat(65_536) }),
        ),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
}

#[tokio::test]
async fn a_pull_request_job_is_read_by_its_number() {
    let prs = Arc::new(ScriptedPrs::default());
    let router = pr_app(&prs);
    let r = send(
        &router,
        Req::new(Method::GET, "/api/v1/pr-jobs/7").user(Some("viewer")),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(r.json()["state"], "opening");
    let calls = prs.calls();
    assert!(
        matches!(calls.as_slice(), [PrCall::Job(who, id)] if *id == PrJobId::from(7) && who.id == hub_platform::session_auth::user_id("viewer")),
        "{calls:?}"
    );

    prs.says_job(Ok(None));
    let r = send(&router, Req::new(Method::GET, "/api/v1/pr-jobs/8")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    prs.says_job(Err(WriteError::Unavailable));
    let r = send(&router, Req::new(Method::GET, "/api/v1/pr-jobs/8")).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);

    let before = prs.calls().len();
    for id in ["0", "-1", "abc", "7x", "1.5", "9223372036854775808"] {
        let r = send(&router, Req::new(Method::GET, &format!("/api/v1/pr-jobs/{id}"))).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{id}");
    }
    assert_eq!(prs.calls().len(), before, "a bad id never reaches the desk");
}

#[tokio::test]
async fn the_pull_request_links_are_listed_by_swimlane_and_state_with_a_cursor() {
    let prs = Arc::new(ScriptedPrs::default());
    let router = pr_app(&prs);
    let r = send(
        &router,
        Req::new(
            Method::GET,
            "/api/v1/prs?swimlane=sit1&state=merged&cursor=40&limit=5",
        )
        .user(Some("viewer")),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    let body = r.json();
    assert_eq!(body["items"][0]["pr_number"], 3);
    assert_eq!(body["next"], "11");
    let calls = prs.calls();
    let [PrCall::Links(who, q)] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(
        who.id,
        hub_platform::session_auth::user_id("viewer"),
        "the caller as the session knows them"
    );
    assert_eq!(q.swimlane.as_ref().map(domain::SwimlaneId::as_str), Some("sit1"));
    assert_eq!(q.state, Some(PrState::Merged));
    assert_eq!(q.page.limit(), 5);
    assert_eq!(q.page.cursor().map(domain::Cursor::as_str), Some("40"));

    // Without a query: every swimlane and state, the default page.
    let r = send(&router, Req::new(Method::GET, PRS).user(Some("viewer"))).await;
    assert_eq!(r.status, StatusCode::OK);
    let calls = prs.calls();
    assert!(
        matches!(calls.last(), Some(PrCall::Links(_, q)) if q.state.is_none() && q.swimlane.is_none() && q.page.limit() == 100)
    );

    let before = prs.calls().len();
    for query in [
        "state=gone",
        "swimlane=Sit%201",
        "limit=0",
        "limit=501",
        "limit=x",
        "cursor=%00",
        "other=1",
    ] {
        let r = send(&router, Req::new(Method::GET, &format!("{PRS}?{query}"))).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{query}");
    }
    assert_eq!(prs.calls().len(), before);
}
