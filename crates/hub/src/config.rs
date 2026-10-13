//! Settings from the environment (`LK_*`), parsed once at start-up (rule 5, S7).
//!
//! Every setting has a typed field and a default that is safe for production. A malformed value is an error that names the
//! variable and never prints the value, because the value may be a secret (the database URL holds a password).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use domain::Secret;
use hub_platform::git::mirror::RemoteUrl;
use hub_platform::git::payload::RepoFullName;

/// A setting that could not be read. Carries the variable name only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{0} is not valid")]
    Invalid(&'static str),
}

/// How fast one client may call, as a token bucket: `per_second` refills, up to `burst` at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub per_second: f64,
    pub burst: u32,
}

/// Everything about request limits (T1). The defaults are the ones in the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitSettings {
    /// Largest request body (S11). Applies to chunked bodies too.
    pub body_max: usize,
    /// Longest time to produce a response head (the body of an SSE stream is not covered).
    pub request_timeout: Duration,
    /// Deepest JSON nesting a request body may have.
    pub json_depth: usize,
    pub per_ip: Rate,
    pub per_user: Rate,
    /// How many buckets each limiter keeps. A flood of new addresses evicts old buckets instead of growing memory.
    pub bucket_capacity: u64,
    /// A bucket nobody has used for this long is dropped.
    pub bucket_idle: Duration,
}

impl Default for LimitSettings {
    fn default() -> Self {
        Self {
            body_max: 4 * 1024 * 1024,
            request_timeout: Duration::from_secs(30),
            json_depth: 64,
            per_ip: Rate {
                per_second: 50.0,
                burst: 200,
            },
            per_user: Rate {
                per_second: 30.0,
                burst: 120,
            },
            bucket_capacity: 100_000,
            bucket_idle: Duration::from_secs(600),
        }
    }
}

/// Where Postgres is and how it is reached (S6): TLS with full verification against the cluster's own CA.
#[derive(Debug)]
pub struct DatabaseSettings {
    pub url: Secret<String>,
    /// The CA that signed the server's certificate (`CloudNativePG`'s own CA). Required: there is no "trust the system" mode.
    pub ca_file: PathBuf,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub statement_timeout: Duration,
}

/// Where the read-only Git credential comes from (Q17, A12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCredentialSource {
    /// The token is this secret in Secret Manager (the default, S7).
    SecretManager(String),
    /// The token is in this mounted file (Q17's interim: a Kubernetes Secret volume).
    MountedFile(PathBuf),
}

/// The Git mirrors and the webhook (T7). All of it is absent unless the remotes are set.
#[derive(Debug, Clone)]
pub struct GitSettings {
    pub base_url: RemoteUrl,
    pub tenant_url: RemoteUrl,
    /// `owner/name` as GitHub reports it in webhook payloads.
    pub base_repo: RepoFullName,
    pub tenant_repo: RepoFullName,
    /// The name of the webhook secret in Secret Manager.
    pub webhook_secret: String,
    pub credential: GitCredentialSource,
    pub fetch_timeout: Duration,
    /// Distinct pushes that can wait for the fetch worker.
    pub queue_capacity: usize,
    /// Index entries the tree-index cache holds.
    pub tree_index_entries: u64,
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
    pub database: Option<DatabaseSettings>,
    /// The Git mirrors and the webhook. `None` when no remote is configured.
    pub git: Option<GitSettings>,
    /// What the blob cache may hold (`LK_BLOB_CACHE_MIB`, default 512 MiB), in bytes.
    pub blob_cache_bytes: u64,
    /// OTLP gRPC endpoint of an in-cluster collector. No export when unset.
    pub otlp_endpoint: Option<String>,
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
    #[allow(clippy::too_many_lines)]
    pub fn from_parts(
        lookup: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&std::path::Path) -> std::io::Result<String>,
    ) -> Result<Self, ConfigError> {
        let get = |name: &'static str| lookup(name).filter(|v| !v.is_empty());
        let parse = |name: &'static str| -> Result<Option<String>, ConfigError> {
            let Some(value) = get(name) else { return Ok(None) };
            // A value with a control character is never a valid setting, and would make a log line ambiguous.
            if value.chars().any(char::is_control) {
                return Err(ConfigError::Invalid(name));
            }
            Ok(Some(value))
        };
        let addr = |name: &'static str, default: &str| -> Result<SocketAddr, ConfigError> {
            parse(name)?
                .unwrap_or_else(|| default.to_owned())
                .parse()
                .map_err(|_| ConfigError::Invalid(name))
        };
        let number = |name: &'static str, default: u64, min: u64, max: u64| -> Result<u64, ConfigError> {
            let Some(value) = parse(name)? else {
                return Ok(default);
            };
            let n: u64 = value.parse().map_err(|_| ConfigError::Invalid(name))?;
            if (min..=max).contains(&n) {
                Ok(n)
            } else {
                Err(ConfigError::Invalid(name))
            }
        };
        let float = |name: &'static str, default: f64| -> Result<f64, ConfigError> {
            let Some(value) = parse(name)? else {
                return Ok(default);
            };
            let n: f64 = value.parse().map_err(|_| ConfigError::Invalid(name))?;
            if n.is_finite() && n > 0.0 && n <= 1_000_000.0 {
                Ok(n)
            } else {
                Err(ConfigError::Invalid(name))
            }
        };
        let defaults = LimitSettings::default();
        let limits = LimitSettings {
            body_max: usize::try_from(number(
                "LK_BODY_MAX_BYTES",
                defaults.body_max as u64,
                1024,
                64 * 1024 * 1024,
            )?)
            .map_err(|_| ConfigError::Invalid("LK_BODY_MAX_BYTES"))?,
            request_timeout: Duration::from_secs(number(
                "LK_REQUEST_TIMEOUT_SECS",
                defaults.request_timeout.as_secs(),
                1,
                600,
            )?),
            json_depth: usize::try_from(number("LK_JSON_DEPTH", defaults.json_depth as u64, 1, 256)?)
                .map_err(|_| ConfigError::Invalid("LK_JSON_DEPTH"))?,
            per_ip: Rate {
                per_second: float("LK_IP_RATE_PER_SEC", defaults.per_ip.per_second)?,
                burst: u32::try_from(number(
                    "LK_IP_BURST",
                    u64::from(defaults.per_ip.burst),
                    1,
                    100_000,
                )?)
                .map_err(|_| ConfigError::Invalid("LK_IP_BURST"))?,
            },
            per_user: Rate {
                per_second: float("LK_USER_RATE_PER_SEC", defaults.per_user.per_second)?,
                burst: u32::try_from(number(
                    "LK_USER_BURST",
                    u64::from(defaults.per_user.burst),
                    1,
                    100_000,
                )?)
                .map_err(|_| ConfigError::Invalid("LK_USER_BURST"))?,
            },
            bucket_capacity: number("LK_BUCKET_CAPACITY", defaults.bucket_capacity, 100, 10_000_000)?,
            bucket_idle: defaults.bucket_idle,
        };

        // The database URL holds a password. It is read from a mounted file (`LK_DATABASE_URL_FILE`), so it is never in the
        // process environment, where a child process, a crash dump or `/proc/<pid>/environ` could show it. The plain
        // variable exists for local development and is refused when the file variable is also set.
        let database_url = match (parse("LK_DATABASE_URL_FILE")?, parse("LK_DATABASE_URL")?) {
            (Some(_), Some(_)) => return Err(ConfigError::Invalid("LK_DATABASE_URL")),
            (Some(path), None) => {
                let text = read_file(std::path::Path::new(&path))
                    .map_err(|_| ConfigError::Invalid("LK_DATABASE_URL_FILE"))?;
                let url = text.trim().to_owned();
                if url.is_empty() || url.chars().any(char::is_control) {
                    return Err(ConfigError::Invalid("LK_DATABASE_URL_FILE"));
                }
                Some(url)
            }
            (None, url) => url,
        };
        let database = match database_url {
            None => None,
            Some(url) => Some(DatabaseSettings {
                url: Secret::new(url),
                ca_file: parse("LK_DB_CA_FILE")?
                    .map(PathBuf::from)
                    .ok_or(ConfigError::Missing("LK_DB_CA_FILE"))?,
                max_connections: u32::try_from(number("LK_DB_MAX_CONNECTIONS", 16, 1, 512)?)
                    .map_err(|_| ConfigError::Invalid("LK_DB_MAX_CONNECTIONS"))?,
                acquire_timeout: Duration::from_secs(number("LK_DB_ACQUIRE_TIMEOUT_SECS", 5, 1, 60)?),
                statement_timeout: Duration::from_secs(number("LK_DB_STATEMENT_TIMEOUT_SECS", 30, 1, 600)?),
            }),
        };

        let git_settings = {
            let url = |name: &'static str| -> Result<Option<RemoteUrl>, ConfigError> {
                parse(name)?
                    .map(|v| RemoteUrl::https(&v).map_err(|_| ConfigError::Invalid(name)))
                    .transpose()
            };
            let repo = |name: &'static str| -> Result<Option<RepoFullName>, ConfigError> {
                parse(name)?
                    .map(|v| RepoFullName::parse(&v).map_err(|_| ConfigError::Invalid(name)))
                    .transpose()
            };
            let secret_name = |name: &'static str, default: &str| -> Result<String, ConfigError> {
                let value = parse(name)?.unwrap_or_else(|| default.to_owned());
                if ports::is_valid_secret_name(&value) {
                    Ok(value)
                } else {
                    Err(ConfigError::Invalid(name))
                }
            };
            let (base_url, tenant_url) = (url("LK_GIT_BASE_URL")?, url("LK_GIT_TENANT_URL")?);
            let (base_repo, tenant_repo) = (repo("LK_GIT_BASE_REPO")?, repo("LK_GIT_TENANT_REPO")?);
            if base_url.is_none() && tenant_url.is_none() && base_repo.is_none() && tenant_repo.is_none() {
                None
            } else {
                let credential = match (parse("LK_GIT_TOKEN_FILE")?, parse("LK_GIT_TOKEN_SECRET")?) {
                    (Some(_), Some(_)) => return Err(ConfigError::Invalid("LK_GIT_TOKEN_FILE")),
                    (Some(path), None) => GitCredentialSource::MountedFile(PathBuf::from(path)),
                    (None, _) => GitCredentialSource::SecretManager(secret_name(
                        "LK_GIT_TOKEN_SECRET",
                        "github-read-token",
                    )?),
                };
                Some(GitSettings {
                    base_url: base_url.ok_or(ConfigError::Missing("LK_GIT_BASE_URL"))?,
                    tenant_url: tenant_url.ok_or(ConfigError::Missing("LK_GIT_TENANT_URL"))?,
                    base_repo: base_repo.ok_or(ConfigError::Missing("LK_GIT_BASE_REPO"))?,
                    tenant_repo: tenant_repo.ok_or(ConfigError::Missing("LK_GIT_TENANT_REPO"))?,
                    webhook_secret: secret_name("LK_GIT_WEBHOOK_SECRET", "github-webhook-secret")?,
                    credential,
                    fetch_timeout: Duration::from_secs(number("LK_GIT_FETCH_TIMEOUT_SECS", 120, 5, 1800)?),
                    queue_capacity: usize::try_from(number("LK_GIT_QUEUE_CAPACITY", 256, 1, 100_000)?)
                        .map_err(|_| ConfigError::Invalid("LK_GIT_QUEUE_CAPACITY"))?,
                    tree_index_entries: number("LK_TREE_INDEX_ENTRIES", 400_000, 1_000, 50_000_000)?,
                })
            }
        };

        let trusted_types: Vec<String> = parse("LK_TRUSTED_TYPES")?
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
            web_addr: addr("LK_WEB_ADDR", "0.0.0.0:8443")?,
            ops_addr: addr("LK_OPS_ADDR", "0.0.0.0:9090")?,
            mirror_dir: parse("LK_MIRROR_DIR")?
                .map_or_else(|| PathBuf::from("/var/lib/lanekeeper/mirror"), PathBuf::from),
            web_dist: parse("LK_WEB_DIST")?.map_or_else(|| PathBuf::from("/app/web/dist"), PathBuf::from),
            trusted_proxy_hops: usize::try_from(number("LK_TRUSTED_PROXY_HOPS", 1, 0, 8)?)
                .map_err(|_| ConfigError::Invalid("LK_TRUSTED_PROXY_HOPS"))?,
            trusted_types,
            limits,
            max_connections: usize::try_from(number("LK_MAX_CONNECTIONS", 4096, 1, 1_000_000)?)
                .map_err(|_| ConfigError::Invalid("LK_MAX_CONNECTIONS"))?,
            database,
            git: git_settings,
            blob_cache_bytes: number("LK_BLOB_CACHE_MIB", 512, 1, 4096)?.saturating_mul(1024 * 1024),
            otlp_endpoint: parse("LK_OTLP_ENDPOINT")?,
            shutdown_grace: Duration::from_secs(number("LK_SHUTDOWN_GRACE_SECS", 5, 0, 60)?),
            shutdown_drain: Duration::from_secs(number("LK_SHUTDOWN_DRAIN_SECS", 25, 1, 120)?),
        })
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
            blob_cache_bytes: 512 * 1024 * 1024,
            otlp_endpoint: None,
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
            ("LK_GIT_TOKEN_SECRET", "x"),
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
            ("LK_GIT_WEBHOOK_SECRET", "has space"),
            ("LK_GIT_TOKEN_SECRET", "../x"),
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
