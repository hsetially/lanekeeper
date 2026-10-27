//! Settings from the environment (`LK_*`), parsed once at start-up (rule 5, S7).
//!
//! Every setting has a typed field and a default that is safe for production. A malformed value is an error that names the
//! variable and never prints the value, because the value may be a secret (the database URL holds a password).
//!
//! The settings are in groups, one file each under `config/`, and every group is read through [`Reader`]: process and
//! listeners (this file), request limits, the database, Git, Google Cloud, the web certificate and the platform (the two CAs,
//! this replica's identity and the agent endpoint) and identity (Entra sign-in, the first admins, the audit checkpoints);
//! `hub migrate` has its own small group, [`MigratorSettings`], that the
//! serving hub never reads. A group arrives with the task that needs it, and `tests/hardening.rs` reads every `LK_*` literal in
//! these files to check the pod contract against them.

mod database;
mod gcp;
mod git;
mod identity;
mod limits;
mod migrator;
mod platform;
mod reader;
mod registry;
mod web_tls;
mod writes;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

pub use database::DatabaseSettings;
pub use gcp::GcpSettings;
pub use git::{GitCredentialSource, GitSettings};
pub use identity::{CheckpointSettings, DEFAULT_AUTHORITY_HOST, DEFAULT_REDIRECT_PATH, IdentitySettings};
pub use limits::{LimitSettings, Rate};
pub use migrator::{DEFAULT_TIMEOUT_SECS, JOB_ONLY_VARIABLES, MigratorSettings};
pub use platform::{CaSettings, DEFAULT_AGENT_ADDR, DEFAULT_REPLICA_ADDR, PlatformSettings};
pub use reader::Reader;
pub use registry::RegistrySettings;
pub use web_tls::{DEFAULT_RELOAD_SECS, WebTlsSettings};
pub use writes::{
    DEFAULT_BASE_BRANCH, DEFAULT_CONFIG_SERVER_DEPLOYMENT, DEFAULT_GITHUB_API_BASE, WriteJobTimes,
    WritesSettings,
};

/// A setting that could not be read. Carries the variable name only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{0} is not valid")]
    Invalid(&'static str),
}

/// Process-wide settings.
#[derive(Debug)]
pub struct HubSettings {
    /// The web listener (HTTPS, behind the users' load balancer).
    pub web_addr: SocketAddr,
    /// The internal listener: `/healthz`, `/readyz` and `/metrics`, plain HTTP, cluster-internal only.
    pub ops_addr: SocketAddr,
    /// Where the Git mirrors live (T7): the one writable volume besides `/tmp`.
    pub mirror_dir: PathBuf,
    /// The built web app. `index.html` there carries the `__CSP_NONCE__` placeholder.
    pub web_dist: PathBuf,
    /// How many entries at the right end of `X-Forwarded-For` our own proxies added (default 1: the load balancer).
    pub trusted_proxy_hops: usize,
    /// The Trusted Types policy names the web app creates (A14).
    pub trusted_types: Vec<String>,
    pub limits: LimitSettings,
    /// Open connections the web listener accepts at once.
    pub max_connections: usize,
    /// The database. `None` only in the library's defaults; [`HubSettings::require_serving`] refuses that.
    pub database: Option<DatabaseSettings>,
    /// The Git mirrors and the webhook. `None` when no remote is configured.
    pub git: Option<GitSettings>,
    /// Google Cloud: the project the secrets live in. `None` when unset; the real edges refuse that.
    pub gcp: Option<GcpSettings>,
    /// The web certificate's two secret names. `None` only in the library's defaults; serving refuses that.
    pub web_tls: Option<WebTlsSettings>,
    /// The two CAs, this replica's identity and the agent endpoint (T3). `None` only in the library's defaults; serving refuses
    /// that.
    pub platform: Option<PlatformSettings>,
    /// Entra sign-in, the first admins and the audit checkpoints (T4). `None` only in the library's defaults; serving refuses that.
    pub identity: Option<IdentitySettings>,
    /// The registry's limits and the few knobs of them a deployment sets (T5). Always present: every knob has a default.
    pub registry: RegistrySettings,
    /// The write paths: the vault's KMS key, GitHub, Teams and the jobs' intervals (T5). `None` only in the library's defaults;
    /// serving refuses that.
    pub writes: Option<WritesSettings>,
    /// What the blob cache may hold (`LK_BLOB_CACHE_MIB`, default 512 MiB), in bytes.
    pub blob_cache_bytes: u64,
    /// OTLP gRPC endpoint of an in-cluster collector. No export when unset.
    pub otlp_endpoint: Option<String>,
    /// The longest any one step of starting may take (a secret read, the database), so a hang is an exit and not a wait (P11).
    pub start_timeout: Duration,
    /// Seconds readiness fails before connections stop (lets the load balancer notice).
    pub shutdown_grace: Duration,
    /// Seconds in-flight requests get after connections stop.
    pub shutdown_drain: Duration,
}

impl HubSettings {
    /// Read the settings through `lookup`, which is `std::env::var` in `main` and a map in tests. Files named by
    /// `*_FILE` variables are read from disk.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Self::from_parts(lookup, |path| std::fs::read_to_string(path))
    }

    /// Like [`HubSettings::from_lookup`], with the file reader given (a map in tests).
    pub fn from_parts(
        lookup: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&std::path::Path) -> std::io::Result<String>,
    ) -> Result<Self, ConfigError> {
        let reader = Reader::new(&lookup, &read_file);
        let trusted_types: Vec<String> = reader
            .parse("LK_TRUSTED_TYPES")?
            .unwrap_or_else(|| "lanekeeper-docs,lanekeeper-monaco".to_owned())
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        // Policy names go into a CSP header: only the characters the spec allows, so a setting cannot inject a directive.
        let names: BTreeSet<&String> = trusted_types.iter().collect();
        if names.len() != trusted_types.len()
            || trusted_types.iter().any(|n| {
                n.len() > 64
                    || !n
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'#' | b'='))
            })
        {
            return Err(ConfigError::Invalid("LK_TRUSTED_TYPES"));
        }

        Ok(Self {
            web_addr: reader.addr("LK_WEB_ADDR", "0.0.0.0:8443")?,
            ops_addr: reader.addr("LK_OPS_ADDR", "0.0.0.0:9090")?,
            mirror_dir: reader
                .parse("LK_MIRROR_DIR")?
                .map_or_else(|| PathBuf::from("/var/lib/lanekeeper/mirror"), PathBuf::from),
            web_dist: reader
                .parse("LK_WEB_DIST")?
                .map_or_else(|| PathBuf::from("/app/web/dist"), PathBuf::from),
            trusted_proxy_hops: reader.size("LK_TRUSTED_PROXY_HOPS", 1, 0, 8)?,
            trusted_types,
            limits: LimitSettings::read(&reader)?,
            max_connections: reader.size("LK_MAX_CONNECTIONS", 4096, 1, 1_000_000)?,
            database: DatabaseSettings::read(&reader)?,
            git: GitSettings::read(&reader)?,
            gcp: GcpSettings::read(&reader)?,
            web_tls: WebTlsSettings::read(&reader)?,
            platform: PlatformSettings::read(&reader)?,
            identity: IdentitySettings::read(&reader)?,
            registry: RegistrySettings::read(&reader)?,
            writes: WritesSettings::read(&reader)?,
            blob_cache_bytes: reader
                .number("LK_BLOB_CACHE_MIB", 512, 1, 4096)?
                .saturating_mul(1024 * 1024),
            otlp_endpoint: reader.parse("LK_OTLP_ENDPOINT")?,
            start_timeout: Duration::from_secs(reader.number("LK_START_TIMEOUT_SECS", 8, 1, 120)?),
            shutdown_grace: Duration::from_secs(reader.number("LK_SHUTDOWN_GRACE_SECS", 5, 0, 60)?),
            shutdown_drain: Duration::from_secs(reader.number("LK_SHUTDOWN_DRAIN_SECS", 25, 1, 120)?),
        })
    }

    /// What `hub serve` needs and the library's defaults leave out. A settings value built for a test or the web fixture may
    /// have no database and no certificate; the process may not, and it says which variable is missing and starts nothing.
    pub fn require_serving(&self) -> Result<(), ConfigError> {
        if self.database.is_none() {
            return Err(ConfigError::Missing("LK_DATABASE_URL_FILE"));
        }
        if self.web_tls.is_none() {
            return Err(ConfigError::Missing("LK_WEB_CERT_REF"));
        }
        // T3 (G10): the CAs and the replica's identity. Without them no agent can join and no replica can be reached.
        if self.platform.is_none() {
            return Err(ConfigError::Missing("LK_AGENT_CA_KMS_REF"));
        }
        // The mirrors are what every read of a commit goes through; a hub without remotes has nothing to serve.
        if self.git.is_none() {
            return Err(ConfigError::Missing("LK_GIT_BASE_URL"));
        }
        // T4 (S1): without the identity provider nobody can sign in, and a hub that serves a guarded API to nobody is not one to
        // start. It is never replaced by a session store of the hub's own.
        if self.identity.is_none() {
            return Err(ConfigError::Missing("LK_ENTRA_TENANT_ID"));
        }
        // T5 (S8): the token vault is what lets a person raise a pull request with their own GitHub token. A hub without the key
        // that wraps the tokens has no write path to offer, and does not start with half of one.
        if self.writes.is_none() {
            return Err(ConfigError::Missing("LK_VAULT_KMS_REF"));
        }
        Ok(())
    }
}

impl Default for HubSettings {
    /// The defaults with nothing set: no database, no TLS files, no telemetry. For tests.
    fn default() -> Self {
        // An empty environment always parses.
        Self::from_lookup(|_| None).unwrap_or_else(|_| Self {
            web_addr: SocketAddr::from(([0, 0, 0, 0], 8443)),
            ops_addr: SocketAddr::from(([0, 0, 0, 0], 9090)),
            mirror_dir: PathBuf::from("/var/lib/lanekeeper/mirror"),
            web_dist: PathBuf::from("/app/web/dist"),
            trusted_proxy_hops: 1,
            trusted_types: vec!["lanekeeper-docs".to_owned(), "lanekeeper-monaco".to_owned()],
            limits: LimitSettings::default(),
            max_connections: 4096,
            database: None,
            git: None,
            gcp: None,
            web_tls: None,
            platform: None,
            identity: None,
            registry: RegistrySettings::default(),
            writes: None,
            blob_cache_bytes: 512 * 1024 * 1024,
            otlp_endpoint: None,
            start_timeout: Duration::from_secs(8),
            shutdown_grace: Duration::from_secs(5),
            shutdown_drain: Duration::from_secs(25),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let settings = HubSettings::from_lookup(lookup(&[])).unwrap_or_default();
        assert_eq!(settings.limits.body_max, 4 * 1024 * 1024);
        assert_eq!(settings.limits.request_timeout, Duration::from_secs(30));
        assert_eq!(settings.limits.json_depth, 64);
        assert_eq!(settings.trusted_proxy_hops, 1);
        assert_eq!(settings.shutdown_drain, Duration::from_secs(25));
        assert!(settings.database.is_none());
        assert!(settings.otlp_endpoint.is_none());
    }

    #[test]
    fn the_blob_cache_is_512_mib_unless_set_and_is_bounded() {
        let default = HubSettings::from_lookup(lookup(&[])).unwrap_or_default();
        assert_eq!(default.blob_cache_bytes, 512 * 1024 * 1024);
        let set = HubSettings::from_lookup(lookup(&[("LK_BLOB_CACHE_MIB", "128")])).unwrap();
        assert_eq!(set.blob_cache_bytes, 128 * 1024 * 1024);
        for bad in ["0", "lots", "-1", "100000", "1.5"] {
            assert_eq!(
                HubSettings::from_lookup(lookup(&[("LK_BLOB_CACHE_MIB", bad)])).unwrap_err(),
                ConfigError::Invalid("LK_BLOB_CACHE_MIB"),
                "{bad}"
            );
        }
    }

    #[test]
    fn an_invalid_value_names_the_variable_and_never_the_value() {
        let secret = "postgres://u:hunter2@db/x";
        let err = HubSettings::from_lookup(lookup(&[
            ("LK_DATABASE_URL", secret),
            ("LK_DB_CA_FILE", "/ca.pem"),
            ("LK_DB_MAX_CONNECTIONS", "lots"),
        ]))
        .unwrap_err();
        assert_eq!(err, ConfigError::Invalid("LK_DB_MAX_CONNECTIONS"));
        assert!(!format!("{err} {err:?}").contains("hunter2"));
    }

    #[test]
    fn a_database_url_needs_its_ca_file() {
        let err = HubSettings::from_lookup(lookup(&[("LK_DATABASE_URL", "postgres://db/x")])).unwrap_err();
        assert_eq!(err, ConfigError::Missing("LK_DB_CA_FILE"));
    }

    #[test]
    fn the_database_url_is_redacted_in_debug_output() {
        let settings = HubSettings::from_lookup(lookup(&[
            ("LK_DATABASE_URL", "postgres://u:hunter2@db/x"),
            ("LK_DB_CA_FILE", "/ca.pem"),
        ]))
        .unwrap_or_default();
        assert!(!format!("{settings:?}").contains("hunter2"));
    }

    #[test]
    fn trusted_types_names_cannot_inject_a_csp_directive() {
        for bad in ["a; script-src *", "a b", "a,a", "x'y"] {
            assert_eq!(
                HubSettings::from_lookup(lookup(&[("LK_TRUSTED_TYPES", bad)])).unwrap_err(),
                ConfigError::Invalid("LK_TRUSTED_TYPES"),
                "{bad}"
            );
        }
    }

    fn with_files<'a>(
        env: &'a [(&'a str, &'a str)],
        files: &'a [(&'a str, &'a str)],
    ) -> Result<HubSettings, ConfigError> {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let files: HashMap<String, String> = files
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        HubSettings::from_parts(
            move |name| env.get(name).cloned(),
            move |path| {
                files
                    .get(path.to_string_lossy().as_ref())
                    .cloned()
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
            },
        )
    }

    #[test]
    fn the_database_url_is_read_from_a_mounted_file_and_trimmed() {
        let settings = with_files(
            &[
                ("LK_DATABASE_URL_FILE", "/run/db/uri"),
                ("LK_DB_CA_FILE", "/run/ca/ca.pem"),
            ],
            &[("/run/db/uri", "postgres://u:hunter2@db/x\n")],
        )
        .unwrap();
        let database = settings.database.unwrap();
        assert_eq!(database.url.expose(), "postgres://u:hunter2@db/x");
        assert_eq!(database.ca_file, std::path::PathBuf::from("/run/ca/ca.pem"));
    }

    #[test]
    fn a_missing_empty_or_doubled_database_url_is_refused_without_naming_the_path_or_value() {
        let missing = with_files(
            &[("LK_DATABASE_URL_FILE", "/run/db/uri"), ("LK_DB_CA_FILE", "/ca")],
            &[],
        )
        .unwrap_err();
        assert_eq!(missing, ConfigError::Invalid("LK_DATABASE_URL_FILE"));
        assert!(!format!("{missing}").contains("/run/db"));
        let empty = with_files(
            &[("LK_DATABASE_URL_FILE", "/u"), ("LK_DB_CA_FILE", "/ca")],
            &[("/u", " \n")],
        )
        .unwrap_err();
        assert_eq!(empty, ConfigError::Invalid("LK_DATABASE_URL_FILE"));
        let both = with_files(
            &[
                ("LK_DATABASE_URL_FILE", "/u"),
                ("LK_DATABASE_URL", "postgres://x/y"),
                ("LK_DB_CA_FILE", "/ca"),
            ],
            &[("/u", "postgres://a/b")],
        )
        .unwrap_err();
        assert_eq!(both, ConfigError::Invalid("LK_DATABASE_URL"));
    }

    #[test]
    fn out_of_range_numbers_are_refused() {
        for (name, value) in [
            ("LK_BODY_MAX_BYTES", "0"),
            ("LK_TRUSTED_PROXY_HOPS", "99"),
            ("LK_JSON_DEPTH", "100000"),
        ] {
            assert!(
                HubSettings::from_lookup(lookup(&[(name, value)])).is_err(),
                "{name}={value}"
            );
        }
    }
    // ------------------------------------------------------------------------------------------ T1 (plan 18): serving

    const AGENT_KMS: &str =
        "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/agent-ca/cryptoKeyVersions/1";
    const REPLICA_KMS: &str =
        "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/replica-ca/cryptoKeyVersions/3";
    /// The key that wraps the GitHub token vault (T5): a crypto key, not a version.
    const VAULT_KMS: &str = "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/vault";

    /// The platform group (T3): every variable it needs, with the values a deployment would give them.
    fn platform_env<'a>() -> Vec<(&'a str, &'a str)> {
        vec![
            ("LK_AGENT_CA_KMS_REF", AGENT_KMS),
            ("LK_AGENT_CA_CERT_REF", "lanekeeper-agent-ca"),
            ("LK_REPLICA_CA_KMS_REF", REPLICA_KMS),
            ("LK_REPLICA_CA_CERT_REF", "lanekeeper-replica-ca"),
            ("LK_REPLICA_NAME", "lanekeeper-hub-0"),
            (
                "LK_REPLICA_ADDRESS",
                "lanekeeper-hub-0.lanekeeper-hub.lanekeeper.svc:8445",
            ),
            ("LK_HUB_AUDIENCE", "https://hub.lanekeeper.example"),
            ("LK_AGENT_ENDPOINT_HOSTS", "agents.lanekeeper.example"),
        ]
    }

    /// The identity group (T4): the Entra app registration, the places the secrets are named, and the audit checkpoints.
    fn identity_env<'a>() -> Vec<(&'a str, &'a str)> {
        vec![
            ("LK_ENTRA_TENANT_ID", "11111111-1111-4111-8111-111111111111"),
            ("LK_ENTRA_CLIENT_ID", "22222222-2222-4222-8222-222222222222"),
            ("LK_PUBLIC_ORIGIN", "https://hub.lanekeeper.example"),
            ("LK_ENTRA_CLIENT_AUTH_REF", "lanekeeper-entra-client"),
            ("LK_ENTRA_COOKIE_KEY_REF", "lanekeeper-login-cookie"),
            ("LK_AUDIT_CHECKPOINT_BUCKET", "lanekeeper-audit-checkpoints"),
            (
                "LK_AUDIT_CHECKPOINT_KMS_REF",
                "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/audit/cryptoKeyVersions/1",
            ),
        ]
    }

    /// Everything `hub serve` needs: the database, the web certificate, Google, the platform, identity and the Git remotes.
    /// `extra` replaces a variable of the same name or adds one.
    fn serving_env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut env = vec![
            ("LK_DATABASE_URL", "postgres://u:p@db/x"),
            ("LK_DB_CA_FILE", "/ca.pem"),
            ("LK_WEB_CERT_REF", "web-cert"),
            ("LK_WEB_KEY_REF", "web-key"),
            ("LK_GCP_PROJECT", "lanekeeper-prod"),
        ];
        env.extend(platform_env());
        env.extend(identity_env());
        env.extend(git_env(&[]));
        env.push(("LK_VAULT_KMS_REF", VAULT_KMS));
        for (name, value) in extra {
            env.retain(|(n, _)| n != name);
            env.push((name, value));
        }
        env
    }

    #[test]
    fn the_library_defaults_are_not_enough_to_serve_and_the_error_names_the_variable() {
        let empty = HubSettings::from_lookup(lookup(&[])).unwrap();
        assert_eq!(
            empty.require_serving().unwrap_err(),
            ConfigError::Missing("LK_DATABASE_URL_FILE")
        );
        let no_cert = HubSettings::from_lookup(lookup(&[
            ("LK_DATABASE_URL", "postgres://u:p@db/x"),
            ("LK_DB_CA_FILE", "/ca.pem"),
        ]))
        .unwrap();
        assert_eq!(
            no_cert.require_serving().unwrap_err(),
            ConfigError::Missing("LK_WEB_CERT_REF")
        );
        let full = HubSettings::from_lookup(lookup(&serving_env(&[]))).unwrap();
        assert_eq!(full.require_serving(), Ok(()));
    }

    #[test]
    fn serving_needs_the_identity_provider_and_says_which_variable_to_set() {
        let mut env = serving_env(&[]);
        for (name, _) in identity_env() {
            env.retain(|(n, _)| *n != name);
        }
        let without = HubSettings::from_lookup(lookup(&env)).unwrap();
        assert!(without.identity.is_none());
        assert_eq!(
            without.require_serving().unwrap_err(),
            ConfigError::Missing("LK_ENTRA_TENANT_ID")
        );
    }

    #[test]
    fn the_web_certificate_is_two_secret_manager_names_and_needs_both() {
        let only_cert = HubSettings::from_lookup(lookup(&[("LK_WEB_CERT_REF", "web-cert")])).unwrap_err();
        assert_eq!(only_cert, ConfigError::Missing("LK_WEB_KEY_REF"));
        let only_key = HubSettings::from_lookup(lookup(&[("LK_WEB_KEY_REF", "web-key")])).unwrap_err();
        assert_eq!(only_key, ConfigError::Missing("LK_WEB_CERT_REF"));
        let tls = HubSettings::from_lookup(lookup(&serving_env(&[])))
            .unwrap()
            .web_tls
            .unwrap();
        assert_eq!(tls.cert_ref, "web-cert");
        assert_eq!(tls.key_ref, "web-key");
        // G3: reloaded every five minutes unless told otherwise.
        assert_eq!(tls.reload_every, Duration::from_secs(300));
        for (name, bad) in [
            ("LK_WEB_CERT_REF", "has space"),
            ("LK_WEB_CERT_REF", "../x"),
            ("LK_WEB_KEY_REF", "a/b"),
            ("LK_WEB_CERT_RELOAD_SECS", "0"),
            ("LK_WEB_CERT_RELOAD_SECS", "86401"),
            ("LK_WEB_CERT_RELOAD_SECS", "soon"),
        ] {
            assert_eq!(
                HubSettings::from_lookup(lookup(&serving_env(&[(name, bad)]))).unwrap_err(),
                ConfigError::Invalid(name),
                "{name}={bad}"
            );
        }
    }

    #[test]
    fn the_google_project_is_checked_and_the_start_timeout_is_bounded() {
        let settings = HubSettings::from_lookup(lookup(&serving_env(&[]))).unwrap();
        assert_eq!(
            settings.gcp.as_ref().map(|g| g.project.as_str()),
            Some("lanekeeper-prod")
        );
        assert_eq!(settings.start_timeout, Duration::from_secs(8));
        for bad in ["", "Has Space", "a/b", "p?x", "under_score", "example.com:proj"] {
            let result = HubSettings::from_lookup(lookup(&[("LK_GCP_PROJECT", bad)]));
            // An empty value counts as not set; every other bad value is refused by name.
            if bad.is_empty() {
                assert!(result.unwrap().gcp.is_none());
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    ConfigError::Invalid("LK_GCP_PROJECT"),
                    "{bad}"
                );
            }
        }
        for bad in ["0", "121", "x", "-1"] {
            assert_eq!(
                HubSettings::from_lookup(lookup(&[("LK_START_TIMEOUT_SECS", bad)])).unwrap_err(),
                ConfigError::Invalid("LK_START_TIMEOUT_SECS"),
                "{bad}"
            );
        }
        let two = HubSettings::from_lookup(lookup(&[("LK_START_TIMEOUT_SECS", "2")])).unwrap();
        assert_eq!(two.start_timeout, Duration::from_secs(2));
    }

    #[test]
    fn the_git_secret_names_are_references_and_the_old_names_are_gone() {
        let git = HubSettings::from_lookup(lookup(&git_env(&[
            ("LK_GIT_READ_REF", "my-read-token"),
            ("LK_GIT_WEBHOOK_REF", "my-hook-secret"),
        ])))
        .unwrap()
        .git
        .unwrap();
        assert_eq!(git.webhook_secret, "my-hook-secret");
        assert_eq!(
            git.credential,
            GitCredentialSource::SecretManager("my-read-token".to_owned())
        );
        // The old names are not read any more, so a leftover is ignored rather than half-honoured.
        let old = HubSettings::from_lookup(lookup(&git_env(&[("LK_GIT_WEBHOOK_SECRET", "x y")])))
            .unwrap()
            .git
            .unwrap();
        assert_eq!(old.webhook_secret, "github-webhook-secret");
    }

    /// Every `LK_*` name in the settings source files.
    fn variable_names() -> BTreeSet<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![root.join("config.rs")];
        for entry in std::fs::read_dir(root.join("config")).unwrap() {
            files.push(entry.unwrap().path());
        }
        let mut names = BTreeSet::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            // Only the code above the tests: a test may spell a bad name on purpose.
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            names.extend(
                code.split('"')
                    .filter(|s| {
                        s.starts_with("LK_")
                            && s.bytes()
                                .all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit())
                    })
                    .map(str::to_owned),
            );
        }
        names
    }

    #[test]
    fn no_setting_name_looks_like_a_secret() {
        // S7, G9: a variable that carries a secret would be flagged by the pod contract, so none is named like one. The one
        // exception is a path: `LK_GIT_TOKEN_FILE` names a mounted file (Q17's interim), it does not hold a token.
        let exceptions = ["LK_GIT_TOKEN_FILE"];
        let names = variable_names();
        assert!(names.len() > 30, "the scan found only {} names", names.len());
        for name in &names {
            let looks = ["PASSWORD", "TOKEN", "SECRET", "CREDENTIAL"]
                .iter()
                .any(|m| name.contains(m))
                || name.ends_with("_KEY");
            assert!(
                !looks || exceptions.contains(&name.as_str()),
                "{name} looks like a secret"
            );
        }
    }

    proptest::proptest! {
        /// S7, S10: a value that is refused is never in the error, whatever the variable. The marker cannot occur in a fixed message.
        #[test]
        fn secret_settings_are_names_or_files_and_errors_never_print_values(tail in "[ -~]{0,40}") {
            let value = format!("zz9q{tail}");
            for name in [
                "LK_WEB_CERT_REF", "LK_WEB_KEY_REF", "LK_GCP_PROJECT", "LK_WEB_ADDR", "LK_OPS_ADDR",
                "LK_START_TIMEOUT_SECS", "LK_WEB_CERT_RELOAD_SECS", "LK_DATABASE_URL_FILE", "LK_DB_CA_FILE",
                "LK_GIT_BASE_URL", "LK_GIT_READ_REF", "LK_TRUSTED_TYPES", "LK_OTLP_ENDPOINT", "LK_MIRROR_DIR",
                "LK_AGENT_CA_KMS_REF", "LK_AGENT_CA_CERT_REF", "LK_REPLICA_CA_KMS_REF", "LK_REPLICA_CA_CERT_REF",
                "LK_REPLICA_NAME", "LK_REPLICA_ADDRESS", "LK_HUB_AUDIENCE", "LK_AGENT_ENDPOINT_HOSTS", "LK_AGENT_ADDR",
                "LK_REPLICA_ADDR",
            ] {
                let mut env = serving_env(&[]);
                env.retain(|(k, _)| *k != name);
                env.push((name, value.as_str()));
                if let Err(error) = with_files(&env, &[]) {
                    let shown = format!("{error} {error:?}");
                    proptest::prop_assert!(!shown.contains("zz9q"), "{name}: {shown}");
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------ T2 (plan 18): hub migrate

    fn migrator(env: &[(&str, &str)], files: &[(&str, &str)]) -> Result<MigratorSettings, ConfigError> {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let files: HashMap<String, String> = files
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        MigratorSettings::from_parts(
            move |name| env.get(name).cloned(),
            move |path| {
                files
                    .get(path.to_string_lossy().as_ref())
                    .cloned()
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
            },
        )
    }

    #[test]
    fn the_migrator_credential_is_a_mounted_file_read_trimmed_and_redacted() {
        let settings = migrator(
            &[
                ("LK_MIGRATOR_URL_FILE", "/run/migrator/uri"),
                ("LK_DB_CA_FILE", "/run/ca/ca.pem"),
            ],
            &[(
                "/run/migrator/uri",
                "postgres://lanekeeper_migrator:hunter2@db/x\n",
            )],
        )
        .unwrap();
        assert_eq!(
            settings.url.expose(),
            "postgres://lanekeeper_migrator:hunter2@db/x"
        );
        assert_eq!(settings.ca_file, PathBuf::from("/run/ca/ca.pem"));
        assert_eq!(settings.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        assert!(!format!("{settings:?}").contains("hunter2"));
    }

    #[test]
    fn the_migrator_settings_name_what_is_missing_before_they_read_the_file() {
        assert_eq!(
            migrator(&[], &[]).unwrap_err(),
            ConfigError::Missing("LK_MIGRATOR_URL_FILE")
        );
        // The credential is a file and only a file: the plain variable of the hub's own pool is not an alternative.
        assert_eq!(
            migrator(
                &[
                    ("LK_MIGRATOR_URL", "postgres://m:x@db/y"),
                    ("LK_DB_CA_FILE", "/ca")
                ],
                &[]
            )
            .unwrap_err(),
            ConfigError::Missing("LK_MIGRATOR_URL_FILE")
        );
        assert_eq!(
            migrator(&[("LK_MIGRATOR_URL_FILE", "/u")], &[("/u", "postgres://m@db/y")]).unwrap_err(),
            ConfigError::Missing("LK_DB_CA_FILE")
        );
    }

    #[test]
    fn a_missing_empty_or_unreadable_migrator_file_is_refused_without_naming_the_path() {
        let env = [
            ("LK_MIGRATOR_URL_FILE", "/run/migrator/uri"),
            ("LK_DB_CA_FILE", "/ca"),
        ];
        let missing = migrator(&env, &[]).unwrap_err();
        assert_eq!(missing, ConfigError::Invalid("LK_MIGRATOR_URL_FILE"));
        assert!(!format!("{missing}").contains("/run/migrator"));
        assert_eq!(
            migrator(&env, &[("/run/migrator/uri", " \n")]).unwrap_err(),
            ConfigError::Invalid("LK_MIGRATOR_URL_FILE")
        );
        assert_eq!(
            migrator(&env, &[("/run/migrator/uri", "postgres://a\nb")]).unwrap_err(),
            ConfigError::Invalid("LK_MIGRATOR_URL_FILE")
        );
    }

    #[test]
    fn the_migration_time_limit_is_bounded() {
        let base = [("LK_MIGRATOR_URL_FILE", "/u"), ("LK_DB_CA_FILE", "/ca")];
        let files = [("/u", "postgres://lanekeeper_migrator@db/x")];
        let with = |value: &str| {
            let mut env = base.to_vec();
            env.push(("LK_MIGRATE_TIMEOUT_SECS", value));
            migrator(&env, &files)
        };
        assert_eq!(with("90").unwrap().timeout, Duration::from_secs(90));
        assert_eq!(with("7200").unwrap().timeout, Duration::from_secs(7200));
        for bad in ["0", "7201", "soon", "-1", "1.5"] {
            assert_eq!(
                with(bad).unwrap_err(),
                ConfigError::Invalid("LK_MIGRATE_TIMEOUT_SECS"),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_job_only_variables_are_the_ones_the_migrator_reads_and_the_hub_does_not() {
        let source = include_str!("config/migrator.rs");
        let code = source.split("impl MigratorSettings").nth(1).unwrap();
        for name in JOB_ONLY_VARIABLES {
            assert!(code.contains(&format!("\"{name}\"")), "{name} is not read");
        }
        // Nothing of the serving settings reads them.
        let serving = include_str!("config.rs").split("#[cfg(test)]").next().unwrap();
        for name in JOB_ONLY_VARIABLES {
            assert!(
                !serving.contains(&format!("\"{name}\"")),
                "{name} is read by the hub"
            );
        }
    }

    proptest::proptest! {
        /// S7, S10: whatever a migrator variable holds, a refusal names the variable and never the value.
        #[test]
        fn the_migrator_errors_never_print_a_value(tail in "[ -~]{0,40}") {
            let value = format!("zz9q{tail}");
            for name in ["LK_MIGRATOR_URL_FILE", "LK_DB_CA_FILE", "LK_MIGRATE_TIMEOUT_SECS"] {
                let mut env = vec![
                    ("LK_MIGRATOR_URL_FILE", "/u"),
                    ("LK_DB_CA_FILE", "/ca"),
                ];
                env.retain(|(k, _)| *k != name);
                env.push((name, value.as_str()));
                if let Err(error) = migrator(&env, &[("/u", value.as_str())]) {
                    let shown = format!("{error} {error:?}");
                    proptest::prop_assert!(!shown.contains("zz9q"), "{name}: {shown}");
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------ T3 (plan 18): the platform

    #[test]
    fn serving_needs_the_platform_and_then_the_git_remotes_and_names_the_first_that_is_missing() {
        let base = [
            ("LK_DATABASE_URL", "postgres://u:p@db/x"),
            ("LK_DB_CA_FILE", "/ca.pem"),
            ("LK_WEB_CERT_REF", "web-cert"),
            ("LK_WEB_KEY_REF", "web-key"),
        ];
        let no_platform = HubSettings::from_lookup(lookup(&base)).unwrap();
        assert!(no_platform.platform.is_none());
        assert_eq!(
            no_platform.require_serving().unwrap_err(),
            ConfigError::Missing("LK_AGENT_CA_KMS_REF")
        );
        let mut with_platform = base.to_vec();
        with_platform.extend(platform_env());
        let no_git = HubSettings::from_lookup(lookup(&with_platform)).unwrap();
        assert_eq!(
            no_git.require_serving().unwrap_err(),
            ConfigError::Missing("LK_GIT_BASE_URL")
        );
    }

    #[test]
    fn the_platform_is_read_and_the_listeners_default_to_the_ports_of_the_pod_contract() {
        let platform = HubSettings::from_lookup(lookup(&serving_env(&[])))
            .unwrap()
            .platform
            .unwrap();
        assert_eq!(platform.agent_addr, "0.0.0.0:8444".parse::<SocketAddr>().unwrap());
        assert_eq!(
            platform.replica_addr,
            "0.0.0.0:8445".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(platform.agent_ca.kms_ref.as_str(), AGENT_KMS);
        assert_eq!(platform.agent_ca.cert_ref, "lanekeeper-agent-ca");
        assert_eq!(platform.replica_ca.kms_ref.as_str(), REPLICA_KMS);
        assert_eq!(platform.replica_ca.cert_ref, "lanekeeper-replica-ca");
        assert_eq!(platform.replica_name.as_str(), "lanekeeper-hub-0");
        assert_eq!(
            platform.replica_address.as_str(),
            "lanekeeper-hub-0.lanekeeper-hub.lanekeeper.svc:8445"
        );
        assert_eq!(
            platform.replica_address.host(),
            "lanekeeper-hub-0.lanekeeper-hub.lanekeeper.svc"
        );
        assert_eq!(platform.audience.as_str(), "https://hub.lanekeeper.example");
        assert_eq!(platform.agent_hosts, vec!["agents.lanekeeper.example".to_owned()]);
        let moved = HubSettings::from_lookup(lookup(&serving_env(&[
            ("LK_AGENT_ADDR", "127.0.0.1:0"),
            ("LK_REPLICA_ADDR", "127.0.0.1:0"),
            ("LK_AGENT_ENDPOINT_HOSTS", "a.example, b.example"),
        ])))
        .unwrap()
        .platform
        .unwrap();
        assert_eq!(moved.agent_addr.port(), 0);
        assert_eq!(moved.replica_addr.port(), 0);
        assert_eq!(
            moved.agent_hosts,
            vec!["a.example".to_owned(), "b.example".to_owned()]
        );
    }

    #[test]
    fn the_platform_group_is_all_or_nothing_and_a_missing_variable_is_named() {
        for (name, _) in platform_env() {
            let env: Vec<_> = serving_env(&[]).into_iter().filter(|(n, _)| *n != name).collect();
            let error = HubSettings::from_lookup(lookup(&env)).unwrap_err();
            assert_eq!(error, ConfigError::Missing(name), "without {name}");
        }
        // With none of the group set the settings are fine (the library's defaults); serving is what refuses.
        let env: Vec<_> = serving_env(&[])
            .into_iter()
            .filter(|(n, _)| !platform_env().iter().any(|(p, _)| p == n))
            .collect();
        assert!(HubSettings::from_lookup(lookup(&env)).unwrap().platform.is_none());
    }

    #[test]
    fn a_malformed_platform_setting_is_refused_by_name() {
        let toolong = "a".repeat(300);
        for (name, bad) in [
            // A key, not a version of it; another resource kind; a path out of the name.
            (
                "LK_AGENT_CA_KMS_REF",
                "projects/lk/locations/l/keyRings/r/cryptoKeys/agent-ca",
            ),
            (
                "LK_AGENT_CA_KMS_REF",
                "projects/lk/../keyRings/r/cryptoKeys/k/cryptoKeyVersions/1",
            ),
            ("LK_AGENT_CA_KMS_REF", "has space"),
            (
                "LK_REPLICA_CA_KMS_REF",
                "projects/lk/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/latest",
            ),
            ("LK_AGENT_CA_CERT_REF", "a/b"),
            ("LK_REPLICA_CA_CERT_REF", "has space"),
            ("LK_REPLICA_NAME", "pod/0"),
            ("LK_REPLICA_NAME", "-pod"),
            ("LK_REPLICA_NAME", toolong.as_str()),
            ("LK_REPLICA_ADDRESS", "no-port"),
            ("LK_REPLICA_ADDRESS", "host:0"),
            ("LK_REPLICA_ADDRESS", "ho st:8445"),
            ("LK_REPLICA_ADDRESS", "host:99999"),
            // The host goes on a certificate, which takes lower case only.
            ("LK_REPLICA_ADDRESS", "Hub-0.hub.svc:8445"),
            ("LK_HUB_AUDIENCE", toolong.as_str()),
            ("LK_AGENT_ENDPOINT_HOSTS", "UPPER.example"),
            ("LK_AGENT_ENDPOINT_HOSTS", "*.example"),
            ("LK_AGENT_ENDPOINT_HOSTS", "a.example,,"),
            ("LK_AGENT_ENDPOINT_HOSTS", "1,2,3,4,5,6,7,8,9"),
            ("LK_AGENT_ADDR", "not-an-address"),
            ("LK_REPLICA_ADDR", "8445"),
        ] {
            assert_eq!(
                HubSettings::from_lookup(lookup(&serving_env(&[(name, bad)]))).unwrap_err(),
                ConfigError::Invalid(name),
                "{name}={bad}"
            );
        }
    }

    #[test]
    fn the_kms_references_are_resource_names_and_never_a_secret() {
        // They are names of keys. The pod contract flags any variable that looks like a secret, and these are not named like one.
        for (name, _) in platform_env() {
            let upper = name.to_ascii_uppercase();
            assert!(
                !["PASSWORD", "TOKEN", "SECRET", "CREDENTIAL"]
                    .iter()
                    .any(|m| upper.contains(m))
                    && !upper.ends_with("_KEY"),
                "{name}"
            );
        }
    }

    #[test]
    fn git_settings_are_absent_unless_a_remote_is_set() {
        assert!(HubSettings::from_lookup(lookup(&[])).unwrap().git.is_none());
    }

    fn git_env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut env = vec![
            (
                "LK_GIT_BASE_URL",
                "https://github.com/acme/configuration-base-saas.git",
            ),
            ("LK_GIT_TENANT_URL", "https://github.com/acme/csp-tenant-data.git"),
            ("LK_GIT_BASE_REPO", "acme/configuration-base-saas"),
            ("LK_GIT_TENANT_REPO", "acme/csp-tenant-data"),
        ];
        env.extend_from_slice(extra);
        env
    }

    #[test]
    fn git_settings_have_documented_defaults() {
        let git = HubSettings::from_lookup(lookup(&git_env(&[])))
            .unwrap()
            .git
            .unwrap();
        assert_eq!(git.base_repo.as_str(), "acme/configuration-base-saas");
        assert_eq!(git.webhook_secret, "github-webhook-secret");
        assert_eq!(
            git.credential,
            GitCredentialSource::SecretManager("github-read-token".to_owned())
        );
        assert_eq!(git.fetch_timeout, Duration::from_secs(120));
        assert_eq!(git.queue_capacity, 256);
    }

    #[test]
    fn git_settings_may_name_a_mounted_token_file_for_the_interim() {
        let git = HubSettings::from_lookup(lookup(&git_env(&[("LK_GIT_TOKEN_FILE", "/run/git/token")])))
            .unwrap()
            .git
            .unwrap();
        assert_eq!(
            git.credential,
            GitCredentialSource::MountedFile(PathBuf::from("/run/git/token"))
        );
        let both = HubSettings::from_lookup(lookup(&git_env(&[
            ("LK_GIT_TOKEN_FILE", "/run/git/token"),
            ("LK_GIT_READ_REF", "x"),
        ])))
        .unwrap_err();
        assert_eq!(both, ConfigError::Invalid("LK_GIT_TOKEN_FILE"));
    }

    #[test]
    fn git_urls_must_be_https_without_a_credential_and_the_error_never_shows_the_value() {
        for bad in [
            "http://github.com/a/b.git",
            "https://user:ghp_SECRETTOKEN@github.com/a/b.git",
            "file:///srv/x.git",
            "git@github.com:a/b.git",
        ] {
            let err = HubSettings::from_lookup(lookup(&[
                ("LK_GIT_BASE_URL", bad),
                ("LK_GIT_TENANT_URL", "https://github.com/a/c.git"),
                ("LK_GIT_BASE_REPO", "a/b"),
                ("LK_GIT_TENANT_REPO", "a/c"),
            ]))
            .unwrap_err();
            assert_eq!(err, ConfigError::Invalid("LK_GIT_BASE_URL"), "{bad}");
            assert!(!format!("{err} {err:?}").contains("SECRETTOKEN"));
        }
    }

    #[test]
    fn git_settings_need_all_four_remote_variables_or_none() {
        for missing in [
            "LK_GIT_BASE_URL",
            "LK_GIT_TENANT_URL",
            "LK_GIT_BASE_REPO",
            "LK_GIT_TENANT_REPO",
        ] {
            let env: Vec<_> = git_env(&[]).into_iter().filter(|(k, _)| *k != missing).collect();
            assert_eq!(
                HubSettings::from_lookup(lookup(&env)).unwrap_err(),
                ConfigError::Missing(match missing {
                    "LK_GIT_BASE_URL" => "LK_GIT_BASE_URL",
                    "LK_GIT_TENANT_URL" => "LK_GIT_TENANT_URL",
                    "LK_GIT_BASE_REPO" => "LK_GIT_BASE_REPO",
                    _ => "LK_GIT_TENANT_REPO",
                }),
            );
        }
    }

    #[test]
    fn git_repository_names_and_secret_names_are_checked() {
        for (name, value) in [
            ("LK_GIT_BASE_REPO", "no-slash"),
            ("LK_GIT_TENANT_REPO", "a/b/c"),
            ("LK_GIT_WEBHOOK_REF", "has space"),
            ("LK_GIT_READ_REF", "../x"),
            ("LK_GIT_FETCH_TIMEOUT_SECS", "0"),
            ("LK_GIT_QUEUE_CAPACITY", "0"),
        ] {
            assert!(
                HubSettings::from_lookup(lookup(&git_env(&[(name, value)]))).is_err(),
                "{name}={value}"
            );
        }
    }
    // ------------------------------------------------------------------------------------------ T5 (plan 18): registry and writes

    #[test]
    fn serving_needs_the_vault_key_and_says_which_variable_to_set() {
        let mut env = serving_env(&[]);
        env.retain(|(name, _)| *name != "LK_VAULT_KMS_REF");
        let without = HubSettings::from_lookup(lookup(&env)).unwrap();
        assert!(without.writes.is_none());
        assert_eq!(
            without.require_serving().unwrap_err(),
            ConfigError::Missing("LK_VAULT_KMS_REF")
        );
    }

    #[test]
    fn the_vault_key_is_a_crypto_key_and_not_a_version_or_a_ring() {
        for bad in [
            "projects/lk/locations/l/keyRings/r/cryptoKeys/vault/cryptoKeyVersions/1",
            "projects/lk/locations/l/keyRings/r",
            "vault",
            "projects/lk/locations/l/keyRings/r/cryptoKeys/va ult",
        ] {
            assert_eq!(
                HubSettings::from_lookup(lookup(&serving_env(&[("LK_VAULT_KMS_REF", bad)]))).unwrap_err(),
                ConfigError::Invalid("LK_VAULT_KMS_REF"),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_registry_and_write_settings_have_the_documented_defaults() {
        let settings = HubSettings::from_lookup(lookup(&serving_env(&[]))).unwrap();
        let registry = &settings.registry;
        assert!(
            !registry.retention_enabled,
            "retention is off until a person switches it on (G16)"
        );
        assert!(
            registry.config.retention_dry_run,
            "and then it only reports (D118)"
        );
        assert!(registry.config.extra_deny_globs.is_empty());
        assert_eq!(registry.config.corpus_cache_bytes, 256 * 1024 * 1024);
        let writes = settings.writes.unwrap();
        assert_eq!(writes.github_api_base, "https://api.github.com");
        assert_eq!(writes.github_api_host, "api.github.com");
        assert_eq!(writes.base_branch.to_string(), "main");
        assert_eq!(writes.config_server_deployment, "csp-configuration-server");
        assert!(writes.teams_hosts.is_none());
        assert_eq!(writes.jobs, WriteJobTimes::default());
        assert_eq!(WriteJobTimes::default().pr_jobs, Duration::from_secs(5));
    }

    #[test]
    fn the_knobs_of_the_registry_and_the_write_paths_are_read_and_bounded() {
        let settings = HubSettings::from_lookup(lookup(&serving_env(&[
            ("LK_RETENTION_ENABLED", "true"),
            ("LK_RETENTION_DRY_RUN", "false"),
            ("LK_EXTRA_DENY_GLOBS", "secrets/**, **/*.pem"),
            ("LK_CORPUS_CACHE_MIB", "64"),
            ("LK_GITHUB_API_BASE", "https://ghe.example.com/"),
            ("LK_GIT_BASE_BRANCH", "release/base"),
            ("LK_CONFIG_SERVER_DEPLOYMENT", "config-server"),
            ("LK_TEAMS_ALLOWED_HOSTS", "*.logic.azure.com,hooks.example.com"),
            ("LK_WRITES_PR_JOBS_SECS", "1"),
        ])))
        .unwrap();
        assert!(settings.registry.retention_enabled);
        assert!(!settings.registry.config.retention_dry_run);
        assert_eq!(
            settings.registry.config.extra_deny_globs,
            ["secrets/**", "**/*.pem"]
        );
        assert_eq!(settings.registry.deny_globs(), ["secrets/**", "**/*.pem"]);
        assert_eq!(settings.registry.config.corpus_cache_bytes, 64 * 1024 * 1024);
        let writes = settings.writes.unwrap();
        assert_eq!(writes.github_api_host, "ghe.example.com");
        assert_eq!(writes.base_branch.to_string(), "release/base");
        assert_eq!(writes.config_server_deployment, "config-server");
        assert_eq!(
            writes.teams_hosts.unwrap(),
            ["*.logic.azure.com".to_owned(), "hooks.example.com".to_owned()]
        );
        assert_eq!(writes.jobs.pr_jobs, Duration::from_secs(1));
    }

    #[test]
    fn a_registry_or_write_setting_that_is_wrong_is_refused_by_name_and_never_by_value() {
        for (name, bad) in [
            ("LK_RETENTION_ENABLED", "yes"),
            ("LK_RETENTION_ENABLED", "True"),
            ("LK_RETENTION_DRY_RUN", "1"),
            ("LK_EXTRA_DENY_GLOBS", "["),
            ("LK_EXTRA_DENY_GLOBS", "a,,b"),
            ("LK_EXTRA_DENY_GLOBS", ","),
            ("LK_CORPUS_CACHE_MIB", "8"),
            ("LK_CORPUS_CACHE_MIB", "99999"),
            ("LK_GITHUB_API_BASE", "http://api.github.com"),
            ("LK_GITHUB_API_BASE", "https://api.github.com:8443"),
            ("LK_GITHUB_API_BASE", "https://API.github.com"),
            ("LK_GITHUB_API_BASE", "https://api.github.com/api/v3"),
            ("LK_GIT_BASE_BRANCH", "a..b"),
            ("LK_CONFIG_SERVER_DEPLOYMENT", "-bad"),
            ("LK_TEAMS_ALLOWED_HOSTS", "https://hooks.example.com"),
            ("LK_TEAMS_ALLOWED_HOSTS", "*.*.example.com"),
            ("LK_WRITES_RECONCILE_SECS", "0"),
            ("LK_WRITES_EXPIRE_SECS", "100000"),
            ("LK_WRITES_PR_JOBS_SECS", "301"),
            ("LK_WRITES_CLEANUP_SECS", "59"),
        ] {
            let error = HubSettings::from_lookup(lookup(&serving_env(&[(name, bad)]))).unwrap_err();
            assert_eq!(error, ConfigError::Invalid(name), "{name}={bad}");
            assert!(
                !format!("{error} {error:?}").contains(bad) || bad.len() < 3,
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn more_deny_globs_than_the_write_path_takes_are_refused() {
        let many = (0..=hub_writes::MAX_EXTRA_DENY_GLOBS)
            .map(|n| format!("dir{n}/*"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            HubSettings::from_lookup(lookup(&serving_env(&[("LK_EXTRA_DENY_GLOBS", &many)]))).unwrap_err(),
            ConfigError::Invalid("LK_EXTRA_DENY_GLOBS")
        );
    }
}
