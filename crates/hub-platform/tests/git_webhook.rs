//! T7 (S3, S4, S11, S18, D64): `POST /hooks/github`. The one unauthenticated HTTP route of the hub proves itself in a fixed
//! order: body cap, HMAC over the raw bytes (constant time, a bare 401), event type, replay guard, strict parse. Only then does it
//! queue a fetch, and it answers before the fetch happens.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use domain::{RepoKind, Secret};
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use hub_platform::git::fetch_queue::FetchQueue;
use hub_platform::git::metrics::{GitMetrics, WebhookOutcome};
use hub_platform::git::payload::RepoFullName;
use hub_platform::git::webhook::{self, MemoryDeliveryLog, WebhookSettings, WebhookState, verify_signature};
use hub_platform::{FakeSessionAuth, GuardedRouter, PROBLEM_CONTENT_TYPE};
use ports::fakes::FakeSecretSource;
use ports::{SecretError, SecretSource};
use support::fake_github::{Delivery, push_body, signature};
use tower::ServiceExt;

const SECRET: &str = "whsec_PlantedWebhookSecretForTheTest42";
const BASE_REPO: &str = "acme/configuration-base-saas";
const TENANT_REPO: &str = "acme/csp-tenant-data";
const BEFORE: &str = "89abcdef0123456789abcdef0123456789abcdef";
const AFTER: &str = "0123456789abcdef0123456789abcdef01234567";
const ZERO: &str = "0000000000000000000000000000000000000000";

struct Rig {
    router: Router,
    queue: Arc<FetchQueue>,
    deliveries: Arc<MemoryDeliveryLog>,
    metrics: GitMetrics,
}

/// A secret source that counts how often it is asked.
struct CountingSecrets {
    inner: FakeSecretSource,
    reads: AtomicU32,
}

#[async_trait]
impl SecretSource for CountingSecrets {
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(name).await
    }
}

fn rig_with(secrets: Arc<dyn SecretSource>, max_body: usize, queue_capacity: usize) -> Rig {
    let queue = FetchQueue::new(queue_capacity);
    let deliveries = Arc::new(MemoryDeliveryLog::default());
    let metrics = GitMetrics::default();
    let mut settings = WebhookSettings::new(
        "webhook-secret",
        vec![
            (RepoKind::Base, RepoFullName::parse(BASE_REPO).unwrap()),
            (RepoKind::Tenant, RepoFullName::parse(TENANT_REPO).unwrap()),
        ],
    );
    settings.max_body = max_body;
    let state = WebhookState::new(
        settings,
        secrets,
        deliveries.clone(),
        queue.clone(),
        metrics.clone(),
    );
    let router = webhook::mount(
        GuardedRouter::new(Arc::new(FakeSessionAuth::new()), None),
        Arc::new(state),
    );
    let (router, specs) = router.into_parts().unwrap();
    assert_eq!(specs.len(), 1);
    Rig {
        router,
        queue,
        deliveries,
        metrics,
    }
}

fn rig() -> Rig {
    rig_with(
        Arc::new(FakeSecretSource::new().with("webhook-secret", SECRET)),
        1024 * 1024,
        64,
    )
}

async fn send(rig: &Rig, request: Request<Body>) -> (StatusCode, String, Option<String>) {
    let response = rig.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned(), content_type)
}

fn push(delivery_id: &str, repo: &str, git_ref: &str) -> Delivery {
    Delivery::signed(
        SECRET,
        "push",
        delivery_id,
        push_body(repo, git_ref, BEFORE, AFTER),
    )
}

#[tokio::test]
async fn webhook_valid_push_queues_a_fetch_and_answers_202_without_waiting_for_it() {
    let rig = rig();
    let (status, body, _) = send(&rig, push("d-1", TENANT_REPO, "refs/heads/sit1").request()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.is_empty());
    let batch = rig.queue.next_batch().await;
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].repo, RepoKind::Tenant);
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::Queued), 1);
}

#[tokio::test]
async fn webhook_pushes_are_routed_to_the_repository_they_name_and_tags_count() {
    let rig = rig();
    send(&rig, push("d-1", BASE_REPO, "refs/heads/main").request()).await;
    send(&rig, push("d-2", TENANT_REPO, "refs/tags/v2").request()).await;
    send(
        &rig,
        push("d-3", "ACME/Csp-Tenant-Data", "refs/heads/sit2").request(),
    )
    .await;
    let mut batch = rig.queue.next_batch().await;
    batch.sort_by_key(|j| j.repo);
    assert_eq!(batch[0].repo, RepoKind::Base);
    assert_eq!(batch[1].repo, RepoKind::Tenant);
    assert_eq!(
        batch[1].targets.len(),
        2,
        "a tag and a branch, the repository name compared without case"
    );
}

#[tokio::test]
async fn webhook_bad_signature_is_401_and_does_nothing() {
    let rig = rig();
    let mut delivery = push("d-1", TENANT_REPO, "refs/heads/sit1");
    delivery.signature = signature("not-the-secret", &delivery.body);
    let (status, body, content_type) = send(&rig, delivery.request()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
    for secret_text in [SECRET, "not-the-secret", "sit1", TENANT_REPO] {
        assert!(
            !body.contains(secret_text),
            "the answer says nothing about why: {body}"
        );
    }
    assert!(rig.queue.is_empty(), "nothing was queued");
    assert_eq!(
        rig.deliveries.len(),
        0,
        "the delivery id was not recorded: a bad signature must not burn an id"
    );
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::BadSignature), 1);
}

#[tokio::test]
async fn webhook_missing_or_malformed_signature_header_is_401() {
    let rig = rig();
    let good = push("d-1", TENANT_REPO, "refs/heads/sit1");
    let hex_sig = good.signature.strip_prefix("sha256=").unwrap().to_owned();
    for bad in [
        None,
        Some(String::new()),
        Some("sha256=".to_owned()),
        Some("sha256=zz".repeat(32)),
        Some(format!("sha1={hex_sig}")),
        Some(hex_sig.clone()),
        Some(format!("sha256={}", &hex_sig[..62])),
        Some(format!("sha256={hex_sig}00")),
        Some(format!(" sha256={hex_sig}")),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/hooks/github")
            .header("x-github-event", "push")
            .header("x-github-delivery", "d-1");
        if let Some(value) = &bad {
            request = request.header("x-hub-signature-256", value.as_str());
        }
        let (status, _, _) = send(&rig, request.body(Body::from(good.body.clone())).unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad:?}");
    }
    assert!(rig.queue.is_empty());
}

#[tokio::test]
async fn webhook_requires_valid_hmac_over_the_exact_body() {
    let rig = rig();
    let mut delivery = push("d-1", TENANT_REPO, "refs/heads/sit1");
    // One byte of the body differs from what was signed.
    let last = delivery.body.len() - 2;
    delivery.body[last] ^= 0x01;
    let (status, _, _) = send(&rig, delivery.request()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(rig.queue.is_empty());
}

#[tokio::test]
async fn webhook_replayed_delivery_is_ignored() {
    let rig = rig();
    let delivery = push("same-delivery-id", TENANT_REPO, "refs/heads/sit1");
    let (first, _, _) = send(&rig, delivery.request()).await;
    assert_eq!(first, StatusCode::ACCEPTED);
    assert_eq!(rig.queue.next_batch().await.len(), 1);

    let (second, _, _) = send(&rig, delivery.request()).await;
    assert_eq!(
        second,
        StatusCode::ACCEPTED,
        "acknowledged, so GitHub does not retry"
    );
    assert!(rig.queue.is_empty(), "a replay does nothing");
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::Duplicate), 1);
    assert_eq!(rig.deliveries.len(), 1);
}

#[tokio::test]
async fn webhook_non_push_event_is_ignored() {
    let rig = rig();
    for (n, event) in [
        "issues",
        "create",
        "release",
        "workflow_run",
        "pull_request",
        "star",
    ]
    .into_iter()
    .enumerate()
    {
        let delivery = Delivery::signed(
            SECRET,
            event,
            &format!("e-{n}"),
            push_body(TENANT_REPO, "refs/heads/sit1", BEFORE, AFTER),
        );
        let (status, body, _) = send(&rig, delivery.request()).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{event}");
        assert!(body.is_empty());
    }
    assert!(rig.queue.is_empty(), "no event but push queues a fetch");
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::Ignored), 6);
}

#[tokio::test]
async fn webhook_event_header_must_be_present_and_ascii() {
    let rig = rig();
    let body = push_body(TENANT_REPO, "refs/heads/sit1", BEFORE, AFTER);
    let request = Request::builder()
        .method("POST")
        .uri("/hooks/github")
        .header("x-hub-signature-256", signature(SECRET, &body))
        .header("x-github-delivery", "d-1")
        .body(Body::from(body))
        .unwrap();
    let (status, _, _) = send(&rig, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webhook_ping_answers_200_and_queues_nothing() {
    let rig = rig();
    let delivery = Delivery::signed(
        SECRET,
        "ping",
        "p-1",
        br#"{"zen":"Keep it logically awesome.","hook_id":1}"#.to_vec(),
    );
    let (status, _, _) = send(&rig, delivery.request()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rig.queue.is_empty());
}

#[tokio::test]
async fn webhook_delivery_header_is_required_and_bounded() {
    let rig = rig();
    for bad in ["", "has space", "new\nline", &"a".repeat(129)] {
        let mut delivery = push("x", TENANT_REPO, "refs/heads/sit1");
        bad.clone_into(&mut delivery.id);
        let request = if bad.contains('\n') {
            // A header value cannot hold a newline; send none at all.
            Request::builder()
                .method("POST")
                .uri("/hooks/github")
                .header("x-github-event", "push")
                .header("x-hub-signature-256", &delivery.signature)
                .body(Body::from(delivery.body.clone()))
                .unwrap()
        } else {
            delivery.request()
        };
        let (status, _, _) = send(&rig, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    assert!(rig.queue.is_empty());
}

#[tokio::test]
async fn webhook_push_for_an_unknown_repository_or_a_deleted_ref_is_ignored() {
    let rig = rig();
    let (status, _, _) = send(
        &rig,
        push("d-1", "acme/some-other-repo", "refs/heads/main").request(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let deleted = Delivery::signed(
        SECRET,
        "push",
        "d-2",
        push_body(TENANT_REPO, "refs/heads/sit1", BEFORE, ZERO),
    );
    let (status, _, _) = send(&rig, deleted.request()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(rig.queue.is_empty());
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::NotApplicable), 2);
}

#[tokio::test]
async fn webhook_payload_fields_are_typed_so_a_valid_signature_over_garbage_is_400() {
    let rig = rig();
    for (n, body) in [
        &b"not json"[..],
        b"{}",
        br#"{"ref":"refs/heads/x","before":"a","after":"b","repository":{"full_name":"a/b"}}"#,
        br#"{"ref":"refs/heads/../x","before":"89abcdef0123456789abcdef0123456789abcdef","after":"0123456789abcdef0123456789abcdef01234567","repository":{"full_name":"a/b"}}"#,
    ]
    .into_iter()
    .enumerate()
    {
        let delivery = Delivery::signed(SECRET, "push", &format!("g-{n}"), body.to_vec());
        let (status, response, content_type) = send(&rig, delivery.request()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{n}");
        assert_eq!(content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
        assert!(!response.contains("not json") && !response.contains("../x"), "{response}");
    }
    assert!(rig.queue.is_empty());
}

#[tokio::test]
async fn oversized_webhook_body_is_413_before_the_secret_is_even_read() {
    let secrets = Arc::new(CountingSecrets {
        inner: FakeSecretSource::new().with("webhook-secret", SECRET),
        reads: AtomicU32::new(0),
    });
    let rig = rig_with(secrets.clone(), 1024, 64);
    let delivery = Delivery::signed(SECRET, "push", "big", vec![b' '; 2048]);
    let (status, _, content_type) = send(&rig, delivery.request()).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::TooLarge), 1);

    // A body that arrives in chunks without a Content-Length is cut at the cap too.
    let chunks =
        futures::stream::iter((0..8).map(|_| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b' '; 512]))));
    let request = Request::builder()
        .method("POST")
        .uri("/hooks/github")
        .header("x-github-event", "push")
        .header("x-github-delivery", "chunked")
        .header("x-hub-signature-256", signature(SECRET, b""))
        .body(Body::from_stream(chunks))
        .unwrap();
    let (status, _, _) = send(&rig, request).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn webhook_secret_that_cannot_be_read_is_503_and_nothing_is_queued() {
    let rig = rig_with(Arc::new(FakeSecretSource::new()), 1024 * 1024, 64);
    let (status, body, content_type) =
        send(&rig, push("d-1", TENANT_REPO, "refs/heads/sit1").request()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
    assert!(!body.contains("webhook-secret"), "{body}");
    assert!(rig.queue.is_empty());
    assert_eq!(rig.deliveries.len(), 0);
}

#[tokio::test]
async fn webhook_flood_of_pushes_is_coalesced_and_bounded_and_every_push_is_still_acknowledged() {
    let rig = rig_with(
        Arc::new(FakeSecretSource::new().with("webhook-secret", SECRET)),
        1024 * 1024,
        3,
    );
    for n in 0..10 {
        let delivery = push(&format!("f-{n}"), TENANT_REPO, &format!("refs/heads/sit{n}"));
        let (status, _, _) = send(&rig, delivery.request()).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }
    assert_eq!(rig.queue.len(), 3, "the queue holds at most its capacity");
    assert_eq!(rig.metrics.webhooks_total(WebhookOutcome::Queued), 3);
    assert_eq!(
        rig.metrics.webhooks_total(WebhookOutcome::Dropped),
        7,
        "dropped and counted: the poll catches them"
    );
    assert_eq!(rig.queue.dropped_total(), 7);
}

#[tokio::test]
async fn webhook_route_rejects_other_methods() {
    let rig = rig();
    let request = Request::builder()
        .method("GET")
        .uri("/hooks/github")
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = send(&rig, request).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn webhook_logs_hold_no_secret_signature_or_payload_text() {
    let (logs, _guard) = support::log_capture::LogCapture::install();
    let rig = rig();
    let body = serde_json::json!({
        "ref": "refs/heads/sit1",
        "before": BEFORE,
        "after": AFTER,
        "repository": {"full_name": TENANT_REPO},
        "pusher": {"name": "planted-pusher-name"},
        "commits": [{"message": "planted commit message with a password: hunter2"}],
    })
    .to_string()
    .into_bytes();
    let delivery = Delivery::signed(SECRET, "push", "log-1", body);
    let signature_text = delivery.signature.clone();
    send(&rig, delivery.request()).await;
    // A bad signature, a replay, an unparseable payload and a payload for a repository we do not mirror, on the same paths.
    let mut bad = push("log-2", TENANT_REPO, "refs/heads/sit1");
    bad.signature = signature("wrong", &bad.body);
    send(&rig, bad.request()).await;
    send(&rig, delivery.request()).await;
    send(
        &rig,
        Delivery::signed(SECRET, "push", "log-3", b"planted garbage".to_vec()).request(),
    )
    .await;
    send(
        &rig,
        push("log-4", "acme/planted-other-repository", "refs/heads/x").request(),
    )
    .await;

    let text = logs.text();
    assert!(
        text.contains("webhook_push_queued"),
        "the capture saw the handler: {text}"
    );
    for secret in [
        SECRET,
        &signature_text,
        "planted-pusher-name",
        "hunter2",
        "planted garbage",
        "planted-other-repository",
    ] {
        assert!(!text.contains(secret), "{secret:?} is in the logs: {text}");
    }
}

// ---------------------------------------------------------------------------------------------- the comparison

#[test]
fn webhook_signature_compare_is_constant_time() {
    let body = push_body(TENANT_REPO, "refs/heads/sit1", BEFORE, AFTER);
    let good = signature(SECRET, &body);
    assert!(verify_signature(SECRET.as_bytes(), &body, Some(&good)));

    // 1,000 near misses: every one is refused, whatever digit and position differs, and none panics.
    let digits: Vec<char> = good["sha256=".len()..].chars().collect();
    let mut refused = 0;
    for n in 0..1000usize {
        let mut near = digits.clone();
        let position = n % near.len();
        let replacement = if near[position] == 'a' { 'b' } else { 'a' };
        near[position] = replacement;
        if n % 7 == 0 {
            near.truncate(near.len() - (n % 5) - 1);
        }
        if n % 11 == 0 {
            near.push('0');
        }
        let header = format!("sha256={}", near.iter().collect::<String>());
        assert!(
            !verify_signature(SECRET.as_bytes(), &body, Some(&header)),
            "{header}"
        );
        refused += 1;
    }
    assert_eq!(refused, 1000);
    assert!(!verify_signature(SECRET.as_bytes(), &body, None));
    assert!(!verify_signature(b"another secret", &body, Some(&good)));

    // The comparison must not be an early-exit one: the verifier uses `subtle` and writes no `==` or `!=` of its own.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/webhook.rs")).unwrap();
    let start = source
        .find("pub fn verify_signature")
        .expect("verify_signature exists");
    let end = source[start..]
        .find("\n}\n")
        .map(|e| start + e)
        .expect("the function ends");
    let function = &source[start..end];
    assert!(
        function.contains("ct_eq"),
        "the digest is compared with subtle's ct_eq"
    );
    for forbidden in [" == ", " != ", ".eq(", ".ne(", "starts_with(", "contains("] {
        assert!(
            !function.contains(forbidden),
            "an early-exit comparison ({forbidden:?}) in verify_signature"
        );
    }
}
