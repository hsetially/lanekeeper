//! What must hold for every input of the hub's parsers at a trust boundary (plan 18, T1 and T2; rule 13; S7, S10).
//!
//! The fuzz targets and the stable test `crates/hub/tests/corpus_replay.rs` both include this file, so every committed seed is
//! replayed by `cargo test -p hub` with the same checks the fuzzer runs. A function returns the verdict (the input was
//! accepted, or refused) or says why an invariant broke. A seed's name says which verdict it must keep: `accepted_*` or
//! `rejected_*`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use domain::Secret;
use hub::cli::{self, CliError, Command};
use hub::config::{HubSettings, MigratorSettings};
use hub::web_tls::{self, MAX_CHAIN_LEN, PemError};

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
    let word = |s: &str| matches!(s, "serve" | "migrate" | "help" | "--help" | "-h");
    let want = match args.as_slice() {
        [] => Ok(Command::Serve),
        [one] if one == "serve" => Ok(Command::Serve),
        [one] if one == "migrate" => Ok(Command::Migrate),
        [one] if word(one) => Ok(Command::Help),
        [first, ..] if !word(first) => Err(CliError::UnknownCommand),
        _ => Err(CliError::UnexpectedArgument),
    };
    if got != want {
        return Err(format!(
            "the parser and the grammar disagree: {got:?} against {want:?}"
        ));
    }
    match got {
        Ok(_) => Ok(Verdict::Accepted),
        Err(error) => {
            // S10: an argument can be a pasted secret. The message is one of two fixed phrases.
            let shown = error.to_string();
            if shown != "unknown command" && shown != "unexpected argument" {
                return Err(format!("an error with other text in it: {shown}"));
            }
            Ok(Verdict::Rejected)
        }
    }
}

// ------------------------------------------------------------------------------------------------ hub_pem

/// The input is the certificate secret's text, a NUL byte, and the key secret's text.
pub fn hub_pem(data: &[u8]) -> Result<Verdict, String> {
    let (chain, key) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &data[data.len()..]),
    };
    let chain = Secret::new(String::from_utf8_lossy(chain).into_owned());
    let key = Secret::new(String::from_utf8_lossy(key).into_owned());
    match web_tls::decode(&chain, &key) {
        Err(error) => {
            // S7: the secrets' text is never repeated. Every message is one of the fixed phrases of `PemError`.
            let known = [
                PemError::ChainTooLarge,
                PemError::KeyTooLarge,
                PemError::NoCertificate,
                PemError::TooManyCertificates,
                PemError::ChainUnreadable,
                PemError::LeafUnreadable,
                PemError::KeyUnreadable,
                PemError::KeyMismatch,
                PemError::KeyUnusable,
            ];
            if !known.contains(&error) {
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
