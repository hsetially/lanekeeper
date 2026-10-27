//! What must hold for every input of the hub's parsers at a trust boundary (plan 18, T1 and T2; rule 13; S7, S10).
//!
//! The fuzz targets and the stable test `crates/hub/tests/corpus_replay.rs` both include this file, so every committed seed is
//! replayed by `cargo test -p hub` with the same checks the fuzzer runs. A function returns the verdict (the input was
//! accepted, or refused) or says why an invariant broke. A seed's name says which verdict it must keep: `accepted_*` or
//! `rejected_*`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use domain::Secret;
use hub::cli::{self, CaInitArgs, CliError, Command, SeedArgs};
use hub::config::{HubSettings, MigratorSettings, SeedSettings};
use hub::seed::plan::{self as seed_plan, State as SeedState};
use hub::seed::{FieldPath, MAX_CHANGES, MAX_FILE_BYTES, SeedError};
use hub::web_tls::{self, MAX_CHAIN_LEN, PemError};
use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::gcp::wire::CryptoKeyVersionName;

/// Whether the parser took the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected,
}

pub type Check = fn(&[u8]) -> Result<Verdict, String>;

// ------------------------------------------------------------------------------------------------ hub_settings

/// The input is lines of `NAME=value`. A line `FILE_CONTENT=...` is what every `*_FILE` setting reads from disk; a line that
/// is not `LK_*` or `FILE_CONTENT` is ignored. Settings come from the environment and from files, so this is the shape of
/// everything `HubSettings` can be given.
///
/// The verdict is the serving hub's. `hub migrate` reads the same shape of input with its own settings (T2), and the same
/// input is held to the migrator's invariants too (see [`migrator_settings`]); the migrator's verdict is its own.
pub fn hub_settings(data: &[u8]) -> Result<Verdict, String> {
    let (vars, file) = parse_lines(data);
    migrator_settings_with(&vars, &file)?;
    seed_settings_with(&vars, &file)?;
    let lookup = |name: &str| vars.get(name).cloned();
    let read_file = |_: &std::path::Path| Ok(file.clone());
    match HubSettings::from_parts(lookup, read_file) {
        Err(error) => {
            // S7, S10: an error is a variable's name and one of two fixed phrases. No part of any value can be in it.
            let shown = format!("{error}");
            if !is_fixed_settings_error(&shown) || format!("{error:?}").len() > 200 {
                return Err(format!(
                    "an error that is not a variable name and a fixed phrase: {shown}"
                ));
            }
            Ok(Verdict::Rejected)
        }
        Ok(settings) => {
            check_settings(&settings, &vars, &file)?;
            Ok(Verdict::Accepted)
        }
    }
}

fn parse_lines(data: &[u8]) -> (BTreeMap<String, String>, String) {
    let text = String::from_utf8_lossy(data);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut file = String::new();
    for line in text.split('\n') {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name == "FILE_CONTENT" {
            value.clone_into(&mut file);
        } else if name.starts_with("LK_") {
            vars.insert(name.to_owned(), value.to_owned());
        }
    }
    (vars, file)
}

/// The settings of `hub migrate` (T2) over the same input shape as [`hub_settings`]: the verdict is whether the migrator would
/// run with them. Seeds of `hub_settings` that exercise it say so in their names (`*_migrator_*`).
pub fn migrator_settings(data: &[u8]) -> Result<Verdict, String> {
    let (vars, file) = parse_lines(data);
    migrator_settings_with(&vars, &file)
}

fn migrator_settings_with(vars: &BTreeMap<String, String>, file: &str) -> Result<Verdict, String> {
    let lookup = |name: &str| vars.get(name).cloned();
    let read_file = |_: &std::path::Path| Ok(file.to_owned());
    match MigratorSettings::from_parts(lookup, read_file) {
        Err(error) => {
            let shown = format!("{error}");
            if !is_fixed_settings_error(&shown) || format!("{error:?}").len() > 200 {
                return Err(format!(
                    "a migrator error that is not a variable name and a fixed phrase: {shown}"
                ));
            }
            Ok(Verdict::Rejected)
        }
        Ok(settings) => {
            // The limit is inside its range, the CA is a path and not the credential, and the credential is the file's text and
            // is not in the debug output of the settings that hold it (S7), unless the same text is also the CA path.
            let secs = settings.timeout.as_secs();
            if !(1..=7200).contains(&secs) {
                return Err(format!(
                    "the migration time limit accepted outside its range: {secs}"
                ));
            }
            let url = file.trim();
            let shown = format!("{settings:?}");
            let ca = settings.ca_file.display().to_string();
            if url.len() >= 12 && shown.contains(url) && !ca.contains(url) {
                return Err("the migrator URL is in the debug output of its settings".to_owned());
            }
            if settings.url.expose() != url {
                return Err("the migrator URL is not the file's trimmed text".to_owned());
            }
            Ok(Verdict::Accepted)
        }
    }
}

/// The settings of `hub seed` (T7) over the same input shape: the verdict is whether the seed Job would run with them. Seeds of
/// `hub_settings` that exercise it say so in their names (`*_seed_job*`).
pub fn seed_settings(data: &[u8]) -> Result<Verdict, String> {
    let (vars, file) = parse_lines(data);
    seed_settings_with(&vars, &file)
}

fn seed_settings_with(vars: &BTreeMap<String, String>, file: &str) -> Result<Verdict, String> {
    let lookup = |name: &str| vars.get(name).cloned();
    let read_file = |_: &std::path::Path| Ok(file.to_owned());
    match SeedSettings::from_parts(lookup, read_file) {
        Err(error) => {
            let shown = format!("{error}");
            if !is_fixed_settings_error(&shown) || format!("{error:?}").len() > 200 {
                return Err(format!(
                    "a seed error that is not a variable name and a fixed phrase: {shown}"
                ));
            }
            Ok(Verdict::Rejected)
        }
        Ok(settings) => {
            let secs = settings.timeout.as_secs();
            if !(1..=3600).contains(&secs) {
                return Err(format!("the seed time limit accepted outside its range: {secs}"));
            }
            // The credential is the file's trimmed text or the plain variable, and is not in the debug output of its settings (S7).
            let url = settings.url.expose().as_str();
            let from_file = file.trim();
            if url != from_file && vars.get("LK_DATABASE_URL").map(String::as_str) != Some(url) {
                return Err("the seed URL is neither the file's text nor the variable".to_owned());
            }
            let shown = format!("{settings:?}");
            let ca = settings.ca_file.display().to_string();
            if url.len() >= 12 && shown.contains(url) && !ca.contains(url) {
                return Err("the seed URL is in the debug output of its settings".to_owned());
            }
            Ok(Verdict::Accepted)
        }
    }
}

/// A settings error is a variable's name and one of two fixed phrases, and nothing else.
pub fn is_fixed_settings_error(shown: &str) -> bool {
    shown
        .strip_suffix(" is required")
        .or_else(|| shown.strip_suffix(" is not valid"))
        .is_some_and(|name| {
            name.starts_with("LK_")
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        })
}

fn check_settings(settings: &HubSettings, vars: &BTreeMap<String, String>, file: &str) -> Result<(), String> {
    let in_range = |name: &str, secs: u64, min: u64, max: u64| {
        if (min..=max).contains(&secs) {
            Ok(())
        } else {
            Err(format!("{name} accepted outside its range: {secs}"))
        }
    };
    in_range("start timeout", settings.start_timeout.as_secs(), 1, 120)?;
    in_range("drain", settings.shutdown_drain.as_secs(), 1, 120)?;
    in_range("grace", settings.shutdown_grace.as_secs(), 0, 60)?;
    in_range(
        "request timeout",
        settings.limits.request_timeout.as_secs(),
        1,
        600,
    )?;
    if settings.limits.body_max < 1024 || settings.limits.body_max > 64 * 1024 * 1024 {
        return Err("the body cap is outside its range".to_owned());
    }
    if settings.max_connections == 0 || settings.trusted_proxy_hops > 8 {
        return Err("a connection or proxy bound is outside its range".to_owned());
    }
    if let Some(tls) = &settings.web_tls {
        for name in [&tls.cert_ref, &tls.key_ref] {
            if !ports::is_valid_secret_name(name) {
                return Err("a web certificate reference that is not a secret name".to_owned());
            }
        }
        in_range("reload", tls.reload_every.as_secs(), 1, 86_400)?;
    } else if vars.contains_key("LK_WEB_CERT_REF") && !vars["LK_WEB_CERT_REF"].is_empty() {
        return Err("a certificate reference was dropped".to_owned());
    }
    if let Some(gcp) = &settings.gcp {
        let project = gcp.project.as_str();
        if project.is_empty()
            || project.len() > 63
            || !project.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("a project id with characters a resource name cannot hold".to_owned());
        }
    }
    if let Some(platform) = &settings.platform {
        check_platform(platform)?;
    } else if vars.get("LK_REPLICA_NAME").is_some_and(|v| !v.is_empty()) {
        return Err("a replica name was dropped".to_owned());
    }
    if let Some(identity) = &settings.identity {
        check_identity(identity)?;
    } else if vars.get("LK_ENTRA_TENANT_ID").is_some_and(|v| !v.is_empty()) {
        return Err("an Entra tenant was dropped".to_owned());
    }
    check_registry(&settings.registry, vars)?;
    if let Some(writes) = &settings.writes {
        check_writes(writes)?;
    } else if vars.get("LK_VAULT_KMS_REF").is_some_and(|v| !v.is_empty()) {
        return Err("a vault key was dropped".to_owned());
    }
    // The policy names go into a CSP header: nothing else may.
    for name in &settings.trusted_types {
        if name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'#' | b'='))
        {
            return Err("a Trusted Types name that could inject into the CSP".to_owned());
        }
    }
    // S7: the database URL is a secret. It is not in the debug output of the settings that hold it, unless the same text was
    // also given as the CA file's path (a path is not a secret, and the fuzzer can find that coincidence).
    if let Some(database) = &settings.database {
        let url = if vars.contains_key("LK_DATABASE_URL_FILE") {
            file.trim().to_owned()
        } else {
            vars.get("LK_DATABASE_URL").cloned().unwrap_or_default()
        };
        let shown = format!("{database:?}");
        let ca = database.ca_file.display().to_string();
        if url.len() >= 12 && shown.contains(&url) && !ca.contains(&url) {
            return Err("the database URL is in the debug output of its settings".to_owned());
        }
    }
    Ok(())
}

/// What the platform group may hold once it is accepted (T3, S5, S7): the KMS references name key versions and the certificate
/// references name secrets; the names that go on certificates are ones the CA's profile takes, so a setting cannot be accepted
/// here and refused after the CA was reached; and there are as many agent hosts as one certificate carries.
fn check_platform(platform: &hub::config::PlatformSettings) -> Result<(), String> {
    for ca in [&platform.agent_ca, &platform.replica_ca] {
        if CryptoKeyVersionName::parse(&ca.kms_ref).is_none() {
            return Err("a KMS reference that is not a key version".to_owned());
        }
        if !ports::is_valid_secret_name(&ca.cert_ref) {
            return Err("a CA certificate reference that is not a secret name".to_owned());
        }
    }
    if platform.agent_hosts.is_empty()
        || platform.agent_hosts.len() > hub_platform::ca::profile::MAX_DNS_NAMES
    {
        return Err(format!("{} agent hosts accepted", platform.agent_hosts.len()));
    }
    let probe = PeerName::parse("probe").map_err(|_| "the probe name".to_owned())?;
    if CertProfile::hub_server(&probe, &platform.agent_hosts).is_err() {
        return Err("an agent host that the CA's profile refuses".to_owned());
    }
    if CertProfile::replica(
        &platform.replica_name,
        &[platform.replica_address.host().to_owned()],
    )
    .is_err()
    {
        return Err("a replica identity that the CA's profile refuses".to_owned());
    }
    if platform.audience.as_str().is_empty() || platform.audience.as_str().chars().any(char::is_control) {
        return Err("an audience that is empty or holds a control character".to_owned());
    }
    // Nothing in the group is a secret, but nothing in it may print as a value of another setting either: the debug output is
    // the structure and not a copy of the environment.
    if format!("{platform:?}").len() > 4096 {
        return Err("a debug output out of proportion to its fields".to_owned());
    }
    Ok(())
}

/// What the registry's knobs may hold once they are accepted (T5, G16, D79, D118): a switch is `true` or `false` and nothing else,
/// and retention is off unless it was said to be on and reports only unless it was said not to; the deny globs are the variable's
/// entries and every one of them (none dropped, none added), within the limits the write path also has; the registry's own
/// validation passes; and the corpora's memory is inside what the variable allows.
fn check_registry(
    registry: &hub::config::RegistrySettings,
    vars: &BTreeMap<String, String>,
) -> Result<(), String> {
    let var = |name: &str| vars.get(name).map(String::as_str).filter(|v| !v.is_empty());
    if registry.retention_enabled != (var("LK_RETENTION_ENABLED") == Some("true")) {
        return Err("retention is on without the switch, or off with it".to_owned());
    }
    if registry.config.retention_dry_run != (var("LK_RETENTION_DRY_RUN") != Some("false")) {
        return Err("the dry run is off without being switched off".to_owned());
    }
    let wanted: Vec<String> = var("LK_EXTRA_DENY_GLOBS")
        .map(|list| list.split(',').map(|g| g.trim().to_owned()).collect())
        .unwrap_or_default();
    if registry.config.extra_deny_globs != wanted {
        return Err("the deny globs are not the variable's entries".to_owned());
    }
    if registry.config.extra_deny_globs.len() > hub_writes::MAX_EXTRA_DENY_GLOBS
        || registry
            .config
            .extra_deny_globs
            .iter()
            .any(|g| g.is_empty() || g.len() > 256)
    {
        return Err("a deny glob outside the limits the write path also has".to_owned());
    }
    if registry.config.validate().is_err() {
        return Err("settings that the registry's own validation refuses".to_owned());
    }
    let mib = 1024 * 1024;
    if !(16 * mib..=4096 * mib).contains(&registry.config.corpus_cache_bytes) {
        return Err("a corpus cache outside its range".to_owned());
    }
    Ok(())
}

/// What the write paths' group may hold once it is accepted (T5, S8, S18): the vault's key is a Cloud KMS crypto key and not a
/// version (KMS picks the version, so a rotation needs no change here); GitHub is `https://host` with a host in lower case and
/// nothing a URL could add (the client reaches that host and no other); the branch and the Deployment name are ones the crates
/// take; the Teams host patterns are host names or `*.` and a host; and every interval is inside its bounds.
fn check_writes(writes: &hub::config::WritesSettings) -> Result<(), String> {
    if hub_platform::gcp::wire::CryptoKeyName::parse(&writes.vault_key).is_none() {
        return Err("a vault key that is not a crypto key".to_owned());
    }
    if !writes.github_api_base.starts_with("https://") || writes.github_api_host.is_empty() {
        return Err("a GitHub address that is not https and a host".to_owned());
    }
    if writes
        .github_api_host
        .bytes()
        .any(|b| !(b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-')))
        || writes.github_api_base.trim_end_matches('/') != format!("https://{}", writes.github_api_host)
    {
        return Err("a GitHub host with something a host does not have".to_owned());
    }
    if writes.base_branch.to_string().contains("..") {
        return Err("a branch with '..'".to_owned());
    }
    if domain::ServiceRef::new("config-server", &writes.config_server_deployment).is_err() {
        return Err("a Deployment name the write path refuses".to_owned());
    }
    for host in writes.teams_hosts.iter().flatten() {
        let name = host.strip_prefix("*.").unwrap_or(host);
        if name.is_empty()
            || name.starts_with('.')
            || name.contains("..")
            || name
                .bytes()
                .any(|b| !(b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-')))
        {
            return Err("a Teams host pattern that is not a host name".to_owned());
        }
    }
    if writes
        .teams_hosts
        .as_ref()
        .is_some_and(|hosts| hosts.is_empty() || hosts.len() > 16)
    {
        return Err("a Teams host list outside 1 to 16".to_owned());
    }
    let secs = |d: std::time::Duration, min: u64, max: u64, what: &str| {
        if (min..=max).contains(&d.as_secs()) {
            Ok(())
        } else {
            Err(format!("{what} accepted outside its range"))
        }
    };
    secs(writes.jobs.reconcile, 1, 3600, "the reconcile interval")?;
    secs(writes.jobs.expire, 1, 86_400, "the expiry interval")?;
    secs(writes.jobs.pr_jobs, 1, 300, "the pull-request interval")?;
    secs(writes.jobs.cleanup, 60, 7 * 86_400, "the cleanup interval")?;
    Ok(())
}

/// What the identity group may hold once it is accepted (T4, S1, S3, S7, S9): the origin is the one string a state-changing request's
/// `Origin` header is compared with and the host Entra sends the browser back to, so it is `https://host[:port]` and nothing that could
/// carry a path, a query, a userinfo or a control character; the two secrets are names and not values; the checkpoint key is a key
/// version and the bucket a bucket name; and the audiences and the first admins are non-empty, distinct and bounded.
fn check_identity(identity: &hub::config::IdentitySettings) -> Result<(), String> {
    let entra = &identity.config.entra;
    let origin = identity.public_origin();
    let host = origin
        .strip_prefix("https://")
        .ok_or_else(|| format!("an origin that is not HTTPS: {origin}"))?;
    if host.is_empty()
        || host.bytes().any(|b| {
            !(b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
        })
    {
        return Err(format!("an origin with more than a host and a port: {origin}"));
    }
    if entra.authority_host.is_empty()
        || entra
            .authority_host
            .bytes()
            .any(|b| b.is_ascii_uppercase() || b == b'/')
    {
        return Err("an authority host that is not a bare lower-case host".to_owned());
    }
    if !entra.redirect_path.starts_with('/')
        || entra.redirect_path.starts_with("//")
        || entra
            .redirect_path
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '?' || c == '#')
    {
        return Err("a redirect path that could leave the origin".to_owned());
    }
    if entra.accepted_api_audiences.is_empty()
        || entra
            .accepted_api_audiences
            .iter()
            .any(|a| a.is_empty() || a.chars().any(|c| c.is_control() || c.is_whitespace()))
    {
        return Err("an API audience that is empty or holds whitespace".to_owned());
    }
    if let hub_identity::config::ClientAuth::Secret { name } = &entra.client_auth {
        if !ports::is_valid_secret_name(name) {
            return Err("an Entra client secret reference that is not a secret name".to_owned());
        }
    } else {
        return Err(
            "the federated credential, which the pod contract has no volume for, was accepted".to_owned(),
        );
    }
    if !ports::is_valid_secret_name(&entra.login_cookie_key_secret) {
        return Err("a login cookie key reference that is not a secret name".to_owned());
    }
    let admins = &identity.config.bootstrap_admins;
    let distinct: std::collections::BTreeSet<_> = admins.iter().collect();
    if admins.len() > hub_identity::config::MAX_BOOTSTRAP_ADMINS || distinct.len() != admins.len() {
        return Err("the first admins are too many or not distinct".to_owned());
    }
    for key in std::iter::once(&identity.checkpoint.key).chain(&identity.checkpoint.older_keys) {
        if CryptoKeyVersionName::parse(key).is_none() {
            return Err("a checkpoint key that is not a key version".to_owned());
        }
    }
    let bucket = identity.checkpoint.bucket.as_str();
    if !(3..=63).contains(&bucket.len())
        || !bucket
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("a bucket name that is not one".to_owned());
    }
    if format!("{identity:?}").len() > 8192 {
        return Err("a debug output out of proportion to its fields".to_owned());
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ hub_cli_args

/// The input is the arguments, separated by NUL bytes. Compared with a second statement of the grammar, written from the
/// usage text and not from the parser.
pub fn hub_cli_args(data: &[u8]) -> Result<Verdict, String> {
    let args: Vec<String> = if data.is_empty() {
        Vec::new()
    } else {
        data.split(|b| *b == 0)
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect()
    };
    let got = cli::parse(args.iter().map(String::as_str));
    let want = grammar(&args);
    if got != want {
        return Err(format!(
            "the parser and the grammar disagree: {got:?} against {want:?}"
        ));
    }
    match got {
        Ok(_) => Ok(Verdict::Accepted),
        Err(error) => {
            // S10: an argument can be a pasted secret. The message is one of the fixed phrases, and the only text in it that is
            // not fixed is one of the three option names, never a value.
            let shown = error.to_string();
            let fixed = shown == "unknown command"
                || shown == "unexpected argument"
                || OPTIONS
                    .iter()
                    .chain(std::iter::once(&"<file>"))
                    .any(|o| shown == format!("{o} is required") || shown == format!("{o} is not valid"));
            if !fixed {
                return Err(format!("an error with other text in it: {shown}"));
            }
            Ok(Verdict::Rejected)
        }
    }
}

/// The three options of `ca-init`, as the usage text spells them, in the order the usage gives them.
const OPTIONS: [&str; 3] = ["--kms-ref", "--common-name", "--valid-days"];

/// The usage text, as a grammar: `hub [serve] | hub migrate | hub seed [--dry-run] <file> | hub ca-init <three options, each once, in
/// any order> | hub help`.
fn grammar(args: &[String]) -> Result<Command, CliError> {
    let word = |s: &str| matches!(s, "serve" | "migrate" | "help" | "--help" | "-h");
    match args {
        [] => Ok(Command::Serve),
        [one] if one == "serve" => Ok(Command::Serve),
        [one] if one == "migrate" => Ok(Command::Migrate),
        [one] if word(one) => Ok(Command::Help),
        [first, rest @ ..] if first == "seed" => seed_grammar(rest),
        [first, rest @ ..] if first == "ca-init" => ca_init_grammar(rest),
        [first, ..] if !word(first) => Err(CliError::UnknownCommand),
        _ => Err(CliError::UnexpectedArgument),
    }
}

/// `seed`: `--dry-run` at most once and one file, in either order, read left to right: the first word that cannot be either decides.
/// A file is a word of 1 to 4,096 bytes that does not start with a dash.
fn seed_grammar(rest: &[String]) -> Result<Command, CliError> {
    let mut file: Option<&String> = None;
    let mut dry_run = false;
    for word in rest {
        match word.as_str() {
            "--dry-run" if dry_run => return Err(CliError::UnexpectedArgument),
            "--dry-run" => dry_run = true,
            w if w.starts_with('-') || file.is_some() => return Err(CliError::UnexpectedArgument),
            "" => return Err(CliError::InvalidOption("<file>")),
            w if w.len() > 4096 => return Err(CliError::InvalidOption("<file>")),
            _ => file = Some(word),
        }
    }
    let file = file.ok_or(CliError::MissingOption("<file>"))?;
    Ok(Command::Seed(SeedArgs {
        file: std::path::PathBuf::from(file),
        dry_run,
    }))
}

fn ca_init_grammar(rest: &[String]) -> Result<Command, CliError> {
    let mut given: [Option<&str>; 3] = [None, None, None];
    let mut at = 0;
    while at < rest.len() {
        let Some(slot) = OPTIONS.iter().position(|o| *o == rest[at]) else {
            return Err(CliError::UnexpectedArgument);
        };
        if given[slot].is_some() {
            return Err(CliError::UnexpectedArgument);
        }
        let name: &'static str = OPTIONS[slot];
        let Some(value) = rest.get(at + 1) else {
            return Err(CliError::InvalidOption(name));
        };
        let good = match slot {
            0 => key_version(value),
            1 => common_name(value),
            _ => days(value).is_some(),
        };
        if !good {
            return Err(CliError::InvalidOption(name));
        }
        given[slot] = Some(value);
        at += 2;
    }
    let [Some(kms), Some(name), Some(valid)] = given else {
        let missing = given.iter().position(Option::is_none).unwrap_or(0);
        return Err(CliError::MissingOption(OPTIONS[missing]));
    };
    Ok(Command::CaInit(CaInitArgs {
        kms_ref: ports::KeyRef::parse(kms).map_err(|_| CliError::InvalidOption("--kms-ref"))?,
        common_name: name.to_owned(),
        valid_days: days(valid).unwrap_or(0),
    }))
}

/// `projects/P/locations/L/keyRings/R/cryptoKeys/K/cryptoKeyVersions/N`: P, L, R and K of letters, digits, `_` and `-` (at most 63),
/// N of digits (at most 19).
fn key_version(value: &str) -> bool {
    let parts: Vec<&str> = value.split('/').collect();
    let segment = |name: &str| {
        !name.is_empty()
            && name.len() <= 63
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    let [
        projects,
        project,
        locations,
        location,
        key_rings,
        ring,
        crypto_keys,
        key,
        versions,
        number,
    ] = parts.as_slice()
    else {
        return false;
    };
    *projects == "projects"
        && *locations == "locations"
        && *key_rings == "keyRings"
        && *crypto_keys == "cryptoKeys"
        && *versions == "cryptoKeyVersions"
        && [project, location, ring, key]
            .into_iter()
            .all(|name| segment(name))
        && !number.is_empty()
        && number.len() <= 19
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// Up to 64 characters of letters, digits, space, `.`, `_` and `-`, the first a letter or a digit.
fn common_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-'))
}

/// 1 to 3650, as digits and nothing else.
fn days(value: &str) -> Option<u32> {
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = value.parse().ok()?;
    (1..=3650).contains(&n).then_some(n)
}

// ------------------------------------------------------------------------------------------------ hub_seed_file

/// The words a place in an error may be made of: the sections and the fields of the seed file. Nothing else can be in a message of
/// `SeedError` that is not a number (`FieldPath` is built from constants), and this checks that no input got there.
const SEED_SECTIONS: [&str; 4] = ["version", "projects", "swimlanes", "sentinels"];
const SEED_FIELDS: [&str; 14] = [
    "id",
    "display_name",
    "project",
    "cluster",
    "tier",
    "nfs_server",
    "nfs_export",
    "mount_root",
    "agent_service_account",
    "tenants",
    "vm",
    "swimlane",
    "export_root",
    "service_account",
];

fn seed_place_is_fixed(place: &FieldPath) -> bool {
    SEED_SECTIONS.contains(&place.section) && place.field.is_none_or(|f| SEED_FIELDS.contains(&f))
}

/// S10: an error of the seed names a place made of constants and a reason that is fixed text, and never a value of the file.
fn seed_error_is_fixed(error: &SeedError) -> Result<(), String> {
    let fixed = match error {
        SeedError::Invalid { field, .. }
        | SeedError::Duplicate { field }
        | SeedError::UnknownProject { field }
        | SeedError::UnknownSwimlane { field }
        | SeedError::MissingOnCreate { field } => seed_place_is_fixed(field),
        SeedError::Read | SeedError::TooLarge | SeedError::NotText | SeedError::Syntax { .. } => true,
        SeedError::TooManyChanges { changes } => *changes > MAX_CHANGES,
        // The rest need a database and cannot come out of parsing or planning.
        _ => false,
    };
    let shown = format!("{error}");
    if !fixed || shown.is_empty() || shown.len() > 400 || shown.contains(char::is_control) {
        return Err(format!(
            "an error that is not a fixed place and a fixed reason: {shown}"
        ));
    }
    Ok(())
}

/// The input is the bytes of a seed file. Parsing never panics and never reads more than [`MAX_FILE_BYTES`]; an error is fixed text
/// (a place and a reason); an accepted file is inside every limit, survives a print and a parse, and plans against an empty database
/// into at most 200 changes whose events are small enough to record, after which a second plan finds nothing to change (a run is
/// idempotent for any file there is).
pub fn hub_seed_file(data: &[u8]) -> Result<Verdict, String> {
    let parsed = hub::seed::file::parse(data);
    if data.len() > MAX_FILE_BYTES && parsed != Err(SeedError::TooLarge) {
        return Err("a file over the size limit was not refused as too large".to_owned());
    }
    let file = match parsed {
        Err(error) => {
            seed_error_is_fixed(&error)?;
            return Ok(Verdict::Rejected);
        }
        Ok(file) => file,
    };
    // The limits.
    let sections = [
        file.projects.as_ref().map(Vec::len),
        file.swimlanes.as_ref().map(Vec::len),
        file.sentinels.as_ref().map(Vec::len),
    ];
    if sections
        .iter()
        .flatten()
        .any(|n| *n > hub::seed::file::MAX_PER_SECTION)
    {
        return Err("a section over the limit was accepted".to_owned());
    }
    for lane in file.swimlanes.iter().flatten() {
        if lane
            .tenants
            .as_ref()
            .is_some_and(|t| t.len() > hub::seed::file::MAX_TENANTS)
        {
            return Err("more tenants than the limit were accepted".to_owned());
        }
        // An address that is accepted is lower case, ASCII, and a Google service account's: it is compared with a token's email.
        if let Some(account) = &lane.agent_service_account {
            if !account.is_ascii()
                || account.to_ascii_lowercase() != *account
                || !account.ends_with(".gserviceaccount.com")
                || account.matches('@').count() != 1
            {
                return Err("an address that is not a service account was accepted".to_owned());
            }
        }
    }
    for sentinel in file.sentinels.iter().flatten() {
        if hub_platform::ca::profile::PeerName::parse(&sentinel.vm).is_err() {
            return Err("a sentinel name that the CA would refuse was accepted".to_owned());
        }
    }
    // A valid file survives a print and a parse.
    let json = file.to_json();
    match hub::seed::file::parse(json.as_bytes()) {
        Ok(again) if again == file => {}
        other => {
            return Err(format!(
                "the file does not survive a print and a parse: {other:?}"
            ));
        }
    }
    // The plan against an empty database: refused with fixed text, or a bounded list of changes that each make a recordable event.
    match seed_plan::plan(&file, &SeedState::default()) {
        Err(error) => seed_error_is_fixed(&error)?,
        Ok(plan) => {
            if plan.changes.len() > MAX_CHANGES {
                return Err("a plan over the limit on changes was accepted".to_owned());
            }
            let events = plan
                .events(domain::Timestamp::from_unix_millis(0), &"0".repeat(64))
                .map_err(|e| format!("a change that has no event: {e}"))?;
            if events.len() != plan.changes.len() {
                return Err("not one event for each change".to_owned());
            }
            for event in &events {
                if ports::canonical_size(event).is_err() {
                    return Err("an event that cannot be recorded".to_owned());
                }
            }
            // Idempotence: applied, the same file changes nothing.
            let next = SeedState::default().after(&plan);
            match seed_plan::plan(&file, &next) {
                Ok(second) if second.changes.is_empty() => {}
                other => return Err(format!("a second run would change something: {other:?}")),
            }
        }
    }
    Ok(Verdict::Accepted)
}

// ------------------------------------------------------------------------------------------------ hub_pem

/// A CA certificate secret (T3): the text is read as exactly one certificate and nothing else. The verdict is the CA decoder's;
/// `hub_pem` runs it on the first half of its input too, and the seeds named `*_ca_*` pin it.
pub fn ca_certificate(data: &[u8]) -> Result<Verdict, String> {
    let text = match data.iter().position(|b| *b == 0) {
        Some(at) => &data[..at],
        None => data,
    };
    ca_certificate_text(&Secret::new(String::from_utf8_lossy(text).into_owned()))
}

/// Every error the PEM decoders can give (S7). `PemError` has no field, so a message cannot carry the text it was given; the check
/// is that what comes back is one of these values and not something else, which holds for a variant that is added later only when
/// it is listed here too (`corpus_replay` fails to compile, and then to pass, until it is). A substring test of the message against
/// the secret was tried first and is unsound: the fuzzer fed the decoder a piece of its own fixed phrase, and a text that trims to
/// nothing is contained in every message (plan 18, FINAL).
pub const FIXED_PEM_ERRORS: [PemError; 10] = [
    PemError::ChainTooLarge,
    PemError::KeyTooLarge,
    PemError::NoCertificate,
    PemError::TooManyCertificates,
    PemError::ChainUnreadable,
    PemError::LeafUnreadable,
    PemError::KeyUnreadable,
    PemError::KeyMismatch,
    PemError::KeyUnusable,
    PemError::UnexpectedKey,
];

fn ca_certificate_text(text: &Secret<String>) -> Result<Verdict, String> {
    match web_tls::decode_ca_certificate(text) {
        Err(error) => {
            // The error is one of the fixed phrases and never the text.
            if !FIXED_PEM_ERRORS.contains(&error) || error.to_string().is_empty() {
                return Err("an error that is not one of the fixed ones".to_owned());
            }
            Ok(Verdict::Rejected)
        }
        Ok(der) => {
            let (rest, cert) = x509_parser::parse_x509_certificate(&der)
                .map_err(|_| "an accepted certificate that a second read refuses".to_owned())?;
            if !rest.is_empty() {
                return Err("an accepted certificate with bytes after it".to_owned());
            }
            // One certificate and no private key: a text with either more or the other is refused, never accepted.
            if text.expose().matches("-----BEGIN CERTIFICATE-----").count() > 1
                || text.expose().contains("PRIVATE KEY-----")
            {
                return Err(
                    "an accepted secret that holds more than one certificate or a private key".to_owned(),
                );
            }
            if cert.validity().not_after.timestamp() < cert.validity().not_before.timestamp() {
                return Err("an accepted certificate that ends before it begins".to_owned());
            }
            Ok(Verdict::Accepted)
        }
    }
}

/// The input is the certificate secret's text, a NUL byte, and the key secret's text.
pub fn hub_pem(data: &[u8]) -> Result<Verdict, String> {
    let (chain, key) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &data[data.len()..]),
    };
    let chain = Secret::new(String::from_utf8_lossy(chain).into_owned());
    let key = Secret::new(String::from_utf8_lossy(key).into_owned());
    // The same text read as a CA certificate secret (T3) is held to its own invariants whatever the web decoder says.
    ca_certificate_text(&chain)?;
    match web_tls::decode(&chain, &key) {
        Err(error) => {
            // S7: the secrets' text is never repeated. Every message is one of the fixed phrases of `PemError`.
            if !FIXED_PEM_ERRORS.contains(&error) {
                return Err("an error that is not one of the fixed ones".to_owned());
            }
            Ok(Verdict::Rejected)
        }
        Ok(decoded) => {
            if decoded.key.cert.is_empty() || decoded.key.cert.len() > MAX_CHAIN_LEN {
                return Err(format!(
                    "an accepted chain of {} certificates",
                    decoded.key.cert.len()
                ));
            }
            // The leaf is read again by the parser itself, and it agrees about the expiry.
            let (rest, leaf) = x509_parser::parse_x509_certificate(decoded.key.cert[0].as_ref())
                .map_err(|_| "an accepted leaf that a second read refuses".to_owned())?;
            if !rest.is_empty() {
                return Err("an accepted leaf with bytes after it".to_owned());
            }
            if leaf.validity().not_after.timestamp().saturating_mul(1000) != decoded.not_after.unix_millis() {
                return Err("the expiry differs between the two reads".to_owned());
            }
            Ok(Verdict::Accepted)
        }
    }
}
