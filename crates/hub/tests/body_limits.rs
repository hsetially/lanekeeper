//! The request body caps in the composed hub (plan 18, T5, G8; S3, S11, S18; D137): 4 MiB everywhere, 1 MiB for the GitHub webhook,
//! and 12 MiB plus 64 KiB for the two edit routes (`PUT /files`, `POST /drafts`) **for a request that carries a live session**.
//!
//! The composition is the real one (`tests/support/identity_rig.rs`): the per-IP bucket, then `AuthLayer`, `CsrfLayer`, the body cap
//! and the routes, over a real Postgres. The probe module registers the paths and roles of the real routes (the cap table keys on the
//! matched path and the method) with handlers that only count the bytes they were given, so what is proved is the edge and nothing
//! of `hub-writes`. `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
//!
//! What is proved: the large cap is never given to a caller who has no live session (no cookie, a cookie that names no session, a
//! pending user, a Viewer where an Editor is needed), so an anonymous caller cannot make the hub buffer 12 MiB; the cap is exactly
//! the constant of `hub-writes` (a body at it passes and one byte more is `413`, with the declared length and without); every other
//! route keeps 4 MiB with a session; the webhook is 1 MiB with or without one; and the number of large bodies in flight is bounded
//! (`at_most_eight_large_bodies_are_read_at_once`, through `enforce_body_cap`: eight bodies of no declared length are held open, the
//! ninth large body is `429` with `Retry-After`, a body declared at 4 MiB or less is not counted, and a permit comes back when a body
//! ends). The unit test in `src/limits.rs` that takes permits off the semaphore by hand proves tokio's semaphore, not that bound.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

#[path = "../../hub-platform/tests/support/clock.rs"]
mod clock;
#[path = "../../hub-identity/tests/support/fake_https.rs"]
mod fake_https;
#[path = "../../hub-identity/tests/support/mock_oidc.rs"]
mod mock_oidc;
#[path = "support/identity_rig.rs"]
mod rig;
#[path = "support/secrets.rs"]
mod secrets;
#[path = "../examples/support/soft_kms.rs"]
mod soft_kms;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use domain::{Role, UserStatus};
use http::{Method, StatusCode};
use hub::Module;
use hub::limits::LARGE_BODIES_IN_FLIGHT;
use hub_platform::GuardedRouter;
use hub_writes::routes::{DRAFTS_PATH, FILES_PATH, MAX_EDIT_BODY, RESTART_PATH, UPLOAD_PATH};
use rig::{Cookies, IdentityRig, Options, Reply, Req, eventually, send, send_built};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

const MIB: usize = 1024 * 1024;
const GLOBAL: usize = 4 * MIB;
const WEBHOOK: usize = MIB;
const TOKEN_PATH: &str = "/api/v1/me/github-token";

/// The routes of the real paths and roles whose handlers only say how many bytes they were given and count their runs.
struct Probe {
    runs: Arc<AtomicUsize>,
}

impl Module for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        let mut router = router;
        for (method, path, role) in [
            (Method::PUT, FILES_PATH, Role::Editor),
            (Method::POST, DRAFTS_PATH, Role::Editor),
            (Method::POST, UPLOAD_PATH, Role::Editor),
            (Method::POST, RESTART_PATH, Role::Operator),
            (Method::PUT, TOKEN_PATH, Role::Editor),
        ] {
            let runs = Arc::clone(&self.runs);
            router = router.route(&method, path, role, move |body: Bytes| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    body.len().to_string()
                }
            });
        }
        let runs = Arc::clone(&self.runs);
        router.public(
            &Method::POST,
            "/hooks/github",
            "verified by an HMAC in the handler",
            move |body: Bytes| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    body.len().to_string()
                }
            },
        )
    }
}

struct Setup {
    rig: IdentityRig,
    runs: Arc<AtomicUsize>,
    editor: Cookies,
    operator: Cookies,
    viewer: Cookies,
    pending: Cookies,
}

async fn setup() -> Option<Setup> {
    let runs = Arc::new(AtomicUsize::new(0));
    let rig = IdentityRig::start(Options {
        modules: vec![Arc::new(Probe {
            runs: Arc::clone(&runs),
        })],
        ..Options::default()
    })
    .await?;
    let (_, editor) = rig.signed_in(1, Some(Role::Editor), UserStatus::Active).await;
    let (_, operator) = rig.signed_in(2, Some(Role::Operator), UserStatus::Active).await;
    let (_, viewer) = rig.signed_in(3, Some(Role::Viewer), UserStatus::Active).await;
    let (_, pending) = rig.signed_in(4, None, UserStatus::Pending).await;
    Some(Setup {
        rig,
        runs,
        editor,
        operator,
        viewer,
        pending,
    })
}

/// A JSON object whose text is exactly `total` bytes: `{"content":"aaa...a"}`.
fn json_of(total: usize) -> String {
    const FRAME: usize = r#"{"content":""}"#.len();
    format!(r#"{{"content":"{}"}}"#, "a".repeat(total - FRAME))
}

/// A state-changing request as a page makes it: the cookies, the token and the exact `Origin`.
fn as_page(method: Method, path: &str, cookies: &Cookies, body: &str) -> Req {
    Req::new(method, path)
        .with_token(cookies)
        .header("content-type", "application/json")
        .raw_body(body.to_owned())
}

/// The same request with no session at all.
fn anonymous(method: Method, path: &str, body: &str) -> Req {
    Req::new(method, path)
        .header("content-type", "application/json")
        .raw_body(body.to_owned())
}

fn files(id: &str) -> String {
    FILES_PATH.replace("{id}", id)
}

fn restart(id: &str) -> String {
    RESTART_PATH.replace("{id}", id)
}

fn upload(id: &str) -> String {
    UPLOAD_PATH.replace("{id}", id)
}

fn assert_too_large(reply: &Reply, what: &str) {
    assert_eq!(
        reply.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{what}: {}",
        reply.text()
    );
    assert_eq!(
        reply.problem_code().as_deref(),
        Some("payload_too_large"),
        "{what}: {}",
        reply.text()
    );
}

// ---------------------------------------------------------------------------------------------------------------- the edit routes

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_12_mib_cap_needs_a_live_session() {
    let Some(s) = setup().await else { return };
    let app = s.rig.app();
    let big = json_of(6 * MIB);

    // An Editor with a live session: a 6 MiB edit passes the edge and reaches the handler, on both edit routes.
    for (method, path) in [
        (Method::PUT, files("sit1")),
        (Method::POST, DRAFTS_PATH.to_owned()),
    ] {
        let reply = send(app, as_page(method.clone(), &path, &s.editor, &big)).await;
        assert_eq!(reply.status, StatusCode::OK, "{method} {path}: {}", reply.text());
        assert_eq!(reply.text(), big.len().to_string());
    }
    assert_eq!(s.runs.load(Ordering::SeqCst), 2);

    // The same body from callers who have no live session of the right standing is the global cap, and no handler runs.
    let garbage = rig::unknown_session_cookie();
    let cases: Vec<(&str, Req)> = vec![
        ("no cookie at all", anonymous(Method::PUT, &files("sit1"), &big)),
        (
            "a session cookie that names no session",
            anonymous(Method::PUT, &files("sit1"), &big).cookie("__Host-lk_session", &garbage),
        ),
        (
            "a pending user's live session",
            as_page(Method::PUT, &files("sit1"), &s.pending, &big),
        ),
        (
            "a Viewer's live session (the route needs an Editor)",
            as_page(Method::PUT, &files("sit1"), &s.viewer, &big),
        ),
        (
            "no cookie, the drafts route",
            anonymous(Method::POST, DRAFTS_PATH, &big),
        ),
        (
            "a Viewer, the drafts route",
            as_page(Method::POST, DRAFTS_PATH, &s.viewer, &big),
        ),
    ];
    for (what, req) in cases {
        let reply = send(app, req).await;
        assert_too_large(&reply, what);
    }
    assert_eq!(
        s.runs.load(Ordering::SeqCst),
        2,
        "no handler ran for a caller without a live session"
    );

    // A small body from the same callers is the guard's to refuse (401 and 403), so the cap is not a second authorization.
    let small = json_of(1024);
    let reply = send(app, anonymous(Method::PUT, &files("sit1"), &small)).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    let reply = send(app, as_page(Method::PUT, &files("sit1"), &s.viewer, &small)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_edit_cap_is_the_constant_of_hub_writes_and_not_a_byte_more() {
    let Some(s) = setup().await else { return };
    let app = s.rig.app();

    // Exactly at the cap passes, one byte over is 413: the body is JSON, which the edge reads whole.
    let at = json_of(MAX_EDIT_BODY);
    let reply = send(app, as_page(Method::PUT, &files("sit1"), &s.editor, &at)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.text(), MAX_EDIT_BODY.to_string());
    let over = json_of(MAX_EDIT_BODY + 1);
    let reply = send(app, as_page(Method::PUT, &files("sit1"), &s.editor, &over)).await;
    assert_too_large(&reply, "one byte over the edit cap, sent");

    // The declared length alone is enough to refuse it: the body is never read. This is the unauthenticated caller's case too:
    // the largest cap any route has is the most the first layer lets a declared length claim.
    let mut declared = Req::new(Method::PUT, &files("sit1"))
        .with_token(&s.editor)
        .header("content-type", "application/json")
        .header("content-length", &(MAX_EDIT_BODY + 1).to_string());
    let reply = send(app, declared.clone()).await;
    assert_too_large(&reply, "one byte over the edit cap, declared");
    declared = Req::new(Method::PUT, &files("sit1"))
        .header("content-type", "application/json")
        .header("content-length", &(MAX_EDIT_BODY + 1).to_string());
    let reply = send(app, declared).await;
    assert_too_large(&reply, "one byte over the edit cap, declared, no session");
    // And a declared length above the global cap and below the edit cap, with no session, is refused before the body is read as well.
    let reply = send(
        app,
        Req::new(Method::PUT, &files("sit1"))
            .header("content-type", "application/json")
            .header("content-length", &(8 * MIB).to_string()),
    )
    .await;
    assert_too_large(&reply, "8 MiB declared, no session");
}

// ---------------------------------------------------------------------------------------------------------------- everything else

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_other_route_keeps_4_mib_with_a_session() {
    let Some(s) = setup().await else { return };
    let app = s.rig.app();
    // An Operator is above the Editor floor and still gets 4 MiB on a route that is not one of the two: the cap follows the
    // route, not the person.
    let cases = [
        (Method::POST, restart("sit1"), &s.operator),
        (Method::POST, upload("sit1"), &s.editor),
        (Method::PUT, TOKEN_PATH.to_owned(), &s.editor),
        // The same file route with another method is not the edit route (the table keys on both).
        (Method::POST, files("sit1"), &s.editor),
    ];
    for (method, path, cookies) in &cases {
        let at = json_of(GLOBAL);
        let over = json_of(GLOBAL + 1);
        let probe_has_it = !(method == Method::POST && path.ends_with("/files"));
        if probe_has_it {
            let reply = send(app, as_page(method.clone(), path, cookies, &at)).await;
            assert_eq!(
                reply.status,
                StatusCode::OK,
                "{method} {path} at 4 MiB: {}",
                reply.text()
            );
        }
        let reply = send(app, as_page(method.clone(), path, cookies, &over)).await;
        assert_too_large(&reply, &format!("{method} {path} one byte over 4 MiB"));
    }
    // A route nobody registered: the fallback answers, and the cap is the global one there too.
    let reply = send(
        app,
        as_page(Method::POST, "/api/v1/nowhere", &s.editor, &json_of(GLOBAL + 1)),
    )
    .await;
    assert_too_large(&reply, "an unregistered path");
}

// ---------------------------------------------------------------------------------------------------------------- the webhook

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_webhook_body_is_capped_at_1_mib() {
    let Some(s) = setup().await else { return };
    let app = s.rig.app();
    let before = s.runs.load(Ordering::SeqCst);
    // No session: the webhook proves itself with an HMAC, and what it sends is far under a megabyte.
    let at = json_of(WEBHOOK);
    let reply = send(app, anonymous(Method::POST, "/hooks/github", &at)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let over = json_of(WEBHOOK + 1);
    let reply = send(app, anonymous(Method::POST, "/hooks/github", &over)).await;
    assert_too_large(&reply, "1 MiB + 1 to the webhook, JSON");
    // Not JSON: the body is streamed through a limit, and the handler that reads it gets the same refusal.
    let reply = send(
        app,
        Req::new(Method::POST, "/hooks/github")
            .header("content-type", "application/octet-stream")
            .raw_body("a".repeat(WEBHOOK + 1)),
    )
    .await;
    assert_too_large(&reply, "1 MiB + 1 to the webhook, not JSON");
    // The declared length is refused without reading the body.
    let reply = send(
        app,
        Req::new(Method::POST, "/hooks/github")
            .header("content-type", "application/json")
            .header("content-length", &(WEBHOOK + 1).to_string()),
    )
    .await;
    assert_too_large(&reply, "1 MiB + 1 to the webhook, declared");
    // A session does not make the webhook's cap larger.
    let reply = send(app, as_page(Method::POST, "/hooks/github", &s.editor, &over)).await;
    assert_too_large(&reply, "1 MiB + 1 to the webhook, with an Editor's session");
    assert_eq!(
        s.runs.load(Ordering::SeqCst),
        before + 1,
        "only the body at the cap ran"
    );
}

// ---------------------------------------------------------------------------------------------------------------- bodies in flight

/// A request whose body has no declared length and does not end until the test lets it: the hub reads it, and holds the permit that
/// the bound on large bodies hands out, for as long as it stays open.
struct HeldBody {
    /// Dropping this ends the body.
    end: mpsc::Sender<Result<Bytes, Infallible>>,
    reply: tokio::task::JoinHandle<Reply>,
}

/// The one chunk a held body sends before it waits: a complete JSON object, which the probe handler counts and does not parse.
const HELD_CHUNK: &str = r#"{"content":"held"}"#;

/// Send `req` with a streamed body. `started` counts the bodies whose first chunk the hub has taken, and the hub takes a chunk only
/// when it reads the body, which `enforce_body_cap` does after it has the permit for a large body (no layer before it reads a body:
/// the per-IP bucket looks at the declared length, `AuthLayer` at the cookie, `CsrfLayer` at two headers).
fn hold_open(app: &Router, req: &Req, started: &Arc<AtomicUsize>) -> HeldBody {
    let (end, chunks) = mpsc::channel(1);
    end.try_send(Ok(Bytes::from_static(HELD_CHUNK.as_bytes())))
        .unwrap();
    let counted = Arc::clone(started);
    let stream = ReceiverStream::new(chunks).map(move |chunk| {
        counted.fetch_add(1, Ordering::SeqCst);
        chunk
    });
    let request = req.build_with_body(Body::from_stream(stream));
    let app = app.clone();
    HeldBody {
        end,
        reply: tokio::spawn(async move { send_built(&app, request).await }),
    }
}

async fn wait_for_started(started: &Arc<AtomicUsize>, count: usize) {
    eventually(
        Duration::from_secs(20),
        &format!("{count} held bodies to be read by the hub"),
        || {
            let started = Arc::clone(started);
            async move { started.load(Ordering::SeqCst) >= count }
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn at_most_eight_large_bodies_are_read_at_once() {
    let Some(s) = setup().await else { return };
    let app = s.rig.app();
    assert_eq!(
        LARGE_BODIES_IN_FLIGHT, 8,
        "the name of this test and D137 say eight"
    );
    let started = Arc::new(AtomicUsize::new(0));
    let files_path = files("sit1");
    // The Editor's own, eligible request: a live session on an edit route, JSON, and no `Content-Length`.
    let streamed = || {
        Req::new(Method::PUT, &files_path)
            .with_token(&s.editor)
            .header("content-type", "application/json")
    };

    // Eight bodies are open and the hub is reading each of them.
    let mut held: Vec<HeldBody> = (0..LARGE_BODIES_IN_FLIGHT)
        .map(|_| hold_open(app, &streamed(), &started))
        .collect();
    wait_for_started(&started, LARGE_BODIES_IN_FLIGHT).await;
    let runs_before = s.runs.load(Ordering::SeqCst);

    // The ninth large body, in each of the ways a body is large (no declared length; declared above 4 MiB; on the other edit route)
    // is told to come back, and its handler does not run.
    let five = json_of(GLOBAL + MIB);
    for (what, req) in [
        (
            "5 MiB, no declared length",
            as_page(Method::PUT, &files_path, &s.editor, &five),
        ),
        (
            "5 MiB, declared",
            as_page(Method::PUT, &files_path, &s.editor, &five)
                .header("content-length", &five.len().to_string()),
        ),
        (
            "4 MiB + 1 declared",
            as_page(Method::PUT, &files_path, &s.editor, &json_of(GLOBAL + 1))
                .header("content-length", &(GLOBAL + 1).to_string()),
        ),
        (
            "the drafts route, no declared length",
            as_page(Method::POST, DRAFTS_PATH, &s.editor, &five),
        ),
        (
            "an Operator's, no declared length",
            as_page(Method::PUT, &files_path, &s.operator, &five),
        ),
    ] {
        let reply = send(app, req).await;
        assert_eq!(
            reply.status,
            StatusCode::TOO_MANY_REQUESTS,
            "{what}: {}",
            reply.text()
        );
        assert_eq!(reply.header("retry-after"), Some("1"), "{what}");
        assert_eq!(
            reply.problem_code().as_deref(),
            Some("too_many_requests"),
            "{what}"
        );
    }
    assert_eq!(
        s.runs.load(Ordering::SeqCst),
        runs_before,
        "a handler ran for a body that was told to come back"
    );

    // What is not counted goes through while the eight are open: a body declared at 4 MiB or less (at the cap, and a small one) on
    // the very route the eight are on, and a body of no declared length on a route that has no larger cap.
    let at_cap = json_of(GLOBAL);
    let small = json_of(1024);
    for (what, req, len) in [
        (
            "declared at exactly 4 MiB",
            as_page(Method::PUT, &files_path, &s.editor, &at_cap)
                .header("content-length", &at_cap.len().to_string()),
            at_cap.len(),
        ),
        (
            "declared at 1 KiB",
            as_page(Method::PUT, &files_path, &s.editor, &small)
                .header("content-length", &small.len().to_string()),
            small.len(),
        ),
        (
            "1 KiB, no declared length, on a route with the global cap",
            as_page(Method::POST, &restart("sit1"), &s.operator, &small),
            small.len(),
        ),
    ] {
        let reply = send(app, req).await;
        assert_eq!(reply.status, StatusCode::OK, "{what}: {}", reply.text());
        assert_eq!(reply.text(), len.to_string(), "{what}");
    }
    // A caller who is not eligible for the larger cap has the global cap and no permit to wait for: a Viewer's 5 MiB is `413`, not `429`.
    let reply = send(app, as_page(Method::PUT, &files_path, &s.viewer, &five)).await;
    assert_too_large(&reply, "a Viewer's 5 MiB while eight large bodies are open");
    assert_eq!(s.runs.load(Ordering::SeqCst), runs_before + 3);

    // One body ends: its request is answered, its permit comes back, and the next large body is read.
    let first = held.remove(0);
    drop(first.end);
    let reply = first.reply.await.unwrap();
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.text(), HELD_CHUNK.len().to_string());
    let reply = send(app, as_page(Method::PUT, &files_path, &s.editor, &five)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.text(), five.len().to_string());

    // The bound is eight again and not nine: a new body takes the free permit and the one after it is told to come back.
    held.push(hold_open(app, &streamed(), &started));
    wait_for_started(&started, LARGE_BODIES_IN_FLIGHT + 1).await;
    let reply = send(app, as_page(Method::PUT, &files_path, &s.editor, &five)).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{}", reply.text());

    // Every body that was held ends and is answered, and nothing is left holding a permit.
    for body in held {
        drop(body.end);
        let reply = body.reply.await.unwrap();
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    }
    let reply = send(app, as_page(Method::PUT, &files_path, &s.editor, &five)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
}
