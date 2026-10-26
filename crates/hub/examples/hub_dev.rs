//! The hub with software edges, for the process tests and for `just dev` (plan 18, T1, G23).
//!
//! The real binary cannot start without Google: its secrets come from Secret Manager through the workload's identity. This
//! example runs the same `hub::process::entry`, and so the same start, serve and drain code, with the one difference that the
//! secrets are files in a directory, `LK_DEV_SECRETS_DIR`: the secret named `web-cert` is the file `web-cert` there. It is an
//! example target, so it is never part of the image or of a release build of `hub`; `tests/release_build.rs` keeps it that way.
//!
//! Settings are the hub's own `LK_*` variables. `LK_DEV_SECRETS_DIR` is the one addition and is read here, not by the hub.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use domain::Secret;
use hub::edges::Edges;
use hub::process::StartError;
use hub_platform::clock::SystemClock;
use ports::{SecretError, SecretSource, is_valid_secret_name};

/// Secrets that are files in one directory.
#[derive(Debug)]
struct DirSecrets(PathBuf);

#[async_trait]
impl SecretSource for DirSecrets {
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError> {
        if !is_valid_secret_name(name) {
            return Err(SecretError::InvalidName);
        }
        let path = self.0.join(name);
        tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
            .await
            .map_err(|_| SecretError::Unavailable)?
            .map(Secret::new)
            .map_err(|_| SecretError::NotFound)
    }
}

fn main() -> ExitCode {
    hub::process::entry(
        std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned()),
        |name| std::env::var(name).ok(),
        |_settings| {
            let dir = std::env::var_os("LK_DEV_SECRETS_DIR").ok_or(StartError::Config(
                hub::ConfigError::Missing("LK_DEV_SECRETS_DIR"),
            ))?;
            Ok(Edges::new(
                Arc::new(DirSecrets(PathBuf::from(dir))),
                Arc::new(SystemClock),
            ))
        },
    )
}
