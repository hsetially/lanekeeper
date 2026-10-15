//! The properties the MCP crate's fuzz targets assert (S7, S11, S14, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify-07` runs)
//! check exactly the same things. The includer provides two modules at its root, `header` and `token`, which are
//! `src/auth/header.rs` and `src/auth/token.rs` (the fuzz targets include the files, the replay re-exports the library's). A check
//! returns `Err(reason)` for a violation; the fuzz targets turn that into a panic, which libFuzzer reports as a crash.
//!
//! Inputs that a parser rightly rejects are not violations: the checks look at what was accepted.
//!
//! # The targets
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `mcp_bearer_header` | bytes, split at newlines into up to four header values | parsing never panics and gives the same answer twice; no header is `Missing` and two or more are `Duplicate`; an accepted token is non-empty, at most 16 KiB, made of the `b64token` alphabet, and is the end of a `Bearer` value of exactly one header; a token over 16 KiB is never accepted; another scheme is never `Ok` |
//! | `mcp_pat_token` | text | parsing never panics; an accepted token is exactly 49 ASCII bytes with the `lkpat_` prefix, is the text it was parsed from, and is the one spelling of its 32 bytes; flipping one bit of the secret gives another valid token with another hash; the hash is stable; a key text that is accepted decodes to 32 to 1024 bytes, and a refused one gets one of three fixed sentences |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderMap, HeaderValue, header as http_header};

use super::header::{HeaderError, MAX_TOKEN_BYTES, bearer};
use super::token::{
    HmacKey, KeyError, MAX_KEY_BYTES, MIN_KEY_BYTES, PREFIX, PersonalToken, SECRET_BYTES, TOKEN_LEN,
    looks_like_personal_token,
};

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn is_b64token(token: &str) -> bool {
    let body_len = token.find('=').unwrap_or(token.len());
    let (body, padding) = token.split_at(body_len);
    !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/'))
        && padding.bytes().all(|b| b == b'=')
}

/// `mcp_bearer_header`.
pub fn mcp_bearer_header(data: &[u8]) -> Result<(), String> {
    let mut map = HeaderMap::new();
    let mut appended = 0_usize;
    let mut first = None;
    for line in data.split(|&b| b == b'\n').take(4) {
        if let Ok(value) = HeaderValue::from_bytes(line) {
            if first.is_none() {
                first = Some(line.to_vec());
            }
            map.append(http_header::AUTHORIZATION, value);
            appended += 1;
        }
    }
    let answer = bearer(&map).map(str::to_owned);
    // The same answer for the same input.
    if bearer(&map).map(str::to_owned) != answer {
        return Err("the parser answered differently for the same headers".to_owned());
    }
    match (&answer, appended) {
        (Err(HeaderError::Missing), n) if n != 0 => return Err(format!("Missing with {n} headers")),
        (Err(other), 0) if *other != HeaderError::Missing => return Err(format!("{other:?} with no header")),
        (Err(HeaderError::Duplicate), n) if n < 2 => return Err(format!("Duplicate with {n} headers")),
        (Ok(_), n) if n != 1 => return Err(format!("accepted with {n} headers")),
        _ => {}
    }
    if appended >= 2 && answer != Err(HeaderError::Duplicate) {
        return Err("two headers were not refused as ambiguous".to_owned());
    }
    if let Ok(token) = &answer {
        let value = first.ok_or("accepted with no header")?;
        let value = String::from_utf8(value).map_err(|_| "accepted a value that is not text".to_owned())?;
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err(format!("accepted a token of {} bytes", token.len()));
        }
        if !is_b64token(token) {
            return Err("accepted a token outside the b64token alphabet".to_owned());
        }
        let prefix = value
            .strip_suffix(token.as_str())
            .ok_or("the token is not the end of the header value")?;
        if !prefix.eq_ignore_ascii_case("bearer ") {
            return Err(format!("accepted a token after the prefix {prefix:?}"));
        }
    }
    Ok(())
}

/// `mcp_pat_token`.
pub fn mcp_pat_token(data: &[u8]) -> Result<(), String> {
    let candidate = text(data);
    if let Some(token) = PersonalToken::parse(&candidate) {
        if token.as_str() != candidate {
            return Err("the parsed token is not the text it was parsed from".to_owned());
        }
        if candidate.len() != TOKEN_LEN || !candidate.is_ascii() || !candidate.starts_with(PREFIX) {
            return Err(format!("accepted a token of {} bytes", candidate.len()));
        }
        if !looks_like_personal_token(&candidate) {
            return Err("a token that parses does not look like one".to_owned());
        }
        // One spelling per secret: decode, re-encode, compare.
        let body = &candidate[PREFIX.len()..];
        let secret = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| "an accepted token body does not decode".to_owned())?;
        if secret.len() != SECRET_BYTES {
            return Err(format!("an accepted token holds {} bytes", secret.len()));
        }
        if URL_SAFE_NO_PAD.encode(&secret) != body {
            return Err("an accepted token is not the canonical spelling of its bytes".to_owned());
        }
        // The hash is stable, depends on the key, and changes with any bit of the secret.
        let key = HmacKey::from_bytes(&[0x42; 32]).map_err(|e| format!("{e}"))?;
        let other_key = HmacKey::from_bytes(&[0x43; 32]).map_err(|e| format!("{e}"))?;
        let hash = key.lookup(&token);
        if key.lookup(&token) != hash {
            return Err("the hash is not stable".to_owned());
        }
        if other_key.lookup(&token) == hash {
            return Err("two keys gave the same hash".to_owned());
        }
        let bit = usize::from(data.iter().fold(0_u8, |acc, b| acc.wrapping_add(*b))) % (SECRET_BYTES * 8);
        let mut flipped = secret.clone();
        flipped[bit / 8] ^= 1 << (bit % 8);
        let flipped_text = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(&flipped));
        let flipped_token =
            PersonalToken::parse(&flipped_text).ok_or("a flipped bit made the token invalid")?;
        if key.lookup(&flipped_token) == hash {
            return Err("a flipped bit left the hash unchanged".to_owned());
        }
    } else if candidate.len() == TOKEN_LEN
        && candidate.is_ascii()
        && candidate.starts_with(PREFIX)
        && URL_SAFE_NO_PAD
            .decode(&candidate[PREFIX.len()..])
            .is_ok_and(|b| b.len() == SECRET_BYTES)
        && URL_SAFE_NO_PAD.encode(
            URL_SAFE_NO_PAD
                .decode(&candidate[PREFIX.len()..])
                .unwrap_or_default(),
        ) == candidate[PREFIX.len()..]
    {
        return Err("refused a token of the right shape".to_owned());
    }

    // The key text: the same bytes read as a secret.
    match HmacKey::from_secret_text(&candidate) {
        Ok(_) => {
            let decoded = URL_SAFE_NO_PAD
                .decode(candidate.trim())
                .map_err(|_| "an accepted key text does not decode".to_owned())?;
            if !(MIN_KEY_BYTES..=MAX_KEY_BYTES).contains(&decoded.len()) {
                return Err(format!("accepted a key of {} bytes", decoded.len()));
            }
        }
        Err(error) => {
            // The message is one of three fixed sentences: it has no room for the text.
            let said = error.to_string();
            let fixed = [
                "the token key is not base64url",
                "the token key is shorter than 32 bytes",
                "the token key is longer than 1024 bytes",
            ];
            if !fixed.contains(&said.as_str()) {
                return Err("an error message that is not one of the fixed sentences".to_owned());
            }
            if !matches!(error, KeyError::Encoding | KeyError::TooShort | KeyError::TooLong) {
                return Err("an unknown error class".to_owned());
            }
        }
    }
    Ok(())
}
