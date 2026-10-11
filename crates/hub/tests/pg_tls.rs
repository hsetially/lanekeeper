//! S6 (T1): the Postgres pool always uses TLS with full verification against the cluster's CA.
//!
//! Negative cases run against a small fake server that speaks only the start of the Postgres protocol (the SSL request and
//! the TLS handshake), so they need no Docker and always run. The positive case, and a wrong CA against a real server, run
//! against Postgres 16 with `ssl=on` in testcontainers; without a Docker daemon they print `SKIPPED`, and with
//! `LK_REQUIRE_DOCKER=1` (set by `just verify-03b`) a missing daemon fails them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

mod support;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use domain::Secret;
use hub::config::DatabaseSettings;
use hub::db::{DbError, connect, connect_options};
use support::pki::TestCa;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

fn settings(url: &str, ca_file: &Path) -> DatabaseSettings {
    DatabaseSettings {
        url: Secret::new(url.to_owned()),
        ca_file: ca_file.to_path_buf(),
        max_connections: 2,
        acquire_timeout: Duration::from_secs(3),
        statement_timeout: Duration::from_secs(30),
    }
}

/// What a fake server saw of one connection.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    /// The client asked for TLS, we said no (or it never asked).
    NoTls,
    /// The TLS handshake completed.
    Handshake,
    /// The client abandoned the TLS handshake.
    HandshakeFailed,
    /// The client started a plain startup message without asking for TLS.
    PlainStartup,
}

/// A server that answers the SSL request with `S` and then a TLS handshake (with `tls`), or with `N` when `tls` is `None`.
async fn fake_postgres(tls: Option<TlsAcceptor>) -> (SocketAddr, tokio::sync::mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let tx = tx.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                let mut head = [0u8; 8];
                if stream.read_exact(&mut head).await.is_err() {
                    return;
                }
                // SSLRequest: length 8, code 80877103.
                if head != [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f] {
                    let _ = tx.send(Seen::PlainStartup).await;
                    return;
                }
                match tls {
                    None => {
                        let _ = stream.write_all(b"N").await;
                        let _ = tx.send(Seen::NoTls).await;
                    }
                    Some(acceptor) => {
                        if stream.write_all(b"S").await.is_err() {
                            return;
                        }
                        let seen = match acceptor.accept(stream).await {
                            Ok(_) => Seen::Handshake,
                            Err(_) => Seen::HandshakeFailed,
                        };
                        let _ = tx.send(seen).await;
                    }
                }
            });
        }
    });
    (addr, rx)
}

fn acceptor(leaf: &support::pki::Leaf) -> TlsAcceptor {
    let config = hub::serve::tls13_server_config(vec![leaf.cert.clone()], leaf.key.clone_key()).unwrap();
    TlsAcceptor::from(config)
}

async fn first(rx: &mut tokio::sync::mpsc::Receiver<Seen>) -> Seen {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

fn write_ca(dir: &Path, name: &str, ca: &TestCa) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, ca.cert_pem()).unwrap();
    path
}

#[test]
fn the_pool_options_force_verify_full_whatever_the_url_says() {
    let dir = tempfile::tempdir().unwrap();
    let ca = write_ca(dir.path(), "ca.pem", &TestCa::new("ca"));
    let options = connect_options(&settings(
        "postgres://u:hunter2@db.internal:5432/x?sslmode=disable",
        &ca,
    ))
    .unwrap();
    assert!(matches!(
        options.ssl_mode(),
        sqlx::postgres::PgSslMode::VerifyFull
    ));
    assert_eq!(options.host(), "db.internal");
    // sqlx prints the password in its own Debug output; ours must not.
    let debug = format!("{options:?}");
    assert!(!debug.contains("hunter2") && !debug.contains("\"u\""), "{debug}");
}

#[test]
fn a_bad_url_is_an_error_without_the_url_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let ca = write_ca(dir.path(), "ca.pem", &TestCa::new("ca"));
    let error = connect_options(&settings("not a url with hunter2", &ca)).unwrap_err();
    assert_eq!(error, DbError::Url);
    assert!(!format!("{error} {error:?}").contains("hunter2"));
}

#[tokio::test]
async fn pg_pool_refuses_a_server_that_offers_no_tls() {
    let dir = tempfile::tempdir().unwrap();
    let ca = write_ca(dir.path(), "ca.pem", &TestCa::new("ca"));
    let (addr, mut seen) = fake_postgres(None).await;
    let url = format!("postgres://u:p@localhost:{}/x?sslmode=disable", addr.port());
    let error = connect(&settings(&url, &ca)).await.unwrap_err();
    assert_eq!(error, DbError::Connect);
    assert_eq!(
        first(&mut seen).await,
        Seen::NoTls,
        "the client asked for TLS and went no further"
    );
    // And never a plain startup message: it did not fall back to one.
    if let Ok(Some(next)) = tokio::time::timeout(Duration::from_millis(300), seen.recv()).await {
        assert_ne!(
            next,
            Seen::PlainStartup,
            "the client fell back to a plaintext startup"
        );
    }
}

#[tokio::test]
async fn pg_pool_refuses_wrong_ca_and_wrong_host_name() {
    let dir = tempfile::tempdir().unwrap();
    let right = TestCa::new("cnpg-ca");
    let wrong = TestCa::new("someone-elses-ca");
    let right_file = write_ca(dir.path(), "right.pem", &right);
    let wrong_file = write_ca(dir.path(), "wrong.pem", &wrong);
    let server = right.leaf(&["localhost"]);
    let (addr, mut seen) = fake_postgres(Some(acceptor(&server))).await;
    let url = format!("postgres://u:p@localhost:{}/x", addr.port());

    // The server's certificate is signed by a CA the pool does not trust.
    assert_eq!(
        connect(&settings(&url, &wrong_file)).await.unwrap_err(),
        DbError::Connect
    );
    assert_eq!(first(&mut seen).await, Seen::HandshakeFailed, "wrong CA");

    // Trusted CA, but the name in the certificate is not the name connected to.
    let other_name = right.leaf(&["db.other.example"]);
    let (addr2, mut seen2) = fake_postgres(Some(acceptor(&other_name))).await;
    let url2 = format!("postgres://u:p@localhost:{}/x", addr2.port());
    assert_eq!(
        connect(&settings(&url2, &right_file)).await.unwrap_err(),
        DbError::Connect
    );
    assert_eq!(first(&mut seen2).await, Seen::HandshakeFailed, "wrong host name");

    // The right CA and the right name complete the handshake (the fake then has no more to say, so the connection fails).
    let _ = connect(&settings(&url, &right_file)).await;
    assert_eq!(first(&mut seen).await, Seen::Handshake, "right CA and name");
}

// ------------------------------------------------------------------------------------------ a real Postgres with TLS

const IMAGE: &str = "pgvector/pgvector";
const IMAGE_TAG: &str = "pg16@sha256:7b822b0aac60967beb1ea5e576b8602c94c300a157d187f385ae3e0da199b90a";

fn require_docker() -> bool {
    std::env::var("LK_REQUIRE_DOCKER").is_ok_and(|v| v == "1")
}

#[tokio::test]
async fn pg_pool_connects_to_a_real_tls_postgres_and_refuses_the_wrong_ca() {
    use testcontainers::core::IntoContainerPort;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{GenericImage, ImageExt};

    let dir = tempfile::tempdir().unwrap();
    let ca = TestCa::new("cnpg-ca");
    let wrong = TestCa::new("wrong-ca");
    let ca_file = write_ca(dir.path(), "ca.pem", &ca);
    let wrong_file = write_ca(dir.path(), "wrong.pem", &wrong);
    let server = ca.leaf(&["localhost", "127.0.0.1"]);
    let script = "install -d -m 755 /etc/lk-tls-run \
        && install -o postgres -g postgres -m 600 /etc/lk-tls/server.key /etc/lk-tls-run/server.key \
        && install -o postgres -g postgres -m 644 /etc/lk-tls/server.crt /etc/lk-tls-run/server.crt \
        && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_min_protocol_version=TLSv1.3 \
           -c ssl_cert_file=/etc/lk-tls-run/server.crt -c ssl_key_file=/etc/lk-tls-run/server.key -c fsync=off";
    let started = GenericImage::new(IMAGE, IMAGE_TAG)
        .with_exposed_port(5432.tcp())
        .with_entrypoint("sh")
        .with_env_var("POSTGRES_PASSWORD", "lanekeeper-test-only")
        .with_copy_to("/etc/lk-tls/server.crt", server.cert_pem.clone().into_bytes())
        .with_copy_to("/etc/lk-tls/server.key", server.key_pem.clone().into_bytes())
        .with_cmd(["-c", script])
        .start()
        .await;
    let container = match started {
        Ok(c) => c,
        Err(error) => {
            assert!(
                !require_docker(),
                "LK_REQUIRE_DOCKER=1 but the Postgres container cannot start: {error}"
            );
            eprintln!("SKIPPED (set LK_REQUIRE_DOCKER=1 to make this a failure): {error}");
            return;
        }
    };
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    // `localhost`, which the certificate names, whatever address Docker publishes the port on.
    let url = format!("postgres://postgres:lanekeeper-test-only@localhost:{port}/postgres?sslmode=disable");

    // The right CA: the pool comes up and a query runs over TLS 1.3 (the server refuses anything older).
    let mut pool = None;
    for _ in 0..60 {
        match connect(&settings(&url, &ca_file)).await {
            Ok(p) => {
                pool = Some(p);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    let pool = pool.expect("the pool never connected to the TLS Postgres");
    let ssl: bool = sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(ssl, "the connection is not using TLS");
    let version: String = sqlx::query_scalar("SELECT version FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, "TLSv1.3");
    let timeout: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(timeout, "30s", "the statement timeout is set on every connection");
    assert!(hub::ops::ReadyCheck::ready(&hub::db::PostgresReady(pool.clone())).await);

    // The wrong CA is refused by a real server too.
    assert_eq!(
        connect(&settings(&url, &wrong_file)).await.unwrap_err(),
        DbError::Connect
    );
    drop(Arc::new(pool));
}
