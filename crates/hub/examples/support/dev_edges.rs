//! The software edges of the dev example (plan 18, T1 to T8, G23), shared by `examples/hub_dev.rs`, the process tests and the
//! whole-process tests that start the hub inside the test (`tests/route_table_full.rs`, `tests/errors.rs`).
//!
//! NOT for production: nothing under `src/` names this file, `tests/release_build.rs` keeps it that way, and the real binary fills
//! `Edges` with Google (`Edges::google`). The including crate declares `mod soft_kms` (the software KMS) at its root.
//!
//! The knobs are `LK_DEV_*` names read through the `lookup` the caller gives, so that a test which starts the hub in its own process
//! passes a map and never touches the environment (the workspace forbids `unsafe`, and `set_var` is unsafe):
//!
//! - `LK_DEV_SECRETS_DIR` (required): the secret named `web-cert` is the file `web-cert` there. The software parts write what the hub
//!   reads at start into the same directory (the CA certificates, the Entra client secret and cookie key, the preview key).
//! - `LK_DEV_GITHUB_API`: a plain-HTTP server of the caller's that stands in for GitHub (nothing listens at the default, so every
//!   GitHub call fails the way a GitHub that is down does).
//! - `LK_DEV_GOOGLE_TOKENS`: comma-separated `token=service-account-email` pairs, the Google ID tokens an agent may join with for the
//!   audience `LK_HUB_AUDIENCE`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
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

use crate::soft_kms::SoftKms;

/// Secrets that are files in one directory.
#[derive(Debug)]
pub struct DirSecrets(pub PathBuf);

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
pub fn dev_edges(
    settings: &HubSettings,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Edges, StartError> {
    let invalid = || StartError::Config(hub::ConfigError::Invalid("LK_DEV_SECRETS_DIR"));
    let dir = PathBuf::from(lookup("LK_DEV_SECRETS_DIR").ok_or(StartError::Config(
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
    // The key that hashes personal MCP tokens (D52), read only when `LK_MCP_AUTH=personal_token`. Unlike the keys above it is kept if it
    // is there: a token made before a restart should still work, and two replicas of a test share one. A file that is there and is not
    // a key stays as it is, so that a test of the hub refusing to start on one can put it there.
    let hmac_key = dir.join(hub_mcp::auth::pat::KEY_SECRET_NAME);
    if !hmac_key.exists() {
        let mut key = [0u8; 32];
        aws_lc_rs::rand::fill(&mut key).map_err(|_| invalid())?;
        std::fs::write(
            hmac_key,
            base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, key),
        )
        .map_err(|_| invalid())?;
    }
    // The Google ID tokens an agent may join with, if the caller names any (an agent test does; `just dev` starts none).
    let verifier = FakeTokenVerifier::new();
    let bad_tokens = || StartError::Config(hub::ConfigError::Invalid("LK_DEV_GOOGLE_TOKENS"));
    for pair in lookup("LK_DEV_GOOGLE_TOKENS")
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
        &lookup("LK_DEV_GITHUB_API").unwrap_or_else(|| "http://127.0.0.1:9".to_owned()),
        GithubClientConfig::default(),
    )
    .map_err(|_| StartError::Config(hub::ConfigError::Invalid("LK_DEV_GITHUB_API")))?;
    Ok(Edges::new(
        Arc::new(DirSecrets(dir)),
        Arc::new(SystemClock),
        kms.clone(),
        kms,
        Arc::new(verifier),
        // No identity provider behind the sign-in: the routes exist and every sign-in fails. `examples/hub_dev.rs` adds the dev
        // sign-in helper for a person at a keyboard (`just dev`).
        hub::testing::unreachable_identity(),
    )
    .with_writes(WriteEdges::new(Arc::new(FakeKmsEnvelope::new()), github)))
}
