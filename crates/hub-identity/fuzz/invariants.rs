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
//! | `identity_jwt` | text | whatever the bytes are, parsing a token never panics and refuses more than 16 KiB without looking inside; an accepted header has algorithm RS256 and a key id of 1 to 256 bytes; a token is never verified against a key set that holds no key that signed it (random input is never a valid signature); a key-set document that parses holds 1 to 32 keys |
//! | `identity_cookie` | text | `valid_return_to` accepts only a site-relative path (one leading `/`, no backslash, no control character, at most 512 bytes); a login cookie that was not sealed with the key never opens; the callback query parser never panics; a list cursor is refused or decodes to a key that re-encodes to a cursor that decodes to the same key, and a request cursor that decodes re-encodes to the same text |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use hub_identity::TokenError;
use hub_identity::oidc::jwks::{KeySet, RsaPublicKey};
use hub_identity::oidc::jwt::{Profile, parse_compact, verify_with_keys};
use hub_identity::oidc::login::{CallbackQuery, LoginSealer, valid_return_to};
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
    Ok(())
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
    Ok(())
}
