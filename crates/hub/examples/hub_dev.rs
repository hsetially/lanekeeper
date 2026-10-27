//! The hub with software edges, for the process tests and for `just dev` (plan 18, T1, G23).
//!
//! The real binary cannot start without Google: its secrets come from Secret Manager through the workload's identity. This
//! example runs the same `hub::process::entry`, and so the same start, serve and drain code, with the one difference that the
//! secrets are files in a directory, `LK_DEV_SECRETS_DIR`: the secret named `web-cert` is the file `web-cert` there. The CA keys
//! are software keys made at start (`soft_kms`), and the CA certificates are made for them by `hub::ca_init::run`, the same
//! ceremony `hub ca-init` runs, and written into that directory under the names `LK_AGENT_CA_CERT_REF` and
//! `LK_REPLICA_CA_CERT_REF` give. No Google ID token is accepted: the verifier knows no token. The write paths' two edges (T5) are
//! software too: a KMS envelope whose key the process holds, and GitHub's client pointed at `LK_DEV_GITHUB_API`, a plain-HTTP
//! server of the caller's (nothing listens at the default, so every GitHub call fails the way a GitHub that is down does). It is an
//! example target, so it is never part of the image or of a release build of `hub`; `tests/release_build.rs` keeps it that way.
//!
//! Settings are the hub's own `LK_*` variables. `LK_DEV_SECRETS_DIR`, `LK_DEV_GITHUB_API` and `LK_DEV_GOOGLE_TOKENS` (comma-separated
//! `token=service-account-email` pairs: the Google ID tokens an agent may join with, for the audience `LK_HUB_AUDIENCE`) are the
//! additions and are read here, not by the hub.

#[path = "support/soft_kms.rs"]
mod soft_kms;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use domain::{Secret, ShortText};
use hub::cli::CaInitArgs;
use hub::config::HubSettings;
use hub::edges::{Edges, WriteEdges};
use hub::process::StartError;
use hub_platform::clock::SystemClock;
use hub_writes::github::{GithubClient, GithubClientConfig};
use ports::fakes::{FakeKmsEnvelope, FakeTokenVerifier};
use ports::{GoogleIdentity, SecretError, SecretSource, is_valid_secret_name};
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
    let identity = settings
        .identity
        .as_ref()
        .ok_or(StartError::Config(hub::ConfigError::Missing(
            "LK_ENTRA_TENANT_ID",
        )))?;
    let kms = Arc::new(SoftKms::new(&[
        &platform.agent_ca.kms_ref,
        &platform.replica_ca.kms_ref,
        &identity.checkpoint.key,
    ]));
    // The two secrets of the Entra sign-in: a client secret nobody will present, and the key that seals the login cookie, new on
    // every start (a login in flight when the hub restarts is simply begun again).
    let name_of = |client_auth: &hub_identity::ClientAuth| match client_auth {
        hub_identity::ClientAuth::Secret { name } => name.clone(),
        hub_identity::ClientAuth::Federated { .. } => String::new(),
    };
    let mut key = [0u8; 32];
    aws_lc_rs::rand::fill(&mut key).map_err(|_| invalid())?;
    std::fs::write(
        dir.join(name_of(&identity.config.entra.client_auth)),
        "dev-entra-client-secret",
    )
    .map_err(|_| invalid())?;
    std::fs::write(
        dir.join(&identity.config.entra.login_cookie_key_secret),
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, key),
    )
    .map_err(|_| invalid())?;
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
    // The write path's preview key (D46): 32 bytes or more, new on every start like the cookie key (a preview in flight is begun again).
    let mut preview = [0u8; 32];
    aws_lc_rs::rand::fill(&mut preview).map_err(|_| invalid())?;
    std::fs::write(
        dir.join(hub_writes::PREVIEW_KEY_SECRET),
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, preview),
    )
    .map_err(|_| invalid())?;
    // The Google ID tokens an agent may join with, if the caller names any (an agent test does; `just dev` starts none).
    let verifier = FakeTokenVerifier::new();
    let bad_tokens = || StartError::Config(hub::ConfigError::Invalid("LK_DEV_GOOGLE_TOKENS"));
    for pair in std::env::var("LK_DEV_GOOGLE_TOKENS")
        .unwrap_or_default()
        .split(',')
        .filter(|pair| !pair.is_empty())
    {
        let (token, email) = pair.split_once('=').ok_or_else(bad_tokens)?;
        let audience = platform.audience.as_str();
        verifier.accept_google(
            token,
            audience,
            GoogleIdentity {
                subject: ShortText::parse("dev-subject").map_err(|_| bad_tokens())?,
                email: ShortText::parse(email).map_err(|_| bad_tokens())?,
                audience: ShortText::parse(audience).map_err(|_| bad_tokens())?,
            },
        );
    }
    let github = GithubClient::for_tests(
        &std::env::var("LK_DEV_GITHUB_API").unwrap_or_else(|_| "http://127.0.0.1:9".to_owned()),
        GithubClientConfig::default(),
    )
    .map_err(|_| StartError::Config(hub::ConfigError::Invalid("LK_DEV_GITHUB_API")))?;
    Ok(Edges::new(
        Arc::new(DirSecrets(dir)),
        Arc::new(SystemClock),
        kms.clone(),
        kms,
        Arc::new(verifier),
        // No identity provider behind the sign-in yet (T8 gives `just dev` one): the routes exist and every sign-in fails.
        hub::testing::unreachable_identity(),
    )
    .with_writes(WriteEdges::new(Arc::new(FakeKmsEnvelope::new()), github)))
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
