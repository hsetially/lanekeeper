//! The config-server calls in the whole agent (T12, Q37, S17): the agent reaches the config-server only to answer a
//! command from the hub, and only at the address it was configured with.
//!
//! The agent runs as `agent::app::App` against the fake hub, the fake Kubernetes API server, a scripted file system and a
//! fake config-server (in memory, so that everything runs in virtual time).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{
    AgentReply, AppName, ContentHash, Expected, HubCommand, NfsPath, RequestId, ServeRequest, ServiceRef,
    TenantId, Timestamp,
};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use support::app_rig::{AppRig, Setup};
use support::fake_configserver::FakeConfigServer;
use support::fake_hub::ConnHandle;
use support::fake_kube::FakeKube;
use support::k8s_objects::{deployment, pod};
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

const NS: &str = "sit1";
const SERVER: &str = "csp-configuration-server";

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn cluster() -> FakeKube {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, SERVER).build());
    kube.apply(pod(NS, "server-1", SERVER, "2026-10-10T11:00:00Z"));
    kube.apply(deployment(NS, "web").build());
    kube.apply(pod(NS, "web-1", "web", "2026-10-10T11:00:00Z"));
    kube
}

fn setup(server: &FakeConfigServer, kube: &FakeKube, source: &Arc<ScriptedSource>) -> Setup {
    Setup {
        kube: Some(kube.clone()),
        source: Some(source.clone()),
        config_server_dialer: Some(server.dialer()),
        extra_env_owned: vec![
            ("LK_CONFIG_SERVER_URL".to_owned(), server.url().to_owned()),
            ("LK_CONFIG_SERVER_DEPLOYMENT".to_owned(), format!("{NS}/{SERVER}")),
        ],
        ..Setup::default()
    }
}

fn allow_nothing(app: &AppRig) {
    app.rig.server.set_config(pb::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: Vec::new(),
        env_allowlist: Vec::new(),
        tenants: vec!["sit1".to_owned()],
    });
}

async fn command(conn: &ConnHandle, id: &str, command: HubCommand) -> FromAgent {
    conn.send(ToAgent::Command(command)).await;
    let id = id.to_owned();
    conn.wait_for(move |m| match m {
        FromAgent::Reply(AgentReply::Op(r)) => r.request_id.as_str() == id,
        FromAgent::Reply(
            AgentReply::Notify { request_id, .. }
            | AgentReply::Served { request_id, .. }
            | AgentReply::File { request_id, .. }
            | AgentReply::Cluster { request_id, .. },
        ) => request_id.as_str() == id,
        _ => false,
    })
    .await
}

#[allow(
    clippy::too_many_lines,
    reason = "one scenario told in order: a busy session, then the two questions the hub asks"
)]
#[tokio::test(start_paused = true)]
async fn notify_only_on_hub_command() {
    let server = FakeConfigServer::start();
    server.serve("/web/sit1,default/master/a.yml", 200, "a: 1\n");
    let kube = cluster();
    let source = Arc::new(ScriptedSource::new());
    source.write("svc/a.yml", b"a: 1\n");
    let mut app = AppRig::start(setup(&server, &kube, &source)).await;
    allow_nothing(&app);
    let conn = app.connected().await;

    // A busy ten minutes: files appear and change, pods come and go. None of it is a reason to touch the config-server.
    std::fs::create_dir_all(app.dir.path().join("svc")).unwrap();
    std::fs::write(app.dir.path().join("svc/a.yml"), b"a: 1\n").unwrap();
    for round in 0..10 {
        source.write(
            &format!("svc/new-{round}.yml"),
            format!("n: {round}\n").as_bytes(),
        );
        kube.apply(pod(NS, &format!("web-{round}"), "web", "2026-10-10T12:00:00Z"));
        sleep(Duration::from_secs(60)).await;
    }
    // The hub reads, writes (refused), restarts and asks for a report. Still no reason.
    command(
        &conn,
        "read",
        HubCommand::ReadFile {
            request_id: rid("read"),
            path: NfsPath::parse("svc/a.yml").unwrap(),
        },
    )
    .await;
    command(
        &conn,
        "write",
        HubCommand::WriteFile {
            request_id: rid("write"),
            path: NfsPath::parse("svc/a.yml").unwrap(),
            expected: Expected::Hash {
                hash: ContentHash::from_bytes([9; 32]),
            },
            bytes: Bytes::from_static(b"a: 2\n"),
        },
    )
    .await;
    command(
        &conn,
        "restart",
        HubCommand::RestartDeployment {
            request_id: rid("restart"),
            service: ServiceRef::new(NS, "web").unwrap(),
        },
    )
    .await;
    command(
        &conn,
        "report",
        HubCommand::RequestClusterReport {
            request_id: rid("report"),
        },
    )
    .await;
    assert!(
        server.requests().is_empty(),
        "the agent reached the config-server on its own: {:#?}",
        server.requests()
    );

    // The hub asks, and the agent makes exactly one call for each question.
    let notified = command(
        &conn,
        "notify",
        HubCommand::NotifyConfigServer {
            request_id: rid("notify"),
            paths: vec![NfsPath::parse("web/a.yml").unwrap()],
        },
    )
    .await;
    assert!(
        matches!(
            &notified,
            FromAgent::Reply(AgentReply::Notify { status: 200, .. })
        ),
        "{notified:?}"
    );
    assert_eq!(server.notifications().len(), 1);
    assert_eq!(server.notifications()[0].paths, ["web/a.yml"]);
    assert_eq!(server.fetches().len(), 0);

    let fetched = command(
        &conn,
        "fetch",
        HubCommand::FetchServed {
            request_id: rid("fetch"),
            request: ServeRequest {
                application: AppName::parse("web").unwrap(),
                tenant: TenantId::parse("sit1").unwrap(),
                channel: None,
                file: NfsPath::parse("a.yml").unwrap(),
            },
        },
    )
    .await;
    let FromAgent::Reply(AgentReply::Served { status, bytes, .. }) = fetched else {
        panic!("a served response, got {fetched:?}");
    };
    assert_eq!((status, &bytes[..]), (200, &b"a: 1\n"[..]));
    assert_eq!(
        server.requests().len(),
        2,
        "one POST and one GET, and nothing else"
    );

    // Quiet again afterwards: the calls are not repeated by anything.
    sleep(Duration::from_secs(300)).await;
    assert_eq!(server.requests().len(), 2);
    app.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_agent_reports_the_config_server_start_time_on_connect() {
    let server = FakeConfigServer::start();
    let kube = cluster();
    let source = Arc::new(ScriptedSource::new());
    let app = AppRig::start(setup(&server, &kube, &source)).await;
    allow_nothing(&app);
    let conn = app.connected().await;
    let report = conn
        .wait_for(|m| matches!(m, FromAgent::Cluster(r) if r.full))
        .await;
    let FromAgent::Cluster(report) = report else {
        unreachable!()
    };
    assert_eq!(
        report.config_server_started_at,
        Some(Timestamp::from_unix_millis(1_791_630_000_000)),
        "2026-10-10T11:00:00Z"
    );
    assert!(
        server.requests().is_empty(),
        "knowing the start time takes no call to the config-server"
    );
}

#[tokio::test(start_paused = true)]
async fn without_a_configured_url_the_config_server_commands_are_unsupported() {
    let kube = cluster();
    let source = Arc::new(ScriptedSource::new());
    let app = AppRig::start(Setup {
        kube: Some(kube),
        source: Some(source),
        ..Setup::default()
    })
    .await;
    let conn = app.connected().await;
    for (id, cmd) in [
        (
            "n",
            HubCommand::NotifyConfigServer {
                request_id: rid("n"),
                paths: vec![NfsPath::parse("a.yml").unwrap()],
            },
        ),
        (
            "f",
            HubCommand::FetchServed {
                request_id: rid("f"),
                request: ServeRequest {
                    application: AppName::parse("web").unwrap(),
                    tenant: TenantId::parse("sit1").unwrap(),
                    channel: None,
                    file: NfsPath::parse("a.yml").unwrap(),
                },
            },
        ),
    ] {
        let reply = command(&conn, id, cmd).await;
        assert!(
            matches!(&reply, FromAgent::Reply(AgentReply::Op(r)) if r.error == Some(domain::OpError::Unsupported)),
            "{reply:?}"
        );
    }
}
