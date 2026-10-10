//! What the agent writes to its log (T7, S10, S21): paths, hashes and ids, never file content, tokens, keys or the
//! values of environment variables, and never the Kubernetes API server's own text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use agent::config::LogLevel;
use agent::ops::logging;
use domain::{AgentReply, ContentHash, Expected, HubCommand, NfsPath, RequestId};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use support::app_rig::{AppRig, Setup};
use support::fake_kube::FakeKube;
use support::k8s_objects::{deployment, pod};
use support::log_capture::LogCapture;
use support::rig::WI_TOKEN;
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

const NS: &str = "sit1";
const FILE_MARKER: &str = "FILE-CONTENT-MARKER-5c1d";
const WRITE_MARKER: &str = "WRITTEN-BYTES-MARKER-a9e2";
const ALLOWED_ENV_MARKER: &str = "ALLOWED-ENV-VALUE-MARKER-31b7";
const SECRET_ENV_MARKER: &str = "SECRET-ENV-VALUE-MARKER-77c0";
const OTHER_ENV_MARKER: &str = "OTHER-ENV-VALUE-MARKER-0f4d";

/// The 32-byte private scalar inside a PKCS#8 P-256 key.
fn private_scalar(pkcs8: &[u8]) -> Vec<u8> {
    let marker = [0x02_u8, 0x01, 0x01, 0x04, 0x20];
    let at = pkcs8.windows(marker.len()).position(|w| w == marker).unwrap() + marker.len();
    pkcs8[at..at + 32].to_vec()
}

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn cluster_with_env() -> FakeKube {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "web")
            .env("CONFIG_CLIENT_CACHE_TTL", ALLOWED_ENV_MARKER)
            .env("DB_PASSWORD", SECRET_ENV_MARKER)
            .env("JAVA_OPTS", OTHER_ENV_MARKER)
            .build(),
    );
    kube.apply(pod(NS, "web-1", "web", "2026-10-10T11:00:00Z"));
    kube
}

fn config_allowing(names: &[&str]) -> pb::AgentConfig {
    pb::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: Vec::new(),
        env_allowlist: names.iter().map(|n| (*n).to_owned()).collect(),
        tenants: vec!["sit1".to_owned()],
    }
}

/// A full session at every log level, including the libraries' own: joining, connecting, scanning, writing, reading,
/// reporting the cluster, restarting, renewing the certificate and shutting down.
#[tokio::test(start_paused = true)]
async fn log_capture_contains_only_paths_and_hashes() {
    let capture = LogCapture::new();
    let _installed = capture.install();

    let source = Arc::new(ScriptedSource::new());
    source.write("svc/a.yml", format!("key: {FILE_MARKER}\r\n").as_bytes());
    let kube = cluster_with_env();
    let mut app = AppRig::start_on(
        support::rig::Rig::new(),
        Setup {
            kube: Some(kube.clone()),
            source: Some(source.clone()),
            renewal: true,
            ..Setup::default()
        },
    )
    .await;
    app.rig
        .server
        .set_config(config_allowing(&["CONFIG_CLIENT_CACHE_TTL", "DB_PASSWORD"]));
    // The key of the identity the agent connects with must not be logged.
    let key_scalar = private_scalar(app.identity.current().key().pkcs8_der());
    let conn = app.connected().await;
    let report = conn.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;
    // The allowed value is sent (that is its purpose); the secret-named one is not, and neither is the other.
    let FromAgent::Cluster(report) = report else {
        unreachable!()
    };
    let values: Vec<_> = report.deployments[0]
        .env_values
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(values, vec!["CONFIG_CLIENT_CACHE_TTL"]);

    // The file is read, written (with a wrong hash, then a right one), and changed on the other side.
    std::fs::create_dir_all(app.dir.path().join("svc")).unwrap();
    std::fs::write(
        app.dir.path().join("svc/a.yml"),
        format!("key: {FILE_MARKER}\r\n"),
    )
    .unwrap();
    let path = NfsPath::parse("svc/a.yml").unwrap();
    conn.send(ToAgent::Command(HubCommand::ReadFile {
        request_id: rid("read-1"),
        path: path.clone(),
    }))
    .await;
    let read = conn
        .wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::File { .. })))
        .await;
    let FromAgent::Reply(AgentReply::File { hash, .. }) = read else {
        unreachable!()
    };
    for (id, expected) in [
        (
            "write-1",
            Expected::Hash {
                hash: ContentHash::from_bytes([1; 32]),
            },
        ),
        ("write-2", Expected::Hash { hash }),
    ] {
        conn.send(ToAgent::Command(HubCommand::WriteFile {
            request_id: rid(id),
            path: path.clone(),
            expected,
            bytes: bytes::Bytes::from(format!("key: {WRITE_MARKER}\r\n")),
        }))
        .await;
        conn.wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::Op(r)) if r.request_id.as_str() == id))
            .await;
    }
    source.write("svc/b.yml", format!("other: {FILE_MARKER}\n").as_bytes());
    conn.wait_for(|m| matches!(m, FromAgent::Delta(_))).await;

    // A failure of the Kubernetes API, and the certificate renewed over the stream half way through its lifetime.
    kube.fail_next_lists(&[500]);
    sleep(Duration::from_secs(13 * 3600)).await;
    app.stop().await.unwrap();

    let log = capture.text();
    assert!(log.contains("svc/a.yml"), "the log names paths:\n{log}");
    for secret in [
        FILE_MARKER,
        WRITE_MARKER,
        ALLOWED_ENV_MARKER,
        SECRET_ENV_MARKER,
        OTHER_ENV_MARKER,
        WI_TOKEN,
        "BEGIN PRIVATE KEY",
        "BEGIN EC PRIVATE KEY",
    ] {
        assert!(!log.contains(secret), "{secret} reached the log");
    }
    assert!(
        !log.contains(WI_TOKEN.split('.').nth(1).unwrap()),
        "the payload of the token reached the log"
    );
    let hex = key_scalar.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    assert!(!log.contains(&hex), "key bytes reached the log");
}

/// The Kubernetes client crates log the API server's `Status` text. Under the production filter that never shows.
#[tokio::test(start_paused = true)]
async fn kube_status_messages_never_reach_the_log() {
    async fn run_denied() {
        let kube = cluster_with_env();
        let mut app = AppRig::start(Setup {
            kube: Some(kube.clone()),
            ..Setup::default()
        })
        .await;
        // The first lists are refused, with a message of the server's own (the fake's "denied by the fake RBAC").
        kube.deny_everything(403);
        let _ = app.connected().await;
        sleep(Duration::from_secs(60)).await;
        app.stop().await.unwrap();
    }

    // Control: with every target at every level, the server's text does reach the log, so the test below can fail.
    let everything = LogCapture::new();
    {
        let _installed = everything.install();
        run_denied().await;
    }
    assert!(
        everything.text().contains("denied by the fake RBAC"),
        "the control no longer sees the server's text; this test has stopped testing the filter:\n{}",
        everything.text()
    );

    // The production filter drops it, and still says that the watch failed, with the status and the kind.
    let production = LogCapture::new();
    {
        let subscriber = logging::subscriber(LogLevel::Trace, production.clone());
        let _installed = tracing::subscriber::set_default(subscriber);
        run_denied().await;
    }
    let log = production.text();
    assert!(!log.contains("denied by the fake RBAC"), "{log}");
    assert!(!log.contains("Forbidden"), "{log}");
    assert!(
        log.contains("the Kubernetes watch failed"),
        "our own line stays:\n{log}"
    );
    assert!(log.contains("\"status\":403"), "{log}");
}

#[tokio::test(start_paused = true)]
async fn every_log_line_is_one_json_object_with_the_fields_operators_search_for() {
    let capture = LogCapture::new();
    let subscriber = logging::subscriber(LogLevel::Debug, capture.clone());
    let _installed = tracing::subscriber::set_default(subscriber);
    let mut app = AppRig::start(Setup::default()).await;
    let conn = app.connected().await;
    conn.send(ToAgent::Command(HubCommand::ReadFile {
        request_id: rid("read-1"),
        path: NfsPath::parse("svc/missing.yml").unwrap(),
    }))
    .await;
    conn.wait_for(|m| matches!(m, FromAgent::Reply(_))).await;
    app.stop().await.unwrap();

    let log = capture.text();
    let lines: Vec<&str> = log.lines().collect();
    assert!(lines.len() >= 3, "{log}");
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert!(
            value["level"].is_string() && value["target"].is_string() && value["timestamp"].is_string(),
            "{line}"
        );
    }
    assert!(
        log.contains("\"request_id\":\"read-1\"") || log.contains("read-1"),
        "{log}"
    );
}
