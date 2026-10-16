//! The properties the fuzz target `mcp_cursor` asserts (S11, S22): the cursor a paging tool hands out and takes back.
//!
//! Included, with `#[path]`, by `fuzz_targets/mcp_cursor.rs` and by `tests/mcp_auth/corpus_replay.rs`. The includer provides one
//! module at its root, `cursor`, which is `src/output/cursor.rs` (the fuzz target includes the file; the replay re-exports the
//! library's).
//!
//! # The input
//!
//! Text up to the first newline is the tool name the cursor is read for; the rest is read as a cursor. The same bytes also make a
//! cursor to encode: its first byte is the skip count, and the printable ASCII after it (a space ends it) is the inner key.
//!
//! # What must hold
//!
//! | Check | Why |
//! |---|---|
//! | `decode` never panics and gives the same answer twice | a cursor is untrusted input of every paging tool |
//! | a cursor that decodes is exactly what `encode` makes for its parts: one spelling per value | no second spelling to slip past a comparison |
//! | a cursor that decodes was made for the tool it was read for, is at most 512 characters, and skips at most 200 | the contract's `Cursor` and the replay rule |
//! | `decode(encode(x)) == x` for every `x` that can be encoded, and an encoded cursor uses the base64url alphabet only | the two are inverse |
//! | a cursor made for one tool is refused for another | a cursor cannot be replayed against another query |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use super::cursor::{MAX_LEN, MAX_SKIP, PREFIX, Wrapped, decode, encode};

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `mcp_cursor`.
pub fn mcp_cursor(data: &[u8]) -> Result<(), String> {
    let (tag_bytes, rest) = match data.iter().position(|&b| b == b'\n') {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &[][..]),
    };
    let tag = text(tag_bytes);
    let candidate = text(rest);

    // Reading.
    let read = decode(&candidate, &tag);
    if decode(&candidate, &tag) != read {
        return Err("decoding answered differently for the same cursor".to_owned());
    }
    if let Ok(wrapped) = &read {
        if wrapped.tag != tag {
            return Err("a cursor was accepted for a tool that did not make it".to_owned());
        }
        if candidate.len() > MAX_LEN {
            return Err(format!("accepted a cursor of {} characters", candidate.len()));
        }
        if wrapped.skip > MAX_SKIP {
            return Err(format!("accepted a skip of {}", wrapped.skip));
        }
        if encode(&wrapped.tag, wrapped.inner.as_deref(), wrapped.skip).as_deref() != Some(candidate.as_str())
        {
            return Err("an accepted cursor is not the one spelling of its parts".to_owned());
        }
    }

    // Making: the same bytes as the parts of a cursor.
    let (skip, inner_bytes) = rest
        .split_first()
        .map_or((0, &[][..]), |(&b, r)| (u32::from(b), r));
    let inner: String = text(inner_bytes)
        .chars()
        .take_while(|c| c.is_ascii_graphic())
        .collect();
    let inner = (!inner.is_empty()).then_some(inner);
    if let Some(made) = encode(&tag, inner.as_deref(), skip) {
        if made.len() > MAX_LEN {
            return Err(format!("made a cursor of {} characters", made.len()));
        }
        let body = made
            .strip_prefix(PREFIX)
            .ok_or_else(|| "a cursor without the prefix".to_owned())?;
        if !body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("a cursor outside the base64url alphabet".to_owned());
        }
        let expected = Wrapped {
            tag: tag.clone(),
            inner: inner.clone(),
            skip,
        };
        if decode(&made, &tag) != Ok(expected) {
            return Err("decode(encode(x)) is not x".to_owned());
        }
        let other = format!("{tag}x");
        if decode(&made, &other).is_ok() {
            return Err("a cursor was accepted for another tool".to_owned());
        }
    } else if skip <= MAX_SKIP {
        // It can only be refused for its tag, its inner key or its length.
        let tag_fine =
            !tag.is_empty() && tag.len() <= 32 && tag.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
        let inner_len = inner.as_deref().map_or(0, str::len);
        if tag_fine && inner_len <= 300 {
            return Err("refused to make a cursor from small, plain parts".to_owned());
        }
    }
    Ok(())
}
