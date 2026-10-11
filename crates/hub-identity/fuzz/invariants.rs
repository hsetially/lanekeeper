//! The properties the identity crate's fuzz targets assert (S1, S2, S3, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/hub-identity/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify`
//! runs) check exactly the same things. A check returns `Err(reason)` for a violation; the fuzz targets turn that into a
//! panic, which libFuzzer reports as a crash.
//!
//! Inputs that a parser rightly rejects are not violations: the checks look at what was accepted.
//!
//! # The targets
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `identity_jwt` | text | whatever the bytes are, parsing a token never panics and refuses more than 16 KiB without looking inside; an accepted header has algorithm RS256 and a key id of 1 to 256 bytes; a token is never verified against a key set that holds no key that signed it (random input is never a valid signature); a key-set document that parses holds 1 to 32 keys; the Entra profile accepts claims only with the `mcp.access` scope, an `nbf` and a GUID `tid` and `oid`, and its identity is exactly `tid:oid`; the Google profile accepts claims only with a verified, non-empty email and a subject |
//! | `identity_cookie` | text | `valid_return_to` accepts only a site-relative path (one leading `/`, no backslash, no control character, at most 512 bytes); a login cookie that was not sealed with the key never opens; the callback query parser never panics; a list cursor is refused or decodes to a key that re-encodes to a cursor that decodes to the same key, and a request cursor that decodes re-encodes to the same text; a `Cookie` header that parses yields only non-empty values made of cookie octets, within the size cap; a session or CSRF token text that is accepted is the one spelling of its 32 bytes; the CSRF check lets a state-changing request with a session through only when the header, the cookie and the stored hash agree and `Origin` is exactly the public origin |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use http::{HeaderMap, HeaderValue, Method, header};
use hub_identity::TokenError;
use hub_identity::csrf::check as csrf_check;
use hub_identity::oidc::jwks::{KeySet, RsaPublicKey};
use hub_identity::oidc::jwt::{Claims, Profile, parse_compact, verify_with_keys};
use hub_identity::oidc::login::{CallbackQuery, LoginSealer, valid_return_to};
use hub_identity::session::cookie::{MAX_COOKIE_VALUE_BYTES, RandomToken, RequestCookies};
use hub_identity::tokens::{entra, google};
use hub_identity::users::cursor;

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

const TOKEN_LIMIT: usize = 16 * 1024;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A key set that holds one key with the id `k`. Its modulus is not a real RSA key, so nothing verifies against it.
fn fixed_keys() -> KeySet {
    let key = RsaPublicKey::from_components(vec![0xC1; 256], vec![1, 0, 1]).expect("a plausible key");
    KeySet::from_keys([("k".to_owned(), key)])
}

pub fn identity_jwt(data: &[u8]) -> Result<(), String> {
    let token = text(data);
    match parse_compact(&token) {
        Ok(parsed) => {
            if token.len() > TOKEN_LIMIT {
                return Err("a token over the limit was parsed".to_owned());
            }
            if parsed.kid.is_empty() || parsed.kid.len() > 256 {
                return Err("an accepted header has a key id of the wrong size".to_owned());
            }
        }
        Err(TokenError::Oversize) => {
            if token.len() <= TOKEN_LIMIT {
                return Err("a token within the limit was called oversize".to_owned());
            }
        }
        Err(_) => {}
    }
    let profile = Profile {
        issuer: "https://issuer.test".to_owned(),
        audiences: vec!["aud".to_owned()],
        tenant: Some("tid".to_owned()),
    };
    if verify_with_keys(&token, &fixed_keys(), &profile, 1_800_000_000).is_ok() {
        return Err("random input was verified as a signed token".to_owned());
    }
    // The same bytes as a key-set document.
    if let Ok(set) = KeySet::parse(data) {
        if set.is_empty() || set.len() > 32 {
            return Err("a key set that parsed holds a wrong number of keys".to_owned());
        }
    }
    token_claims(&token)
}

/// Claims made of the lines of the input, as if a signature had already been checked: what the Entra and Google profiles
/// accept from them (T4).
fn claims_of(text: &str) -> Claims {
    let mut lines = text.lines().map(str::to_owned);
    let mut next = || lines.next();
    let tid = next();
    let oid = next();
    let scp = next();
    let email = next();
    let name = next();
    let subject = next();
    let verified = next();
    let nbf = next();
    Claims {
        issuer: String::new(),
        audience: Vec::new(),
        exp: 0,
        nbf: nbf.and_then(|n| n.parse().ok()),
        nonce: None,
        tid,
        oid,
        subject,
        preferred_username: email.clone(),
        email,
        email_verified: verified.map(|v| v == "true"),
        name,
        scp,
    }
}

fn token_claims(text: &str) -> Result<(), String> {
    let claims = claims_of(text);
    if let Ok(user) = entra::identity_from_claims(&claims) {
        let scope_ok = claims
            .scp
            .as_deref()
            .is_some_and(|s| s.split(' ').any(|x| x == entra::MCP_SCOPE));
        if !scope_ok || claims.nbf.is_none() {
            return Err("an Entra token without the MCP scope or without nbf was accepted".to_owned());
        }
        let tid = claims.tid.as_deref().unwrap_or_default().to_ascii_lowercase();
        let oid = claims.oid.as_deref().unwrap_or_default().to_ascii_lowercase();
        if user.id.to_string() != format!("{tid}:{oid}") {
            return Err("an Entra identity is not exactly tid:oid of the claims".to_owned());
        }
    }
    if let Ok(identity) = google::identity_from_claims(&claims, "aud") {
        if claims.email_verified != Some(true) {
            return Err("a Google token with an unverified email was accepted".to_owned());
        }
        if identity.email.as_str().is_empty() || identity.subject.as_str().is_empty() {
            return Err("a Google identity has an empty email or subject".to_owned());
        }
    }
    Ok(())
}

/// The public origin the CSRF check is run with, and the text of the one CSRF token it accepts (32 zero bytes).
const ORIGIN: &str = "https://lanekeeper.example.com";
const KNOWN_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// Whether a state-changing request with these header values passes the CSRF check for a session whose token is the known one.
fn csrf_passes(origin: &str, token: &str, cookie_header: &str) -> bool {
    let (Ok(origin), Ok(token), Ok(cookie)) = (
        HeaderValue::from_str(origin),
        HeaderValue::from_str(token),
        HeaderValue::from_str(cookie_header),
    ) else {
        return false;
    };
    let mut headers = HeaderMap::new();
    headers.append(header::ORIGIN, origin);
    headers.append("x-csrf-token", token);
    headers.append(header::COOKIE, cookie);
    let Some(known) = RandomToken::from_text(KNOWN_TOKEN) else {
        return false;
    };
    csrf_check(&Method::POST, &headers, ORIGIN, Some(&known.hash())).is_ok()
}

pub fn identity_cookie(data: &[u8]) -> Result<(), String> {
    let value = text(data);
    if valid_return_to(&value) {
        let b = value.as_bytes();
        let ok = b.first() == Some(&b'/')
            && !matches!(b.get(1), Some(b'/' | b'\\'))
            && b.len() <= 512
            && !value.chars().any(|c| c.is_control() || c == '\\');
        if !ok {
            return Err("an unsafe path was accepted as a return_to".to_owned());
        }
    }
    let sealer = LoginSealer::from_key([7u8; 32]);
    if sealer.open(value.trim()).is_ok() {
        return Err("a value that was not sealed with the key opened".to_owned());
    }
    let _ = CallbackQuery::parse(&value);
    // The list cursors come from the client too (T2).
    if let Ok(user) = cursor::decode_user(&value) {
        if cursor::decode_user(&cursor::encode_user(&user)) != Ok(user) {
            return Err("a user cursor did not survive a round trip".to_owned());
        }
    }
    if let Ok(id) = cursor::decode_request(&value) {
        if cursor::encode_request(id) != value {
            return Err("a request cursor was accepted in a non-canonical form".to_owned());
        }
    }
    // The `Cookie` header (T3, S2).
    if let Ok(cookies) = RequestCookies::parse([value.as_str()]) {
        for v in [&cookies.session, &cookies.csrf, &cookies.login]
            .into_iter()
            .flatten()
        {
            let octets = v
                .bytes()
                .all(|b| matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e));
            if v.is_empty() || v.len() > MAX_COOKIE_VALUE_BYTES || !octets {
                return Err(
                    "an accepted cookie value is empty, too long or not made of cookie octets".to_owned(),
                );
            }
        }
    }
    // A session or CSRF token has exactly one spelling.
    if let Some(token) = RandomToken::from_text(&value) {
        if token.to_text().expose() != &value {
            return Err("a token text was accepted in a non-canonical spelling".to_owned());
        }
    }
    // The CSRF check (T3, S3). Made-up header values never pass; only the known token does, and only with the exact origin.
    let cookie = format!("__Host-lk_csrf={value}");
    if csrf_passes(ORIGIN, &value, &cookie) && value != KNOWN_TOKEN {
        return Err("the CSRF check passed with a token that is not the session's".to_owned());
    }
    if csrf_passes(&value, KNOWN_TOKEN, &format!("__Host-lk_csrf={KNOWN_TOKEN}")) && value != ORIGIN {
        return Err("the CSRF check passed with an Origin that is not the public origin".to_owned());
    }
    if !csrf_passes(ORIGIN, KNOWN_TOKEN, &format!("__Host-lk_csrf={KNOWN_TOKEN}")) {
        return Err("the CSRF check refused the known good request".to_owned());
    }
    Ok(())
}
