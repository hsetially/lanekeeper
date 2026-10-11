//! Token verification (S1, S22): a compact JWS with an RS256 signature, checked in this order and no other.
//!
//! 1. The size, before any parsing ([`ports::MAX_TOKEN_BYTES`]).
//! 2. The shape: exactly three base64url segments, a small JSON header, a plausible RSA signature length.
//! 3. The algorithm: RS256 only. `none`, the HMAC family and everything else are refused, and so is `crit`. The header's `jku`, `jwk`,
//!    `x5u` and `x5c` are never read: the key comes from the issuer's cached key set, by `kid`.
//! 4. The signature, with the key for the `kid`. Nothing in the payload is trusted before this passes.
//! 5. The claims: issuer, audience, tenant, then `exp`, `nbf` and `iat` with 60 s of skew.
//!
//! [`parse_compact`], [`verify_with_keys`] and [`check_claims`] are synchronous so the fuzz targets and the corpus replay can
//! drive them without a network. [`JwtVerifier::verify`] adds the cached key lookup.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ports::MAX_TOKEN_BYTES;
use serde::Deserialize;

use crate::clock::Clock;
use crate::error::{IdentityError, TokenError};
use crate::oidc::jwks::{JwksCache, KeySet, RsaPublicKey};

/// How far a clock may differ from the issuer's before `nbf`, `iat` and `exp` are held against the token.
pub const CLOCK_SKEW_SECS: i64 = 60;
/// The largest decoded header. A real one is under 200 bytes.
const MAX_HEADER_BYTES: usize = 1024;
/// RSA signatures of 2048 to 8192 bit keys.
const SIGNATURE_BYTES: std::ops::RangeInclusive<usize> = 256..=1024;
const MAX_KID_LEN: usize = 256;
const MAX_AUDIENCES: usize = 16;

/// What a profile of tokens must satisfy: the issuer, the accepted audiences and, for Entra, the tenant.
#[derive(Debug, Clone)]
pub struct Profile {
    pub issuer: String,
    pub audiences: Vec<String>,
    /// If set, the `tid` claim must be present and equal to this.
    pub tenant: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
    crit: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Flag {
    Bool(bool),
    Text(String),
}

#[derive(Deserialize)]
struct RawClaims {
    iss: Option<String>,
    aud: Option<Audience>,
    exp: Option<serde_json::Number>,
    nbf: Option<serde_json::Number>,
    iat: Option<serde_json::Number>,
    nonce: Option<String>,
    tid: Option<String>,
    oid: Option<String>,
    sub: Option<String>,
    email: Option<String>,
    email_verified: Option<Flag>,
    preferred_username: Option<String>,
    name: Option<String>,
    scp: Option<String>,
}

/// Claims of a token whose signature, issuer, audience, tenant (if required) and times have been checked.
/// `Debug` shows no value: claims hold personal data.
#[derive(Clone, PartialEq, Eq)]
pub struct Claims {
    pub issuer: String,
    pub audience: Vec<String>,
    pub exp: i64,
    /// `nbf` in seconds, if the token has one (it is checked against the clock either way).
    pub nbf: Option<i64>,
    pub nonce: Option<String>,
    pub tid: Option<String>,
    pub oid: Option<String>,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub preferred_username: Option<String>,
    pub name: Option<String>,
    /// Space-separated scopes of an access token.
    pub scp: Option<String>,
}

impl std::fmt::Debug for Claims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claims")
            .field("exp", &self.exp)
            .finish_non_exhaustive()
    }
}

impl Claims {
    /// True if the space-separated `scp` claim lists `scope`.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scp
            .as_deref()
            .is_some_and(|s| s.split(' ').any(|x| x == scope))
    }
}

/// A token split into its parts, with the header checked and the signature decoded. Nothing is verified yet.
#[derive(Debug)]
pub struct Parsed<'a> {
    pub kid: String,
    /// `header.payload`, the bytes the signature covers.
    signing_input: &'a str,
    payload_b64: &'a str,
    signature: Vec<u8>,
}

/// Steps 1 to 3.
pub fn parse_compact(token: &str) -> Result<Parsed<'_>, TokenError> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(TokenError::Oversize);
    }
    if !token.is_ascii() {
        return Err(TokenError::Malformed);
    }
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(sig_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(TokenError::Malformed);
    };
    if header_b64.is_empty() || payload_b64.is_empty() || sig_b64.is_empty() {
        return Err(TokenError::Malformed);
    }
    if header_b64.len() > MAX_HEADER_BYTES * 4 / 3 + 4 {
        return Err(TokenError::Malformed);
    }
    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|_| TokenError::Malformed)?;
    if header_bytes.len() > MAX_HEADER_BYTES {
        return Err(TokenError::Malformed);
    }
    let header: Header = serde_json::from_slice(&header_bytes).map_err(|_| TokenError::Malformed)?;
    if header.alg != "RS256" || header.crit.is_some() {
        return Err(TokenError::UnsupportedAlgorithm);
    }
    let kid = match header.kid {
        Some(k) if !k.is_empty() && k.len() <= MAX_KID_LEN => k,
        _ => return Err(TokenError::MissingKeyId),
    };
    let signature = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| TokenError::Malformed)?;
    if !SIGNATURE_BYTES.contains(&signature.len()) {
        return Err(TokenError::Malformed);
    }
    let signing_len = header_b64.len() + 1 + payload_b64.len();
    Ok(Parsed {
        kid,
        signing_input: &token[..signing_len],
        payload_b64,
        signature,
    })
}

// The cast is the point: a NumericDate with a fraction counts as its whole seconds, and the range is checked first.
#[allow(clippy::cast_possible_truncation)]
fn as_seconds(n: &serde_json::Number) -> Option<i64> {
    n.as_i64().or_else(|| {
        let f = n.as_f64()?;
        // NumericDate may carry fractions; only finite values within i64 are accepted.
        (f.is_finite() && f.abs() < 9.0e15).then(|| f.floor() as i64)
    })
}

/// Step 5, on an already authenticated payload.
pub fn check_claims(payload_b64: &str, profile: &Profile, now_secs: i64) -> Result<Claims, TokenError> {
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| TokenError::Malformed)?;
    let raw: RawClaims = serde_json::from_slice(&payload).map_err(|_| TokenError::Malformed)?;

    if raw.iss.as_deref() != Some(profile.issuer.as_str()) {
        return Err(TokenError::WrongIssuer);
    }
    let audience = match raw.aud {
        Some(Audience::One(a)) => vec![a],
        Some(Audience::Many(list)) if list.len() <= MAX_AUDIENCES => list,
        Some(Audience::Many(_)) | None => return Err(TokenError::WrongAudience),
    };
    if !audience.iter().any(|a| profile.audiences.contains(a)) {
        return Err(TokenError::WrongAudience);
    }
    if let Some(tenant) = &profile.tenant {
        match raw.tid.as_deref() {
            Some(t) if t.eq_ignore_ascii_case(tenant) => {}
            Some(_) => return Err(TokenError::WrongTenant),
            None => return Err(TokenError::MissingClaim),
        }
    }
    let exp = raw
        .exp
        .as_ref()
        .and_then(as_seconds)
        .ok_or(TokenError::MissingClaim)?;
    if now_secs > exp.saturating_add(CLOCK_SKEW_SECS) {
        return Err(TokenError::Expired);
    }
    for later in [&raw.nbf, &raw.iat].into_iter().flatten() {
        let at = as_seconds(later).ok_or(TokenError::Malformed)?;
        if at > now_secs.saturating_add(CLOCK_SKEW_SECS) {
            return Err(TokenError::NotYetValid);
        }
    }
    let email_verified = raw.email_verified.map(|f| match f {
        Flag::Bool(b) => b,
        Flag::Text(t) => t == "true",
    });
    Ok(Claims {
        issuer: profile.issuer.clone(),
        audience,
        exp,
        nbf: raw.nbf.as_ref().and_then(as_seconds),
        nonce: raw.nonce,
        tid: raw.tid,
        oid: raw.oid,
        subject: raw.sub,
        email: raw.email,
        email_verified,
        preferred_username: raw.preferred_username,
        name: raw.name,
        scp: raw.scp,
    })
}

/// Steps 1 to 5 against a key set already in hand.
pub fn verify_with_keys(
    token: &str,
    keys: &KeySet,
    profile: &Profile,
    now_secs: i64,
) -> Result<Claims, TokenError> {
    let parsed = parse_compact(token)?;
    let key = keys.get(&parsed.kid).ok_or(TokenError::UnknownKey)?;
    finish(&parsed, key, profile, now_secs)
}

fn finish(
    parsed: &Parsed<'_>,
    key: &RsaPublicKey,
    profile: &Profile,
    now_secs: i64,
) -> Result<Claims, TokenError> {
    if !key.verify_rs256(parsed.signing_input.as_bytes(), &parsed.signature) {
        return Err(TokenError::BadSignature);
    }
    check_claims(parsed.payload_b64, profile, now_secs)
}

/// Verifies tokens of one issuer against its cached key set.
pub struct JwtVerifier {
    keys: Arc<JwksCache>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for JwtVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtVerifier").finish_non_exhaustive()
    }
}

impl JwtVerifier {
    pub fn new(keys: Arc<JwksCache>, clock: Arc<dyn Clock>) -> Self {
        Self { keys, clock }
    }

    /// Verify `token` against `profile`. The key set is refreshed at most once, and at most every 30 s, for an unknown `kid`.
    pub async fn verify(&self, token: &str, profile: &Profile) -> Result<Claims, IdentityError> {
        let parsed = parse_compact(token)?;
        let key = self.keys.key(&parsed.kid).await?;
        let now_secs = self.clock.now().unix_millis().div_euclid(1000);
        Ok(finish(&parsed, &key, profile, now_secs)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        URL_SAFE_NO_PAD.encode(s)
    }

    fn token(header: &str, payload: &str, sig_len: usize) -> String {
        format!(
            "{}.{}.{}",
            b64(header),
            b64(payload),
            URL_SAFE_NO_PAD.encode(vec![7u8; sig_len])
        )
    }

    #[test]
    fn oversize_tokens_are_refused_before_parsing() {
        let huge = "a".repeat(MAX_TOKEN_BYTES + 1);
        assert_eq!(parse_compact(&huge).unwrap_err(), TokenError::Oversize);
    }

    #[test]
    fn shape_errors_are_malformed() {
        for t in ["", "a", "a.b", "a.b.c.d", "..", "a..c", "é.b.c", "!!.b.c"] {
            assert_eq!(parse_compact(t).unwrap_err(), TokenError::Malformed, "{t:?}");
        }
        // Valid base64 that is not JSON.
        assert_eq!(
            parse_compact(&token("nope", "{}", 256)).unwrap_err(),
            TokenError::Malformed
        );
        // A signature too short to be RSA.
        assert_eq!(
            parse_compact(&token(r#"{"alg":"RS256","kid":"k"}"#, "{}", 32)).unwrap_err(),
            TokenError::Malformed
        );
    }

    #[test]
    fn only_rs256_with_a_kid_gets_past_the_header() {
        for alg in ["none", "HS256", "HS512", "RS384", "ES256", "PS256", "rs256", ""] {
            let t = token(&format!(r#"{{"alg":"{alg}","kid":"k"}}"#), "{}", 256);
            assert_eq!(
                parse_compact(&t).unwrap_err(),
                TokenError::UnsupportedAlgorithm,
                "alg {alg:?}"
            );
        }
        let crit = token(r#"{"alg":"RS256","kid":"k","crit":["x"],"x":1}"#, "{}", 256);
        assert_eq!(
            parse_compact(&crit).unwrap_err(),
            TokenError::UnsupportedAlgorithm
        );
        for h in [
            r#"{"alg":"RS256"}"#,
            r#"{"alg":"RS256","kid":""}"#,
            r#"{"alg":"RS256","kid":null}"#,
        ] {
            assert_eq!(
                parse_compact(&token(h, "{}", 256)).unwrap_err(),
                TokenError::MissingKeyId,
                "{h}"
            );
        }
        assert_eq!(
            parse_compact(&token(r#"{"alg":"RS256","kid":"k"}"#, "{}", 256))
                .unwrap()
                .kid,
            "k"
        );
    }

    fn profile() -> Profile {
        Profile {
            issuer: "https://issuer.test".to_owned(),
            audiences: vec!["aud-1".to_owned()],
            tenant: Some("tid-1".to_owned()),
        }
    }

    fn claims_b64(json: &str) -> String {
        b64(json)
    }

    #[test]
    fn claim_checks_each_have_their_own_error() {
        let now = 1_000_000;
        let ok = format!(
            r#"{{"iss":"https://issuer.test","aud":"aud-1","tid":"tid-1","exp":{},"nbf":{now},"iat":{now}}}"#,
            now + 60
        );
        assert!(check_claims(&claims_b64(&ok), &profile(), now).is_ok());

        let cases: [(&str, TokenError); 8] = [
            (
                r#"{"iss":"https://other","aud":"aud-1","tid":"tid-1","exp":2000000}"#,
                TokenError::WrongIssuer,
            ),
            (
                r#"{"aud":"aud-1","tid":"tid-1","exp":2000000}"#,
                TokenError::WrongIssuer,
            ),
            (
                r#"{"iss":"https://issuer.test","aud":"nope","tid":"tid-1","exp":2000000}"#,
                TokenError::WrongAudience,
            ),
            (
                r#"{"iss":"https://issuer.test","tid":"tid-1","exp":2000000}"#,
                TokenError::WrongAudience,
            ),
            (
                r#"{"iss":"https://issuer.test","aud":"aud-1","tid":"tid-2","exp":2000000}"#,
                TokenError::WrongTenant,
            ),
            (
                r#"{"iss":"https://issuer.test","aud":"aud-1","exp":2000000}"#,
                TokenError::MissingClaim,
            ),
            (
                r#"{"iss":"https://issuer.test","aud":"aud-1","tid":"tid-1"}"#,
                TokenError::MissingClaim,
            ),
            (
                r#"{"iss":"https://issuer.test","aud":"aud-1","tid":"tid-1","exp":2000000,"nbf":"soon"}"#,
                TokenError::Malformed,
            ),
        ];
        for (json, want) in cases {
            assert_eq!(
                check_claims(&claims_b64(json), &profile(), now).unwrap_err(),
                want,
                "{json}"
            );
        }
    }

    #[test]
    fn time_claims_allow_sixty_seconds_of_skew_and_no_more() {
        let now = 1_000_000;
        let with = |extra: &str| {
            claims_b64(&format!(
                r#"{{"iss":"https://issuer.test","aud":["x","aud-1"],"tid":"tid-1"{extra}}}"#
            ))
        };
        // exp: valid until 60 s after it.
        assert!(check_claims(&with(&format!(r#","exp":{}"#, now - 60)), &profile(), now).is_ok());
        assert_eq!(
            check_claims(&with(&format!(r#","exp":{}"#, now - 61)), &profile(), now).unwrap_err(),
            TokenError::Expired
        );
        // nbf and iat: valid from 60 s before.
        let exp = format!(r#","exp":{}"#, now + 600);
        assert!(check_claims(&with(&format!(r#"{exp},"nbf":{}"#, now + 60)), &profile(), now).is_ok());
        assert_eq!(
            check_claims(&with(&format!(r#"{exp},"nbf":{}"#, now + 61)), &profile(), now).unwrap_err(),
            TokenError::NotYetValid
        );
        assert_eq!(
            check_claims(&with(&format!(r#"{exp},"iat":{}"#, now + 3600)), &profile(), now).unwrap_err(),
            TokenError::NotYetValid
        );
        // Fractional dates are accepted; an absurd one is not a date.
        assert!(check_claims(&with(&format!(r#","exp":{}.5"#, now + 5)), &profile(), now).is_ok());
        assert_eq!(
            check_claims(&with(r#","exp":1e300"#), &profile(), now).unwrap_err(),
            TokenError::MissingClaim
        );
    }

    #[test]
    fn scope_matching_is_by_whole_word() {
        let c = Claims {
            issuer: String::new(),
            audience: vec![],
            exp: 0,
            nbf: None,
            nonce: None,
            tid: None,
            oid: None,
            subject: None,
            email: None,
            email_verified: None,
            preferred_username: None,
            name: None,
            scp: Some("openid mcp.access.extra mcp.access".to_owned()),
        };
        assert!(c.has_scope("mcp.access"));
        assert!(!c.has_scope("mcp"));
        assert!(!c.has_scope("mcp.acc"));
    }
}
