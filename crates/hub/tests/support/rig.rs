//! Everything a hub needs to start in a process test: a TLS Postgres with the schema applied, a web certificate, a web root and
//! a secrets directory.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use crate::child::{self, Child};
use crate::git_remote::HttpsRemote;
use crate::support::pki::{Leaf, TestCa};
use crate::support::write_dist;
use crate::tls_pg::{self, TlsPg};

/// The KMS key versions of the two CAs. The dev example makes software keys under these names.
pub const AGENT_CA_KMS: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/agent-ca/cryptoKeyVersions/1";
pub const REPLICA_CA_KMS: &str =
    "projects/p/locations/l/keyRings/r/cryptoKeys/replica-ca/cryptoKeyVersions/1";
/// The key version that signs the audit chain's checkpoints (T4). The dev example makes a software key under this name too.
pub const CHECKPOINT_KMS: &str =
    "projects/p/locations/l/keyRings/r/cryptoKeys/audit-checkpoint/cryptoKeyVersions/1";

/// The identity group of the settings (T4): the Entra app registration, the places its two secrets are named (the dev example
/// writes them into the secrets directory), and the audit checkpoints. Shared by every test that starts a hub.
#[must_use]
pub fn identity_env() -> Vec<(&'static str, String)> {
    vec![
        ("LK_PUBLIC_ORIGIN", "https://localhost".to_owned()),
        (
            "LK_ENTRA_TENANT_ID",
            "11111111-1111-4111-8111-111111111111".to_owned(),
        ),
        (
            "LK_ENTRA_CLIENT_ID",
            "22222222-2222-4222-8222-222222222222".to_owned(),
        ),
        ("LK_ENTRA_CLIENT_AUTH_REF", "entra-client".to_owned()),
        ("LK_ENTRA_COOKIE_KEY_REF", "login-cookie".to_owned()),
        ("LK_AUDIT_CHECKPOINT_BUCKET", "audit-checkpoints".to_owned()),
        ("LK_AUDIT_CHECKPOINT_KMS_REF", CHECKPOINT_KMS.to_owned()),
    ]
}

/// A database, a web certificate, a Git remote, a web root and a secrets directory: everything a hub needs to start.
pub struct Rig {
    pub pg: TlsPg,
    pub ca: TestCa,
    pub leaf: Leaf,
    pub secrets: tempfile::TempDir,
    pub dist: tempfile::TempDir,
    /// The Git remote both mirrors are filled from, served over HTTPS under `ca`.
    pub git: HttpsRemote,
    /// Where the mirrors live.
    pub mirrors: tempfile::TempDir,
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
        // The Git remote is public to the test: it asks for no credential, so the token below is never sent. It exists because the
        // settings name a secret for it and the hub reads that secret before each fetch.
        std::fs::write(secrets.path().join("github-read-token"), "planted-git-read-token").unwrap();
        // The child trusts the test CA for the Git remote's certificate (and for nothing it did not trust before).
        std::fs::write(secrets.path().join("rig-ca.pem"), ca.cert_pem()).unwrap();
        let git = HttpsRemote::start(&leaf).await;
        Some(Self {
            pg,
            ca,
            leaf,
            secrets,
            dist: write_dist(),
            git,
            mirrors: tempfile::tempdir().unwrap(),
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
            ("LK_AGENT_ADDR", "127.0.0.1:0".to_owned()),
            ("LK_REPLICA_ADDR", "127.0.0.1:0".to_owned()),
            ("LK_AGENT_CA_KMS_REF", AGENT_CA_KMS.to_owned()),
            ("LK_AGENT_CA_CERT_REF", "agent-ca-cert".to_owned()),
            ("LK_REPLICA_CA_KMS_REF", REPLICA_CA_KMS.to_owned()),
            ("LK_REPLICA_CA_CERT_REF", "replica-ca-cert".to_owned()),
            ("LK_REPLICA_NAME", "hub-0".to_owned()),
            ("LK_REPLICA_ADDRESS", "localhost:8445".to_owned()),
            ("LK_HUB_AUDIENCE", "https://hub.lanekeeper.test".to_owned()),
            ("LK_AGENT_ENDPOINT_HOSTS", "localhost".to_owned()),
            ("LK_GIT_BASE_URL", self.git.url()),
            ("LK_GIT_TENANT_URL", self.git.url()),
            ("LK_GIT_BASE_REPO", "acme/configuration-base-saas".to_owned()),
            ("LK_GIT_TENANT_REPO", "acme/csp-tenant-data".to_owned()),
            ("LK_MIRROR_DIR", self.mirrors.path().display().to_string()),
            (
                "SSL_CERT_FILE",
                self.secrets.path().join("rig-ca.pem").display().to_string(),
            ),
        ]
        .into_iter()
        .chain(identity_env())
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
