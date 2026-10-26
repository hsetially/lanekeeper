//! A real Postgres 16 with TLS 1.3 for the process tests: the hub's pool always verifies the server (S6), so a plain server
//! would be refused. The image is the one the dev compose file pins by digest, the way `pg_tls.rs` starts it.
//!
//! Without a Docker daemon a test prints `SKIPPED`, and with `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) it fails instead.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::path::PathBuf;
use std::time::Duration;

use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::support::pki::TestCa;

const IMAGE: &str = "pgvector/pgvector";
const IMAGE_TAG: &str = "pg16@sha256:7b822b0aac60967beb1ea5e576b8602c94c300a157d187f385ae3e0da199b90a";

/// The server's password. It is planted: a test fails if it shows up anywhere the hub writes.
pub const PLANTED_PASSWORD: &str = "planted-db-password-4711";

pub fn require_docker() -> bool {
    std::env::var("LK_REQUIRE_DOCKER").is_ok_and(|v| v == "1")
}

/// A running TLS Postgres and the files a hub needs to reach it.
pub struct TlsPg {
    pub port: u16,
    /// Files: `db-url` (the connection URL, with the planted password) and `db-ca.pem` (the CA that signed the server).
    pub dir: tempfile::TempDir,
    _container: ContainerAsync<GenericImage>,
}

impl TlsPg {
    pub fn url_file(&self) -> PathBuf {
        self.dir.path().join("db-url")
    }

    pub fn ca_file(&self) -> PathBuf {
        self.dir.path().join("db-ca.pem")
    }

    pub fn url(&self) -> String {
        format!(
            "postgres://postgres:{PLANTED_PASSWORD}@localhost:{}/postgres?sslmode=disable",
            self.port
        )
    }
}

/// Start the server, or `None` (after saying why) when Docker is not there and not required.
pub async fn start(ca: &TestCa) -> Option<TlsPg> {
    let server = ca.leaf(&["localhost"]);
    let script = "install -d -m 755 /etc/lk-tls-run \
        && install -o postgres -g postgres -m 600 /etc/lk-tls/server.key /etc/lk-tls-run/server.key \
        && install -o postgres -g postgres -m 644 /etc/lk-tls/server.crt /etc/lk-tls-run/server.crt \
        && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_min_protocol_version=TLSv1.3 \
           -c ssl_cert_file=/etc/lk-tls-run/server.crt -c ssl_key_file=/etc/lk-tls-run/server.key -c fsync=off";
    let started = GenericImage::new(IMAGE, IMAGE_TAG)
        .with_exposed_port(5432.tcp())
        .with_entrypoint("sh")
        .with_env_var("POSTGRES_PASSWORD", PLANTED_PASSWORD)
        .with_copy_to("/etc/lk-tls/server.crt", server.cert_pem.clone().into_bytes())
        .with_copy_to("/etc/lk-tls/server.key", server.key_pem.clone().into_bytes())
        .with_cmd(["-c", script])
        .start()
        .await;
    let container = match started {
        Ok(container) => container,
        Err(error) => {
            assert!(
                !require_docker(),
                "LK_REQUIRE_DOCKER=1 but the Postgres container cannot start: {error}"
            );
            eprintln!("SKIPPED (set LK_REQUIRE_DOCKER=1 to make this a failure): {error}");
            return None;
        }
    };
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("db-ca.pem"), ca.cert_pem()).unwrap();
    let pg = TlsPg {
        port,
        dir,
        _container: container,
    };
    std::fs::write(pg.url_file(), pg.url()).unwrap();
    wait_until_it_accepts(&pg).await;
    Some(pg)
}

/// The container is "started" when its process is; the server takes a few seconds more to accept connections.
async fn wait_until_it_accepts(pg: &TlsPg) {
    let settings = hub::config::DatabaseSettings {
        url: domain::Secret::new(pg.url()),
        ca_file: pg.ca_file(),
        max_connections: 1,
        acquire_timeout: Duration::from_secs(3),
        statement_timeout: Duration::from_secs(30),
    };
    for _ in 0..60 {
        if hub::db::connect(&settings).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("the TLS Postgres never accepted a connection");
}
