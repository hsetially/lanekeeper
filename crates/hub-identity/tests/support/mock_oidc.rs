//! A mock Entra: discovery, a JWKS, a token endpoint, and a minter of RS256 tokens whose claims, header and signing key
//! the test controls. It is served through [`FakeHttps`], so no socket is involved.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::KeySize;
use aws_lc_rs::signature::{KeyPair, RSA_PKCS1_SHA256, RsaKeyPair};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hub_identity::config::{ClientAuth, EntraConfig};
use hub_identity::http::{HttpsError, HttpsRequest, HttpsResponse};
use serde_json::{Value, json};

use super::fake_https::{FakeHttps, json_response};

pub const TID: &str = "11111111-1111-4111-8111-111111111111";
pub const CLIENT_ID: &str = "22222222-2222-4222-8222-222222222222";
pub const OID: &str = "33333333-3333-4333-8333-333333333333";
pub const HOST: &str = "login.test";
pub const ORIGIN: &str = "https://lanekeeper.example.com";
/// 2027-01-15T08:00:00Z, the instant tests run at.
pub const NOW_MS: i64 = 1_800_000_000_000;
pub const NOW_S: i64 = NOW_MS / 1000;

pub struct TestKey {
    pub kid: String,
    pub pair: RsaKeyPair,
}

/// Four RSA keys, generated once per test binary: 0, 1 and 2 are published in turn; 3 is never published (a forger's).
pub fn keys() -> &'static [TestKey] {
    static KEYS: OnceLock<Vec<TestKey>> = OnceLock::new();
    KEYS.get_or_init(|| {
        (0..4)
            .map(|i| TestKey {
                kid: format!("key-{i}"),
                pair: RsaKeyPair::generate(KeySize::Rsa2048).unwrap(),
            })
            .collect()
    })
}

pub fn public_jwk(key: &TestKey) -> Value {
    let public = key.pair.public_key();
    json!({
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": key.kid,
        "n": URL_SAFE_NO_PAD.encode(public.modulus().big_endian_without_leading_zero()),
        "e": URL_SAFE_NO_PAD.encode(public.exponent().big_endian_without_leading_zero()),
    })
}

/// The pieces of a token the test may bend.
#[derive(Clone)]
pub struct TokenSpec {
    pub header: Value,
    pub claims: Value,
    /// Index into [`keys`] that signs it.
    pub signer: usize,
}

impl TokenSpec {
    /// A valid ID token for this tenant and client at [`NOW_S`], signed by key 0, carrying `nonce`.
    pub fn valid(nonce: &str) -> Self {
        Self {
            header: json!({"alg": "RS256", "typ": "JWT", "kid": "key-0"}),
            claims: json!({
                "iss": format!("https://{HOST}/{TID}/v2.0"),
                "aud": CLIENT_ID,
                "tid": TID,
                "oid": OID,
                "sub": "pairwise-subject",
                "nonce": nonce,
                "iat": NOW_S, "nbf": NOW_S, "exp": NOW_S + 3600,
                "name": "Ada Lovelace",
                "email": "ada@example.com",
            }),
            signer: 0,
        }
    }

    pub fn claim(mut self, name: &str, value: Value) -> Self {
        self.claims[name] = value;
        self
    }

    pub fn without_claim(mut self, name: &str) -> Self {
        self.claims.as_object_mut().unwrap().remove(name);
        self
    }

    pub fn header(mut self, name: &str, value: Value) -> Self {
        self.header[name] = value;
        self
    }

    pub fn signed_by(mut self, index: usize) -> Self {
        self.signer = index;
        self
    }

    pub fn mint(&self) -> String {
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&self.header).unwrap()),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&self.claims).unwrap())
        );
        let pair = &keys()[self.signer].pair;
        let mut sig = vec![0u8; pair.public_modulus_len()];
        pair.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signing_input.as_bytes(),
            &mut sig,
        )
        .unwrap();
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig))
    }
}

struct MockState {
    published: Vec<usize>,
    id_token: Option<String>,
    token_status: u16,
    jwks_cache_control: Option<String>,
    jwks_status: u16,
    discovery_issuer: String,
    token_host: String,
}

pub struct MockOidc {
    state: Mutex<MockState>,
    pub jwks_fetches: AtomicUsize,
    pub discovery_fetches: AtomicUsize,
    token_forms: Mutex<Vec<HashMap<String, String>>>,
    /// Set by the test to make the token endpoint fail in transport.
    pub token_transport_error: Mutex<bool>,
}

impl MockOidc {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(MockState {
                published: vec![0],
                id_token: None,
                token_status: 200,
                jwks_cache_control: Some("public, max-age=3600".to_owned()),
                jwks_status: 200,
                discovery_issuer: format!("https://{HOST}/{TID}/v2.0"),
                token_host: HOST.to_owned(),
            }),
            jwks_fetches: AtomicUsize::new(0),
            discovery_fetches: AtomicUsize::new(0),
            token_forms: Mutex::new(Vec::new()),
            token_transport_error: Mutex::new(false),
        })
    }

    /// The HTTPS client to hand to the code under test.
    pub fn https(self: &Arc<Self>) -> Arc<FakeHttps> {
        let mock = Arc::clone(self);
        Arc::new(FakeHttps::new(move |req| mock.handle(req)))
    }

    pub fn publish_only(&self, indices: &[usize]) {
        self.state.lock().unwrap().published = indices.to_vec();
    }

    pub fn set_id_token(&self, token: String) {
        self.state.lock().unwrap().id_token = Some(token);
    }

    pub fn set_token_status(&self, status: u16) {
        self.state.lock().unwrap().token_status = status;
    }

    pub fn set_jwks_cache_control(&self, value: Option<&str>) {
        self.state.lock().unwrap().jwks_cache_control = value.map(str::to_owned);
    }

    pub fn set_jwks_status(&self, status: u16) {
        self.state.lock().unwrap().jwks_status = status;
    }

    pub fn set_discovery_issuer(&self, issuer: &str) {
        issuer.clone_into(&mut self.state.lock().unwrap().discovery_issuer);
    }

    pub fn set_token_endpoint_host(&self, host: &str) {
        host.clone_into(&mut self.state.lock().unwrap().token_host);
    }

    /// Every form the token endpoint received, as field maps.
    pub fn token_forms(&self) -> Vec<HashMap<String, String>> {
        self.token_forms.lock().unwrap().clone()
    }

    fn handle(&self, req: &HttpsRequest) -> Result<HttpsResponse, HttpsError> {
        let url = req.url();
        let path = url.path();
        let st = self.state.lock().unwrap();
        if path.ends_with("/.well-known/openid-configuration") {
            self.discovery_fetches.fetch_add(1, Ordering::SeqCst);
            let doc = json!({
                "issuer": st.discovery_issuer,
                "authorization_endpoint": format!("https://{HOST}/{TID}/oauth2/v2.0/authorize"),
                "token_endpoint": format!("https://{}/{TID}/oauth2/v2.0/token", st.token_host),
                "jwks_uri": format!("https://{HOST}/{TID}/discovery/v2.0/keys"),
            });
            return Ok(json_response(200, &doc.to_string(), Some("max-age=86400")));
        }
        if path.ends_with("/discovery/v2.0/keys") {
            self.jwks_fetches.fetch_add(1, Ordering::SeqCst);
            let jwks =
                json!({"keys": st.published.iter().map(|i| public_jwk(&keys()[*i])).collect::<Vec<_>>()});
            return Ok(json_response(
                st.jwks_status,
                &jwks.to_string(),
                st.jwks_cache_control.as_deref(),
            ));
        }
        if path.ends_with("/oauth2/v2.0/token") {
            if *self.token_transport_error.lock().unwrap() {
                return Err(HttpsError::Connect);
            }
            let form: HashMap<String, String> = url::form_urlencoded::parse(req.body())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            self.token_forms.lock().unwrap().push(form);
            let body = match (&st.id_token, st.token_status) {
                (Some(t), 200) => {
                    json!({"token_type": "Bearer", "access_token": "ACCESS-TOKEN-DO-NOT-KEEP", "id_token": t})
                }
                _ => {
                    json!({"error": "invalid_grant", "error_description": "AADSTS70008: the code has expired. code=SECRET-CODE"})
                }
            };
            let status = if st.id_token.is_some() {
                st.token_status
            } else {
                400
            };
            return Ok(json_response(status, &body.to_string(), None));
        }
        Ok(json_response(404, "{}", None))
    }
}

/// An `EntraConfig` for the mock, with the client authentication of the caller's choice.
pub fn config(client_auth: ClientAuth) -> EntraConfig {
    EntraConfig {
        tenant_id: TID.parse().unwrap(),
        client_id: CLIENT_ID.parse().unwrap(),
        authority_host: HOST.to_owned(),
        public_origin: ORIGIN.to_owned(),
        redirect_path: "/auth/callback".to_owned(),
        accepted_api_audiences: vec!["api://lanekeeper".to_owned()],
        client_auth,
        login_cookie_key_secret: "login-cookie-key".to_owned(),
    }
}

pub const CLIENT_SECRET: &str = "s3cr3t-client-secret-value";

pub fn secret_auth() -> ClientAuth {
    ClientAuth::Secret {
        name: "entra-client-secret".to_owned(),
    }
}

/// A `SecretSource` holding the client secret and the cookie key.
pub fn secrets() -> Arc<ports::fakes::FakeSecretSource> {
    Arc::new(
        ports::fakes::FakeSecretSource::new()
            .with("entra-client-secret", CLIENT_SECRET)
            .with("login-cookie-key", &URL_SAFE_NO_PAD.encode([5u8; 32])),
    )
}
