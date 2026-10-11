//! The authorization-code flow with PKCE for the web UI (S1, S7): `begin` makes the redirect to Entra, `complete` handles
//! the callback.
//!
//! **Login state** is one sealed cookie, `__Host-lk_login`: `{state, nonce, PKCE verifier, return_to, expiry}` in
//! AES-256-GCM (aws-lc-rs, a random 96-bit nonce, the cookie name as associated data). The key is a secret from `SecretSource`,
//! so any replica can finish a login another began and no table is needed (decision B3). It lives 10 minutes and the callback
//! clears it. A code is single use at Entra, and the ID token's nonce must equal the cookie's, so a token from an earlier login
//! fails (residual risk R3 in the threat model: there is no server-side one-time record).
//!
//! The callback checks, in order: no `error` from the provider, `code` and `state` present, the cookie opens and has not
//! expired, `state` equals the cookie's (constant time), the code exchange succeeds, the ID token verifies (signature, issuer,
//! audience, tenant, times), the nonce equals the cookie's (constant time), and `tid` and `oid` are GUIDs.

use std::sync::Arc;

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::{Guid, Secret, ShortText, Timestamp, UserId};
use ports::SecretSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use crate::clock::Clock;
use crate::config::EntraConfig;
use crate::error::{IdentityError, TokenError};
use crate::http::{HttpsClient, HttpsRequest};
use crate::oidc::client_auth::ClientCredentials;
use crate::oidc::discovery::DiscoveryCache;
use crate::oidc::jwks::JwksCache;
use crate::oidc::jwt::{JwtVerifier, Profile};

/// The cookie that carries the login state. Also its associated data, so a sealed value cannot be replayed as another cookie.
pub const LOGIN_COOKIE_NAME: &str = "__Host-lk_login";
/// How long a login may take, from the redirect to the callback.
pub const LOGIN_LIFETIME_SECS: i64 = 10 * 60;
/// The longest cookie value opened, and the longest callback query read.
const MAX_COOKIE_BYTES: usize = 2048;
const MAX_QUERY_BYTES: usize = 4096;
const MAX_CODE_BYTES: usize = 4096;
const MAX_STATE_BYTES: usize = 128;
const MAX_RETURN_TO: usize = 512;
const TOKEN_SCOPE: &str = "openid profile email";

/// A `return_to` the callback may redirect to: a path on this site, nothing else.
///
/// It starts with `/`, then not `/` or `\` (that would be a protocol-relative URL), has no backslash or control character,
/// and is at most 512 bytes. Anything else is replaced by `/`.
pub fn valid_return_to(s: &str) -> bool {
    let b = s.as_bytes();
    b.first() == Some(&b'/')
        && !matches!(b.get(1), Some(b'/' | b'\\'))
        && b.len() <= MAX_RETURN_TO
        && !s.chars().any(|c| c.is_control() || c == '\\')
}

fn random_token() -> Result<String, IdentityError> {
    let mut buf = [0u8; 32];
    aws_lc_rs::rand::fill(&mut buf).map_err(|_| IdentityError::Crypto)?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// What the cookie holds, after it has been opened.
pub struct LoginState {
    pub state: String,
    pub nonce: String,
    pub verifier: Secret<String>,
    pub return_to: String,
    pub expires: Timestamp,
}

impl std::fmt::Debug for LoginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginState").finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct Wire {
    s: String,
    n: String,
    v: String,
    r: String,
    x: i64,
}

/// A sealed login cookie value, ready for `Set-Cookie`.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedLoginCookie(String);

impl SealedLoginCookie {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SealedLoginCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealedLoginCookie([redacted])")
    }
}

/// Seals and opens [`LoginState`] with a 32-byte key.
pub struct LoginSealer {
    key: Secret<[u8; 32]>,
}

impl std::fmt::Debug for LoginSealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginSealer").finish_non_exhaustive()
    }
}

impl LoginSealer {
    /// From the secret's text: 32 bytes as base64url, with or without padding.
    pub fn from_secret(material: &Secret<String>) -> Result<Self, IdentityError> {
        let text = material.expose().trim().trim_end_matches('=');
        let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| IdentityError::Crypto)?;
        let key: [u8; 32] = bytes.try_into().map_err(|_| IdentityError::Crypto)?;
        Ok(Self::from_key(key))
    }

    pub fn from_key(key: [u8; 32]) -> Self {
        Self {
            key: Secret::new(key),
        }
    }

    fn aead(&self) -> Result<LessSafeKey, IdentityError> {
        UnboundKey::new(&AES_256_GCM, self.key.expose())
            .map(LessSafeKey::new)
            .map_err(|_| IdentityError::Crypto)
    }

    pub fn seal(&self, state: &LoginState) -> Result<SealedLoginCookie, IdentityError> {
        let wire = Wire {
            s: state.state.clone(),
            n: state.nonce.clone(),
            v: state.verifier.expose().clone(),
            r: state.return_to.clone(),
            x: state.expires.unix_millis(),
        };
        let mut buf = serde_json::to_vec(&wire).map_err(|_| IdentityError::Crypto)?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        aws_lc_rs::rand::fill(&mut nonce_bytes).map_err(|_| IdentityError::Crypto)?;
        self.aead()?
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(LOGIN_COOKIE_NAME.as_bytes()),
                &mut buf,
            )
            .map_err(|_| IdentityError::Crypto)?;
        let mut out = Vec::with_capacity(NONCE_LEN + buf.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&buf);
        Ok(SealedLoginCookie(URL_SAFE_NO_PAD.encode(out)))
    }

    /// Open a cookie value. Any failure (size, encoding, tag, JSON) is [`IdentityError::LoginStateInvalid`].
    pub fn open(&self, value: &str) -> Result<LoginState, IdentityError> {
        if value.len() > MAX_COOKIE_BYTES {
            return Err(IdentityError::LoginStateInvalid);
        }
        let raw = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| IdentityError::LoginStateInvalid)?;
        if raw.len() < NONCE_LEN + 16 {
            return Err(IdentityError::LoginStateInvalid);
        }
        let (nonce_bytes, ciphertext) = raw.split_at(NONCE_LEN);
        let nonce =
            Nonce::try_assume_unique_for_key(nonce_bytes).map_err(|_| IdentityError::LoginStateInvalid)?;
        let mut buf = ciphertext.to_vec();
        let plain = self
            .aead()?
            .open_in_place(nonce, Aad::from(LOGIN_COOKIE_NAME.as_bytes()), &mut buf)
            .map_err(|_| IdentityError::LoginStateInvalid)?;
        let wire: Wire = serde_json::from_slice(plain).map_err(|_| IdentityError::LoginStateInvalid)?;
        // The plaintext copy held the PKCE verifier.
        zeroize::Zeroize::zeroize(&mut buf);
        Ok(LoginState {
            state: wire.s,
            nonce: wire.n,
            verifier: Secret::new(wire.v),
            return_to: wire.r,
            expires: Timestamp::from_unix_millis(wire.x),
        })
    }
}

/// The result of [`OidcLogin::begin`].
#[derive(Debug)]
pub struct LoginStart {
    /// Where to send the browser.
    pub redirect_to: Url,
    /// The value of the `__Host-lk_login` cookie, to set with `Max-Age` of [`LOGIN_LIFETIME_SECS`].
    pub cookie: SealedLoginCookie,
}

/// The query of `/auth/callback`. Duplicate parameters are refused.
pub struct CallbackQuery {
    code: Option<Secret<String>>,
    state: Option<String>,
    error: bool,
}

impl std::fmt::Debug for CallbackQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackQuery").finish_non_exhaustive()
    }
}

impl CallbackQuery {
    pub fn parse(raw: &str) -> Result<Self, IdentityError> {
        if raw.len() > MAX_QUERY_BYTES {
            return Err(IdentityError::BadRequest);
        }
        let (mut code, mut state, mut error) = (None, None, false);
        for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
            match k.as_ref() {
                "code" if code.is_none() => code = Some(Secret::new(v.into_owned())),
                "state" if state.is_none() => state = Some(v.into_owned()),
                "error" => error = true,
                "code" | "state" => return Err(IdentityError::BadRequest),
                _ => {}
            }
        }
        Ok(Self { code, state, error })
    }
}

/// A user who completed Entra sign-in. Identity is `tid` plus `oid`; email and name are for display only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub user: UserId,
    pub email: Option<ShortText>,
    pub display_name: Option<ShortText>,
    /// A path on this site, already checked by [`valid_return_to`].
    pub return_to: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
}

/// The sign-in flow. One per hub process, shared by all requests.
pub struct OidcLogin {
    cfg: EntraConfig,
    http: Arc<dyn HttpsClient>,
    discovery: Arc<DiscoveryCache>,
    jwt: JwtVerifier,
    credentials: ClientCredentials,
    sealer: LoginSealer,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for OidcLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcLogin")
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

impl OidcLogin {
    /// Validate the configuration and read the cookie key. Refuse to start on any error. Nothing is fetched from Entra yet.
    pub async fn new(
        cfg: EntraConfig,
        http: Arc<dyn HttpsClient>,
        secrets: Arc<dyn SecretSource>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, IdentityError> {
        cfg.validate()?;
        let sealer = LoginSealer::from_secret(&secrets.get(&cfg.login_cookie_key_secret).await?)?;
        let discovery = Arc::new(DiscoveryCache::new(&cfg, Arc::clone(&http), Arc::clone(&clock))?);
        let jwks = Arc::new(JwksCache::new(
            Arc::clone(&http),
            Arc::clone(&discovery) as Arc<_>,
            Arc::clone(&clock),
        ));
        Ok(Self {
            jwt: JwtVerifier::new(jwks, Arc::clone(&clock)),
            credentials: ClientCredentials::new(cfg.client_auth.clone(), secrets),
            cfg,
            http,
            discovery,
            sealer,
            clock,
        })
    }

    /// The verifier for this tenant's tokens, shared with the MCP token check (T4).
    pub fn verifier(&self) -> &JwtVerifier {
        &self.jwt
    }

    /// Step 1: the redirect to Entra and the cookie to set. An invalid `return_to` becomes `/`.
    pub async fn begin(&self, return_to: Option<&str>) -> Result<LoginStart, IdentityError> {
        let discovery = self.discovery.get().await?;
        let verifier = random_token()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_token()?;
        let nonce = random_token()?;
        let return_to = return_to.filter(|r| valid_return_to(r)).unwrap_or("/").to_owned();
        let expires = Timestamp::from_unix_millis(
            self.clock
                .now()
                .unix_millis()
                .saturating_add(LOGIN_LIFETIME_SECS * 1000),
        );
        let cookie = self.sealer.seal(&LoginState {
            state: state.clone(),
            nonce: nonce.clone(),
            verifier: Secret::new(verifier),
            return_to,
            expires,
        })?;
        let mut redirect_to = discovery.authorization_endpoint.clone();
        redirect_to
            .query_pairs_mut()
            .clear()
            .append_pair("client_id", self.cfg.client_id.as_str())
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &self.cfg.redirect_uri())
            .append_pair("response_mode", "query")
            .append_pair("scope", TOKEN_SCOPE)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(LoginStart { redirect_to, cookie })
    }

    /// Step 2: the callback. `cookie` is the value of `__Host-lk_login`, if the browser sent one. The caller clears the
    /// cookie on every outcome.
    pub async fn complete(
        &self,
        query: &CallbackQuery,
        cookie: Option<&str>,
    ) -> Result<SignIn, IdentityError> {
        if query.error {
            return Err(IdentityError::BadRequest);
        }
        let (Some(code), Some(state)) = (&query.code, &query.state) else {
            return Err(IdentityError::BadRequest);
        };
        if code.expose().is_empty() || code.expose().len() > MAX_CODE_BYTES || state.len() > MAX_STATE_BYTES {
            return Err(IdentityError::BadRequest);
        }
        let login = self
            .sealer
            .open(cookie.ok_or(IdentityError::LoginStateInvalid)?)?;
        if self.clock.now() >= login.expires {
            return Err(IdentityError::LoginStateExpired);
        }
        if !bool::from(state.as_bytes().ct_eq(login.state.as_bytes())) {
            return Err(IdentityError::StateMismatch);
        }

        let id_token = self.exchange(code, &login.verifier).await?;
        let claims = self
            .jwt
            .verify(
                id_token.expose(),
                &Profile {
                    issuer: self.cfg.issuer(),
                    audiences: vec![self.cfg.client_id.to_string()],
                    tenant: Some(self.cfg.tenant_id.to_string()),
                },
            )
            .await?;
        let nonce = claims.nonce.as_deref().ok_or(TokenError::MissingClaim)?;
        if !bool::from(nonce.as_bytes().ct_eq(login.nonce.as_bytes())) {
            return Err(TokenError::NonceMismatch.into());
        }
        let tid = claims.tid.as_deref().ok_or(TokenError::MissingClaim)?;
        let oid = claims.oid.as_deref().ok_or(TokenError::MissingClaim)?;
        let user = UserId::new(
            Guid::parse(tid).map_err(|_| TokenError::BadIdentity)?,
            Guid::parse(oid).map_err(|_| TokenError::BadIdentity)?,
        );
        // The cookie is authenticated, but a path from it is still checked before it is used as a redirect.
        let return_to = if valid_return_to(&login.return_to) {
            login.return_to.clone()
        } else {
            "/".to_owned()
        };
        Ok(SignIn {
            user,
            email: claims
                .email
                .as_deref()
                .or(claims.preferred_username.as_deref())
                .and_then(|e| ShortText::parse(e).ok()),
            display_name: claims.name.as_deref().and_then(|n| ShortText::parse(n).ok()),
            return_to,
        })
    }

    /// Redeem the code. Only the ID token is read from the answer; an access or refresh token is never kept.
    async fn exchange(
        &self,
        code: &Secret<String>,
        verifier: &Secret<String>,
    ) -> Result<Secret<String>, IdentityError> {
        let discovery = self.discovery.get().await?;
        let credentials = self.credentials.fields().await?;
        let redirect_uri = self.cfg.redirect_uri();
        let mut fields: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("client_id", self.cfg.client_id.as_str()),
            ("code", code.expose()),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", verifier.expose()),
            ("scope", TOKEN_SCOPE),
        ];
        fields.extend(credentials.iter().map(|(k, v)| (*k, v.expose().as_str())));
        let response = self
            .http
            .send(HttpsRequest::post_form(discovery.token_endpoint.clone(), &fields))
            .await?;
        if response.status != 200 {
            tracing::warn!(
                status = response.status,
                "token endpoint refused the authorization code"
            );
            return Err(IdentityError::TokenEndpoint);
        }
        let parsed: TokenResponse =
            serde_json::from_slice(&response.body).map_err(|_| IdentityError::TokenEndpoint)?;
        parsed
            .id_token
            .map(Secret::new)
            .ok_or(IdentityError::TokenEndpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealer() -> LoginSealer {
        LoginSealer::from_key([9u8; 32])
    }

    fn state(expires: i64) -> LoginState {
        LoginState {
            state: "st".to_owned(),
            nonce: "no".to_owned(),
            verifier: Secret::new("ve".to_owned()),
            return_to: "/swimlanes".to_owned(),
            expires: Timestamp::from_unix_millis(expires),
        }
    }

    #[test]
    fn return_to_rejects_absolute_and_protocol_relative_urls() {
        for bad in [
            "",
            "swimlanes",
            "https://evil.example.com",
            "//evil.example.com",
            "/\\evil.example.com",
            "\\\\evil",
            "/a\\b",
            "/a\nb",
            "/a\rLocation: x",
            "javascript:alert(1)",
            " /x",
        ] {
            assert!(!valid_return_to(bad), "{bad:?}");
        }
        assert!(!valid_return_to(&format!("/{}", "a".repeat(MAX_RETURN_TO))));
        for good in ["/", "/swimlanes/sit1?tab=diff", "/a/b#frag", "/%2F%2Fevil"] {
            assert!(valid_return_to(good), "{good:?}");
        }
    }

    #[test]
    fn login_cookie_is_sealed_and_tamper_evident() {
        let sealed = sealer().seal(&state(5)).unwrap();
        let text = sealed.as_str();
        assert!(
            !text.contains("swimlanes"),
            "the state is not readable in the cookie"
        );
        let opened = sealer().open(text).unwrap();
        assert_eq!(
            (
                opened.state.as_str(),
                opened.nonce.as_str(),
                opened.return_to.as_str()
            ),
            ("st", "no", "/swimlanes")
        );
        assert_eq!(opened.verifier.expose(), "ve");
        assert_eq!(opened.expires.unix_millis(), 5);

        // Flip each byte in turn: no change is accepted.
        let raw = URL_SAFE_NO_PAD.decode(text).unwrap();
        for i in 0..raw.len() {
            let mut bad = raw.clone();
            bad[i] ^= 1;
            assert!(sealer().open(&URL_SAFE_NO_PAD.encode(bad)).is_err(), "byte {i}");
        }
        // Truncated, padded, empty and oversize values are refused.
        assert!(sealer().open("").is_err());
        assert!(sealer().open(&text[..text.len() - 2]).is_err());
        assert!(sealer().open(&"A".repeat(MAX_COOKIE_BYTES + 1)).is_err());
    }

    #[test]
    fn login_cookie_of_another_key_is_rejected() {
        let sealed = sealer().seal(&state(5)).unwrap();
        let other = LoginSealer::from_key([8u8; 32]);
        assert_eq!(
            other.open(sealed.as_str()).unwrap_err(),
            IdentityError::LoginStateInvalid
        );
    }

    #[test]
    fn two_seals_of_the_same_state_differ() {
        let a = sealer().seal(&state(5)).unwrap();
        let b = sealer().seal(&state(5)).unwrap();
        assert_ne!(a.as_str(), b.as_str(), "a random nonce per seal");
    }

    #[test]
    fn the_key_secret_must_be_exactly_32_bytes_of_base64url() {
        let ok = Secret::new(URL_SAFE_NO_PAD.encode([1u8; 32]));
        assert!(LoginSealer::from_secret(&ok).is_ok());
        let padded = Secret::new(format!("{}=", URL_SAFE_NO_PAD.encode([1u8; 32])));
        assert!(LoginSealer::from_secret(&padded).is_ok());
        for bad in [
            URL_SAFE_NO_PAD.encode([1u8; 16]),
            URL_SAFE_NO_PAD.encode([1u8; 33]),
            "not base64 !!".to_owned(),
            String::new(),
        ] {
            assert!(LoginSealer::from_secret(&Secret::new(bad)).is_err());
        }
    }

    #[test]
    fn callback_query_parsing_is_strict() {
        assert!(CallbackQuery::parse("code=a&state=b").is_ok());
        assert!(
            CallbackQuery::parse("code=a&code=b&state=b").is_err(),
            "duplicate code"
        );
        assert!(
            CallbackQuery::parse("code=a&state=b&state=c").is_err(),
            "duplicate state"
        );
        assert!(
            CallbackQuery::parse(&"a=b&".repeat(MAX_QUERY_BYTES)).is_err(),
            "oversize"
        );
        assert!(
            CallbackQuery::parse("code=a&state=b&extra=1").is_ok(),
            "unknown parameters are ignored"
        );
    }
}
