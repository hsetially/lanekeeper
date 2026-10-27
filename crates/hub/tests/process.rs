//! The hub as a process (plan 18, T1; S6, S7, S10, S16): it starts against a real Postgres, serves, drains on SIGTERM and exits
//! cleanly, exits at once on a second signal, and writes no secret.
//!
//! The program is the `hub_dev` example: the same `hub::process::entry` as the binary, with secrets read from files instead of
//! Secret Manager (the real binary cannot start without Google, plan 18 G23; its refusals are in `refuses_to_start.rs`).
//! The database is a real Postgres 16 with TLS 1.3, because the pool verifies the server and has no mode that does not.
//! `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing Docker daemon a failure and not a skip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

#[path = "support/child.rs"]
mod child;
#[path = "support/git_remote.rs"]
mod git_remote;
#[path = "support/net.rs"]
mod net;
#[path = "support/rig.rs"]
mod rig;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use rig::Rig;
use tokio::io::AsyncWriteExt;

async fn eventually<F, Fut>(within: Duration, what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + within;
    loop {
        if check().await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {within:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn becomes_ready(ops: SocketAddr) {
    eventually(Duration::from_secs(20), "/readyz answering 200", || async move {
        net::http_get(ops, "/readyz").await.status == 200
    })
    .await;
}

#[tokio::test]
async fn the_hub_serves_healthz_and_readyz_and_exits_zero_on_sigterm() {
    let Some(rig) = Rig::start().await else { return };
    let mut hub = rig.spawn(&[]).await;
    let (web, ops) = hub.wait_listening(Duration::from_secs(30));

    let health = net::http_get(ops, "/healthz").await;
    assert_eq!((health.status, health.body.as_str()), (200, "ok\n"));
    becomes_ready(ops).await;
    let ready = net::http_get(ops, "/readyz").await;
    assert_eq!((ready.status, ready.body.as_str()), (200, "ready\n"));

    // The web listener serves the app over TLS 1.3 with the certificate from the secrets, and the CSP of every page.
    let page = net::https_get(web, &rig.ca.cert_der(), "/").await;
    assert_eq!(page.status, 200, "{page:?}");
    assert!(page.body.contains(r#"<div id="root">"#), "{}", page.body);
    assert!(
        !page.body.contains("__CSP_NONCE__"),
        "the nonce placeholder was left in the page"
    );
    let csp = page.header("content-security-policy").unwrap();
    assert!(
        csp.contains("'nonce-") && csp.contains("require-trusted-types-for 'script'"),
        "{csp}"
    );
    // The identity module is composed in (T4): a guarded route answers a request with no session `401`, and a path under a prefix
    // the server owns that no route answers is a typed `404` and never the app.
    let me = net::https_get(web, &rig.ca.cert_der(), "/api/v1/me").await;
    assert_eq!(me.status, 401, "{me:?}");
    for path in [
        "/api/v1/nothing-here",
        "/auth/nothing-here",
        "/mcp",
        "/.well-known/nothing-here",
    ] {
        let missing = net::https_get(web, &rig.ca.cert_der(), path).await;
        assert_eq!(missing.status, 404, "{path}: {missing:?}");
        assert!(
            !missing.body.contains(r#"<div id="root">"#),
            "{path} answered with the app"
        );
    }
    // The certificate's expiry is exported, and the hub is serving only TLS 1.3.
    let metrics = net::http_get(ops, "/metrics").await.body;
    assert!(
        metrics.contains("lanekeeper_web_cert_not_after_seconds"),
        "{metrics}"
    );
    let tls12 = net::client_config(&rig.ca.cert_der(), &[&rustls::version::TLS12]);
    assert!(
        net::tls_connect(web, tls12).await.is_err(),
        "the hub accepted TLS 1.2"
    );

    // SIGTERM: readiness fails first, then the hub exits 0 within the grace and drain periods.
    let told = Instant::now();
    hub.signal("TERM");
    eventually(
        Duration::from_secs(3),
        "/readyz answering 503 while shutting down",
        || async move {
            let reply = net::http_get(ops, "/readyz").await;
            reply.status == 503 && reply.body.contains("shutting_down")
        },
    )
    .await;
    let status = hub.exit_within(Duration::from_secs(15));
    assert_eq!(status.code(), Some(0), "{}", hub.output());
    assert!(
        told.elapsed() >= Duration::from_secs(1),
        "the grace period was skipped"
    );
    let output = hub.output();
    assert!(output.contains(r#""message":"stopped""#), "{output}");
    // Every line the hub wrote is one JSON object (S10), so a path or a name with a newline cannot forge a second line.
    for line in output.lines() {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "not JSON: {line}"
        );
    }
}

#[tokio::test]
async fn the_agent_and_replica_listeners_speak_tls_13_under_their_own_cas() {
    let Some(rig) = Rig::start().await else { return };
    let mut hub = rig.spawn(&[("LK_SHUTDOWN_GRACE_SECS", "0")]).await;
    let all = hub.wait_listening_all(Duration::from_secs(30));
    becomes_ready(all.ops).await;

    // The CA certificates are public: the example wrote them where the hub read them (they are Secret Manager secrets in
    // production). Each is the only root its listener's certificate chains to.
    let read_ca = |name: &str| {
        let pem = std::fs::read_to_string(rig.secrets.path().join(name)).unwrap();
        rustls::pki_types::CertificateDer::from(
            hub::web_tls::decode_ca_certificate(&domain::Secret::new(pem)).unwrap(),
        )
    };
    let (agent_ca, replica_ca) = (read_ca("agent-ca-cert"), read_ca("replica-ca-cert"));
    assert_ne!(agent_ca, replica_ca, "two CAs, two keys");
    let h2 = |ca: &rustls::pki_types::CertificateDer<'static>, version| {
        net::client_config_with(ca, &[version], b"h2")
    };
    for (what, addr, own, other) in [
        ("the agent listener", all.agent, &agent_ca, &replica_ca),
        ("the replica listener", all.replica, &replica_ca, &agent_ca),
    ] {
        // Its own CA's certificate, TLS 1.3, h2: the handshake completes, and the client sees it negotiated both.
        let stream = net::tls_connect(addr, h2(own, &rustls::version::TLS13))
            .await
            .unwrap();
        let (_, connection) = stream.get_ref();
        assert_eq!(
            connection.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3),
            "{what}"
        );
        assert_eq!(connection.alpn_protocol(), Some(b"h2".as_slice()), "{what}");
        // The other listener's CA does not vouch for it, and neither does TLS 1.2 get in.
        assert!(
            net::tls_connect(addr, h2(other, &rustls::version::TLS13))
                .await
                .is_err(),
            "{what} chains to the other CA"
        );
        assert!(
            net::tls_connect(addr, h2(own, &rustls::version::TLS12))
                .await
                .is_err(),
            "{what} accepted TLS 1.2"
        );
    }
    // The web listener's certificate is not the agent CA's: the three trust roots are three.
    assert!(
        net::tls_connect(
            all.agent,
            net::client_config_with(&rig.ca.cert_der(), &[&rustls::version::TLS13], b"h2")
        )
        .await
        .is_err(),
        "the agent listener chains to the web CA"
    );

    // The platform's counters are exported on the hub's registry, and the web app still answers beside them.
    let metrics = net::http_get(all.ops, "/metrics").await.body;
    for name in [
        "lanekeeper_gateway_",
        "lanekeeper_job_",
        "lanekeeper_agent_",
        "lanekeeper_events_",
    ] {
        assert!(metrics.contains(name), "no {name} metric in:\n{metrics}");
    }
    hub.signal("TERM");
    assert_eq!(
        hub.exit_within(Duration::from_secs(15)).code(),
        Some(0),
        "{}",
        hub.output()
    );
}

#[tokio::test]
async fn the_jobs_hold_their_leases_while_the_hub_runs_and_release_them_when_it_stops() {
    let Some(rig) = Rig::start().await else { return };
    let mut hub = rig.spawn(&[("LK_SHUTDOWN_GRACE_SECS", "0")]).await;
    let all = hub.wait_listening_all(Duration::from_secs(30));
    becomes_ready(all.ops).await;
    let ready = net::http_get(all.ops, "/readyz").await;
    assert_eq!((ready.status, ready.body.as_str()), (200, "ready\n"));
    // The jobs hold their leases while the hub is ready: another holder of the same name cannot take them.
    let pool = rig.pg.app_pool().await;
    let holders: i64 = sqlx::query_scalar("SELECT count(*) FROM leases WHERE expires_at > now()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        holders, 5,
        "the four platform jobs and the audit log's shipper (T4) are held by this hub"
    );
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM leases ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        names,
        [
            // The identity module's shipper of the audit log (T4) holds its lease for as long as the hub runs; its checkpoint and its
            // nightly verification take theirs for the length of one run, which is not now.
            "audit.shipper",
            "git-poll",
            "git-tree-index",
            "outbox.prune",
            "webhook-deliveries-prune"
        ]
    );
    // Stopping releases them at once, so another replica takes them without waiting for the lease to run out.
    hub.signal("TERM");
    assert_eq!(
        hub.exit_within(Duration::from_secs(15)).code(),
        Some(0),
        "{}",
        hub.output()
    );
    let still_held: i64 = sqlx::query_scalar("SELECT count(*) FROM leases WHERE expires_at > now()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(still_held, 0, "a hub that stopped cleanly released its leases");
}

#[tokio::test]
async fn a_second_signal_exits_at_once() {
    let Some(rig) = Rig::start().await else { return };
    // No grace and a long drain, so that one request in flight holds the first signal's drain open.
    let mut hub = rig
        .spawn(&[("LK_SHUTDOWN_GRACE_SECS", "0"), ("LK_SHUTDOWN_DRAIN_SECS", "60")])
        .await;
    let (web, ops) = hub.wait_listening(Duration::from_secs(30));
    becomes_ready(ops).await;

    // A request that has sent its head and promised a body it does not send: the limits layer waits for the body.
    let config = net::client_config(&rig.ca.cert_der(), &[&rustls::version::TLS13]);
    let mut stuck = net::tls_connect(web, config).await.unwrap();
    stuck
        .write_all(
            b"POST /api/v1/x HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n{\"a\":",
        )
        .await
        .unwrap();
    stuck.flush().await.unwrap();

    hub.signal("TERM");
    eventually(Duration::from_secs(5), "the drain starting", || async move {
        net::http_get(ops, "/readyz").await.status == 503
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !hub.has_exited(),
        "the hub left while a request was in flight, before any second signal"
    );

    let second = Instant::now();
    hub.signal("TERM");
    let status = hub.exit_within(Duration::from_secs(5));
    assert!(
        second.elapsed() < Duration::from_secs(4),
        "the second signal was not acted on at once"
    );
    assert_eq!(
        status.code(),
        Some(1),
        "told twice is not a clean stop: {}",
        hub.output()
    );
    assert!(hub.output().contains("a second stop request"), "{}", hub.output());
    drop(stuck);
}

#[tokio::test]
async fn sigterm_right_after_the_ops_port_opens_still_exits_cleanly() {
    let Some(rig) = Rig::start().await else { return };
    let mut hub = rig.spawn(&[]).await;
    // The line is written when both listeners are bound, which is as early as a client can see the ops port. The signal is
    // sent before anything has been asked of the hub.
    hub.wait_listening(Duration::from_secs(30));
    hub.signal("TERM");
    let status = hub.exit_within(Duration::from_secs(15));
    assert_eq!(status.code(), Some(0), "{}", hub.output());
}

#[tokio::test]
async fn sigint_stops_the_hub_the_same_way() {
    let Some(rig) = Rig::start().await else { return };
    let mut hub = rig.spawn(&[("LK_SHUTDOWN_GRACE_SECS", "0")]).await;
    hub.wait_listening(Duration::from_secs(30));
    hub.signal("INT");
    assert_eq!(
        hub.exit_within(Duration::from_secs(15)).code(),
        Some(0),
        "{}",
        hub.output()
    );
}

#[tokio::test]
async fn no_planted_secret_appears_in_the_process_output() {
    let Some(rig) = Rig::start().await else { return };
    // Things that must not appear: the database password (also in the URL file), the web key, and a recognisable slice of it.
    let key_slice: String = rig.leaf.key_pem.lines().nth(1).unwrap().to_owned();
    let planted = [
        tls_pg::PLANTED_PASSWORD.to_owned(),
        tls_pg::APP_ROLE.1.to_owned(),
        tls_pg::MIGRATOR_ROLE.1.to_owned(),
        key_slice,
        rig.pg.url(),
    ];

    // A run that serves, takes some requests (one of them bad) and stops.
    let mut hub = rig.spawn(&[("LK_SHUTDOWN_GRACE_SECS", "0")]).await;
    let (web, ops) = hub.wait_listening(Duration::from_secs(30));
    becomes_ready(ops).await;
    let _ = net::https_get(web, &rig.ca.cert_der(), "/").await;
    let _ = net::https_get(web, &rig.ca.cert_der(), "/api/v1/../etc/passwd").await;
    let _ = net::http_get(ops, "/metrics").await;
    hub.signal("TERM");
    assert_eq!(hub.exit_within(Duration::from_secs(15)).code(), Some(0));
    let served = hub.output();

    // A run that cannot start, because the key is not the certificate's.
    std::fs::write(
        rig.secrets.path().join("web-key"),
        rig.ca.leaf(&["other"]).key_pem,
    )
    .unwrap();
    let mut refused = rig.spawn(&[]).await;
    let status = refused.exit_within(Duration::from_secs(30));
    assert_eq!(status.code(), Some(1));
    let failed = refused.output();
    assert!(
        failed.contains("LK_WEB_CERT_REF"),
        "the failure names the setting: {failed}"
    );

    for (what, output) in [
        ("a run that served", &served),
        ("a run that could not start", &failed),
    ] {
        for secret in &planted {
            assert!(
                !output.contains(secret.as_str()),
                "{what} wrote a planted secret: {secret}"
            );
        }
    }
}

#[tokio::test]
async fn readyz_follows_the_schema_and_a_hub_that_is_ahead_of_it_says_which_check() {
    use sqlx::Connection;

    let Some(rig) = Rig::start().await else { return };
    let mut hub = rig.spawn(&[("LK_SHUTDOWN_GRACE_SECS", "0")]).await;
    let (_web, ops) = hub.wait_listening(Duration::from_secs(30));
    becomes_ready(ops).await;

    // The schema loses what migration 8 added (a restore to an older backup, or a person at a console): the hub is not ready,
    // and says which check, without serving errors in the meantime.
    let options = hub::db::connect_options_for(
        &domain::Secret::new(rig.pg.url_for("postgres", tls_pg::PLANTED_PASSWORD)),
        &rig.pg.ca_file(),
        Duration::from_secs(30),
    )
    .unwrap();
    let mut admin = hub::db::connect_one(&options).await.unwrap();
    sqlx::query("ALTER TABLE blobs RENAME COLUMN touched_at TO touched_at_gone")
        .execute(&mut admin)
        .await
        .unwrap();
    eventually(
        Duration::from_secs(5),
        "/readyz failing on the schema",
        || async move {
            let reply = net::http_get(ops, "/readyz").await;
            reply.status == 503 && reply.body.contains("schema")
        },
    )
    .await;
    let reply = net::http_get(ops, "/readyz").await;
    assert!(
        !reply.body.contains("postgres") && !reply.body.contains("touched_at"),
        "the body names a check, not the SQL: {reply:?}"
    );

    sqlx::query("ALTER TABLE blobs RENAME COLUMN touched_at_gone TO touched_at")
        .execute(&mut admin)
        .await
        .unwrap();
    becomes_ready(ops).await;
    admin.close().await.unwrap();
    hub.signal("TERM");
    assert_eq!(hub.exit_within(Duration::from_secs(15)).code(), Some(0));
}
