//! The file as the config-server serves it (T6, plan A4, D79, D86, D88): the agent is asked once per request, within a timeout and a
//! concurrency limit; a denied file is never asked for; what the config-server says is data (its bytes become text, its status becomes
//! a status of ours and its words go nowhere). Over a real Postgres, with a scripted agent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use domain::{AgentReply, AppName, FileRole, HubCommand, ServeRequest, SwimlaneId, TenantId};
use hub_registry::config::RegistryConfig;
use hub_registry::{RegistryApi, Tagged};
use ports::ReadError;
use support::reads::{Reads, nfs};

fn request(file: &str) -> ServeRequest {
    ServeRequest {
        application: AppName::parse("core").unwrap(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: None,
        file: nfs(file),
    }
}

fn sit1() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

type Script = Arc<dyn Fn(&HubCommand) -> Option<AgentReply> + Send + Sync>;

fn answer(status: u16, body: &'static [u8]) -> Script {
    Arc::new(move |cmd: &HubCommand| match cmd {
        HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Served {
            request_id: request_id.clone(),
            status,
            bytes: Bytes::from_static(body),
        }),
        _ => None,
    })
}

async fn world(config: RegistryConfig) -> Option<Reads> {
    let mut r = Reads::with(&["sit1"], config).await?;
    r.sql(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) \
         VALUES ('sit1', 'ns', 'core', '{\"SPRING_APPLICATION_NAME\": \"core\"}')",
    )
    .await;
    r.world
        .tenant_commit("sit1", &[("data/config/core.yml", Some("name: from git\n"))]);
    r.world
        .inventory(&[
            ("application.yml", "common: base\n"),
            ("core.yml", "port: 2\n"),
            ("core-sit1.yml", "name: sit1\n"),
            ("keys/server.pem", "-----BEGIN PRIVATE KEY-----\nAAAA\n"),
        ])
        .await;
    r.world.whole().await;
    Some(r)
}

#[tokio::test]
async fn the_answer_has_the_servers_text_the_engines_role_and_sources_and_the_placeholders_left() {
    let Some(r) = world(RegistryConfig::default()).await else {
        return;
    };
    r.gateway
        .attach(&sit1(), answer(200, b"port: 2\nname: ${NOT_THERE}\n"));
    let v = Reads::viewer();
    let Tagged::Fresh { etag, value } = r
        .service
        .served_tagged(&v, &sit1(), &request("core.yml"), None)
        .await
        .unwrap()
    else {
        panic!("no tag was given");
    };
    assert_eq!(value.text.as_deref(), Some("port: 2\nname: ${NOT_THERE}\n"));
    assert_eq!(value.role, FileRole::PropertySource);
    assert!(!value.content_withheld);
    assert_eq!(
        value
            .unresolved_placeholders
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["NOT_THERE"],
        "what is still in the served text is what the config-server did not replace"
    );
    assert!(
        !value.sources.is_empty(),
        "the layers the engine's effective config is made of"
    );
    assert_eq!(value.hash, ports::content_hash(b"port: 2\nname: ${NOT_THERE}\n"));
    // The agent was asked once, with exactly this request.
    let asked: Vec<HubCommand> = r.gateway.received(&sit1());
    assert_eq!(asked.len(), 1);
    assert!(
        matches!(&asked[0], HubCommand::FetchServed { request, .. } if *request == request_of("core.yml"))
    );
    // A matching tag is a 304, after the round trip (the answer is the thing asked for).
    let again = r
        .service
        .served_tagged(&v, &sit1(), &request("core.yml"), Some(&etag))
        .await
        .unwrap();
    assert!(matches!(again, Tagged::NotModified { .. }));
    // Another answer from the config-server is another tag.
    r.gateway.attach(&sit1(), answer(200, b"port: 3\n"));
    let Tagged::Fresh { etag: changed, .. } = r
        .service
        .served_tagged(&v, &sit1(), &request("core.yml"), Some(&etag))
        .await
        .unwrap()
    else {
        panic!("the old tag no longer matches");
    };
    assert_ne!(changed, etag);
}

fn request_of(file: &str) -> ServeRequest {
    request(file)
}

#[tokio::test]
async fn a_denied_file_is_withheld_and_the_agent_is_not_asked() {
    let Some(r) = world(RegistryConfig::default()).await else {
        return;
    };
    r.gateway
        .attach(&sit1(), answer(200, b"-----BEGIN PRIVATE KEY-----\nAAAA\n"));
    let v = Reads::viewer();
    let Tagged::Fresh { value, .. } = r
        .service
        .served_tagged(&v, &sit1(), &request("keys/server.pem"), None)
        .await
        .unwrap()
    else {
        panic!("no tag was given");
    };
    assert!(value.content_withheld && value.text.is_none() && value.sources.is_empty());
    assert!(
        r.gateway.received(&sit1()).is_empty(),
        "D79: nothing of a denied file is fetched"
    );
}

#[tokio::test]
async fn what_the_agent_and_the_config_server_say_become_our_statuses_and_their_words_go_nowhere() {
    let Some(r) = world(RegistryConfig::default()).await else {
        return;
    };
    let v = Reads::viewer();
    let ask = |file: &str| {
        let (service, v, req) = (r.service.clone(), v.clone(), request(file));
        async move { service.served_tagged(&v, &sit1(), &req, None).await }
    };
    // No agent attached.
    assert_eq!(ask("core.yml").await.unwrap_err(), ReadError::Unavailable);
    // The config-server's own answers.
    r.gateway
        .attach(&sit1(), answer(404, b"no such file at /srv/config/secret-path"));
    assert_eq!(ask("core.yml").await.unwrap_err(), ReadError::NotFound);
    for status in [400, 401, 403, 500, 502, 503] {
        r.gateway.attach(&sit1(), answer(status, b"internal detail"));
        assert_eq!(
            ask("core.yml").await.unwrap_err(),
            ReadError::Unavailable,
            "{status}"
        );
    }
    // Something other than a served file.
    r.gateway.attach(
        &sit1(),
        Arc::new(|cmd: &HubCommand| match cmd {
            HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Notify {
                request_id: request_id.clone(),
                status: 200,
            }),
            _ => None,
        }),
    );
    assert_eq!(ask("core.yml").await.unwrap_err(), ReadError::Unavailable);
    // A tenant the swimlane does not deploy, a swimlane that is not there, and a file that is not text.
    let other = ServeRequest {
        tenant: TenantId::parse("sit9").unwrap(),
        ..request("core.yml")
    };
    assert_eq!(
        r.service
            .served_tagged(&v, &sit1(), &other, None)
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.service
            .served_tagged(
                &v,
                &SwimlaneId::parse("nope").unwrap(),
                &request("core.yml"),
                None
            )
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    r.gateway.attach(&sit1(), answer(200, &[0xFF, 0xFE, 0x41]));
    let Tagged::Fresh { value, .. } = r
        .service
        .served_tagged(&v, &sit1(), &request("core.yml"), None)
        .await
        .unwrap()
    else {
        panic!("no tag was given");
    };
    assert!(
        value.text.is_none(),
        "bytes that are not text are not made into text"
    );
}

#[tokio::test]
async fn a_silent_agent_is_given_the_timeout_and_no_more_than_the_limit_are_in_flight() {
    let config = RegistryConfig {
        served_timeout: Duration::from_millis(300),
        served_concurrency: 2,
        ..RegistryConfig::default()
    };
    let Some(r) = world(config).await else { return };
    let v = Reads::viewer();
    // The agent never answers.
    r.gateway.attach(&sit1(), Arc::new(|_: &HubCommand| None));
    let started = Instant::now();
    assert_eq!(
        r.service
            .served_tagged(&v, &sit1(), &request("core.yml"), None)
            .await
            .unwrap_err(),
        ReadError::Unavailable
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(250) && took < Duration::from_secs(3),
        "{took:?}"
    );

    // Four at once against a limit of two: two go to the agent and wait for it, and two are refused before the agent is asked.
    let before = r.gateway.received(&sit1()).len();
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let (service, v) = (r.service.clone(), v.clone());
        tasks.push(tokio::spawn(async move {
            service
                .served_tagged(&v, &sit1(), &request("core.yml"), None)
                .await
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap().unwrap_err(), ReadError::Unavailable);
    }
    assert_eq!(
        r.gateway.received(&sit1()).len() - before,
        2,
        "no more round trips than the limit; the others never reached the agent"
    );
    // The slots come back.
    r.gateway.attach(&sit1(), answer(200, b"port: 2\n"));
    assert!(
        r.service
            .served_tagged(&v, &sit1(), &request("core.yml"), None)
            .await
            .is_ok()
    );
}
