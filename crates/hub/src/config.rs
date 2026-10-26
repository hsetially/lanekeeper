//! Settings from the environment (`LK_*`), parsed once at start-up (rule 5, S7).
//!
//! Every setting has a typed field and a default that is safe for production. A malformed value is an error that names the
//! variable and never prints the value, because the value may be a secret (the database URL holds a password).
//!
//! The settings are in groups, one file each under `config/`, and every group is read through [`Reader`]: process and
//! listeners (this file), request limits, the database, Git, Google Cloud and the web certificate; `hub migrate` has its own
//! small group, [`MigratorSettings`], that the serving hub never reads. A group arrives with the task that needs it, and
//! `tests/hardening.rs` reads every `LK_*` literal in these files to check the pod contract against them.

mod database;
mod gcp;
mod git;
mod limits;
mod migrator;
mod reader;
mod web_tls;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

pub use database::DatabaseSettings;
pub use gcp::GcpSettings;
pub use git::{GitCredentialSource, GitSettings};
pub use limits::{LimitSettings, Rate};
pub use migrator::{DEFAULT_TIMEOUT_SECS, JOB_ONLY_VARIABLES, MigratorSettings};
pub use reader::Reader;
pub use web_tls::{DEFAULT_RELOAD_SECS, WebTlsSettings};

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

    fn serving_env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut env = vec![
            ("LK_DATABASE_URL", "postgres://u:p@db/x"),
            ("LK_DB_CA_FILE", "/ca.pem"),
            ("LK_WEB_CERT_REF", "web-cert"),
            ("LK_WEB_KEY_REF", "web-key"),
            ("LK_GCP_PROJECT", "lanekeeper-prod"),
        ];
        env.extend_from_slice(extra);
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
}
