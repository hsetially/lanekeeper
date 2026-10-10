//! `FetchServed` (T12, D88, Q37): the agent GETs what the config-server serves for `(application, tenant, channel, file)`
//! and hands back the status and the bytes. It asks the configured server only, never faster than five times a second,
//! never takes more than 2 MiB, and never for a path the deny list covers (D79, S17).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::NfsRoot;
use agent::clock::Clock;
use agent::configserver::{
    self, ConfigServerClient, ConfigServerError, FETCH_PER_SECOND, MAX_RESPONSE_BYTES, RateLimiter,
};
use agent::deny::DenyList;
use agent::dispatch::{CommandHandler, Dispatcher, OpLimits};
use agent::fileops::FileOps;
use domain::{
    AgentReply, AppName, ChannelName, HubCommand, NfsPath, OpError, OpResult, RequestId, ServeRequest,
    TenantId,
};
use proto::convert::FromAgent;
use support::clock::TestClock;
use support::fake_configserver::{FakeConfigServer, segment_decode};
use support::raw_server::Reply;
use tempfile::TempDir;
use tokio::time::{Instant, advance};

const LIMITS: OpLimits = OpLimits {
    max_file_bytes: 2 * 1024 * 1024,
};

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn request(app: &str, tenant: &str, channel: Option<&str>, file: &str) -> ServeRequest {
    ServeRequest {
        application: AppName::parse(app).unwrap(),
        tenant: TenantId::parse(tenant).unwrap(),
        channel: channel.map(|c| ChannelName::parse(c).unwrap()),
        file: NfsPath::parse(file).unwrap(),
    }
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(TestClock::starting_at(1_791_633_600_000))
}

fn client_with(server: &FakeConfigServer, deny: DenyList) -> ConfigServerClient {
    server.client(deny, clock())
}

struct Rig {
    dispatcher: Dispatcher,
    deny: DenyList,
    _dir: TempDir,
}

fn rig(server: &FakeConfigServer) -> Rig {
    rig_on(server, clock())
}

fn rig_on(server: &FakeConfigServer, clock: Arc<dyn Clock>) -> Rig {
    let dir = TempDir::new().unwrap();
    let deny = DenyList::default();
    let ops = FileOps::new(
        NfsRoot::open(dir.path()).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    )
    .with_deny(deny.clone());
    let dispatcher = Dispatcher::new(ops).with_config_server(Arc::new(server.client(deny.clone(), clock)));
    Rig {
        dispatcher,
        deny,
        _dir: dir,
    }
}

async fn fetch(rig: &Rig, id: &str, request: ServeRequest) -> FromAgent {
    rig.dispatcher
        .handle(
            HubCommand::FetchServed {
                request_id: rid(id),
                request,
            },
            LIMITS,
        )
        .await
        .expect("a FetchServed always gets an answer")
}

fn served(reply: FromAgent) -> (RequestId, u16, Vec<u8>) {
    match reply {
        FromAgent::Reply(AgentReply::Served {
            request_id,
            status,
            bytes,
        }) => (request_id, status, bytes.to_vec()),
        other => panic!("a ServedResponse, got {other:?}"),
    }
}

fn op(reply: FromAgent) -> OpResult {
    match reply {
        FromAgent::Reply(AgentReply::Op(result)) => result,
        other => panic!("an OpResult, got {other:?}"),
    }
}

// ------------------------------------------------------------------------------------------------ the request

#[tokio::test(start_paused = true)]
async fn fetch_served_url_and_accept_header() {
    let server = FakeConfigServer::start();
    server.serve(
        "/tx-infinity-api/sit1,default/master/tx-infinity-core.yml",
        200,
        "a: 1\n",
    );
    server.serve(
        "/tx-infinity-api/sit1,default/master/remote-itm-teller/receipt-ci_1.bmp",
        200,
        vec![0x42, 0x4d, 0, 1],
    );
    server.serve(
        "/tx-infinity-api/sit1,default/master/xsl/print.xsl",
        200,
        "<xsl/>",
    );
    let client = client_with(&server, DenyList::default());

    let text = client
        .fetch_served(&request("tx-infinity-api", "sit1", None, "tx-infinity-core.yml"))
        .await
        .unwrap();
    assert_eq!((text.status, &text.body[..]), (200, &b"a: 1\n"[..]));

    let binary = client
        .fetch_served(&request(
            "tx-infinity-api",
            "sit1",
            Some("remote-itm-teller"),
            "receipt-ci_1.bmp",
        ))
        .await
        .unwrap();
    assert_eq!((binary.status, &binary.body[..]), (200, &[0x42, 0x4d, 0, 1][..]));

    let nested = client
        .fetch_served(&request("tx-infinity-api", "sit1", None, "xsl/print.xsl"))
        .await
        .unwrap();
    assert_eq!(nested.status, 200);

    let seen = server.fetches();
    assert_eq!(seen.len(), 3);
    for r in &seen {
        assert_eq!(r.method, "GET");
        assert_eq!(r.header("host"), Some(server.authority()));
        assert!(r.body.is_empty());
    }
    assert_eq!(
        seen[0].target,
        "/tx-infinity-api/sit1,default/master/tx-infinity-core.yml"
    );
    assert_eq!(seen[0].header("accept"), None, "a text file is asked for as text");
    assert_eq!(
        seen[1].target,
        "/tx-infinity-api/sit1,default/master/remote-itm-teller/receipt-ci_1.bmp"
    );
    assert_eq!(
        seen[1].header("accept"),
        Some("application/octet-stream"),
        "a binary file is asked for as bytes"
    );
    assert_eq!(
        seen[2].target,
        "/tx-infinity-api/sit1,default/master/xsl/print.xsl"
    );
}

#[test]
fn binary_is_everything_the_domain_does_not_call_text() {
    // docs/domain-model.md: structured (.yml .yaml .json .properties) and text (.xsl .xml .txt, extension-less) are text;
    // everything else is binary. The case of the extension does not matter.
    for text in [
        "a.yml",
        "a.YAML",
        "a.json",
        "a.properties",
        "a.xsl",
        "a.xml",
        "a.txt",
        "Makefile",
        "dir/noext",
        "a.b.yml",
        "dir.d/noext",
        "dir/.hidden",
        "trailing.",
    ] {
        assert!(!configserver::is_binary(&NfsPath::parse(text).unwrap()), "{text}");
    }
    for binary in [
        "a.bmp",
        "a.PNG",
        "x/y.jpg",
        "a.jks",
        "a.pdf",
        "a.zip",
        "a.bin",
        "a.yml.bak",
        "a.xsl.gz",
    ] {
        assert!(
            configserver::is_binary(&NfsPath::parse(binary).unwrap()),
            "{binary}"
        );
    }
}

/// File names that are valid `NfsPath`s and would do harm in a URL if they were pasted in as they are.
const HOSTILE_FILES: &[&str] = &[
    "a b.yml",
    "q?x=1.yml",
    "frag#ment.yml",
    "%2e%2e/%2fetc.yml",
    "..a/b..",
    "a/.../b",
    "semi;colon,comma.yml",
    "é/ü.yml",
    "x@evil.example/y.yml",
    "http:/evil.example/x",
    "a:b/c",
    "update-resources",
    "a%00b",
    "ta\u{202e}b.yml",
    "[::1]/x",
    "a'b\"c<d>.yml",
];

#[tokio::test(start_paused = true)]
async fn fetch_served_url_cannot_escape() {
    let server = FakeConfigServer::start();
    for file in HOSTILE_FILES {
        // The rate limit is not what is being tested: a fresh client per file.
        let client_for_one = client_with(&server, DenyList::default());
        let before = server.fetches().len();
        let req = request("tx-infinity-api", "sit1", Some("remote-itm-teller"), file);
        let outcome = client_for_one.fetch_served(&req).await;
        assert!(outcome.is_ok(), "{file:?}: {outcome:?}");

        let fetches = server.fetches();
        assert_eq!(fetches.len(), before + 1, "{file:?}: exactly one GET");
        let target = &fetches[before].target;
        let prefix = "/tx-infinity-api/sit1,default/master/remote-itm-teller/";
        assert!(target.starts_with(prefix), "{file:?}: {target}");
        for bad in ['?', '#', ' ', '\\', '\r', '\n', '"', '<', '>', '\''] {
            assert!(!target.contains(bad), "{file:?}: {bad:?} in {target}");
        }
        assert!(target.is_ascii(), "{file:?}: {target}");
        assert!(!target.contains("//"), "{file:?}: {target}");
        // Each segment of the file decodes to the component it came from, and no segment is a dot segment.
        let segments: Vec<String> = target[prefix.len()..].split('/').map(segment_decode).collect();
        let parsed = NfsPath::parse(file).unwrap();
        let expected: Vec<&str> = parsed.components().collect();
        assert_eq!(segments, expected, "{file:?}");
        assert!(
            target.split('/').all(|s| s != ".." && s != "."),
            "{file:?}: a dot segment in {target}"
        );
        assert_eq!(fetches[before].header("host"), Some(server.authority()));
    }
    // Every connection went to the configured host and port, and nothing else: no name in a message is ever dialled.
    let dialed = server.dialed();
    assert_eq!(dialed.len(), HOSTILE_FILES.len());
    assert!(
        dialed
            .iter()
            .all(|(host, port)| host == "config-server.test" && *port == 8888),
        "{dialed:?}"
    );
    assert!(
        server.requests().iter().all(|r| r.target != "/update-resources"),
        "a file name never reaches the notify endpoint"
    );
}

#[test]
fn fetch_served_names_that_look_like_urls_stay_inside_their_segments() {
    // Application and channel names are tokens (letters, digits, dot, underscore, hyphen); the tenant is `[a-z0-9_-]`.
    // None of them can carry a slash, a colon or an at sign, so none can change the host or the path structure.
    for bad in [
        "a/b", "a:b", "a@b", "a b", "a?b", "a#b", "a%2fb", "", ".hidden", "a\\b",
    ] {
        assert!(AppName::parse(bad).is_err(), "{bad:?}");
        assert!(ChannelName::parse(bad).is_err(), "{bad:?}");
    }
    for bad in ["sit1,default", "SIT1", "a.b", "a/b", ""] {
        assert!(TenantId::parse(bad).is_err(), "{bad:?}");
    }
    let target = configserver::served_target(&request("app.v2_x-y", "sit-1_a", Some("ch.1"), "f.yml"));
    assert_eq!(target, "/app.v2_x-y/sit-1_a,default/master/ch.1/f.yml");
}

// ------------------------------------------------------------------------------------------------ what comes back

#[tokio::test(start_paused = true)]
async fn fetch_served_answers_with_served_response() {
    let server = FakeConfigServer::start();
    server.serve("/app/sit1,default/master/a.yml", 200, "key: value\r\n");
    let rig = rig(&server);
    let (id, status, bytes) = served(fetch(&rig, "f1", request("app", "sit1", None, "a.yml")).await);
    assert_eq!(id.as_str(), "f1");
    assert_eq!((status, bytes.as_slice()), (200, &b"key: value\r\n"[..]));
    // And the hub's own validation accepts it.
    let wire = fetch(&rig, "f2", request("app", "sit1", None, "a.yml")).await;
    assert_eq!(
        FromAgent::from_proto(wire.clone().into_proto()).unwrap(),
        Some(wire)
    );
}

#[tokio::test(start_paused = true)]
async fn fetch_served_error_status_comes_back_without_the_servers_text() {
    let server = FakeConfigServer::start();
    server.serve(
        "/app/sit1,default/master/missing.yml",
        404,
        "{\"path\":\"/app/sit1,default/master/missing.yml\"}",
    );
    server.serve(
        "/app/sit1,default/master/broken.yml",
        500,
        "stack trace: SERVER-INTERNALS",
    );
    let rig = rig(&server);
    for (file, expect) in [("missing.yml", 404), ("broken.yml", 500)] {
        let (_, status, bytes) = served(fetch(&rig, "f3", request("app", "sit1", None, file)).await);
        assert_eq!(status, expect);
        assert!(bytes.is_empty(), "{file}: only a 2xx carries bytes");
    }
}

#[tokio::test(start_paused = true)]
async fn fetch_served_caps_body_at_2mib() {
    let server = FakeConfigServer::start();
    let at_cap = vec![b'a'; MAX_RESPONSE_BYTES];
    let over_cap = vec![b'b'; MAX_RESPONSE_BYTES + 1];
    server.serve("/app/sit1,default/master/at-cap.xml", 200, at_cap.clone());
    server.serve("/app/sit1,default/master/over-cap.xml", 200, over_cap);
    let rig = rig(&server);

    let (_, status, bytes) = served(fetch(&rig, "c1", request("app", "sit1", None, "at-cap.xml")).await);
    assert_eq!(
        (status, bytes.len()),
        (200, MAX_RESPONSE_BYTES),
        "exactly 2 MiB is served whole"
    );
    assert_eq!(bytes, at_cap);

    // One byte more is refused outright: a truncated file would be mistaken for the real one.
    let result = op(fetch(&rig, "c2", request("app", "sit1", None, "over-cap.xml")).await);
    assert_eq!((result.ok, result.error), (false, Some(OpError::Unsupported)));

    // The same reply fits in a message the hub accepts.
    let wire = FromAgent::Reply(AgentReply::Served {
        request_id: rid("c1"),
        status: 200,
        bytes: bytes.into(),
    });
    assert!(FromAgent::from_proto(wire.into_proto()).is_ok());
}

#[tokio::test(start_paused = true)]
async fn fetch_served_connect_failure_and_timeout_answer_io() {
    let absent = FakeConfigServer::absent();
    let rig_absent = rig(&absent);
    let result = op(fetch(&rig_absent, "u1", request("app", "sit1", None, "a.yml")).await);
    assert_eq!((result.ok, result.error), (false, Some(OpError::Io)));

    let hanging = FakeConfigServer::start();
    hanging.serve_reply("/app/sit1,default/master/a.yml", Reply::Hang);
    let rig_hang = rig(&hanging);
    let started = Instant::now();
    let result = op(fetch(&rig_hang, "u2", request("app", "sit1", None, "a.yml")).await);
    assert_eq!((result.ok, result.error), (false, Some(OpError::Io)));
    assert_eq!(started.elapsed(), Duration::from_secs(5), "five seconds, once");
    assert_eq!(
        hanging.fetches().len(),
        1,
        "a read is not retried: the hub asks again if it wants to"
    );
}

#[tokio::test(start_paused = true)]
async fn fetch_served_without_a_configured_config_server_is_unsupported() {
    let dir = TempDir::new().unwrap();
    let ops = FileOps::new(
        NfsRoot::open(dir.path()).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    );
    let reply = Dispatcher::new(ops)
        .handle(
            HubCommand::FetchServed {
                request_id: rid("n1"),
                request: request("app", "sit1", None, "a.yml"),
            },
            LIMITS,
        )
        .await
        .unwrap();
    assert_eq!(op(reply).error, Some(OpError::Unsupported));
}

// ------------------------------------------------------------------------------------------------ the rate limit

#[tokio::test(start_paused = true)]
async fn fetch_served_rate_limit_5_per_s() {
    let server = FakeConfigServer::start();
    server.serve("/app/sit1,default/master/a.yml", 200, "a: 1\n");
    let rig = rig(&server);
    assert_eq!(FETCH_PER_SECOND, 5);

    let mut statuses = Vec::new();
    for i in 0..8 {
        let (_, status, bytes) =
            served(fetch(&rig, &format!("r{i}"), request("app", "sit1", None, "a.yml")).await);
        if status == 429 {
            assert!(bytes.is_empty(), "the agent's own refusal carries no body");
        }
        statuses.push(status);
    }
    assert_eq!(statuses, [200, 200, 200, 200, 200, 429, 429, 429]);
    assert_eq!(
        server.fetches().len(),
        5,
        "a refused request never reaches the config-server"
    );

    // Nothing is allowed again until the first of the five is a second old.
    advance(Duration::from_millis(999)).await;
    let (_, status, _) = served(fetch(&rig, "r9", request("app", "sit1", None, "a.yml")).await);
    assert_eq!(status, 429);
    advance(Duration::from_millis(1)).await;
    let (_, status, _) = served(fetch(&rig, "r10", request("app", "sit1", None, "a.yml")).await);
    assert_eq!(status, 200);
    assert_eq!(server.fetches().len(), 6);
}

#[tokio::test(start_paused = true)]
async fn the_limit_holds_in_every_one_second_window() {
    // Not a bucket that refills in a lump: at no instant are there more than five requests in the last second.
    let clock: Arc<dyn Clock> = Arc::new(TestClock::starting_at(0));
    let limiter = RateLimiter::new(Arc::clone(&clock), FETCH_PER_SECOND, Duration::from_secs(1));
    let mut allowed_at = Vec::new();
    for _ in 0..40 {
        if limiter.try_acquire() {
            allowed_at.push(clock.instant());
        }
        advance(Duration::from_millis(130)).await;
    }
    assert!(
        allowed_at.len() > 5,
        "the limiter lets requests through as time passes"
    );
    for (i, at) in allowed_at.iter().enumerate() {
        let in_window = allowed_at[i..]
            .iter()
            .take_while(|later| **later - *at < Duration::from_secs(1))
            .count();
        assert!(
            in_window <= FETCH_PER_SECOND,
            "{in_window} requests within a second of {i}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn the_limiter_is_bounded_whatever_the_load() {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::starting_at(0));
    let limiter = RateLimiter::new(clock, 5, Duration::from_secs(1));
    for _ in 0..100_000 {
        let _ = limiter.try_acquire();
    }
    assert!(
        limiter.tracked() <= 5,
        "it remembers at most one window's worth: {}",
        limiter.tracked()
    );
}

// ------------------------------------------------------------------------------------------------ the deny list

fn denied(result: &OpResult) -> bool {
    !result.ok && result.error == Some(OpError::Denied) && result.current_hash.is_none()
}

#[tokio::test(start_paused = true)]
async fn fetch_served_refuses_denied_names_without_a_request() {
    let server = FakeConfigServer::start();
    for target in [
        "/app/sit1,default/master/keys/server.pem",
        "/app/sit1,default/master/Store.JKS",
        "/app/sit1,default/master/trust.keystore",
        "/private-app/sit1,default/master/a.yml",
        "/app/sit1,default/master/private/a.yml",
        "/app/sit1,default/master/ok.yml",
    ] {
        server.serve(target, 200, "MARKER-SERVED-BYTES");
    }
    let rig = rig(&server);
    let cases = [
        request("app", "sit1", None, "keys/server.pem"),
        request("app", "sit1", None, "Store.JKS"),
        request("app", "sit1", None, "trust.keystore"),
        request("private-app", "sit1", None, "a.yml"),
        request("app", "sit1", Some("private"), "a.yml"),
        request("app", "sit1", None, "private/a.yml"),
    ];
    for (i, case) in cases.into_iter().enumerate() {
        let name = case.file.to_string();
        let reply = fetch(&rig, &format!("d{i}"), case).await;
        let text = format!("{reply:?}");
        assert!(!text.contains("MARKER-SERVED-BYTES"), "{name}: {text}");
        assert!(
            !text.contains(&name),
            "{name}: the answer names the denied path: {text}"
        );
        let result = op(reply);
        assert!(denied(&result), "{name}: {result:?}");
    }
    assert!(server.requests().is_empty(), "a denied request is never made");

    // An ordinary file through the same client is served, so the refusals above were the deny list and not a broken rig.
    let (_, status, bytes) = served(fetch(&rig, "d-ok", request("app", "sit1", None, "ok.yml")).await);
    assert_eq!((status, &bytes[..]), (200, &b"MARKER-SERVED-BYTES"[..]));
}

#[tokio::test(start_paused = true)]
async fn fetch_served_follows_the_same_deny_list_as_file_reads() {
    let server = FakeConfigServer::start();
    server.serve("/app/sit1,default/master/x.secret", 200, "MARKER-HUB-GLOB");
    let rig = rig(&server);
    // Not denied yet.
    let (_, status, _) = served(fetch(&rig, "s1", request("app", "sit1", None, "x.secret")).await);
    assert_eq!(status, 200);
    // The hub adds a glob to the one shared list; the next request sees it.
    rig.deny.set_hub_globs(&["*.secret"]);
    let result = op(fetch(&rig, "s2", request("app", "sit1", None, "x.secret")).await);
    assert!(denied(&result), "{result:?}");
    // And takes it back.
    rig.deny.set_hub_globs(&[]);
    let (_, status, _) = served(fetch(&rig, "s3", request("app", "sit1", None, "x.secret")).await);
    assert_eq!(status, 200);
    // A built-in glob never goes.
    let result = op(fetch(&rig, "s4", request("app", "sit1", None, "x.pem")).await);
    assert!(denied(&result));
}

#[tokio::test(start_paused = true)]
async fn fetch_served_checks_the_tenant_copy_the_server_would_pick() {
    // The config-server prefers `<name>-<tenant>.<ext>` over `<name>.<ext>` (D82). A request for the base name is therefore
    // a request for the tenant copy too, and the copy's name is checked as well.
    let server = FakeConfigServer::start();
    server.serve(
        "/app/sit1,default/master/credentials.yml",
        200,
        "MARKER-TENANT-COPY",
    );
    server.serve("/app/sit1,default/master/vault", 200, "MARKER-TENANT-COPY");
    let rig = rig(&server);
    rig.deny.set_hub_globs(&["credentials-sit1.yml", "vault-sit1"]);
    for (i, file) in ["credentials.yml", "vault"].into_iter().enumerate() {
        let reply = fetch(&rig, &format!("t{i}"), request("app", "sit1", None, file)).await;
        assert!(!format!("{reply:?}").contains("MARKER-TENANT-COPY"));
        assert!(denied(&op(reply)), "{file}");
    }
    assert!(server.requests().is_empty());
    // Another tenant's copy is not this tenant's business: for `sit2` the copy would be `credentials-sit2.yml`.
    let (_, status, _) = served(fetch(&rig, "t2", request("app", "sit2", None, "credentials.yml")).await);
    assert_eq!(
        status, 404,
        "the request was made (the fake serves nothing for sit2)"
    );
    assert_eq!(server.fetches().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn denied_requests_do_not_use_the_rate_limit() {
    let server = FakeConfigServer::start();
    server.serve("/app/sit1,default/master/a.yml", 200, "a: 1\n");
    let rig = rig(&server);
    for i in 0..50 {
        let result = op(fetch(&rig, &format!("x{i}"), request("app", "sit1", None, "k.pem")).await);
        assert!(denied(&result));
    }
    for i in 0..FETCH_PER_SECOND {
        let (_, status, _) =
            served(fetch(&rig, &format!("y{i}"), request("app", "sit1", None, "a.yml")).await);
        assert_eq!(status, 200, "request {i}: the refusals before it cost nothing");
    }
}

#[test]
fn the_client_error_codes_are_the_documented_ones() {
    assert_eq!(ConfigServerError::Denied.code(), OpError::Denied);
    assert_eq!(ConfigServerError::Unreachable.code(), OpError::Io);
    assert_eq!(ConfigServerError::TooLarge.code(), OpError::Unsupported);
    assert_eq!(ConfigServerError::InvalidRequest.code(), OpError::Denied);
}
