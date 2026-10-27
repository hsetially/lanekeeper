//! The hub with software edges, for the process tests and for `just dev` (plan 18, T1, G23).
//!
//! The real binary cannot start without Google: its secrets come from Secret Manager through the workload's identity. This
//! example runs the same `hub::process::entry`, and so the same start, serve and drain code, with the one difference that the
//! secrets are files in a directory, `LK_DEV_SECRETS_DIR`: the secret named `web-cert` is the file `web-cert` there. The CA keys
//! are software keys made at start (`soft_kms`), and the CA certificates are made for them by `hub::ca_init::run`, the same
//! ceremony `hub ca-init` runs, and written into that directory under the names `LK_AGENT_CA_CERT_REF` and
//! `LK_REPLICA_CA_CERT_REF` give. No Google ID token is accepted: the verifier knows no token. It is an example target, so it
//! is never part of the image or of a release build of `hub`; `tests/release_build.rs` keeps it that way.
//!
//! Settings are the hub's own `LK_*` variables. `LK_DEV_SECRETS_DIR` is the one addition and is read here, not by the hub.

#[path = "support/soft_kms.rs"]
mod soft_kms;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use domain::Secret;
use hub::cli::CaInitArgs;
use hub::config::HubSettings;
use hub::edges::Edges;
use hub::process::StartError;
use hub_platform::clock::SystemClock;
use ports::fakes::FakeTokenVerifier;
use ports::{SecretError, SecretSource, is_valid_secret_name};
use soft_kms::SoftKms;

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

/// Software edges for the settings given: files for secrets, software keys for the two CAs, and CA certificates made for them.
fn dev_edges(settings: &HubSettings) -> Result<Edges, StartError> {
    let invalid = || StartError::Config(hub::ConfigError::Invalid("LK_DEV_SECRETS_DIR"));
    let dir = PathBuf::from(std::env::var_os("LK_DEV_SECRETS_DIR").ok_or(StartError::Config(
        hub::ConfigError::Missing("LK_DEV_SECRETS_DIR"),
    ))?);
    let platform = settings
        .platform
        .as_ref()
        .ok_or(StartError::Config(hub::ConfigError::Missing(
            "LK_AGENT_CA_KMS_REF",
        )))?;
    let kms = Arc::new(SoftKms::new(&[
        &platform.agent_ca.kms_ref,
        &platform.replica_ca.kms_ref,
    ]));
    // The keys are new on every start, so the certificates are too: a certificate left from an earlier run would not match.
    let runtime = tokio::runtime::Handle::current();
    for ca in [&platform.agent_ca, &platform.replica_ca] {
        let args = CaInitArgs {
            kms_ref: ca.kms_ref.clone(),
            common_name: "Lanekeeper dev CA".to_owned(),
            valid_days: 30,
        };
        let pem = runtime
            .block_on(hub::ca_init::run(&args, kms.clone(), &*kms, &SystemClock))
            .map_err(|_| invalid())?;
        std::fs::write(dir.join(&ca.cert_ref), pem).map_err(|_| invalid())?;
    }
    Ok(Edges::new(
        Arc::new(DirSecrets(dir)),
        Arc::new(SystemClock),
        kms.clone(),
        kms,
        Arc::new(FakeTokenVerifier::new()),
    ))
}

fn main() -> ExitCode {
    hub::process::entry(
        std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned()),
        |name| std::env::var(name).ok(),
        dev_edges,
    )
}
