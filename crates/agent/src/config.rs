//! Configuration of the agent (T1): the environment, the limits, and the hub-controlled tunables.
//!
//! Everything is validated at startup, so a bad deployment fails at once with a clear message instead of misbehaving
//! later (S11). Errors name the variable and the rule that failed, never the value (S10, S21).
//!
//! Variables are read with the `LK_` prefix; an empty value counts as unset.
//!
//! | Variable | Rule | Default |
//! |---|---|---|
//! | `LK_HUB_ENDPOINT` | `https://host[:port]` | required |
//! | `LK_HUB_AUDIENCE` | text of at most 256 bytes, the ID token audience | required |
//! | `LK_HUB_CA_FILE` | absolute path of the pinned hub CA (PEM) | required |
//! | `LK_SWIMLANE` | swimlane id | required |
//! | `LK_CLUSTER`, `LK_PROJECT`, `LK_NFS_SERVER`, `LK_NFS_EXPORT` | text of at most 256 bytes, sent in `Hello` | required |
//! | `LK_NFS_ROOT` | absolute path of the mounted export, at most 256 bytes | required |
//! | `LK_CERT_SECRET` | Secret name, holds the certificate and key | required |
//! | `LK_JOIN_TOKEN_SECRET` | Secret name of the join-token fallback | none |
//! | `LK_JOIN_MODE` | `auto`, `workload-identity` or `token` | `auto` |
//! | `LK_NAMESPACES` | 1 to 64 distinct namespaces, comma separated | required |
//! | `LK_SPOOL_DIR` | absolute path, outside the NFS root | `/var/lib/lanekeeper/spool` |
//! | `LK_SPOOL_MAX_BYTES` | 4 MiB to 64 GiB | 512 MiB |
//! | `LK_SPOOL_MAX_ENTRIES` | 1 to 10,000,000 | 100,000 |
//! | `LK_TMP_DIR` | absolute path, outside the NFS root | `/tmp` |
//! | `LK_IGNORE_GLOBS` | up to 64 globs, added to the built-in ones | none |
//! | `LK_SYNC_JOB_NAME_GLOBS` | up to 64 globs | `*dataload*` |
//! | `LK_SYNC_JOB_LABEL` | `key=value` | none |
//! | `LK_HELM_HINT_CHART_GLOBS` | up to 64 globs | `csp-tenant-data-*` |
//! | `LK_CONFIG_SERVER_URL` | `http://host[:port]` | none (notify and fetch answer "unsupported") |
//! | `LK_CONFIG_SERVER_DEPLOYMENT` | `namespace/name`, the namespace must be watched | none |
//! | `LK_METADATA_URL` | `http://host[:port]` | `http://metadata.google.internal` |
//! | `LK_HEALTH_ADDR` | socket address | `0.0.0.0:9090` |
//! | `LK_POOL_THREADS` | 1 to 16 | 4 |
//! | `LK_LOG` | `error`, `warn`, `info`, `debug` or `trace` | `info` |
//!
//! Glob lists are separated by commas, so a pattern with a comma inside braces cannot be written; list the patterns
//! separately instead.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::hash::BuildHasher;
use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::path::{Component, PathBuf};
use std::time::Duration;

use domain::{AgentConfig, ShortText, SwimlaneId, TenantId, TextError};
use globset::Glob;
use http::Uri;

/// Hard limits. Where the contract already defines one, it is re-exported from there and never redefined.
pub mod limits {
    use std::ops::RangeInclusive;
    use std::time::Duration;

    /// The most namespaces the agent may watch. The list is the whole blast radius of its RBAC (S17).
    pub const MAX_NAMESPACES: usize = 64;
    /// The most entries in one glob list.
    pub const MAX_GLOBS: usize = 64;
    /// The most bytes of one file the agent reads into a delta or accepts in a write (D79). A hub value can only lower it.
    pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
    /// The most file bytes in one `ScanDelta` message (3 MiB). This is the contract's own constant.
    pub const MAX_DELTA_BYTES: usize = proto::limits::MAX_SCAN_DELTA_BYTES;
    /// The longest hub scan interval the agent accepts, and the shortest.
    pub const SCAN_INTERVAL_SECS: RangeInclusive<u32> = 5..=300;
    /// The heartbeat is never faster than 10 s: a faster one would break the idle-traffic budget (P3).
    pub const HEARTBEAT_INTERVAL_SECS: RangeInclusive<u32> = 10..=300;
    /// Worker threads for hashing (a dedicated rayon pool, never the global one).
    pub const POOL_THREADS: RangeInclusive<usize> = 1..=16;
    /// Spool size bounds in bytes. The floor holds one maximum-size delta with its framing.
    pub const SPOOL_BYTES: RangeInclusive<u64> = (4 * 1024 * 1024)..=(64 * 1024 * 1024 * 1024);
    /// Spool entry bounds.
    pub const SPOOL_ENTRIES: RangeInclusive<u64> = 1..=10_000_000;
    /// A full stat walk while changes are pending (a burst, bounded by [`MAX_DEFER`]).
    pub const PENDING_WALK_INTERVAL: Duration = Duration::from_secs(1);
    /// How long the tree must be quiet before a delta is released (D75).
    pub const QUIET_PERIOD: Duration = Duration::from_secs(3);
    /// The longest a delta is held back while changes keep arriving (D75).
    pub const MAX_DEFER: Duration = Duration::from_secs(30);
    /// NFS attribute caching can hide changes, so everything is rehashed this often (T4).
    pub const FULL_REHASH_INTERVAL: Duration = Duration::from_secs(15 * 60);

    const _: () = assert!(
        MAX_FILE_BYTES <= MAX_DELTA_BYTES as u64,
        "one file must fit in one delta message"
    );
}

/// Why the environment cannot be turned into [`Settings`]. Carries variable names and rules, never values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("{0} is not valid Unicode")]
    NotUnicode(&'static str),
    #[error("{name} is invalid: {reason}")]
    Invalid {
        name: &'static str,
        reason: Cow<'static, str>,
    },
}

/// Where the environment comes from, so tests never touch the process environment.
pub trait EnvSource {
    /// The raw value of `name`, if it is set.
    fn var_os(&self, name: &str) -> Option<OsString>;
}

/// The process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var_os(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
}

impl<S: BuildHasher> EnvSource for HashMap<String, String, S> {
    fn var_os(&self, name: &str) -> Option<OsString> {
        self.get(name).map(OsString::from)
    }
}

/// A Kubernetes object name that passed the DNS-1123 rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KubeName(String);

impl KubeName {
    /// A DNS-1123 label (namespaces): lowercase letters, digits and `-`, at most 63 characters.
    pub fn label(s: &str) -> Result<Self, Cow<'static, str>> {
        dns_label(s)?;
        Ok(Self(s.to_owned()))
    }

    /// A DNS-1123 subdomain (Secrets, Deployments): dot-separated labels, at most 253 characters.
    pub fn subdomain(s: &str) -> Result<Self, Cow<'static, str>> {
        if s.len() > 253 || s.split('.').try_for_each(dns_label).is_err() {
            return Err(Cow::Borrowed(
                "must be a DNS-1123 subdomain: dot-separated labels of lowercase letters, digits and '-', at most 253 characters",
            ));
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn dns_label(s: &str) -> Result<(), Cow<'static, str>> {
    let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    let b = s.as_bytes();
    let ends_ok = b.first().copied().is_some_and(alnum) && b.last().copied().is_some_and(alnum);
    if b.len() > 63 || !ends_ok || !b.iter().all(|&c| alnum(c) || c == b'-') {
        return Err(Cow::Borrowed(
            "must be a DNS-1123 label: lowercase letters, digits and '-', 1 to 63 characters, starting and ending with a letter or digit",
        ));
    }
    Ok(())
}

/// `scheme://authority` and nothing else: no credentials, path or query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl(String);

impl BaseUrl {
    /// Parse `s`, which must use exactly `scheme`, name a host, and carry no credentials, path or query.
    pub fn parse(s: &str, scheme: &str) -> Result<Self, Cow<'static, str>> {
        if s.len() > 256 {
            return Err(Cow::Borrowed("is longer than 256 bytes"));
        }
        if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(Cow::Borrowed("contains whitespace or a control character"));
        }
        let uri: Uri = s.parse().map_err(|_| Cow::Borrowed("is not a valid URL"))?;
        if uri.scheme_str() != Some(scheme) {
            return Err(Cow::Owned(format!("must use the {scheme} scheme")));
        }
        let Some(authority) = uri.authority() else {
            return Err(Cow::Borrowed("must name a host"));
        };
        if authority.as_str().contains('@') {
            return Err(Cow::Borrowed("must not contain credentials"));
        }
        if uri.path() != "/" || uri.query().is_some() {
            return Err(Cow::Borrowed("must not have a path or a query"));
        }
        Ok(Self(format!("{scheme}://{authority}")))
    }

    /// `scheme://authority`, with no trailing slash.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BaseUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Deployment in a watched namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentRef {
    pub namespace: KubeName,
    pub name: KubeName,
}

/// `key=value`, matched against Job labels (Q3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSelector {
    pub key: String,
    pub value: String,
}

/// How the agent proves itself at `Join` (S5, Q26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinMode {
    /// Workload Identity when the metadata server answers, the join token only when it is unreachable.
    Auto,
    /// Workload Identity only.
    WorkloadIdentity,
    /// The join token only; the metadata server is never contacted.
    Token,
}

/// The least that is logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// What the agent tells the hub about its mount, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NfsSettings {
    pub server: ShortText,
    pub export: ShortText,
    /// Where the export is mounted in this pod. Used to open the cap-std handle once, and in `Hello`.
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolSettings {
    pub dir: PathBuf,
    pub max_bytes: u64,
    pub max_entries: u64,
}

/// The validated environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub hub_endpoint: BaseUrl,
    pub hub_audience: ShortText,
    pub hub_ca_file: PathBuf,
    pub swimlane: SwimlaneId,
    pub cluster: ShortText,
    pub project: ShortText,
    pub nfs: NfsSettings,
    pub cert_secret: KubeName,
    pub join_token_secret: Option<KubeName>,
    pub join_mode: JoinMode,
    pub namespaces: Vec<KubeName>,
    pub spool: SpoolSettings,
    pub tmp_dir: PathBuf,
    /// The built-in ignore globs (`.nfs*`, `.lanekeeper-tmp-*`) followed by `LK_IGNORE_GLOBS`.
    pub ignore_globs: Vec<ShortText>,
    pub sync_job_name_globs: Vec<ShortText>,
    pub sync_job_label: Option<LabelSelector>,
    pub helm_hint_chart_globs: Vec<ShortText>,
    pub config_server_url: Option<BaseUrl>,
    pub config_server_deployment: Option<DeploymentRef>,
    pub metadata_url: BaseUrl,
    pub health_addr: SocketAddr,
    pub pool_threads: usize,
    pub log_level: LogLevel,
}

/// The globs that are always ignored: NFS silly-rename files and the agent's own temp files (A9).
const DEFAULT_IGNORE_GLOBS: [&str; 2] = [".nfs*", ".lanekeeper-tmp-*"];
const DEFAULT_SYNC_JOB_GLOBS: &str = "*dataload*";
const DEFAULT_HELM_HINT_GLOBS: &str = "csp-tenant-data-*";
const DEFAULT_METADATA_URL: &str = "http://metadata.google.internal";
const DEFAULT_HEALTH_ADDR: &str = "0.0.0.0:9090";
const DEFAULT_SPOOL_DIR: &str = "/var/lib/lanekeeper/spool";
const DEFAULT_TMP_DIR: &str = "/tmp";

impl Settings {
    /// Read and validate the whole environment.
    pub fn from_env<E: EnvSource + ?Sized>(env: &E) -> Result<Self, SettingsError> {
        let nfs_root = required(env, "LK_NFS_ROOT", |s| {
            // The root also goes into `Hello`, which takes short text.
            let path = abs_path(s)?;
            text(s)?;
            Ok(path)
        })?;
        let namespaces = required(env, "LK_NAMESPACES", |s| {
            let names = list(s, limits::MAX_NAMESPACES, KubeName::label)?;
            let distinct: BTreeSet<&str> = names.iter().map(KubeName::as_str).collect();
            if distinct.len() != names.len() {
                return Err(Cow::Borrowed("lists a namespace twice"));
            }
            Ok(names)
        })?;
        let join_token_secret = optional(env, "LK_JOIN_TOKEN_SECRET", KubeName::subdomain)?;
        let join_mode = defaulted(env, "LK_JOIN_MODE", "auto", join_mode)?;
        if join_mode == JoinMode::Token && join_token_secret.is_none() {
            return Err(invalid("LK_JOIN_MODE", "token needs LK_JOIN_TOKEN_SECRET"));
        }
        let spool_dir = defaulted(env, "LK_SPOOL_DIR", DEFAULT_SPOOL_DIR, abs_path)?;
        let tmp_dir = defaulted(env, "LK_TMP_DIR", DEFAULT_TMP_DIR, abs_path)?;
        for (name, dir) in [("LK_SPOOL_DIR", &spool_dir), ("LK_TMP_DIR", &tmp_dir)] {
            // Spool files inside the root would be tracked as config and sent again, forever.
            if dir.starts_with(&nfs_root) || nfs_root.starts_with(dir) {
                return Err(invalid(name, "must not contain or lie inside LK_NFS_ROOT"));
            }
        }
        let config_server_deployment = optional(env, "LK_CONFIG_SERVER_DEPLOYMENT", deployment)?;
        if let Some(d) = &config_server_deployment {
            if !namespaces.contains(&d.namespace) {
                return Err(invalid(
                    "LK_CONFIG_SERVER_DEPLOYMENT",
                    "its namespace must be in LK_NAMESPACES",
                ));
            }
        }
        let mut ignore_globs = texts_of(&DEFAULT_IGNORE_GLOBS);
        ignore_globs.extend(optional(env, "LK_IGNORE_GLOBS", globs)?.unwrap_or_default());

        Ok(Self {
            hub_endpoint: required(env, "LK_HUB_ENDPOINT", |s| BaseUrl::parse(s, "https"))?,
            hub_audience: required(env, "LK_HUB_AUDIENCE", text)?,
            hub_ca_file: required(env, "LK_HUB_CA_FILE", abs_path)?,
            swimlane: required(env, "LK_SWIMLANE", |s| {
                SwimlaneId::parse(s)
                    .map_err(|_| Cow::Borrowed("must be a swimlane id: lowercase letters, digits and '-'"))
            })?,
            cluster: required(env, "LK_CLUSTER", text)?,
            project: required(env, "LK_PROJECT", text)?,
            nfs: NfsSettings {
                server: required(env, "LK_NFS_SERVER", text)?,
                export: required(env, "LK_NFS_EXPORT", text)?,
                root: nfs_root,
            },
            cert_secret: required(env, "LK_CERT_SECRET", KubeName::subdomain)?,
            join_token_secret,
            join_mode,
            namespaces,
            spool: SpoolSettings {
                dir: spool_dir,
                max_bytes: defaulted(env, "LK_SPOOL_MAX_BYTES", "536870912", |s| {
                    uint(s, &limits::SPOOL_BYTES)
                })?,
                max_entries: defaulted(env, "LK_SPOOL_MAX_ENTRIES", "100000", |s| {
                    uint(s, &limits::SPOOL_ENTRIES)
                })?,
            },
            tmp_dir,
            ignore_globs,
            sync_job_name_globs: defaulted(env, "LK_SYNC_JOB_NAME_GLOBS", DEFAULT_SYNC_JOB_GLOBS, globs)?,
            sync_job_label: optional(env, "LK_SYNC_JOB_LABEL", label_selector)?,
            helm_hint_chart_globs: defaulted(
                env,
                "LK_HELM_HINT_CHART_GLOBS",
                DEFAULT_HELM_HINT_GLOBS,
                globs,
            )?,
            config_server_url: optional(env, "LK_CONFIG_SERVER_URL", |s| BaseUrl::parse(s, "http"))?,
            config_server_deployment,
            metadata_url: defaulted(env, "LK_METADATA_URL", DEFAULT_METADATA_URL, |s| {
                BaseUrl::parse(s, "http")
            })?,
            health_addr: defaulted(env, "LK_HEALTH_ADDR", DEFAULT_HEALTH_ADDR, |s| {
                s.parse()
                    .map_err(|_| Cow::Borrowed("must be a socket address such as 0.0.0.0:9090"))
            })?,
            pool_threads: defaulted(env, "LK_POOL_THREADS", "4", |s| {
                let n = uint(s, &(1..=16))?;
                usize::try_from(n).map_err(|_| Cow::Borrowed("is out of range"))
            })?,
            log_level: defaulted(env, "LK_LOG", "info", log_level)?,
        })
    }
}

// ---------------------------------------------------------------- parsing helpers

type Reason = Cow<'static, str>;

fn invalid(name: &'static str, reason: &'static str) -> SettingsError {
    SettingsError::Invalid {
        name,
        reason: Cow::Borrowed(reason),
    }
}

/// The value of `name`. An empty value counts as unset.
fn raw<E: EnvSource + ?Sized>(env: &E, name: &'static str) -> Result<Option<String>, SettingsError> {
    match env.var_os(name) {
        None => Ok(None),
        Some(v) => match v.into_string() {
            Ok(s) if s.is_empty() => Ok(None),
            Ok(s) => Ok(Some(s)),
            Err(_) => Err(SettingsError::NotUnicode(name)),
        },
    }
}

fn parsed<T>(
    name: &'static str,
    value: &str,
    parse: impl FnOnce(&str) -> Result<T, Reason>,
) -> Result<T, SettingsError> {
    parse(value).map_err(|reason| SettingsError::Invalid { name, reason })
}

fn required<E: EnvSource + ?Sized, T>(
    env: &E,
    name: &'static str,
    parse: impl FnOnce(&str) -> Result<T, Reason>,
) -> Result<T, SettingsError> {
    let value = raw(env, name)?.ok_or(SettingsError::Missing(name))?;
    parsed(name, &value, parse)
}

fn optional<E: EnvSource + ?Sized, T>(
    env: &E,
    name: &'static str,
    parse: impl FnOnce(&str) -> Result<T, Reason>,
) -> Result<Option<T>, SettingsError> {
    raw(env, name)?
        .map(|value| parsed(name, &value, parse))
        .transpose()
}

fn defaulted<E: EnvSource + ?Sized, T>(
    env: &E,
    name: &'static str,
    default: &str,
    parse: impl FnOnce(&str) -> Result<T, Reason>,
) -> Result<T, SettingsError> {
    let value = raw(env, name)?;
    parsed(name, value.as_deref().unwrap_or(default), parse)
}

/// Short, non-blank text without control characters.
fn text(s: &str) -> Result<ShortText, Reason> {
    if s.trim().is_empty() {
        return Err(Cow::Borrowed("must not be blank"));
    }
    ShortText::parse(s).map_err(|e| match e {
        TextError::TooLong => Cow::Borrowed("is longer than 256 bytes"),
        TextError::ControlChar => Cow::Borrowed("contains a control character"),
    })
}

fn abs_path(s: &str) -> Result<PathBuf, Reason> {
    if s.len() > 4096 {
        return Err(Cow::Borrowed("is longer than 4096 bytes"));
    }
    if s.chars().any(char::is_control) {
        return Err(Cow::Borrowed("contains a control character"));
    }
    let path = PathBuf::from(s);
    if !path.is_absolute() {
        return Err(Cow::Borrowed("must be an absolute path"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Cow::Borrowed("must not contain '..'"));
    }
    Ok(path)
}

fn uint(s: &str, range: &RangeInclusive<u64>) -> Result<u64, Reason> {
    match s.parse::<u64>() {
        Ok(n) if range.contains(&n) => Ok(n),
        _ => Err(Cow::Owned(format!(
            "must be a whole number from {} to {}",
            range.start(),
            range.end()
        ))),
    }
}

/// Comma-separated items, trimmed. An empty item or more than `max` items is an error.
fn list<T>(s: &str, max: usize, mut item: impl FnMut(&str) -> Result<T, Reason>) -> Result<Vec<T>, Reason> {
    let mut out = Vec::new();
    for part in s.split(',') {
        if out.len() == max {
            return Err(Cow::Owned(format!("has more than {max} items")));
        }
        let part = part.trim();
        if part.is_empty() {
            return Err(Cow::Borrowed("contains an empty item"));
        }
        out.push(item(part)?);
    }
    Ok(out)
}

fn globs(s: &str) -> Result<Vec<ShortText>, Reason> {
    list(s, limits::MAX_GLOBS, |g| {
        let glob = text(g)?;
        Glob::new(g).map_err(|_| Cow::Borrowed("contains a glob that does not compile"))?;
        Ok(glob)
    })
}

fn texts_of(items: &[&str]) -> Vec<ShortText> {
    // The built-in constants are valid short text; an invalid one would have failed the defaults test.
    items.iter().filter_map(|s| ShortText::parse(s).ok()).collect()
}

fn label_selector(s: &str) -> Result<LabelSelector, Reason> {
    const RULE: Reason = Cow::Borrowed("must be key=value with Kubernetes label characters");
    let (key, value) = s.split_once('=').ok_or(RULE)?;
    let key_ok = !key.is_empty()
        && key.len() <= 253
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/'));
    let value_ok = value.len() <= 63
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
    if !key_ok || !value_ok {
        return Err(RULE);
    }
    Ok(LabelSelector {
        key: key.to_owned(),
        value: value.to_owned(),
    })
}

fn deployment(s: &str) -> Result<DeploymentRef, Reason> {
    let (namespace, name) = s.split_once('/').ok_or(Cow::Borrowed("must be namespace/name"))?;
    Ok(DeploymentRef {
        namespace: KubeName::label(namespace)?,
        name: KubeName::subdomain(name)?,
    })
}

fn join_mode(s: &str) -> Result<JoinMode, Reason> {
    match s {
        "auto" => Ok(JoinMode::Auto),
        "workload-identity" => Ok(JoinMode::WorkloadIdentity),
        "token" => Ok(JoinMode::Token),
        _ => Err(Cow::Borrowed("must be auto, workload-identity or token")),
    }
}

fn log_level(s: &str) -> Result<LogLevel, Reason> {
    match s {
        "error" => Ok(LogLevel::Error),
        "warn" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "trace" => Ok(LogLevel::Trace),
        _ => Err(Cow::Borrowed("must be error, warn, info, debug or trace")),
    }
}

/// The settings the hub controls through `AgentConfig`, after clamping, plus the fixed timing of the scan loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunables {
    /// Time between stat walks.
    pub scan_interval: Duration,
    pub heartbeat_interval: Duration,
    pub full_rehash_interval: Duration,
    pub pending_walk_interval: Duration,
    pub quiet_period: Duration,
    pub max_defer: Duration,
    pub max_file_bytes: u64,
    /// Hub deny globs. They are added to the built-in ones and never replace them (D79, T11).
    pub deny_globs: Vec<ShortText>,
    pub env_allowlist: Vec<ShortText>,
    pub tenants: Vec<TenantId>,
}

impl Default for Tunables {
    /// What the agent uses until the hub's `AgentConfig` arrives.
    fn default() -> Self {
        Self {
            scan_interval: Duration::from_secs(10),
            heartbeat_interval: Duration::from_secs(10),
            full_rehash_interval: limits::FULL_REHASH_INTERVAL,
            pending_walk_interval: limits::PENDING_WALK_INTERVAL,
            quiet_period: limits::QUIET_PERIOD,
            max_defer: limits::MAX_DEFER,
            max_file_bytes: limits::MAX_FILE_BYTES,
            deny_globs: Vec::new(),
            env_allowlist: Vec::new(),
            tenants: Vec::new(),
        }
    }
}

impl Tunables {
    /// Clamp what the hub sent. The hub is semi-trusted: it may slow the agent down or make it read less, never make it
    /// faster than the budgets allow or read more than 2 MiB per file. A `max_file_bytes` of 0 means "no preference"
    /// and keeps the cap; it never turns into "skip every file". The fixed timing (quiet period, max deferral, full
    /// rehash) does not come from the hub.
    pub fn from_hub(config: &AgentConfig) -> Self {
        let secs = |value: u32, range: &RangeInclusive<u32>| {
            Duration::from_secs(u64::from(value.clamp(*range.start(), *range.end())))
        };
        let max_file_bytes = match config.max_file_bytes {
            0 => limits::MAX_FILE_BYTES,
            n => n.min(limits::MAX_FILE_BYTES),
        };
        Self {
            scan_interval: secs(config.scan_interval_secs, &limits::SCAN_INTERVAL_SECS),
            heartbeat_interval: secs(config.heartbeat_interval_secs, &limits::HEARTBEAT_INTERVAL_SECS),
            max_file_bytes,
            deny_globs: config.deny_globs.clone(),
            env_allowlist: config.env_allowlist.clone(),
            tenants: config.tenants.clone(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::os::unix::ffi::OsStringExt;

    use proptest::prelude::*;

    use super::*;

    /// The smallest environment that parses.
    fn valid_env() -> HashMap<String, String> {
        [
            ("LK_HUB_ENDPOINT", "https://hub.example.com:8443"),
            ("LK_HUB_AUDIENCE", "https://hub.example.com"),
            ("LK_HUB_CA_FILE", "/etc/lanekeeper/hub-ca.pem"),
            ("LK_SWIMLANE", "sit1"),
            ("LK_CLUSTER", "gke-sit1"),
            ("LK_PROJECT", "bank-sit"),
            ("LK_NFS_SERVER", "10.1.2.3"),
            ("LK_NFS_EXPORT", "/export/csp"),
            ("LK_NFS_ROOT", "/mnt/csp-configuration"),
            ("LK_CERT_SECRET", "lanekeeper-agent-cert"),
            ("LK_NAMESPACES", "sit1,sit1-core"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    fn with(extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut env = valid_env();
        for (k, v) in extra {
            env.insert((*k).to_owned(), (*v).to_owned());
        }
        env
    }

    fn without(name: &str) -> HashMap<String, String> {
        let mut env = valid_env();
        env.remove(name);
        env
    }

    fn texts(items: &[&str]) -> Vec<ShortText> {
        items.iter().map(|s| ShortText::parse(s).unwrap()).collect()
    }

    /// `Invalid` for exactly this variable, whatever the reason.
    fn assert_invalid(env: &HashMap<String, String>, name: &str) {
        match Settings::from_env(env) {
            Err(SettingsError::Invalid { name: got, .. }) => assert_eq!(got, name),
            other => panic!("expected {name} to be invalid, got {other:?}"),
        }
    }

    #[test]
    fn settings_parse_valid_env() {
        let s = Settings::from_env(&valid_env()).unwrap();
        assert_eq!(s.hub_endpoint.as_str(), "https://hub.example.com:8443");
        assert_eq!(s.hub_audience.as_str(), "https://hub.example.com");
        assert_eq!(s.hub_ca_file, PathBuf::from("/etc/lanekeeper/hub-ca.pem"));
        assert_eq!(s.swimlane.as_str(), "sit1");
        assert_eq!(s.cluster.as_str(), "gke-sit1");
        assert_eq!(s.project.as_str(), "bank-sit");
        assert_eq!(s.nfs.server.as_str(), "10.1.2.3");
        assert_eq!(s.nfs.export.as_str(), "/export/csp");
        assert_eq!(s.nfs.root, PathBuf::from("/mnt/csp-configuration"));
        assert_eq!(s.cert_secret.as_str(), "lanekeeper-agent-cert");
        assert_eq!(
            s.namespaces.iter().map(KubeName::as_str).collect::<Vec<_>>(),
            ["sit1", "sit1-core"]
        );
        // Defaults.
        assert_eq!(s.join_token_secret, None);
        assert_eq!(s.join_mode, JoinMode::Auto);
        assert_eq!(s.spool.dir, PathBuf::from("/var/lib/lanekeeper/spool"));
        assert_eq!(s.spool.max_bytes, 512 * 1024 * 1024);
        assert_eq!(s.spool.max_entries, 100_000);
        assert_eq!(s.tmp_dir, PathBuf::from("/tmp"));
        assert_eq!(s.ignore_globs, texts(&[".nfs*", ".lanekeeper-tmp-*"]));
        assert_eq!(s.sync_job_name_globs, texts(&["*dataload*"]));
        assert_eq!(s.sync_job_label, None);
        assert_eq!(s.helm_hint_chart_globs, texts(&["csp-tenant-data-*"]));
        assert_eq!(s.config_server_url, None);
        assert_eq!(s.config_server_deployment, None);
        assert_eq!(s.metadata_url.as_str(), "http://metadata.google.internal");
        assert_eq!(s.health_addr, "0.0.0.0:9090".parse::<SocketAddr>().unwrap());
        assert_eq!(s.pool_threads, 4);
        assert_eq!(s.log_level, LogLevel::Info);
    }

    #[test]
    fn settings_parse_every_optional_variable() {
        let env = with(&[
            ("LK_JOIN_TOKEN_SECRET", "lanekeeper-join-token"),
            ("LK_JOIN_MODE", "token"),
            ("LK_SPOOL_DIR", "/var/spool/lk"),
            ("LK_SPOOL_MAX_BYTES", "8388608"),
            ("LK_SPOOL_MAX_ENTRIES", "250"),
            ("LK_TMP_DIR", "/scratch"),
            ("LK_IGNORE_GLOBS", "*.swp, *.bak"),
            ("LK_SYNC_JOB_NAME_GLOBS", "*sync*,*dataload*"),
            ("LK_SYNC_JOB_LABEL", "lanekeeper.io/sync=true"),
            ("LK_HELM_HINT_CHART_GLOBS", "tenant-*"),
            (
                "LK_CONFIG_SERVER_URL",
                "http://csp-configuration-server.sit1:8888",
            ),
            ("LK_CONFIG_SERVER_DEPLOYMENT", "sit1/csp-configuration-server"),
            ("LK_METADATA_URL", "http://127.0.0.1:9999"),
            ("LK_HEALTH_ADDR", "127.0.0.1:8081"),
            ("LK_POOL_THREADS", "2"),
            ("LK_LOG", "debug"),
        ]);
        let s = Settings::from_env(&env).unwrap();
        assert_eq!(
            s.join_token_secret.as_ref().map(KubeName::as_str),
            Some("lanekeeper-join-token")
        );
        assert_eq!(s.join_mode, JoinMode::Token);
        assert_eq!(s.spool.dir, PathBuf::from("/var/spool/lk"));
        assert_eq!(s.spool.max_bytes, 8 * 1024 * 1024);
        assert_eq!(s.spool.max_entries, 250);
        assert_eq!(s.tmp_dir, PathBuf::from("/scratch"));
        assert_eq!(
            s.ignore_globs,
            texts(&[".nfs*", ".lanekeeper-tmp-*", "*.swp", "*.bak"])
        );
        assert_eq!(s.sync_job_name_globs, texts(&["*sync*", "*dataload*"]));
        assert_eq!(
            s.sync_job_label,
            Some(LabelSelector {
                key: "lanekeeper.io/sync".into(),
                value: "true".into()
            })
        );
        assert_eq!(s.helm_hint_chart_globs, texts(&["tenant-*"]));
        assert_eq!(
            s.config_server_url.as_ref().map(BaseUrl::as_str),
            Some("http://csp-configuration-server.sit1:8888")
        );
        let dep = s.config_server_deployment.unwrap();
        assert_eq!(
            (dep.namespace.as_str(), dep.name.as_str()),
            ("sit1", "csp-configuration-server")
        );
        assert_eq!(s.metadata_url.as_str(), "http://127.0.0.1:9999");
        assert_eq!(s.health_addr, "127.0.0.1:8081".parse::<SocketAddr>().unwrap());
        assert_eq!(s.pool_threads, 2);
        assert_eq!(s.log_level, LogLevel::Debug);
    }

    #[test]
    fn settings_reject_missing_and_malformed() {
        // Every required variable is reported by name when it is missing, or set to an empty value.
        for name in REQUIRED.iter().copied() {
            let unset = Settings::from_env(&without(name));
            assert_eq!(unset, Err(SettingsError::Missing(name)), "{name} unset");
            let empty = Settings::from_env(&with(&[(name, "")]));
            assert_eq!(empty, Err(SettingsError::Missing(name)), "{name} empty");
        }

        // A malformed value is reported against the right variable.
        let bad: &[(&str, &str)] = &[
            ("LK_HUB_ENDPOINT", "http://hub.example.com"),
            ("LK_HUB_ENDPOINT", "hub.example.com:8443"),
            ("LK_HUB_AUDIENCE", "line\nbreak"),
            ("LK_HUB_AUDIENCE", &"a".repeat(257)),
            ("LK_HUB_CA_FILE", "relative/ca.pem"),
            ("LK_SWIMLANE", "Not A Slug"),
            ("LK_CLUSTER", "tab\there"),
            ("LK_PROJECT", &"p".repeat(300)),
            ("LK_NFS_SERVER", "bell\u{7}"),
            ("LK_NFS_EXPORT", "nul\u{0}export"),
            ("LK_NFS_ROOT", "mnt/relative"),
            ("LK_NFS_ROOT", "/mnt/../etc"),
            ("LK_NFS_ROOT", &format!("/{}", "r".repeat(300))),
            ("LK_CERT_SECRET", "Upper_Case"),
            ("LK_JOIN_TOKEN_SECRET", "has space"),
            ("LK_JOIN_MODE", "magic"),
            ("LK_NAMESPACES", "ok,Bad_Namespace"),
            ("LK_SPOOL_DIR", "relative/spool"),
            ("LK_SPOOL_MAX_BYTES", "lots"),
            ("LK_SPOOL_MAX_BYTES", "1024"),
            ("LK_SPOOL_MAX_BYTES", "99999999999999999999"),
            ("LK_SPOOL_MAX_ENTRIES", "0"),
            ("LK_SPOOL_MAX_ENTRIES", "-1"),
            ("LK_TMP_DIR", "tmp"),
            ("LK_IGNORE_GLOBS", "[unclosed"),
            ("LK_SYNC_JOB_NAME_GLOBS", "ok,,empty-item"),
            ("LK_SYNC_JOB_LABEL", "no-equals-sign"),
            ("LK_SYNC_JOB_LABEL", "bad key=value"),
            ("LK_SYNC_JOB_LABEL", "key=bad value"),
            ("LK_HELM_HINT_CHART_GLOBS", "[unclosed"),
            ("LK_CONFIG_SERVER_URL", "https://config-server:8888"),
            ("LK_CONFIG_SERVER_DEPLOYMENT", "no-slash"),
            ("LK_CONFIG_SERVER_DEPLOYMENT", "sit1/Bad_Name"),
            ("LK_METADATA_URL", "metadata.google.internal"),
            ("LK_HEALTH_ADDR", "not-an-address"),
            ("LK_HEALTH_ADDR", "0.0.0.0"),
            ("LK_POOL_THREADS", "0"),
            ("LK_POOL_THREADS", "17"),
            ("LK_POOL_THREADS", "four"),
            ("LK_LOG", "verbose"),
            ("LK_LOG", "INFO"),
        ];
        for (name, value) in bad {
            assert_invalid(&with(&[(name, value)]), name);
        }
    }

    const REQUIRED: &[&str] = &[
        "LK_HUB_ENDPOINT",
        "LK_HUB_AUDIENCE",
        "LK_HUB_CA_FILE",
        "LK_SWIMLANE",
        "LK_CLUSTER",
        "LK_PROJECT",
        "LK_NFS_SERVER",
        "LK_NFS_EXPORT",
        "LK_NFS_ROOT",
        "LK_CERT_SECRET",
        "LK_NAMESPACES",
    ];

    #[test]
    fn empty_optional_values_count_as_unset() {
        let env = with(&[
            ("LK_JOIN_TOKEN_SECRET", ""),
            ("LK_JOIN_MODE", ""),
            ("LK_SPOOL_MAX_BYTES", ""),
            ("LK_SYNC_JOB_LABEL", ""),
            ("LK_CONFIG_SERVER_URL", ""),
            ("LK_LOG", ""),
        ]);
        assert_eq!(
            Settings::from_env(&env).unwrap(),
            Settings::from_env(&valid_env()).unwrap()
        );
    }

    #[test]
    fn settings_reject_non_unicode_values() {
        struct Raw(HashMap<String, String>);
        impl EnvSource for Raw {
            fn var_os(&self, name: &str) -> Option<OsString> {
                if name == "LK_SWIMLANE" {
                    return Some(OsString::from_vec(vec![b's', 0xFF, 0xFE]));
                }
                self.0.var_os(name)
            }
        }
        assert_eq!(
            Settings::from_env(&Raw(valid_env())),
            Err(SettingsError::NotUnicode("LK_SWIMLANE"))
        );
    }

    #[test]
    fn settings_lists_are_bounded() {
        let names = |n: usize| (0..n).map(|i| format!("ns{i}")).collect::<Vec<_>>().join(",");
        let globs = |n: usize| (0..n).map(|i| format!("g{i}*")).collect::<Vec<_>>().join(",");

        // The limits themselves are accepted, one more is not.
        let s = Settings::from_env(&with(&[("LK_NAMESPACES", &names(64))])).unwrap();
        assert_eq!(s.namespaces.len(), limits::MAX_NAMESPACES);
        assert_invalid(&with(&[("LK_NAMESPACES", &names(65))]), "LK_NAMESPACES");

        for var in [
            "LK_IGNORE_GLOBS",
            "LK_SYNC_JOB_NAME_GLOBS",
            "LK_HELM_HINT_CHART_GLOBS",
        ] {
            Settings::from_env(&with(&[(var, &globs(64))])).unwrap();
            assert_invalid(&with(&[(var, &globs(65))]), var);
            assert_invalid(&with(&[(var, &format!("{}*", "g".repeat(300)))]), var);
        }

        // Namespaces are distinct and none is empty.
        assert_invalid(&with(&[("LK_NAMESPACES", "a,b,a")]), "LK_NAMESPACES");
        assert_invalid(&with(&[("LK_NAMESPACES", "a,,b")]), "LK_NAMESPACES");
        assert_invalid(&with(&[("LK_NAMESPACES", ",")]), "LK_NAMESPACES");
    }

    #[test]
    fn list_items_are_trimmed() {
        let s = Settings::from_env(&with(&[("LK_NAMESPACES", " a , b ")])).unwrap();
        assert_eq!(
            s.namespaces.iter().map(KubeName::as_str).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn ignore_globs_add_to_the_built_in_ones() {
        let s = Settings::from_env(&with(&[("LK_IGNORE_GLOBS", "*.swp")])).unwrap();
        assert_eq!(s.ignore_globs, texts(&[".nfs*", ".lanekeeper-tmp-*", "*.swp"]));
    }

    #[test]
    fn join_mode_token_needs_a_token_secret() {
        assert_invalid(&with(&[("LK_JOIN_MODE", "token")]), "LK_JOIN_MODE");
        let env = with(&[("LK_JOIN_MODE", "token"), ("LK_JOIN_TOKEN_SECRET", "join")]);
        assert_eq!(Settings::from_env(&env).unwrap().join_mode, JoinMode::Token);
        let env = with(&[("LK_JOIN_MODE", "workload-identity")]);
        assert_eq!(
            Settings::from_env(&env).unwrap().join_mode,
            JoinMode::WorkloadIdentity
        );
    }

    #[test]
    fn spool_and_tmp_dirs_must_be_outside_the_nfs_root() {
        // A spool inside the root would be tracked as config and re-sent forever; a root inside the spool is as bad.
        for (var, value) in [
            ("LK_SPOOL_DIR", "/mnt/csp-configuration/spool"),
            ("LK_SPOOL_DIR", "/mnt/csp-configuration"),
            ("LK_SPOOL_DIR", "/mnt"),
            ("LK_TMP_DIR", "/mnt/csp-configuration/tmp"),
            ("LK_TMP_DIR", "/"),
        ] {
            assert_invalid(&with(&[(var, value)]), var);
        }
        // A sibling that only shares a name prefix is fine.
        Settings::from_env(&with(&[("LK_SPOOL_DIR", "/mnt/csp-configuration-spool")])).unwrap();
    }

    #[test]
    fn config_server_deployment_must_be_in_a_watched_namespace() {
        assert_invalid(
            &with(&[(
                "LK_CONFIG_SERVER_DEPLOYMENT",
                "elsewhere/csp-configuration-server",
            )]),
            "LK_CONFIG_SERVER_DEPLOYMENT",
        );
        Settings::from_env(&with(&[(
            "LK_CONFIG_SERVER_DEPLOYMENT",
            "sit1-core/csp-configuration-server",
        )]))
        .unwrap();
    }

    #[test]
    fn urls_are_scheme_and_authority_only() {
        // The hub is https only; the config-server and the metadata server are plain http inside the cluster / node.
        for bad in [
            "https://user:pass@hub.example.com",
            "https://hub.example.com/path",
            "https://hub.example.com/?q=1",
            "https://hub example.com",
            "https://",
            "ftp://hub.example.com",
        ] {
            assert_invalid(&with(&[("LK_HUB_ENDPOINT", bad)]), "LK_HUB_ENDPOINT");
        }
        for bad in [
            "http://cs:8888/update-resources",
            "http://user@cs:8888",
            "https://cs:8888",
        ] {
            assert_invalid(&with(&[("LK_CONFIG_SERVER_URL", bad)]), "LK_CONFIG_SERVER_URL");
        }
        // A trailing slash is the same URL; the stored form has none.
        let s = Settings::from_env(&with(&[("LK_HUB_ENDPOINT", "https://hub.example.com/")])).unwrap();
        assert_eq!(s.hub_endpoint.as_str(), "https://hub.example.com");
        let s = Settings::from_env(&with(&[("LK_METADATA_URL", "http://[::1]:8080")])).unwrap();
        assert_eq!(s.metadata_url.as_str(), "http://[::1]:8080");
    }

    #[test]
    fn kube_names_follow_dns_1123() {
        assert!(KubeName::label("a").is_ok());
        assert!(KubeName::label(&"a".repeat(63)).is_ok());
        for bad in ["", "-a", "a-", "A", "a_b", "a.b", &"a".repeat(64)] {
            assert!(KubeName::label(bad).is_err(), "label {bad:?}");
        }
        assert!(KubeName::subdomain("a.b-c.d").is_ok());
        assert!(KubeName::subdomain(&"a".repeat(63)).is_ok());
        for bad in [
            "",
            "a..b",
            ".a",
            "a.",
            "a.-b",
            "a.B",
            &"a".repeat(64),
            &format!("{0}.{0}.{0}.{0}.{0}", "a".repeat(62)),
        ] {
            assert!(KubeName::subdomain(bad).is_err(), "subdomain {bad:?}");
        }
    }

    #[test]
    fn built_in_defaults_are_valid() {
        // The defaults go through the same parsers as operator input, so a typo in one fails here, not in the field.
        Settings::from_env(&valid_env()).unwrap();
    }

    proptest! {
        #[test]
        fn settings_parse_never_panics(name_idx in 0usize..30, value in "\\PC{0,300}") {
            let names = ALL_NAMES;
            let mut env = valid_env();
            env.insert(names[name_idx % names.len()].to_owned(), value);
            let _ = Settings::from_env(&env);
        }

        #[test]
        fn settings_errors_never_echo_input(name_idx in 0usize..30, marker in "zq[a-z0-9]{10}wx") {
            let names = ALL_NAMES;
            let mut env = valid_env();
            env.insert(names[name_idx % names.len()].to_owned(), marker.clone());
            if let Err(e) = Settings::from_env(&env) {
                prop_assert!(!e.to_string().contains(&marker), "error echoes the value: {e}");
                prop_assert!(!format!("{e:?}").contains(&marker), "error debug echoes the value: {e:?}");
            }
        }
    }

    const ALL_NAMES: &[&str] = &[
        "LK_HUB_ENDPOINT",
        "LK_HUB_AUDIENCE",
        "LK_HUB_CA_FILE",
        "LK_SWIMLANE",
        "LK_CLUSTER",
        "LK_PROJECT",
        "LK_NFS_SERVER",
        "LK_NFS_EXPORT",
        "LK_NFS_ROOT",
        "LK_CERT_SECRET",
        "LK_JOIN_TOKEN_SECRET",
        "LK_JOIN_MODE",
        "LK_NAMESPACES",
        "LK_SPOOL_DIR",
        "LK_SPOOL_MAX_BYTES",
        "LK_SPOOL_MAX_ENTRIES",
        "LK_TMP_DIR",
        "LK_IGNORE_GLOBS",
        "LK_SYNC_JOB_NAME_GLOBS",
        "LK_SYNC_JOB_LABEL",
        "LK_HELM_HINT_CHART_GLOBS",
        "LK_CONFIG_SERVER_URL",
        "LK_CONFIG_SERVER_DEPLOYMENT",
        "LK_METADATA_URL",
        "LK_HEALTH_ADDR",
        "LK_POOL_THREADS",
        "LK_LOG",
    ];

    // ---------------------------------------------------------------- tunables

    fn hub_config() -> AgentConfig {
        AgentConfig {
            scan_interval_secs: 10,
            heartbeat_interval_secs: 10,
            max_file_bytes: 2 * 1024 * 1024,
            deny_globs: texts(&["*.secret"]),
            env_allowlist: texts(&["CONFIG_CLIENT_CACHE_TTL"]),
            tenants: vec![TenantId::parse("sit1").unwrap()],
        }
    }

    #[test]
    fn tunables_default_to_the_documented_timing() {
        let t = Tunables::default();
        assert_eq!(t.scan_interval, Duration::from_secs(10));
        assert_eq!(t.heartbeat_interval, Duration::from_secs(10));
        assert_eq!(t.full_rehash_interval, Duration::from_secs(900));
        assert_eq!(t.pending_walk_interval, Duration::from_secs(1));
        assert_eq!(t.quiet_period, Duration::from_secs(3));
        assert_eq!(t.max_defer, Duration::from_secs(30));
        assert_eq!(t.max_file_bytes, 2 * 1024 * 1024);
        assert!(t.deny_globs.is_empty() && t.env_allowlist.is_empty() && t.tenants.is_empty());
    }

    #[test]
    fn tunables_keep_values_in_range() {
        let t = Tunables::from_hub(&AgentConfig {
            scan_interval_secs: 60,
            heartbeat_interval_secs: 30,
            max_file_bytes: 1024,
            ..hub_config()
        });
        assert_eq!(t.scan_interval, Duration::from_secs(60));
        assert_eq!(t.heartbeat_interval, Duration::from_secs(30));
        assert_eq!(t.max_file_bytes, 1024);
        assert_eq!(t.deny_globs, texts(&["*.secret"]));
        assert_eq!(t.env_allowlist, texts(&["CONFIG_CLIENT_CACHE_TTL"]));
        assert_eq!(t.tenants, vec![TenantId::parse("sit1").unwrap()]);
        // The fixed timing does not come from the hub.
        assert_eq!(t.quiet_period, Duration::from_secs(3));
        assert_eq!(t.max_defer, Duration::from_secs(30));
    }

    #[test]
    fn tunables_clamp_hub_values() {
        let low = Tunables::from_hub(&AgentConfig {
            scan_interval_secs: 1,
            heartbeat_interval_secs: 1,
            ..hub_config()
        });
        assert_eq!(low.scan_interval, Duration::from_secs(5), "scan floor");
        assert_eq!(
            low.heartbeat_interval,
            Duration::from_secs(10),
            "the heartbeat is never faster than 10 s (P3)"
        );

        let high = Tunables::from_hub(&AgentConfig {
            scan_interval_secs: u32::MAX,
            heartbeat_interval_secs: u32::MAX,
            max_file_bytes: u64::MAX,
            ..hub_config()
        });
        assert_eq!(high.scan_interval, Duration::from_secs(300), "scan ceiling");
        assert_eq!(
            high.heartbeat_interval,
            Duration::from_secs(300),
            "heartbeat ceiling"
        );
        assert_eq!(
            high.max_file_bytes,
            2 * 1024 * 1024,
            "the hub cannot raise the 2 MiB file cap"
        );

        // 0 is not "skip every file": it falls back to the cap.
        let zero = Tunables::from_hub(&AgentConfig {
            max_file_bytes: 0,
            ..hub_config()
        });
        assert_eq!(zero.max_file_bytes, 2 * 1024 * 1024);
    }

    proptest! {
        #[test]
        fn tunables_always_within_bounds(scan in any::<u32>(), beat in any::<u32>(), bytes in any::<u64>()) {
            let t = Tunables::from_hub(&AgentConfig {
                scan_interval_secs: scan,
                heartbeat_interval_secs: beat,
                max_file_bytes: bytes,
                ..hub_config()
            });
            prop_assert!(t.scan_interval >= Duration::from_secs(5) && t.scan_interval <= Duration::from_secs(300));
            prop_assert!(t.heartbeat_interval >= Duration::from_secs(10) && t.heartbeat_interval <= Duration::from_secs(300));
            prop_assert!(t.max_file_bytes >= 1 && t.max_file_bytes <= limits::MAX_FILE_BYTES);
        }
    }
}
