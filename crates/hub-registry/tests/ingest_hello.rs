//! `hello` (T1, S5, D88, D79): who may join, what the agent is told, and what a new stream starts from.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use domain::{AgentConfig, SwimlaneId};
use hub_registry::metrics::{MessageKind, Outcome};
use hub_registry::{ManualClock, RegistryConfig};
use ports::{AgentIdentity, ReportSink, SinkError};
use support::rig::{Rig, T, delta, entry, hello, part, root};

#[tokio::test]
async fn nothing_is_accepted_before_hello() {
    let Some(rig) = Rig::new().await else { return };
    assert_eq!(rig.heartbeat(root(1)).await, Err(SinkError::HelloRequired));
    assert_eq!(
        rig.delta(delta(1, None, root(1), vec![])).await,
        Err(SinkError::HelloRequired)
    );
    assert_eq!(
        rig.cluster(domain::ClusterReport {
            full: true,
            deployments: vec![],
            release_hints: vec![],
            config_server_started_at: None,
            sync_windows: vec![],
        })
        .await,
        Err(SinkError::HelloRequired)
    );
    assert_eq!(rig.count("SELECT count(*) FROM agents").await, 0);
}

#[tokio::test]
async fn a_payload_that_names_another_swimlane_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    let other = AgentIdentity::new(SwimlaneId::parse("sit2").unwrap());
    assert_eq!(
        rig.ingest.hello(&other, hello("sit1")).await.unwrap_err(),
        SinkError::IdentityMismatch,
        "S5: an identity can only act for its own swimlane"
    );
    assert_eq!(rig.count("SELECT count(*) FROM agents").await, 0);
}

#[tokio::test]
async fn a_swimlane_no_admin_created_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    let ghost = AgentIdentity::new(SwimlaneId::parse("ghost").unwrap());
    assert_eq!(
        rig.ingest.hello(&ghost, hello("ghost")).await.unwrap_err(),
        SinkError::IdentityMismatch,
        "A15"
    );
    for table in ["agents", "agent_acks", "swimlane_registry"] {
        assert_eq!(
            rig.count(&format!("SELECT count(*) FROM {table}")).await,
            0,
            "{table} must not get a row for a swimlane nobody created"
        );
    }
    assert_eq!(rig.heartbeat(root(1)).await, Err(SinkError::HelloRequired));
}

#[tokio::test]
async fn hello_stores_the_agent_and_answers_with_its_configuration() {
    let Some(rig) = Rig::new().await else { return };
    rig.db.seed_tenant(&rig.pool, "sit1", "sit1").await;
    rig.db.seed_tenant(&rig.pool, "sit1", "sit5").await;
    let config: AgentConfig = rig.hello().await;
    assert_eq!(config.scan_interval_secs, 10);
    assert_eq!(config.heartbeat_interval_secs, 10);
    assert_eq!(config.max_file_bytes, 2 * 1024 * 1024);
    let globs: Vec<&str> = config.deny_globs.iter().map(domain::ShortText::as_str).collect();
    assert!(
        globs.contains(&"*.pem") && globs.contains(&"*private*"),
        "D79: {globs:?}"
    );
    let env: Vec<&str> = config
        .env_allowlist
        .iter()
        .map(domain::ShortText::as_str)
        .collect();
    assert_eq!(
        env,
        [
            "SPRING_APPLICATION_NAME",
            "SPRING_PROFILES_ACTIVE",
            "CONFIG_CLIENT_CACHE_TTL",
            "CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED"
        ],
        "D88"
    );
    let tenants: Vec<&str> = config.tenants.iter().map(domain::TenantId::as_str).collect();
    assert_eq!(tenants, ["sit1", "sit5"], "a set of tenants, not one (Q34)");

    let version: String = sqlx::query_scalar("SELECT agent_version FROM agents WHERE swimlane_id = 'sit1'")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(version, "0.1.0");
    assert_eq!(rig.count("SELECT count(*) FROM agent_acks").await, 1);
    assert_eq!(rig.count("SELECT count(*) FROM swimlane_registry").await, 1);
    // The agent filled in what the admin left empty.
    let (cluster, server): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT cluster, nfs_server FROM swimlanes WHERE id = 'sit1'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(
        (cluster.as_deref(), server.as_deref()),
        (Some("gke-sit1"), Some("nfs.internal"))
    );
}

#[tokio::test]
async fn an_agent_does_not_overwrite_what_the_admin_configured() {
    let Some(rig) = Rig::new().await else { return };
    sqlx::query("UPDATE swimlanes SET nfs_server = 'admin-set.internal', nfs_export = '/admin/export' WHERE id = 'sit1'")
        .execute(&rig.pool)
        .await
        .unwrap();
    rig.hello().await;
    let (server, export): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT nfs_server, nfs_export FROM swimlanes WHERE id = 'sit1'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(server.as_deref(), Some("admin-set.internal"));
    assert_eq!(export.as_deref(), Some("/admin/export"));
}

#[tokio::test]
async fn a_second_hello_keeps_the_swimlanes_history_and_its_root() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    let joined: i64 = sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM joined_at) * 1000)::bigint FROM agents")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    rig.clock.advance_secs(3600);
    rig.hello().await;
    let after: i64 = sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM joined_at) * 1000)::bigint FROM agents")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(joined, after, "joined_at is the first hello");
    assert_eq!(rig.root().await, Some(root(1)));
    assert!(rig.state("a.yml").await.is_some());
    assert_eq!(rig.count("SELECT count(*) FROM agents").await, 1);
    assert_eq!(
        rig.heartbeat(root(1)).await.unwrap(),
        domain::HeartbeatAction::None,
        "a reconnecting agent whose tree has not changed is asked for nothing"
    );
}

#[tokio::test]
async fn a_new_stream_drops_the_staged_parts_of_the_one_before() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    rig.delta(part(1, 0, true, None, root(1), vec![entry("a.yml", b"x: 1", T)]))
        .await
        .unwrap();
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 1);
    rig.hello().await;
    assert_eq!(
        rig.count("SELECT count(*) FROM delta_staging").await,
        0,
        "A14: the agent sends the whole delta again"
    );
    assert!(
        rig.state("a.yml").await.is_none(),
        "a stream that ended early applied nothing"
    );
}

#[tokio::test]
async fn hello_sweeps_staged_parts_older_than_the_ttl_for_every_swimlane() {
    let Some(rig) = Rig::with("sit1", RegistryConfig::default()).await else {
        return;
    };
    rig.db.seed_swimlane(&rig.pool, "sit2").await;
    // An old part of another swimlane, staged a day and a bit ago.
    sqlx::query(
        "INSERT INTO delta_staging (swimlane_id, seq, part, new_root, metadata, staged_at) \
         VALUES ('sit2', 9, 0, $1::bytea, '{}'::jsonb, timestamptz 'epoch' + ($2::bigint * interval '1 millisecond'))",
    )
    .bind(root(9).as_bytes().as_slice())
    .bind(ManualClock::T0_MS - 25 * 3600 * 1000)
    .execute(&rig.pool)
    .await
    .unwrap();
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 1);
    rig.hello().await;
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 0);
}

#[tokio::test]
async fn the_sweep_can_be_called_on_its_own_and_keeps_fresh_parts() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    rig.delta(part(1, 0, true, None, root(1), vec![entry("a.yml", b"x: 1", T)]))
        .await
        .unwrap();
    assert_eq!(
        rig.ingest.sweep_staging().await.unwrap(),
        0,
        "a fresh part waits for its delta"
    );
    rig.clock.advance_secs(25 * 3600);
    assert_eq!(rig.ingest.sweep_staging().await.unwrap(), 1);
}

#[tokio::test]
async fn streams_are_bounded_and_a_refused_hello_changes_nothing() {
    // Rule 5: the map of live streams has a size limit. A swimlane over it is refused and leaves no trace; the swimlane that is
    // already in comes back as often as it likes (a reconnecting agent must never be locked out by other swimlanes).
    let config = RegistryConfig {
        max_streams: 1,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.db.seed_swimlane(&rig.pool, "sit2").await;
    rig.hello().await;

    let other = AgentIdentity::new(SwimlaneId::parse("sit2").unwrap());
    assert_eq!(
        rig.ingest.hello(&other, hello("sit2")).await.unwrap_err(),
        SinkError::Unavailable,
        "no room for another stream"
    );
    for table in ["agents", "agent_acks", "swimlane_registry"] {
        assert_eq!(
            rig.count(&format!(
                "SELECT count(*) FROM {table} WHERE swimlane_id = 'sit2'"
            ))
            .await,
            0,
            "{table}: a refused hello writes nothing"
        );
    }
    assert_eq!(
        rig.ingest
            .heartbeat(
                &other,
                domain::Heartbeat {
                    scan_seq: 1,
                    merkle_root: root(1),
                    file_count: 0
                }
            )
            .await,
        Err(SinkError::HelloRequired),
        "the refused swimlane has no stream"
    );
    assert_eq!(
        rig.metrics
            .message_count(MessageKind::Hello, Outcome::Unavailable),
        1
    );

    rig.hello().await;
    rig.delta(delta(1, None, root(1), vec![entry("a.yml", b"x: 1", T)]))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_hello_that_fails_gives_its_place_back() {
    // An unknown swimlane must not use up one of the places: the bound is on swimlanes that are really connected.
    let config = RegistryConfig {
        max_streams: 1,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    let ghost = AgentIdentity::new(SwimlaneId::parse("ghost").unwrap());
    assert_eq!(
        rig.ingest.hello(&ghost, hello("ghost")).await.unwrap_err(),
        SinkError::IdentityMismatch
    );
    rig.hello().await;
}

#[tokio::test]
async fn the_admins_deny_globs_reach_the_agent_and_never_replace_the_built_in_ones() {
    let config = RegistryConfig {
        extra_deny_globs: vec!["secrets/*".to_owned()],
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    let globs: Vec<String> = rig
        .hello()
        .await
        .deny_globs
        .iter()
        .map(|g| g.as_str().to_owned())
        .collect();
    assert!(globs.contains(&"secrets/*".to_owned()));
    assert!(globs.contains(&"*.jks".to_owned()));
}
