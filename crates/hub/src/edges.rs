//! The edges of the process (plan 18, T1 and T3): the ports that reach outside it.
//!
//! Everything the hub does that leaves the process goes through a port in this struct, so a test fills the struct with
//! software and the real binary fills it with Google. No fake is compiled into the `hub` binary: `Edges::google` is the only
//! constructor outside tests and examples, and `GoogleEndpoints::production()` is the only value it uses. Each task adds the
//! edges it needs: T1 the secret source and the clock, T3 the KMS signer (the CA's keys), the public half of a KMS key (only
//! `hub ca-init` asks) and the verifier of Google ID tokens (agent join). T4 adds the identity module's three: its HTTPS clients
//! (Entra, and Google's keys) and the checkpoint bucket. T5 adds the write paths' two: the KMS key that wraps the token vault, and
//! GitHub.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use domain::Timestamp;
use hub_identity::audit::{CheckpointBucket, GcsCheckpointBucket, MetadataTokenSource};
use hub_identity::http::{HttpsClient, HyperHttpsClient};
use hub_identity::tokens::GoogleIdTokens;
use hub_platform::clock::{Clock, SystemClock};
use hub_platform::gcp::http::{GoogleHttp, HttpLimits};
use hub_platform::gcp::secret_manager::{DEFAULT_CACHE_ENTRIES, MAX_CACHE_TTL};
use hub_platform::gcp::token::WorkloadIdentityTokens;
use hub_platform::gcp::{
    CallLimits, CloudKmsEnvelope, CloudKmsSigner, GoogleApi, GoogleEndpoints, GoogleMetrics,
    SecretManagerSource, Transport,
};
use hub_writes::github::{GitDataApi, GithubApi, GithubClient, GithubClientConfig};
use ports::{
    AuthError, GoogleIdentity, KeyRef, KmsEnvelope, KmsError, KmsSigner, SecretSource, TokenVerifier,
    VerifiedUser,
};

use crate::config::{ConfigError, HubSettings};
use crate::process::StartError;

/// The one host the Google ID token verifier may reach: the keys that sign the tokens (S18).
const GOOGLE_KEYS_HOST: &str = "www.googleapis.com";

/// The public half of a KMS signing key. Only the one-off CA ceremony (`hub ca-init`) needs it: the runtime never reads a key,
/// it asks KMS to sign (A8). A port of the hub's own so that the ceremony can be tested with a software key.
#[async_trait]
pub trait PublicKeys: Send + Sync + 'static {
    /// The `SubjectPublicKeyInfo`, DER, of the key version `key`.
    async fn spki_der(&self, key: &KeyRef) -> Result<Vec<u8>, KmsError>;
}

/// Cloud KMS as [`PublicKeys`].
#[derive(Debug, Clone)]
pub struct CloudPublicKeys(pub CloudKmsSigner);

#[async_trait]
impl PublicKeys for CloudPublicKeys {
    async fn spki_der(&self, key: &KeyRef) -> Result<Vec<u8>, KmsError> {
        Ok(self.0.public_key(key).await?.spki_der().to_vec())
    }
}

/// The two ports a CA ceremony needs: a signer and the public half of the key it signs with.
pub type KmsClients = (Arc<dyn KmsSigner>, Arc<dyn PublicKeys>);

/// The ports the identity module reaches outside the process through (T4). Each HTTPS client is allowed to reach the hosts it
/// needs and no others (S18), so a redirect or a document that names another host is refused by the client and not by luck.
#[derive(Clone)]
pub struct IdentityEdges {
    /// Entra: the discovery document, the tenant's keys and the token endpoint. Allowed to reach the authority host only.
    pub entra: Arc<dyn HttpsClient>,
    /// Google's key set, for the ID tokens that agents join with (`www.googleapis.com`).
    pub google_keys: Arc<dyn HttpsClient>,
    /// The bucket with the retention lock that takes one object per audit checkpoint (S9): Cloud Storage in production.
    pub checkpoints: Arc<dyn CheckpointBucket>,
}

impl fmt::Debug for IdentityEdges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityEdges").finish_non_exhaustive()
    }
}

/// The ports the write paths reach outside the process through (T5): the KMS key that wraps the data key of each stored GitHub
/// token (S8), and GitHub's API, which checks a token and makes the pull request with it. GitHub is one client with two faces, so the
/// pull-request path and the token check cannot be given different hosts.
#[derive(Clone)]
pub struct WriteEdges {
    pub envelope: Arc<dyn KmsEnvelope>,
    pub github: Arc<dyn GithubApi>,
    pub git: Arc<dyn GitDataApi>,
}

impl fmt::Debug for WriteEdges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteEdges").finish_non_exhaustive()
    }
}

impl WriteEdges {
    /// The two faces of one GitHub client, and the envelope.
    #[must_use]
    pub fn new(envelope: Arc<dyn KmsEnvelope>, github: GithubClient) -> Self {
        let github = Arc::new(github);
        Self {
            envelope,
            github: github.clone(),
            git: github,
        }
    }
}

/// The ports that reach outside the process.
#[derive(Clone)]
pub struct Edges {
    /// Runtime secrets (S7): Secret Manager in production.
    pub secrets: Arc<dyn SecretSource>,
    /// Wall time for instants that go into data and into expiry checks. Waiting uses tokio's clock.
    pub clock: Arc<dyn Clock>,
    /// Signs for the two CAs (S5, S6): Cloud KMS in production. The hub never holds a CA's private key.
    pub signer: Arc<dyn KmsSigner>,
    /// The public key of a CA's KMS key, for `hub ca-init` only.
    pub public_keys: Arc<dyn PublicKeys>,
    /// Verifies the Google ID token an agent joins with (S5). In production it accepts Google ID tokens and refuses an Entra token
    /// outright: the join is not a place an Entra token can mean anything.
    pub tokens: Arc<dyn TokenVerifier>,
    /// What the identity module reaches Entra, Google's keys and the checkpoint bucket through (T4).
    pub identity: IdentityEdges,
    /// The Google clients' counters, registered on the hub's metrics. Empty when no Google client exists.
    pub google_metrics: GoogleMetrics,
    /// The write paths' envelope and GitHub (T5). `None` is a setup that has none to give: the hub does not start on it (there is
    /// no stand-in that would accept a token it could not keep).
    pub writes: Option<WriteEdges>,
}

impl fmt::Debug for Edges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Edges").finish_non_exhaustive()
    }
}

/// The Google clients that need no project: the API with its token, and where the services are.
struct GoogleClients {
    api: GoogleApi,
    endpoints: GoogleEndpoints,
    metrics: GoogleMetrics,
}

impl GoogleClients {
    /// Builds the clients. It makes no call: the first one is the first read, under the start-up timeout.
    fn production() -> Result<Self, StartError> {
        let endpoints = GoogleEndpoints::production();
        let http = GoogleHttp::production(HttpLimits::default()).map_err(|_| StartError::Google)?;
        let metrics = GoogleMetrics::default();
        let transport = Transport::new(http, CallLimits::default(), metrics.clone());
        let tokens = Arc::new(WorkloadIdentityTokens::new(
            transport.clone(),
            endpoints.metadata.clone(),
        ));
        Ok(Self {
            api: GoogleApi::new(transport, tokens),
            endpoints,
            metrics,
        })
    }

    fn kms(&self) -> CloudKmsSigner {
        CloudKmsSigner::new(self.api.clone(), self.endpoints.kms.clone())
    }
}

impl Edges {
    /// Edges from parts the caller made: software in tests and in the dev example.
    #[must_use]
    pub fn new(
        secrets: Arc<dyn SecretSource>,
        clock: Arc<dyn Clock>,
        signer: Arc<dyn KmsSigner>,
        public_keys: Arc<dyn PublicKeys>,
        tokens: Arc<dyn TokenVerifier>,
        identity: IdentityEdges,
    ) -> Self {
        Self {
            secrets,
            clock,
            signer,
            public_keys,
            tokens,
            identity,
            google_metrics: GoogleMetrics::default(),
            writes: None,
        }
    }

    /// The same edges with the write paths' (T5).
    #[must_use]
    pub fn with_writes(mut self, writes: WriteEdges) -> Self {
        self.writes = Some(writes);
        self
    }

    /// The real edges: Secret Manager and Cloud KMS through the workload's identity, Google's keys for ID tokens, and the system
    /// clock. It makes no call; the first one is the first secret read, under the start-up timeout.
    pub fn google(settings: &HubSettings) -> Result<Self, StartError> {
        let gcp = settings
            .gcp
            .as_ref()
            .ok_or(StartError::Config(ConfigError::Missing("LK_GCP_PROJECT")))?;
        let clients = GoogleClients::production()?;
        let secrets = SecretManagerSource::new(
            clients.api.clone(),
            clients.endpoints.secret_manager.clone(),
            gcp.project.clone(),
            MAX_CACHE_TTL,
            DEFAULT_CACHE_ENTRIES,
        );
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let kms = clients.kms();
        // The identity module (T4): its settings name the authority and the bucket, so a hub without them has no real edges.
        let identity = settings
            .identity
            .as_ref()
            .ok_or(StartError::Config(ConfigError::Missing("LK_ENTRA_TENANT_ID")))?;
        let client = |hosts: &[&str]| -> Result<Arc<dyn HttpsClient>, StartError> {
            Ok(Arc::new(
                HyperHttpsClient::new(hosts).map_err(|_| StartError::Google)?,
            ))
        };
        let google_keys = client(&[GOOGLE_KEYS_HOST])?;
        let checkpoints = GcsCheckpointBucket::new(
            client(&[GcsCheckpointBucket::HOST])?,
            Arc::new(MetadataTokenSource::new()),
            &identity.checkpoint.bucket,
        )
        .map_err(|_| StartError::Config(ConfigError::Invalid("LK_AUDIT_CHECKPOINT_BUCKET")))?;
        let tokens = GoogleOnly::new(GoogleIdTokens::new(
            Arc::clone(&google_keys),
            Arc::new(IdentityClock(Arc::clone(&clock))),
        ));
        // The write paths (T5): the vault's KMS key, and GitHub at the one host the settings name (S18). Neither makes a call here.
        let writes = settings
            .writes
            .as_ref()
            .ok_or(StartError::Config(ConfigError::Missing("LK_VAULT_KMS_REF")))?;
        let envelope = CloudKmsEnvelope::new(
            clients.api.clone(),
            clients.endpoints.kms.clone(),
            &writes.vault_key,
        )
        .map_err(|_| StartError::Config(ConfigError::Invalid("LK_VAULT_KMS_REF")))?;
        let github = GithubClient::new(GithubClientConfig {
            api_base: writes.github_api_base.clone(),
            allowed_hosts: vec![writes.github_api_host.clone()],
            ..GithubClientConfig::default()
        })
        .map_err(|_| StartError::Config(ConfigError::Invalid("LK_GITHUB_API_BASE")))?;
        Ok(Self {
            secrets: Arc::new(secrets),
            clock,
            signer: Arc::new(kms.clone()),
            public_keys: Arc::new(CloudPublicKeys(kms)),
            tokens: Arc::new(tokens),
            identity: IdentityEdges {
                entra: client(&[identity.authority_host()])?,
                google_keys,
                checkpoints: Arc::new(checkpoints),
            },
            google_metrics: clients.metrics,
            writes: Some(WriteEdges::new(Arc::new(envelope), github)),
        })
    }

    /// The Cloud KMS client alone, for `hub ca-init`: it needs a KMS key and the workload's identity, no project and no secrets.
    pub fn google_kms() -> Result<KmsClients, StartError> {
        let kms = GoogleClients::production()?.kms();
        Ok((Arc::new(kms.clone()), Arc::new(CloudPublicKeys(kms))))
    }
}

/// The hub's clock as the identity module's, so that one injected clock serves both (G14).
#[derive(Debug)]
pub struct IdentityClock(pub Arc<dyn Clock>);

impl hub_identity::Clock for IdentityClock {
    fn now(&self) -> Timestamp {
        self.0.now()
    }

    fn instant(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}

/// The hub's clock as the registry's, so that one injected clock serves it too (G14).
#[derive(Debug)]
pub struct RegistryClock(pub Arc<dyn Clock>);

impl hub_registry::Clock for RegistryClock {
    fn now(&self) -> Timestamp {
        self.0.now()
    }
}

/// The hub's clock as the write paths', for the same reason.
#[derive(Debug)]
pub struct WritesClock(pub Arc<dyn Clock>);

impl hub_writes::Clock for WritesClock {
    fn now(&self) -> Timestamp {
        self.0.now()
    }
}

/// The verifier the platform's `Join` is given: Google ID tokens, and nothing else. An Entra token is refused as unverifiable,
/// never accepted, and that is on purpose and stays after the identity module is wired (T4): `Join` has no use for an Entra token,
/// so the one verifier it can reach does not know how to read one. The identity module's own verifier (`Identity::tokens`, Entra
/// and Google both) is what MCP is given.
pub struct GoogleOnly {
    google: GoogleIdTokens,
}

impl fmt::Debug for GoogleOnly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GoogleOnly").finish_non_exhaustive()
    }
}

impl GoogleOnly {
    #[must_use]
    pub fn new(google: GoogleIdTokens) -> Self {
        Self { google }
    }
}

#[async_trait]
impl TokenVerifier for GoogleOnly {
    async fn entra_access_token(&self, _jwt: &str) -> Result<VerifiedUser, AuthError> {
        Err(AuthError::InvalidToken)
    }

    async fn google_id_token(&self, jwt: &str, aud: &str) -> Result<GoogleIdentity, AuthError> {
        self.google.verify(jwt, aud).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> HubSettings {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        HubSettings::from_lookup(move |name| map.get(name).cloned()).unwrap()
    }

    #[test]
    fn the_real_edges_need_a_google_project_and_say_which_variable() {
        let error = Edges::google(&settings(&[])).unwrap_err();
        assert_eq!(error, StartError::Config(ConfigError::Missing("LK_GCP_PROJECT")));
        assert_eq!(error.exit_code(), 2);
    }

    /// The identity group, as a pod sets it, and the vault's key (T5).
    const IDENTITY_ENV: [(&str, &str); 9] = [
        (
            "LK_VAULT_KMS_REF",
            "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/vault",
        ),
        ("LK_GCP_PROJECT", "lanekeeper-prod"),
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
    ];

    // The GitHub client builds a `tower` buffer, which needs a runtime to spawn its worker on (the process builds the edges inside
    // one: `process::entry` enters it first).
    #[tokio::test]
    async fn the_real_edges_are_built_without_a_call() {
        // Building the clients reads the platform's root certificates and nothing else: no network, no credential.
        let edges = Edges::google(&settings(&IDENTITY_ENV));
        assert!(edges.is_ok(), "{edges:?}");
    }

    #[test]
    fn the_real_edges_need_the_identity_settings_too_and_say_which_variable() {
        let error = Edges::google(&settings(&[("LK_GCP_PROJECT", "lanekeeper-prod")])).unwrap_err();
        assert_eq!(
            error,
            StartError::Config(ConfigError::Missing("LK_ENTRA_TENANT_ID"))
        );
        assert_eq!(error.exit_code(), 2);
    }

    #[tokio::test]
    async fn the_real_edges_need_the_vault_key_too_and_say_which_variable() {
        let without: Vec<(&str, &str)> = IDENTITY_ENV
            .iter()
            .copied()
            .filter(|(name, _)| *name != "LK_VAULT_KMS_REF")
            .collect();
        let error = Edges::google(&settings(&without)).unwrap_err();
        assert_eq!(
            error,
            StartError::Config(ConfigError::Missing("LK_VAULT_KMS_REF"))
        );
        assert_eq!(error.exit_code(), 2);
    }

    #[tokio::test]
    async fn the_real_edges_carry_the_write_paths_envelope_and_github() {
        let edges = Edges::google(&settings(&IDENTITY_ENV)).unwrap();
        assert!(
            edges.writes.is_some(),
            "the real edges always have a vault to keep tokens in"
        );
        // And the same edges built by hand have none until they are given one: no stand-in is made on the way.
        let by_hand = Edges::new(
            edges.secrets.clone(),
            edges.clock.clone(),
            edges.signer.clone(),
            edges.public_keys.clone(),
            edges.tokens.clone(),
            edges.identity.clone(),
        );
        assert!(by_hand.writes.is_none());
    }

    #[test]
    fn the_kms_client_alone_needs_no_project() {
        assert!(Edges::google_kms().is_ok());
    }

    #[tokio::test]
    async fn the_join_verifier_accepts_no_entra_token() {
        let http: Arc<dyn HttpsClient> = Arc::new(HyperHttpsClient::new(&[GOOGLE_KEYS_HOST]).unwrap());
        let verifier = GoogleOnly::new(GoogleIdTokens::new(
            http,
            Arc::new(IdentityClock(Arc::new(SystemClock))),
        ));
        assert_eq!(
            verifier.entra_access_token("any.token.here").await.unwrap_err(),
            AuthError::InvalidToken
        );
        // And a Google token that is not a token at all is refused before any key is fetched: no network in this test.
        assert!(verifier.google_id_token("", "aud").await.is_err());
    }
}
