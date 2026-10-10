//! What the config-server commands must not leave behind (T12, S10, S17, D79, D88): searched for in everything the agent
//! sends the hub, writes to its log and counts in its metrics.
//!
//! Two promises:
//!
//! - `FetchServed` follows the deny list of every other path to a file. A denied request is refused before anything is
//!   asked of the config-server, and neither the bytes the config-server would have served, nor the denied name, appear
//!   anywhere (T11 left this for T12).
//! - Environment variable values leave the cluster only for names on the hub's allowlist, and not even for those when the
//!   name looks like a secret (D88, A19). They are not in the stream (but for the one allowed) and never in the log.
//!
//! The test builds on the capture helper of T11: planted markers, searched for in the stream, the log and the metrics. A
//! search that finds nothing proves nothing unless it could have found something, so each test plants a marker that is
//! allowed to leave and checks that the search sees it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use domain::{AgentReply, AppName, HubCommand, NfsPath, OpError, RequestId, ServeRequest, TenantId};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use support::app_rig::{AppRig, Setup};
use support::capture::{Capture, Marker};
use support::fake_configserver::FakeConfigServer;
use support::fake_hub::ConnHandle;
use support::fake_kube::FakeKube;
use support::k8s_objects::{deployment, pod};
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

const NS: &str = "sit1";

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn serve_request(app: &str, file: &str) -> ServeRequest {
    ServeRequest {
        application: AppName::parse(app).unwrap(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: None,
        file: NfsPath::parse(file).unwrap(),
    }
}

async fn ask(conn: &ConnHandle, id: &str, command: HubCommand) -> FromAgent {
    conn.send(ToAgent::Command(command)).await;
    let id = id.to_owned();
    conn.wait_for(move |m| match m {
        FromAgent::Reply(AgentReply::Op(r)) => r.request_id.as_str() == id,
        FromAgent::Reply(
            AgentReply::Served { request_id, .. }
            | AgentReply::Notify { request_id, .. }
            | AgentReply::Cluster { request_id, .. },
        ) => request_id.as_str() == id,
        _ => false,
    })
    .await
}

fn config(allowlist: &[&str]) -> pb::AgentConfig {
    pb::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: vec!["*.hubsecret".to_owned()],
        env_allowlist: allowlist.iter().map(|n| (*n).to_owned()).collect(),
        tenants: vec!["sit1".to_owned()],
    }
}

/// Everything found anywhere, as readable lines.
fn found(capture: &Capture, conns: &[ConnHandle], app: &AppRig) -> Vec<String> {
    let mut found = Vec::new();
    for conn in conns {
        found.extend(capture.on_stream(conn));
    }
    found.extend(capture.in_logs());
    found.extend(capture.in_metrics(&app.metrics.render()));
    found
}

#[allow(
    clippy::too_many_lines,
    reason = "one scenario told in order: the requests, then everything that was captured"
)]
#[tokio::test(start_paused = true)]
async fn fetch_served_never_returns_denied_bytes_or_names() {
    let capture = Capture::new(vec![
        Marker::text("served_denied_bytes", "MARKER-DENIED-BYTES-3f9a-never-leave"),
        Marker::text("denied_name", "MARKER-DENIED-NAME-b71c"),
        Marker::text("denied_app", "MARKER-privatedir-0e2d"),
        Marker::text("served_ordinary_bytes", "MARKER-ORDINARY-BYTES-aa01-may-leave"),
    ]);
    let _logs = capture.logs().install();

    let server = FakeConfigServer::start();
    // The config-server would serve all of these. The agent must not ask for the denied ones, and must not pass on what
    // it would have said.
    let denied_bytes = "MARKER-DENIED-BYTES-3f9a-never-leave";
    for target in [
        "/web/sit1,default/master/keys/MARKER-DENIED-NAME-b71c.pem",
        "/web/sit1,default/master/MARKER-DENIED-NAME-b71c.hubsecret",
        "/MARKER-privatedir-0e2d/sit1,default/master/a.yml",
        "/web/sit1,default/master/dir/store.JKS",
    ] {
        server.serve(target, 200, denied_bytes);
    }
    server.serve(
        "/web/sit1,default/master/a.yml",
        200,
        "MARKER-ORDINARY-BYTES-aa01-may-leave",
    );

    let source = Arc::new(ScriptedSource::new());
    let app = AppRig::start(Setup {
        source: Some(source),
        config_server_dialer: Some(server.dialer()),
        extra_env_owned: vec![("LK_CONFIG_SERVER_URL".to_owned(), server.url().to_owned())],
        ..Setup::default()
    })
    .await;
    app.rig.server.set_config(config(&[]));
    let conn = app.connected().await;

    let cases = [
        (
            "deny-pem",
            serve_request("web", "keys/MARKER-DENIED-NAME-b71c.pem"),
        ),
        (
            "deny-hub",
            serve_request("web", "MARKER-DENIED-NAME-b71c.hubsecret"),
        ),
        ("deny-dir", serve_request("MARKER-privatedir-0e2d", "a.yml")),
        ("deny-jks", serve_request("web", "dir/store.JKS")),
    ];
    // The hub's glob arrives with the configuration, before any command; give the first connection time to apply it.
    sleep(Duration::from_secs(1)).await;
    for (id, request) in cases {
        let reply = ask(
            &conn,
            id,
            HubCommand::FetchServed {
                request_id: rid(id),
                request,
            },
        )
        .await;
        let FromAgent::Reply(AgentReply::Op(result)) = reply else {
            panic!("{id}: expected a refusal, got {reply:?}");
        };
        assert_eq!(
            (result.ok, result.error, result.current_hash),
            (false, Some(OpError::Denied), None),
            "{id}"
        );
    }
    assert!(
        server.requests().is_empty(),
        "no denied request was made: {:#?}",
        server.requests()
    );

    // The same client still serves an ordinary file, and that one is seen by the search: it could have found a leak.
    let reply = ask(
        &conn,
        "ok",
        HubCommand::FetchServed {
            request_id: rid("ok"),
            request: serve_request("web", "a.yml"),
        },
    )
    .await;
    assert!(
        matches!(&reply, FromAgent::Reply(AgentReply::Served { status: 200, .. })),
        "{reply:?}"
    );

    let all = found(&capture, std::slice::from_ref(&conn), &app);
    let leaked: Vec<_> = all
        .iter()
        .filter(|f| !f.starts_with("served_ordinary_bytes"))
        .collect();
    assert!(leaked.is_empty(), "denied material left the agent: {leaked:#?}");
    assert!(
        all.iter()
            .any(|f| f.starts_with("served_ordinary_bytes in stream")),
        "the ordinary file never reached the stream, so the search was blind: {all:#?}"
    );
    assert!(
        !all.iter().any(|f| f.starts_with("served_ordinary_bytes in the")),
        "served bytes reached the log or the metrics: {all:#?}"
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "one scenario told in order: the cluster, the session, and then everything that was captured"
)]
#[tokio::test(start_paused = true)]
async fn no_env_values_outside_allowlist_in_logs_or_stream() {
    let capture = Capture::new(vec![
        Marker::text("allowed_value", "MARKER-ENV-ALLOWED-8d21"),
        Marker::text("unlisted_value", "MARKER-ENV-UNLISTED-44ce"),
        Marker::text("password_value", "MARKER-ENV-PASSWORD-9b05"),
        Marker::text("token_value", "MARKER-ENV-TOKEN-1e7f"),
        Marker::text("key_value", "MARKER-ENV-KEY-6a30"),
        Marker::text("credential_value", "MARKER-ENV-CREDENTIAL-d2b8"),
        Marker::text("secret_value", "MARKER-ENV-SECRET-57c4"),
        Marker::text("long_value", &"L".repeat(300)),
        Marker::text("control_value", "MARKER-ENV-CONTROL-0c19\u{7}bell"),
    ]);
    let _logs = capture.logs().install();

    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "web")
            .env("CONFIG_CLIENT_CACHE_TTL", "MARKER-ENV-ALLOWED-8d21")
            .env("JAVA_OPTS", "MARKER-ENV-UNLISTED-44ce")
            .env("DB_PASSWORD", "MARKER-ENV-PASSWORD-9b05")
            .env("API_TOKEN", "MARKER-ENV-TOKEN-1e7f")
            .env("SIGNING_KEY", "MARKER-ENV-KEY-6a30")
            .env("SERVICE_CREDENTIALS", "MARKER-ENV-CREDENTIAL-d2b8")
            .env("APP_SECRET", "MARKER-ENV-SECRET-57c4")
            .env("LONG_ONE", &"L".repeat(300))
            .env("CONTROL_ONE", "MARKER-ENV-CONTROL-0c19\u{7}bell")
            .env_from_secret("FROM_A_SECRET", "db-secret")
            .build(),
    );
    kube.apply(pod(NS, "web-1", "web", "2026-10-10T11:00:00Z"));

    let server = FakeConfigServer::start();
    let source = Arc::new(ScriptedSource::new());
    let app = AppRig::start(Setup {
        kube: Some(kube.clone()),
        source: Some(source),
        config_server_dialer: Some(server.dialer()),
        extra_env_owned: vec![("LK_CONFIG_SERVER_URL".to_owned(), server.url().to_owned())],
        ..Setup::default()
    })
    .await;
    // The hub's allowlist is generous and partly wrong: it lists secret-named variables, a long one and one with a bell.
    app.rig.server.set_config(config(&[
        "CONFIG_CLIENT_CACHE_TTL",
        "DB_PASSWORD",
        "API_TOKEN",
        "SIGNING_KEY",
        "SERVICE_CREDENTIALS",
        "APP_SECRET",
        "LONG_ONE",
        "CONTROL_ONE",
        "FROM_A_SECRET",
    ]));
    let conn = app.connected().await;
    let first = conn.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;
    let FromAgent::Cluster(report) = first else {
        unreachable!()
    };

    // What the report says: the one allowed value, and the names of the rest.
    let info = &report
        .deployments
        .iter()
        .find(|d| d.service.name() == "web")
        .unwrap();
    let values: Vec<(&str, &str)> = info
        .env_values
        .iter()
        .map(|v| (v.name.as_str(), v.value.as_str()))
        .collect();
    assert_eq!(values, [("CONFIG_CLIENT_CACHE_TTL", "MARKER-ENV-ALLOWED-8d21")]);
    let mut names: Vec<&str> = info.env_names.iter().map(domain::ShortText::as_str).collect();
    names.sort_unstable();
    assert!(names.contains(&"JAVA_OPTS"), "names are reported: {names:?}");
    assert!(names.contains(&"DB_PASSWORD"), "{names:?}");

    // The cluster report on request, and the config-server commands, all in one session.
    let asked = ask(
        &conn,
        "report",
        HubCommand::RequestClusterReport {
            request_id: rid("report"),
        },
    )
    .await;
    assert!(matches!(asked, FromAgent::Reply(AgentReply::Cluster { .. })));
    ask(
        &conn,
        "notify",
        HubCommand::NotifyConfigServer {
            request_id: rid("notify"),
            paths: vec![NfsPath::parse("web/a.yml").unwrap()],
        },
    )
    .await;
    ask(
        &conn,
        "fetch",
        HubCommand::FetchServed {
            request_id: rid("fetch"),
            request: serve_request("web", "a.yml"),
        },
    )
    .await;
    // Pods come and go, so that deltas follow.
    kube.apply(pod(NS, "web-2", "web", "2026-10-10T12:00:00Z"));
    sleep(Duration::from_secs(30)).await;

    let all = found(&capture, std::slice::from_ref(&conn), &app);
    let leaked: Vec<_> = all
        .iter()
        .filter(|f| !f.starts_with("allowed_value in stream"))
        .collect();
    assert!(
        leaked.is_empty(),
        "env values outside the allowlist left the agent: {leaked:#?}"
    );
    assert!(
        all.iter().any(|f| f.starts_with("allowed_value in stream")),
        "the allowed value never reached the stream, so the search was blind: {all:#?}"
    );
    assert!(
        !all.iter().any(|f| f.starts_with("allowed_value in the")),
        "even an allowed value is for the hub, never for the log or the metrics: {all:#?}"
    );
}
