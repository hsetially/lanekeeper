//! Everything a hub needs to start in a process test: a TLS Postgres with the schema applied, a web certificate, a web root and
//! a secrets directory.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use crate::child::{self, Child};
use crate::support::pki::{Leaf, TestCa};
use crate::support::write_dist;
use crate::tls_pg::{self, TlsPg};

/// A database, a web certificate, a web root and a secrets directory: everything a hub needs to start.
pub struct Rig {
    pub pg: TlsPg,
    pub ca: TestCa,
    pub leaf: Leaf,
    pub secrets: tempfile::TempDir,
    pub dist: tempfile::TempDir,
}

impl Rig {
    /// A database with the roles and every migration applied (what a hub finds after the migration Job has run).
    pub async fn start() -> Option<Self> {
        let rig = Self::start_unmigrated().await?;
        rig.pg.migrate().await;
        Some(rig)
    }

    /// A database with the roles and no schema: what a hub finds when nobody has run `hub migrate`.
    pub async fn start_unmigrated() -> Option<Self> {
        let ca = TestCa::new("lanekeeper-test-ca");
        let pg = tls_pg::start(&ca).await?;
        let leaf = ca.leaf(&["localhost"]);
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(secrets.path().join("web-cert"), &leaf.cert_pem).unwrap();
        std::fs::write(secrets.path().join("web-key"), &leaf.key_pem).unwrap();
        Some(Self {
            pg,
            ca,
            leaf,
            secrets,
            dist: write_dist(),
        })
    }

    pub fn env(&self, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = [
            ("LK_DATABASE_URL_FILE", self.pg.url_file().display().to_string()),
            ("LK_DB_CA_FILE", self.pg.ca_file().display().to_string()),
            ("LK_WEB_ADDR", "127.0.0.1:0".to_owned()),
            ("LK_OPS_ADDR", "127.0.0.1:0".to_owned()),
            ("LK_WEB_DIST", self.dist.path().display().to_string()),
            ("LK_WEB_CERT_REF", "web-cert".to_owned()),
            ("LK_WEB_KEY_REF", "web-key".to_owned()),
            ("LK_DEV_SECRETS_DIR", self.secrets.path().display().to_string()),
            ("LK_TRUSTED_PROXY_HOPS", "0".to_owned()),
            ("LK_START_TIMEOUT_SECS", "20".to_owned()),
            ("LK_SHUTDOWN_GRACE_SECS", "1".to_owned()),
            ("LK_SHUTDOWN_DRAIN_SECS", "5".to_owned()),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
        for (name, value) in extra {
            env.retain(|(n, _)| n != name);
            env.push(((*name).to_owned(), (*value).to_owned()));
        }
        env
    }

    pub async fn spawn(&self, extra: &[(&str, &str)]) -> Child {
        let program: PathBuf = tokio::task::spawn_blocking(child::example_binary).await.unwrap();
        Child::spawn(&program, &[], &self.env(extra))
    }
}
