//! A real Postgres 16 with TLS 1.3 for the process tests: the hub's pool always verifies the server (S6), so a plain server
//! would be refused. The image is the one the dev compose file pins by digest, the way `pg_tls.rs` starts it.
//!
//! The database has the roles of `deploy/dev/init.sql` (T2): the hub connects as `lanekeeper_app`, and `migrate()` applies the
//! migrations as `lanekeeper_migrator` through the hub's own `hub::migrate::run`, so every process test also exercises it.
//! The superuser exists only to create the roles; nothing under test knows its password.
//!
//! Without a Docker daemon a test prints `SKIPPED`, and with `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) it fails instead.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::support::pki::TestCa;

const IMAGE: &str = "pgvector/pgvector";
const IMAGE_TAG: &str = "pg16@sha256:7b822b0aac60967beb1ea5e576b8602c94c300a157d187f385ae3e0da199b90a";

/// The superuser's password. It is planted: a test fails if it shows up anywhere the hub writes.
pub const PLANTED_PASSWORD: &str = "planted-db-password-4711";

/// The roles of `deploy/dev/init.sql`, with the passwords that file gives them. A test fails if one shows up in the hub's output.
pub const APP_ROLE: (&str, &str) = ("lanekeeper_app", "lanekeeper-app-dev-only");
pub const MIGRATOR_ROLE: (&str, &str) = ("lanekeeper_migrator", "lanekeeper-migrator-dev-only");

/// The database every role is granted on, as `deploy/dev/init.sql` names it.
pub const DATABASE: &str = "lanekeeper";

pub fn require_docker() -> bool {
    std::env::var("LK_REQUIRE_DOCKER").is_ok_and(|v| v == "1")
}

/// A running TLS Postgres and the files a hub needs to reach it.
pub struct TlsPg {
    pub port: u16,
    /// Files: `db-url` (the hub's connection URL: `lanekeeper_app`), `migrator-url` (`lanekeeper_migrator`) and `db-ca.pem`
    /// (the CA that signed the server).
    pub dir: tempfile::TempDir,
    _container: ContainerAsync<GenericImage>,
}

impl TlsPg {
    /// The file the hub reads its database URL from.
    pub fn url_file(&self) -> PathBuf {
        self.dir.path().join("db-url")
    }

    /// The file `hub migrate` reads the migrator's URL from.
    pub fn migrator_url_file(&self) -> PathBuf {
        self.dir.path().join("migrator-url")
    }

    pub fn ca_file(&self) -> PathBuf {
        self.dir.path().join("db-ca.pem")
    }

    /// A URL for `user`. `sslmode=disable` is a lie the hub overrides: its pool always verifies the server.
    pub fn url_for(&self, user: &str, password: &str) -> String {
        format!(
            "postgres://{user}:{password}@localhost:{}/{DATABASE}?sslmode=disable",
            self.port
        )
    }

    /// What the hub connects with: the app role.
    pub fn url(&self) -> String {
        self.url_for(APP_ROLE.0, APP_ROLE.1)
    }

    pub fn migrator_url(&self) -> String {
        self.url_for(MIGRATOR_ROLE.0, MIGRATOR_ROLE.1)
    }

    pub fn superuser_url(&self) -> String {
        self.url_for("postgres", PLANTED_PASSWORD)
    }

    /// The settings `hub migrate` would read, for the library call.
    pub fn migrator_settings(&self) -> hub::config::MigratorSettings {
        self.migrator_settings_with(&self.migrator_url())
    }

    /// The same, for a URL the test chooses (another role, a wrong password).
    pub fn migrator_settings_with(&self, url: &str) -> hub::config::MigratorSettings {
        hub::config::MigratorSettings {
            url: domain::Secret::new(url.to_owned()),
            ca_file: self.ca_file(),
            timeout: Duration::from_secs(120),
        }
    }

    /// Apply the migrations as the migrator, the way the Job does.
    pub async fn migrate(&self) -> hub::migrate::Report {
        hub::migrate::run(&self.migrator_settings()).await.unwrap()
    }

    /// The pool of the hub: `lanekeeper_app`, TLS verified.
    pub async fn app_pool(&self) -> sqlx::PgPool {
        hub::db::connect(&hub::config::DatabaseSettings {
            url: domain::Secret::new(self.url()),
            ca_file: self.ca_file(),
            max_connections: 4,
            acquire_timeout: Duration::from_secs(5),
            statement_timeout: Duration::from_secs(30),
        })
        .await
        .unwrap()
    }

    /// Run `deploy/dev/init.sql` as the superuser: the same SQL the dev compose file runs, which creates the two roles.
    async fn init_roles(&self) {
        let sql =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/dev/init.sql"))
                .unwrap();
        let options = hub::db::connect_options_for(
            &domain::Secret::new(self.superuser_url()),
            &self.ca_file(),
            Duration::from_secs(60),
        )
        .unwrap();
        let mut conn = hub::db::connect_one(&options).await.unwrap();
        sqlx::raw_sql(&sql).execute(&mut conn).await.unwrap();
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
        .with_env_var("POSTGRES_DB", DATABASE)
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
    std::fs::write(pg.migrator_url_file(), pg.migrator_url()).unwrap();
    wait_until_it_accepts(&pg).await;
    pg.init_roles().await;
    Some(pg)
}

/// The container is "started" when its process is; the server takes a few seconds more to accept connections.
async fn wait_until_it_accepts(pg: &TlsPg) {
    let settings = hub::config::DatabaseSettings {
        url: domain::Secret::new(pg.superuser_url()),
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
