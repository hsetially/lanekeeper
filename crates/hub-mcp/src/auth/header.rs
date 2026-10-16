//! The `Authorization` header of an MCP request (S11, S14).
//!
//! This file has no dependency on the rest of the crate, so the fuzz target `mcp_bearer_header` includes it as it is. The parser is
//! strict (one header, the `Bearer` scheme, one space, a token of the RFC 6750 `b64token` alphabet), bounded (a token over
//! [`MAX_TOKEN_BYTES`] is refused by its length alone, before any character is looked at) and returns a slice of the header, so
//! it allocates nothing and cannot invent a token that was not sent.

use http::{HeaderMap, header};

/// The longest token the server looks at, the same bound as `ports::MAX_TOKEN_BYTES` (a test keeps the two equal).
pub const MAX_TOKEN_BYTES: usize = 16 * 1024;

/// Why the header did not yield a token. The class decides the challenge: a missing header is answered without an error code
/// (RFC 6750 §3.1), the others with `invalid_token`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeaderError {
    /// No `Authorization` header.
    Missing,
    /// More than one. Two credentials are ambiguous, so neither is used.
    Duplicate,
    /// Another scheme (`Basic`, `Token`, ...) or no scheme.
    NotBearer,
    /// A `Bearer` without a usable token: empty, a space or other character in it, not visible ASCII.
    Malformed,
    /// A token longer than [`MAX_TOKEN_BYTES`].
    TooLong,
}

/// The bearer token of `headers`.
///
/// # Errors
///
/// [`HeaderError`] with the class of the problem. No variant carries any of the input.
pub fn bearer(headers: &HeaderMap) -> Result<&str, HeaderError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next().ok_or(HeaderError::Missing)?;
    if values.next().is_some() {
        return Err(HeaderError::Duplicate);
    }
    let bytes = value.as_bytes();
    let space = bytes.iter().position(|&b| b == b' ');
    let (scheme, rest) = match space {
        Some(at) => (&bytes[..at], &bytes[at + 1..]),
        None => (bytes, &bytes[bytes.len()..]),
    };
    if !scheme.eq_ignore_ascii_case(b"bearer") {
        return Err(HeaderError::NotBearer);
    }
    if space.is_none() {
        // The scheme alone.
        return Err(HeaderError::Malformed);
    }
    // The length first: a token this long is refused without looking at a single character of it.
    if rest.len() > MAX_TOKEN_BYTES {
        return Err(HeaderError::TooLong);
    }
    if !is_b64token(rest) {
        return Err(HeaderError::Malformed);
    }
    // `is_b64token` accepted ASCII only, so this is a view of the header's own text.
    value
        .to_str()
        .ok()
        .and_then(|text| text.get(text.len() - rest.len()..))
        .ok_or(HeaderError::Malformed)
}

/// RFC 6750 `b64token`: `1*( ALPHA / DIGIT / "-" / "." / "_" / "~" / "+" / "/" ) *"="`.
fn is_b64token(token: &[u8]) -> bool {
    let body_len = token.iter().position(|&b| b == b'=').unwrap_or(token.len());
    let (body, padding) = token.split_at(body_len);
    !body.is_empty()
        && body
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/'))
        && padding.iter().all(|&b| b == b'=')
}
