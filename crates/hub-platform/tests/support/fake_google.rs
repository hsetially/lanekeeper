//! A fake Google: the metadata server, Secret Manager and Cloud KMS, as one HTTP/1.1 server behind an in-memory pipe.
//!
//! It speaks the same REST shapes as the real services and enforces what the real ones enforce that matters here: a bearer
//! token on every API call (401 otherwise), CRC-32C checksums on requests and the "verified" flags in the answers, real
//! AES-256-GCM with additional authenticated data behind encrypt and decrypt, and real P-256 signatures over the digest
//! behind asymmetricSign. It records every request, and a test can script faults per route: a status, a hang, a cut
//! connection, a damaged answer.
//!
//! The server is reached through a [`Dialer`], not a socket, so a test with a paused clock moves it deterministically.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hub_platform::gcp::http::{BoxedIo, Dialer};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use ports::{KeyRef, KmsSigner};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsAcceptor;

use super::soft_kms::SoftKmsSigner;

pub const PROJECT: &str = "lk-test-project";
pub const SIGN_KEY: &str =
    "projects/lk-test-project/locations/europe-west2/keyRings/lk/cryptoKeys/agent-ca/cryptoKeyVersions/1";
pub const UNKNOWN_SIGN_KEY: &str =
    "projects/lk-test-project/locations/europe-west2/keyRings/lk/cryptoKeys/absent/cryptoKeyVersions/1";
pub const ENVELOPE_KEY: &str =
    "projects/lk-test-project/locations/europe-west2/keyRings/lk/cryptoKeys/token-envelope";
pub const OTHER_ENVELOPE_KEY: &str =
    "projects/lk-test-project/locations/europe-west2/keyRings/lk/cryptoKeys/other";

/// The routes a test can script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    Token,
    SecretAccess,
    Sign,
    Encrypt,
    Decrypt,
    PublicKey,
}

/// What a scripted fault does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Answer with this status and an error body.
    Status(u16),
    /// Never answer.
    Hang,
    /// Close the connection without an answer.
    Disconnect,
    /// Answer 200 with a body whose checksum does not match.
    DamagedAnswer,
    /// Answer 200 with text that is not JSON.
    Garbage,
    /// Answer 200 with this many bytes of filler.
    Oversize(usize),
}

/// One request the server saw.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub route: Option<Route>,
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub metadata_flavor: Option<String>,
    pub host: Option<String>,
    pub body: Value,
}

struct Tokens {
    issued: u64,
    valid: HashSet<String>,
    expires_in: u64,
}

struct State {
    log: Mutex<Vec<Recorded>>,
    dials: Mutex<Vec<(String, u16)>>,
    tokens: Mutex<Tokens>,
    secrets: Mutex<HashMap<String, String>>,
    signer: SoftKmsSigner,
    sign_key: KeyRef,
    envelopes: HashMap<String, LessSafeKey>,
    faults: Mutex<HashMap<Route, VecDeque<Fault>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct FakeGoogle {
    state: Arc<State>,
}

impl fmt::Debug for FakeGoogle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeGoogle").finish_non_exhaustive()
    }
}

impl Default for FakeGoogle {
    fn default() -> Self {
        Self::new()
    }
}

fn crc(data: &[u8]) -> i64 {
    // CRC-32C, written out so that the fake does not share code with the implementation under test.
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    i64::from(!crc)
}

fn parse_crc(value: &Value) -> Option<i64> {
    match value {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn error_body(status: u16) -> Value {
    json!({"error": {"code": status, "message": "fake google error", "status": "FAKE"}})
}

impl FakeGoogle {
    pub fn new() -> Self {
        let sign_key = KeyRef::parse(SIGN_KEY).unwrap();
        let signer = SoftKmsSigner::new(&[&sign_key]);
        let mut envelopes = HashMap::new();
        for name in [ENVELOPE_KEY, OTHER_ENVELOPE_KEY] {
            let mut key = [0u8; 32];
            SystemRandom::new().fill(&mut key).unwrap();
            envelopes.insert(
                name.to_owned(),
                LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap()),
            );
        }
        Self {
            state: Arc::new(State {
                log: Mutex::new(Vec::new()),
                dials: Mutex::new(Vec::new()),
                tokens: Mutex::new(Tokens {
                    issued: 0,
                    valid: HashSet::new(),
                    expires_in: 3599,
                }),
                secrets: Mutex::new(HashMap::new()),
                signer,
                sign_key,
                envelopes,
                faults: Mutex::new(HashMap::new()),
            }),
        }
    }

    #[must_use]
    pub fn with_secret(self, name: &str, value: &str) -> Self {
        self.set_secret(name, value);
        self
    }

    pub fn set_secret(&self, name: &str, value: &str) {
        lock(&self.state.secrets).insert(name.to_owned(), value.to_owned());
    }

    pub fn remove_secret(&self, name: &str) {
        lock(&self.state.secrets).remove(name);
    }

    /// The token lifetime the metadata server reports from now on.
    pub fn set_token_lifetime(&self, seconds: u64) {
        lock(&self.state.tokens).expires_in = seconds;
    }

    /// Every token issued so far stops working (the next call with one gets a 401).
    pub fn revoke_tokens(&self) {
        lock(&self.state.tokens).valid.clear();
    }

    pub fn script(&self, route: Route, fault: Fault, times: usize) {
        let mut faults = lock(&self.state.faults);
        let queue = faults.entry(route).or_default();
        for _ in 0..times {
            queue.push_back(fault);
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        lock(&self.state.log).clone()
    }

    pub fn count(&self, route: Route) -> usize {
        lock(&self.state.log)
            .iter()
            .filter(|r| r.route == Some(route))
            .count()
    }

    pub fn dials(&self) -> Vec<(String, u16)> {
        lock(&self.state.dials).clone()
    }

    /// The signing keys' verifier: whether `signature` is the fake KMS key's signature over `digest`.
    pub fn verifies(&self, key: &KeyRef, digest: &[u8; 32], signature: &[u8]) -> bool {
        self.state.signer.verify(key, digest, signature)
    }

    pub fn public_key_spki_der(&self, key: &KeyRef) -> Option<Vec<u8>> {
        self.state.signer.public_key_spki_der(key)
    }

    /// A dialer whose connections go to this server in the clear.
    pub fn dialer(&self) -> Arc<dyn Dialer> {
        Arc::new(FakeDialer {
            state: Arc::clone(&self.state),
            tls: None,
        })
    }

    /// A dialer whose connections go to this server over TLS with `config`.
    pub fn tls_dialer(&self, config: Arc<rustls::ServerConfig>) -> Arc<dyn Dialer> {
        Arc::new(FakeDialer {
            state: Arc::clone(&self.state),
            tls: Some(TlsAcceptor::from(config)),
        })
    }
}

struct FakeDialer {
    state: Arc<State>,
    tls: Option<TlsAcceptor>,
}

impl fmt::Debug for FakeDialer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeDialer").finish_non_exhaustive()
    }
}

#[async_trait]
impl Dialer for FakeDialer {
    async fn dial(&self, host: &str, port: u16) -> io::Result<BoxedIo> {
        lock(&self.state.dials).push((host.to_owned(), port));
        let (client, server) = tokio::io::duplex(256 * 1024);
        let state = Arc::clone(&self.state);
        let tls = self.tls.clone();
        tokio::spawn(async move {
            match tls {
                Some(acceptor) => {
                    if let Ok(stream) = acceptor.accept(server).await {
                        serve(state, stream).await;
                    }
                }
                None => serve(state, server).await,
            }
        });
        Ok(Box::new(client))
    }
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(state: Arc<State>, stream: S) {
    let service = hyper::service::service_fn(move |request: Request<Incoming>| {
        let state = Arc::clone(&state);
        async move { handle(state, request).await }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await;
}

/// An error from the service function closes the connection without an answer.
#[derive(Debug)]
struct Disconnected;

impl fmt::Display for Disconnected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("disconnected")
    }
}

impl std::error::Error for Disconnected {}

fn route_of(method: &str, path: &str) -> Option<Route> {
    if method == "GET" && path == "/computeMetadata/v1/instance/service-accounts/default/token" {
        Some(Route::Token)
    } else if method == "GET"
        && path.starts_with("/v1/projects/")
        && path.ends_with("/versions/latest:access")
    {
        Some(Route::SecretAccess)
    } else if method == "POST" && path.starts_with("/v1/") && path.ends_with(":asymmetricSign") {
        Some(Route::Sign)
    } else if method == "POST" && path.starts_with("/v1/") && path.ends_with(":encrypt") {
        Some(Route::Encrypt)
    } else if method == "POST" && path.starts_with("/v1/") && path.ends_with(":decrypt") {
        Some(Route::Decrypt)
    } else if method == "GET" && path.starts_with("/v1/") && path.ends_with("/publicKey") {
        Some(Route::PublicKey)
    } else {
        None
    }
}

fn reply(status: u16, body: &Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
        .unwrap()
}

async fn handle(
    state: Arc<State>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Disconnected> {
    let (parts, body) = request.into_parts();
    let bytes = body
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let method = parts.method.as_str().to_owned();
    let path = parts.uri.path().to_owned();
    let route = route_of(&method, &path);
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let body_json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    lock(&state.log).push(Recorded {
        route,
        method,
        path: path.clone(),
        authorization: header("authorization"),
        metadata_flavor: header("metadata-flavor"),
        host: header("host"),
        body: body_json.clone(),
    });
    let Some(route) = route else {
        return Ok(reply(404, &error_body(404)));
    };

    let fault = lock(&state.faults).get_mut(&route).and_then(VecDeque::pop_front);
    match fault {
        Some(Fault::Hang) => std::future::pending::<()>().await,
        Some(Fault::Disconnect) => return Err(Disconnected),
        Some(Fault::Status(status)) => return Ok(reply(status, &error_body(status))),
        Some(Fault::Garbage) => {
            return Ok(Response::builder()
                .status(200)
                .body(Full::new(Bytes::from_static(b"<html>not json")))
                .unwrap());
        }
        Some(Fault::Oversize(n)) => {
            return Ok(Response::builder()
                .status(200)
                .body(Full::new(Bytes::from(vec![b'x'; n])))
                .unwrap());
        }
        Some(Fault::DamagedAnswer) | None => {}
    }
    let damaged = fault == Some(Fault::DamagedAnswer);

    if route == Route::Token {
        if header("metadata-flavor").as_deref() != Some("Google") {
            return Ok(reply(403, &error_body(403)));
        }
        let mut tokens = lock(&state.tokens);
        tokens.issued += 1;
        let token = format!("ya29.fake-token-{}", tokens.issued);
        tokens.valid.insert(token.clone());
        return Ok(reply(
            200,
            &json!({"access_token": token, "expires_in": tokens.expires_in, "token_type": "Bearer"}),
        ));
    }

    let authorized = header("authorization")
        .and_then(|h| h.strip_prefix("Bearer ").map(str::to_owned))
        .is_some_and(|t| lock(&state.tokens).valid.contains(&t));
    if !authorized {
        return Ok(reply(401, &error_body(401)));
    }

    let (status, answer) = match route {
        Route::SecretAccess => secret_access(&state, &path, damaged),
        Route::Sign => sign(&state, &path, &body_json, damaged).await,
        Route::Encrypt => encrypt(&state, &path, &body_json, damaged),
        Route::Decrypt => decrypt(&state, &path, &body_json, damaged),
        Route::PublicKey => public_key(&state, &path, damaged),
        Route::Token => (500, error_body(500)),
    };
    Ok(reply(status, &answer))
}

fn secret_access(state: &State, path: &str, damaged: bool) -> (u16, Value) {
    // projects/P/secrets/NAME/versions/latest:access
    let parts: Vec<&str> = path.trim_start_matches("/v1/").split('/').collect();
    let [
        "projects",
        project,
        "secrets",
        secret,
        "versions",
        "latest:access",
    ] = parts.as_slice()
    else {
        return (400, error_body(400));
    };
    if *project != PROJECT {
        return (404, error_body(404));
    }
    let Some(value) = lock(&state.secrets).get(*secret).cloned() else {
        return (404, error_body(404));
    };
    let data_crc = crc(value.as_bytes()) ^ i64::from(damaged);
    (
        200,
        json!({
            "name": format!("projects/123/secrets/{secret}/versions/1"),
            "payload": {"data": b64(value.as_bytes()), "dataCrc32c": data_crc.to_string()},
        }),
    )
}

async fn sign(state: &State, path: &str, body: &Value, damaged: bool) -> (u16, Value) {
    let name = path
        .trim_start_matches("/v1/")
        .trim_end_matches(":asymmetricSign");
    let Ok(key) = KeyRef::parse(name) else {
        return (400, error_body(400));
    };
    if !state.signer.has(&key) {
        return (404, error_body(404));
    }
    let Some(digest) = body["digest"]["sha256"]
        .as_str()
        .and_then(|d| STANDARD.decode(d).ok())
    else {
        return (400, error_body(400));
    };
    let Ok(digest): Result<[u8; 32], _> = digest.try_into() else {
        return (400, error_body(400));
    };
    // The real service refuses a request whose checksum does not match.
    if parse_crc(&body["digestCrc32c"]) != Some(crc(&digest)) {
        return (400, error_body(400));
    }
    let signature = state.signer.sign_digest(key, digest).await.unwrap();
    let signature_crc = crc(signature.as_bytes()) ^ i64::from(damaged);
    (
        200,
        json!({
            "name": name,
            "signature": b64(signature.as_bytes()),
            "signatureCrc32c": signature_crc.to_string(),
            "verifiedDigestCrc32c": true,
        }),
    )
}

fn encrypt(state: &State, path: &str, body: &Value, damaged: bool) -> (u16, Value) {
    let key_name = path.trim_start_matches("/v1/").trim_end_matches(":encrypt");
    let Some(key) = state.envelopes.get(key_name) else {
        return (404, error_body(404));
    };
    let (Some(plaintext), Some(aad)) = (
        body["plaintext"].as_str().and_then(|d| STANDARD.decode(d).ok()),
        body["additionalAuthenticatedData"]
            .as_str()
            .and_then(|d| STANDARD.decode(d).ok()),
    ) else {
        return (400, error_body(400));
    };
    if parse_crc(&body["plaintextCrc32c"]) != Some(crc(&plaintext))
        || parse_crc(&body["additionalAuthenticatedDataCrc32c"]) != Some(crc(&aad))
    {
        return (400, error_body(400));
    }
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).unwrap();
    let mut in_out = plaintext;
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut in_out)
        .unwrap();
    let mut ciphertext = nonce.to_vec();
    ciphertext.extend_from_slice(&in_out);
    let ciphertext_crc = crc(&ciphertext) ^ i64::from(damaged);
    (
        200,
        json!({
            "name": format!("{key_name}/cryptoKeyVersions/1"),
            "ciphertext": b64(&ciphertext),
            "ciphertextCrc32c": ciphertext_crc.to_string(),
            "verifiedPlaintextCrc32c": true,
            "verifiedAdditionalAuthenticatedDataCrc32c": true,
        }),
    )
}

fn decrypt(state: &State, path: &str, body: &Value, damaged: bool) -> (u16, Value) {
    let key_name = path.trim_start_matches("/v1/").trim_end_matches(":decrypt");
    let Some(key) = state.envelopes.get(key_name) else {
        return (404, error_body(404));
    };
    let (Some(ciphertext), Some(aad)) = (
        body["ciphertext"].as_str().and_then(|d| STANDARD.decode(d).ok()),
        body["additionalAuthenticatedData"]
            .as_str()
            .and_then(|d| STANDARD.decode(d).ok()),
    ) else {
        return (400, error_body(400));
    };
    if parse_crc(&body["ciphertextCrc32c"]) != Some(crc(&ciphertext))
        || parse_crc(&body["additionalAuthenticatedDataCrc32c"]) != Some(crc(&aad))
    {
        return (400, error_body(400));
    }
    if ciphertext.len() < NONCE_LEN {
        return (400, error_body(400));
    }
    let (nonce, rest) = ciphertext.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().unwrap();
    let mut in_out = rest.to_vec();
    // Wrong associated data, a damaged ciphertext and a truncated one all fail the authentication tag.
    let Ok(plaintext) = key.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut in_out)
    else {
        return (400, error_body(400));
    };
    let plaintext_crc = crc(plaintext) ^ i64::from(damaged);
    (
        200,
        json!({"plaintext": b64(plaintext), "plaintextCrc32c": plaintext_crc.to_string()}),
    )
}

fn public_key(state: &State, path: &str, damaged: bool) -> (u16, Value) {
    let name = path.trim_start_matches("/v1/").trim_end_matches("/publicKey");
    let Ok(key) = KeyRef::parse(name) else {
        return (400, error_body(400));
    };
    let Some(der) = state.signer.public_key_spki_der(&key) else {
        return (404, error_body(404));
    };
    let pem = pem::encode(&pem::Pem::new("PUBLIC KEY", der));
    let pem_crc = crc(pem.as_bytes()) ^ i64::from(damaged);
    (
        200,
        json!({"pem": pem, "pemCrc32c": pem_crc.to_string(), "algorithm": "EC_SIGN_P256_SHA256", "name": name}),
    )
}

// ---------------------------------------------------------------------------------------------------------------------
// A wired-up client stack against the fake, for the tests.

use std::time::Duration;

use hub_platform::gcp::http::{GoogleHttp, HostPolicy, HttpLimits, Origin, tls_config};
use hub_platform::gcp::kms::{CloudKmsEnvelope, CloudKmsSigner};
use hub_platform::gcp::metrics::GoogleMetrics;
use hub_platform::gcp::secret_manager::{DEFAULT_CACHE_ENTRIES, MAX_CACHE_TTL, SecretManagerSource};
use hub_platform::gcp::token::WorkloadIdentityTokens;
use hub_platform::gcp::{CallLimits, GoogleApi, ProjectId, Transport};
use prometheus_client::encoding::text;
use prometheus_client::registry::Registry;

pub const METADATA_HOST: &str = "metadata.test";
pub const SECRETS_HOST: &str = "secrets.test";
pub const KMS_HOST: &str = "kms.test";

/// The fake plus a client stack that talks to it in the clear, with the allowlist widened to the three test hosts.
pub struct Harness {
    pub fake: FakeGoogle,
    pub api: GoogleApi,
    pub transport: Transport,
    pub tokens: Arc<WorkloadIdentityTokens>,
    pub metrics: GoogleMetrics,
    pub secrets_origin: Origin,
    pub kms_origin: Origin,
    pub metadata_origin: Origin,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_limits(CallLimits::default(), HttpLimits::default())
    }

    pub fn with_limits(call: CallLimits, http: HttpLimits) -> Self {
        let fake = FakeGoogle::new();
        let policy = HostPolicy::only_for_tests(&[METADATA_HOST, SECRETS_HOST, KMS_HOST]);
        let tls = tls_config(rustls::RootCertStore::empty()).unwrap();
        let http = GoogleHttp::new(policy, tls, fake.dialer(), http);
        let metrics = GoogleMetrics::default();
        let transport = Transport::new(http, call, metrics.clone());
        let metadata_origin = Origin::parse(&format!("http://{METADATA_HOST}")).unwrap();
        let tokens = Arc::new(WorkloadIdentityTokens::new(
            transport.clone(),
            metadata_origin.clone(),
        ));
        let api = GoogleApi::new(transport.clone(), tokens.clone());
        Self {
            fake,
            api,
            transport,
            tokens,
            metrics,
            secrets_origin: Origin::parse(&format!("http://{SECRETS_HOST}")).unwrap(),
            kms_origin: Origin::parse(&format!("http://{KMS_HOST}")).unwrap(),
            metadata_origin,
        }
    }

    pub fn secret_source(&self) -> SecretManagerSource {
        self.secret_source_with(MAX_CACHE_TTL, DEFAULT_CACHE_ENTRIES)
    }

    pub fn secret_source_with(&self, ttl: Duration, max_entries: usize) -> SecretManagerSource {
        SecretManagerSource::new(
            self.api.clone(),
            self.secrets_origin.clone(),
            ProjectId::parse(PROJECT).unwrap(),
            ttl,
            max_entries,
        )
    }

    pub fn signer(&self) -> CloudKmsSigner {
        CloudKmsSigner::new(self.api.clone(), self.kms_origin.clone())
    }

    pub fn envelope(&self) -> CloudKmsEnvelope {
        CloudKmsEnvelope::new(
            self.api.clone(),
            self.kms_origin.clone(),
            &KeyRef::parse(ENVELOPE_KEY).unwrap(),
        )
        .unwrap()
    }

    /// The Prometheus exposition of the client's metrics.
    pub fn exposition(&self) -> String {
        let mut registry = Registry::with_prefix("lanekeeper");
        self.metrics.register(&mut registry);
        let mut out = String::new();
        text::encode(&mut out, &registry).unwrap();
        out
    }
}
