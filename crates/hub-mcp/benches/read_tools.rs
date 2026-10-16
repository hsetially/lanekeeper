//! P9: the MCP read tools (`P9`, `hub-mcp/read_tools`, and `P9.<tool>`, `hub-mcp/<tool>`, each p95).
//!
//! The real server in process: a `tools/call` request is decoded by axum, passes the gate (bearer token verified, origin and host
//! checked, session bound), is dispatched by rmcp, passes the role check, admission (rate, concurrency, bytes), the typed argument
//! parser and the tool, and its result is shaped, redacted, capped and serialised, then read back as a client reads it from the
//! event stream. Only the ports are fakes, and they answer at target scale from answers computed once (`support`): 40 swimlanes, a
//! page of 200 entries out of 2,000 files, the 6,000-line CRLF file of swimlane 1 with a fake key planted near line 5,000, a
//! comparison and its setting changes from the engine, a grid of 40 swimlanes by 400 settings, 500 text hits, 400 findings and 100
//! doc hits of 1 KB. The sample is therefore this crate's share of a call (A7); the ports' own work is measured in 04, 05 and 14, and
//! prompt 16 measures the whole path.
//!
//! - `<tool>`: one call of that tool per iteration. The caches that hold a file's scan and settings index are off (`cache_bytes = 0`),
//!   so `read_file` and `get_served_config` decode and scan their 6,000-line file on every call: the honest worst case.
//! - `read_tools`: one call per iteration, all 20 tools in a fixed order, on the same cold server.
//! - `read_file_warm`: not budgeted. `read_file` with the caches on (the normal case, one scan of a file per version).
//!
//! The rate limit is out of the way (it has tests of its own); everything else is as configured by default.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::cast_precision_loss
)]

/// The client of the tests: requests to the router in process, the response read from the event stream.
#[path = "../tests/support/mod.rs"]
mod client;
/// The fixtures and the ports that answer from them.
mod support;

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use client::Client;
use criterion::{Criterion, criterion_group, criterion_main};
use domain::{Guid, Role, ShortText, User, UserId, UserStatus};
use hub_mcp::auth::oauth::OAuthSettings;
use hub_mcp::config::{AuthMode, McpConfig, PublicOrigin};
use hub_mcp::{Credentials, McpDeps, McpSettings, McpState, mount};
use hub_platform::{FakeSessionAuth, GuardedRouter};
use ports::VerifiedUser;
use ports::fakes::{FakeTokenVerifier, FakeUsers, FakeWriteService};
use serde_json::{Value, json};
use support::{FixtureDocs, FixtureRegistry, Fixtures, SWIMLANES, fake_token, lane};

const HOST: &str = "hub.example.test";
const TOKEN: &str = "entra-token-for-the-benchmark";
/// The cap on a read result (D58). A result over it is a failure of the benchmark, not a slow sample.
const READ_CAP: usize = 16 * 1024;

fn caller() -> UserId {
    UserId::new(
        Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
        Guid::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
    )
}

/// The twenty read tools and the arguments of one call each, at the sizes a model asks for: the largest page each tool allows.
fn calls() -> Vec<(&'static str, Value)> {
    let big = "tx-infinity-api/big.yml";
    let lanes: Vec<String> = (0..SWIMLANES).map(lane).collect();
    vec![
        ("list_swimlanes", json!({"limit": 200})),
        ("get_swimlane", json!({"swimlane": "sit1"})),
        (
            "list_files",
            json!({"swimlane": "sit1", "prefix": "tx-service-01", "limit": 200}),
        ),
        ("read_file", json!({"swimlane": "sit1", "path": big})),
        (
            "get_effective_config",
            json!({"swimlane": "sit1", "file": big, "tenant": "sit1"}),
        ),
        (
            "compare",
            json!({"left": "nfs:sit1", "right": "nfs:sit2", "file": big}),
        ),
        (
            "compare_tree",
            json!({"left": "nfs:sit1", "right": "nfs:sit2", "limit": 200}),
        ),
        (
            "compare_grid",
            json!({"file": big, "swimlanes": lanes, "limit": 200}),
        ),
        ("search_settings", json!({"q": "timeout", "limit": 200})),
        ("search_text", json!({"pattern": "weight", "max_total": 500})),
        ("find_inconsistencies", json!({"limit": 200})),
        ("get_drift", json!({"swimlane": "sit1", "limit": 200})),
        ("get_pending_restarts", json!({"swimlane": "sit1", "limit": 200})),
        (
            "get_history",
            json!({"swimlane": "sit1", "path": big, "limit": 200}),
        ),
        ("search_docs", json!({"q": "feature flags", "limit": 50})),
        ("read_doc", json!({"path": "/git/main/guide.md"})),
        (
            "grep_docs",
            json!({"pattern": "timeout", "context": 5, "limit": 100}),
        ),
        ("get_docs_for_file", json!({"file": big, "limit": 50})),
        ("get_feature_status", json!({})),
        (
            "get_served_config",
            json!({"swimlane": "sit1", "application": "tx-infinity-api", "tenant": "sit1", "file": "big.yml"}),
        ),
    ]
}

/// A server on the fixtures, and a signed-in client of it with one open session.
struct Server {
    rt: tokio::runtime::Runtime,
    client: Client,
}

impl Server {
    fn start(fixtures: &Arc<Fixtures>, tune: impl FnOnce(&mut McpSettings)) -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let verifier = FakeTokenVerifier::new();
        verifier.accept_entra(
            TOKEN,
            VerifiedUser {
                id: caller(),
                email: None,
                display_name: None,
            },
        );
        let users = FakeUsers::new();
        users.insert(User {
            id: caller(),
            email: ShortText::parse("bench@example.test").unwrap(),
            display_name: ShortText::parse("Bench").unwrap(),
            role: Some(Role::Viewer),
            status: UserStatus::Active,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        });
        let oauth = OAuthSettings {
            tenant_id: Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
            api_audience: "api://hub-api".to_owned(),
            authority_host: OAuthSettings::DEFAULT_AUTHORITY_HOST.to_owned(),
            authorization_server_mirror: false,
        };
        let mut settings = McpSettings::new(
            McpConfig {
                auth_mode: AuthMode::Oauth,
                ..McpConfig::default()
            },
            PublicOrigin::parse(&format!("https://{HOST}")).unwrap(),
            Some(oauth),
        );
        // No keep-alive timer: nothing waits in a benchmark.
        settings.sse_keep_alive = None;
        // The rate limit has tests of its own; a benchmark of 20,000 calls a minute would measure the limiter's refusals.
        settings.limits.read_per_minute = 60_000_000;
        settings.limits.read_burst = 1_000_000;
        tune(&mut settings);
        let state = McpState::new(
            McpDeps::new(
                Credentials::Oauth(Arc::new(verifier)),
                Arc::new(users),
                Arc::new(FixtureRegistry(Arc::clone(fixtures))),
                Arc::new(FixtureDocs(Arc::clone(fixtures))),
                Arc::new(FakeWriteService::new()),
            ),
            settings,
        )
        .unwrap();
        let (router, _routes) = mount(GuardedRouter::new(Arc::new(FakeSessionAuth::new()), None), &state)
            .into_parts()
            .unwrap();
        let mut client = Client::new(router, Some(TOKEN.to_owned())).at_host(HOST);
        rt.block_on(async {
            client.initialize(&json!({})).await.expect("initialize");
        });
        Self { rt, client }
    }

    /// One call, read back as a client reads it.
    fn call(&mut self, tool: &str, args: &Value) -> Value {
        self.rt.block_on(self.client.call_tool_to_end(tool, args))
    }

    /// A call that must have succeeded and must be within the cap. Returns the bytes the cap counts.
    fn checked(&mut self, tool: &str, args: &Value) -> usize {
        let response = self.call(tool, args);
        let result = &response["result"];
        assert!(
            response.get("error").is_none(),
            "{tool}: a protocol error: {response}"
        );
        assert!(
            result["isError"] != json!(true),
            "{tool}: an error result: {}",
            result["structuredContent"]
        );
        let bytes = serde_json::to_vec(&result["structuredContent"]).unwrap().len()
            + serde_json::to_vec(&result["content"]).unwrap().len();
        assert!(
            bytes <= READ_CAP,
            "{tool}: {bytes} bytes is over the {READ_CAP} byte cap"
        );
        assert!(
            !response.to_string().contains(&fake_token()),
            "{tool}: the planted key is in the result"
        );
        bytes
    }
}

/// Every tool once: the benchmark fails here, before the clock, if a tool answers with an error or too much.
fn preflight(server: &mut Server, label: &str, all: &[(&'static str, Value)]) {
    for (tool, args) in all {
        let bytes = server.checked(tool, args);
        eprintln!("hub-mcp preflight ({label}): {tool}: {bytes} bytes");
    }
    // The planted key sits past the default window: asking for the lines around it must show the marker and never the key.
    let response = server.call(
        "read_file",
        &json!({"swimlane": "sit1", "path": "tx-infinity-api/big.yml", "lines": {"start": 4_995, "end": 5_010}}),
    );
    let text = response.to_string();
    assert!(
        text.contains("[REDACTED"),
        "the planted key was not redacted: {text}"
    );
    assert!(
        !text.contains(&fake_token()),
        "the planted key reached the result"
    );
}

fn tool_benches(c: &mut Criterion) {
    let fixtures = Arc::new(Fixtures::build());
    let all = calls();

    // The honest worst case: nothing cached, so every `read_file` decodes and scans its 6,000 lines.
    let mut cold = Server::start(&fixtures, |s| s.limits.cache_bytes = 0);
    preflight(&mut cold, "cold", &all);
    // The normal case: the scan and the settings index of a file version are kept.
    let mut warm = Server::start(&fixtures, |_| {});
    preflight(&mut warm, "warm", &all);

    let mut group = c.benchmark_group("hub-mcp");
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(4));

    for (tool, args) in &all {
        group.bench_function(*tool, |b| {
            b.iter(|| black_box(cold.call(tool, args)));
        });
    }

    let mut next = 0usize;
    group.bench_function("read_tools", |b| {
        b.iter(|| {
            let (tool, args) = &all[next % all.len()];
            next += 1;
            black_box(cold.call(tool, args))
        });
    });

    let (tool, args) = &all[3];
    assert_eq!(*tool, "read_file");
    group.bench_function("read_file_warm", |b| {
        b.iter(|| black_box(warm.call(tool, args)));
    });
    group.finish();
}

criterion_group!(benches, tool_benches);
criterion_main!(benches);
