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
//! | `identity_jwt` | text | whatever the bytes are, parsing a token never panics and refuses more than 16 KiB without looking inside; an accepted header has algorithm RS256 and a key id of 1 to 256 bytes; a token is never verified against a key set that holds no key that signed it (random input is never a valid signature); a key-set document that parses holds 1 to 32 keys; claims that `check_claims` accepts (the input read as the claims JSON, and its middle segment read as a token payload, because random tokens never get past the signature) name the profile's issuer, one of its audiences and its tenant, and are neither expired nor from the future beyond the skew; the Entra profile accepts claims only with the `mcp.access` scope, an `nbf` and a GUID `tid` and `oid`, and its identity is exactly `tid:oid`; the Google profile accepts claims only with a verified, non-empty email and a subject |
//! | `identity_audit_event` | text | an event that parses is refused only when its diff is oversize; its canonical JSON parses back to the same event and is stable; a stored row passes the hash check only with the right predecessor, event text and hash; a checkpoint object that parses has a signature of 1 to 128 bytes and survives a round trip; a key or signature that is junk never verifies; the denormalised columns the log writes for an event never differ from its stored text, and text of any shape is judged without a panic |
//! | `identity_cookie` | text | `valid_return_to` accepts only a site-relative path (one leading `/`, no backslash, no control character, at most 512 bytes); a login cookie that was not sealed with the key never opens; the callback query parser never panics; a list cursor is refused or decodes to a key that re-encodes to a cursor that decodes to the same key, and a request cursor that decodes re-encodes to the same text; a `Cookie` header that parses yields only non-empty values made of cookie octets, within the size cap; a session or CSRF token text that is accepted is the one spelling of its 32 bytes; the CSRF check lets a state-changing request with a session through only when the header, the cookie and the stored hash agree and `Origin` is exactly the public origin |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use domain::ContentHash;
use http::{HeaderMap, HeaderValue, Method, header};
use hub_identity::TokenError;
use hub_identity::audit::checkpoint::CheckpointObject;
use hub_identity::audit::columns::differing_columns;
use hub_identity::audit::log::prepare;
use hub_identity::audit::verify::{FailureKind, check_row};
use hub_identity::audit::{CheckpointVerifier, EcdsaP256Verifier};
use hub_identity::csrf::check as csrf_check;
use hub_identity::oidc::jwks::{KeySet, RsaPublicKey};
use hub_identity::oidc::jwt::{Claims, Profile, check_claims, parse_compact, verify_with_keys};
use hub_identity::oidc::login::{CallbackQuery, LoginSealer, valid_return_to};
use hub_identity::session::cookie::{MAX_COOKIE_VALUE_BYTES, RandomToken, RequestCookies};
use hub_identity::tokens::{entra, google};
use hub_identity::users::cursor;
use ports::{AuditError, AuditEvent, chain_hash};

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

/// The time all the claim checks run at.
const NOW: i64 = 1_800_000_000;
/// The allowed clock skew of the claim checks, in seconds (`CLOCK_SKEW_SECS` in `oidc/jwt.rs`).
const SKEW: i64 = 60;

fn claim_profile() -> Profile {
    Profile {
        issuer: "https://issuer.test".to_owned(),
        audiences: vec!["aud".to_owned()],
        tenant: Some("tid".to_owned()),
    }
}

/// Unpadded URL-safe base64, as a token's segments are written.
fn base64url(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize]));
        }
    }
    out
}

/// What `check_claims` says about an input, read two ways: the input as the claims JSON itself (so that the fuzzer's mutations
/// land on the JSON and not on its base64), and, if the input is a three-segment token, its middle segment as the payload.
pub fn claim_outcomes(data: &[u8]) -> Vec<Result<Claims, TokenError>> {
    let profile = claim_profile();
    let mut payloads = vec![base64url(data)];
    let token = text(data);
    let mut parts = token.split('.');
    if let (Some(_), Some(payload), Some(_), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    {
        payloads.push(payload.to_owned());
    }
    payloads
        .iter()
        .map(|payload| check_claims(payload, &profile, NOW))
        .collect()
}

/// What claims that `check_claims` accepted must satisfy.
pub fn claim_invariants(claims: &Claims) -> Result<(), String> {
    let profile = claim_profile();
    if claims.issuer != profile.issuer {
        return Err("accepted claims name another issuer".to_owned());
    }
    if claims.audience.is_empty() || !claims.audience.iter().any(|a| profile.audiences.contains(a)) {
        return Err("accepted claims name none of the audiences".to_owned());
    }
    let tenant = profile.tenant.as_deref().unwrap_or_default();
    if !claims
        .tid
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case(tenant))
    {
        return Err("accepted claims are from another tenant, or from none".to_owned());
    }
    if NOW > claims.exp.saturating_add(SKEW) {
        return Err("expired claims were accepted".to_owned());
    }
    if claims.nbf.is_some_and(|nbf| nbf > NOW.saturating_add(SKEW)) {
        return Err("claims that are not yet valid were accepted".to_owned());
    }
    Ok(())
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
    let profile = claim_profile();
    if verify_with_keys(&token, &fixed_keys(), &profile, NOW).is_ok() {
        return Err("random input was verified as a signed token".to_owned());
    }
    // Random input never gets past the signature above, so the claim checks (step 5) are run on it directly.
    for claims in claim_outcomes(data).into_iter().flatten() {
        claim_invariants(&claims)?;
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

/// An event whose columns are held against arbitrary stored text.
const MINIMAL_EVENT: &str = r#"{"at":1791676800000,"actor":{"kind":"system","component":"job"},"action":"settings_changed","via":"system","swimlane":null,"path":null,"hash_before":null,"hash_after":null,"diff":null,"github_login":null,"approval":null,"request_id":null,"attribution":null}"#;

/// The fixed predecessor of the stored-row check.
const PREV: [u8; 32] = [0x5a; 32];

pub fn identity_audit_event(data: &[u8]) -> Result<(), String> {
    // 1. The event, as the hub writes it (S9).
    if let Ok(event) = serde_json::from_slice::<AuditEvent>(data) {
        let too_big = event
            .diff
            .as_ref()
            .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES);
        match prepare(&event) {
            Ok(prepared) => {
                if too_big {
                    return Err("an event with an oversize diff was accepted".to_owned());
                }
                let Ok(again) = serde_json::from_str::<AuditEvent>(prepared.json()) else {
                    return Err("the canonical JSON of an event did not parse".to_owned());
                };
                if again != event {
                    return Err("the canonical JSON parsed to another event".to_owned());
                }
                match prepare(&again) {
                    Ok(p) if p.json() == prepared.json() => {}
                    _ => return Err("the canonical JSON is not stable".to_owned()),
                }
            }
            Err(AuditError::TooLarge) if too_big => {}
            Err(e) => {
                return Err(format!(
                    "an event that parsed was refused for the wrong reason: {e}"
                ));
            }
        }
    }

    // 2. A stored row, judged by its hash alone (S9). The input is the stored text.
    let prev = ContentHash::from_bytes(PREV);
    let good = chain_hash(&prev, data);
    if check_row(&prev, &prev, &good, data).is_err() {
        return Err("a row with the right hash failed the check".to_owned());
    }
    let mut changed_hash = *good.as_bytes();
    changed_hash[(data.len() % 32).min(31)] ^= 1;
    if check_row(&prev, &prev, &ContentHash::from_bytes(changed_hash), data)
        != Err(FailureKind::ChainHashMismatch)
    {
        return Err("a row with a changed hash passed".to_owned());
    }
    if !data.is_empty() {
        let mut changed = data.to_vec();
        let at = usize::from(data[0]) % changed.len();
        changed[at] ^= 1;
        if check_row(&prev, &prev, &good, &changed) != Err(FailureKind::ChainHashMismatch) {
            return Err("a row with a changed event passed".to_owned());
        }
    }
    let other = ContentHash::from_bytes([0xa5; 32]);
    if check_row(&other, &prev, &good, data) != Err(FailureKind::ChainLinkBroken) {
        return Err("a row whose predecessor is not the previous row passed".to_owned());
    }

    // 2b. The columns that copy an event (S9). What the log writes for an event never differs from the event's stored text, and
    // text of any shape is judged without a panic.
    if let Ok(event) = serde_json::from_slice::<AuditEvent>(data) {
        if let Ok(prepared) = prepare(&event) {
            if let Some(columns) = prepared.columns() {
                let differ = differing_columns(prepared.json(), &columns);
                if !differ.is_empty() {
                    return Err(format!(
                        "the columns written for an event differ from it: {differ}"
                    ));
                }
            }
        }
    }
    if let Ok(minimal) = serde_json::from_str::<AuditEvent>(MINIMAL_EVENT) {
        if let Some(columns) = prepare(&minimal).ok().and_then(|p| p.columns()) {
            let _ = differing_columns(&text(data), &columns);
        }
    }

    // 3. A checkpoint object, as the bucket returns it (S9).
    if let Ok(object) = CheckpointObject::parse(data) {
        if object.signature.is_empty() || object.signature.len() > 128 {
            return Err("a checkpoint object with a signature of a wrong size was accepted".to_owned());
        }
        let Ok(bytes) = object.encode() else {
            return Err("an accepted checkpoint object could not be encoded".to_owned());
        };
        if CheckpointObject::parse(&bytes).as_ref() != Ok(&object) {
            return Err("a checkpoint object did not survive a round trip".to_owned());
        }
    }

    // 4. A public key and a signature (S9). Neither is ever trusted for being well formed.
    let _ = EcdsaP256Verifier::new().with_spki_der("v", data);
    let _ = EcdsaP256Verifier::new().with_spki_pem("v", &text(data));
    if let Ok(v) = EcdsaP256Verifier::new().with_spki_der("v", &SPKI) {
        if v.verify("v", b"message", data) {
            return Err("random bytes verified as a checkpoint signature".to_owned());
        }
    }
    Ok(())
}

/// The `SubjectPublicKeyInfo` of a P-256 key nobody holds the private half of: the generator point.
const SPKI: [u8; 91] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86,
    0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04, 0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42,
    0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40, 0xf2, 0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0,
    0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2, 0x96, 0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e,
    0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e, 0x16, 0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6,
    0x40, 0x68, 0x37, 0xbf, 0x51, 0xf5,
];
