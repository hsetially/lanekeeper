//! The cursors this server hands out (S11, D58).
//!
//! A tool that pages gives `next` as `m1.<base64url>` and takes it back as `cursor`. The payload is three lines, `tool`, `skip`
//! and `inner`:
//!
//! - `tool` is the tool that made the cursor. Another tool's cursor is refused, so a cursor cannot be replayed against a query it
//!   was not made for.
//! - `inner` is where the *port's* page began (the port's own keyset cursor), or the last key served for the tools that page in
//!   memory. Empty means the beginning.
//! - `skip` is how many items of that page were already served. A page that the 16 KB cap cut short is asked for again from
//!   `inner`, and the first `skip` items are left out, so that every item appears once and in order whatever the page size and
//!   the cap (`output_cap::paging_is_exact_prop`).
//!
//! Parsing is strict. A cursor is accepted only if it is exactly what [`encode`] would have made, so there is one spelling for
//! each value, and anything else (a padded or re-cased string, a different tool, a skip written with a leading zero) is refused.
//! The cursor is at most 512 characters, the length the contract allows; one that would be longer is not made, and the tool answers
//! `truncated` with a web link instead.
//!
//! This file depends on nothing else of the crate, so the fuzz target `mcp_cursor` can include it as it is.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The prefix of every cursor this server makes.
pub const PREFIX: &str = "m1.";
/// The longest cursor, in characters: the contract's `Cursor` (`maxLength` 512).
pub const MAX_LEN: usize = 512;
/// The most items a cursor may skip: a page is at most 200 items, and a cut page leaves fewer than that behind.
pub const MAX_SKIP: u32 = 200;
/// The longest tool name in a cursor.
const MAX_TAG: usize = 32;

/// A cursor that is not one this server made for this tool. Fixed text, no input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the cursor is not valid")]
pub struct CursorError;

/// What a cursor holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    /// The tool that made it.
    pub tag: String,
    /// The port's cursor, or the last key served. `None`: the beginning.
    pub inner: Option<String>,
    /// How many items from `inner` were already served.
    pub skip: u32,
}

fn tag_ok(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= MAX_TAG && tag.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// Printable ASCII, no space: what a port cursor and a key (a swimlane id, `namespace/name`) are made of.
fn inner_ok(inner: &str) -> bool {
    inner.len() <= MAX_LEN && inner.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// Make a cursor, or `None` when it would be longer than [`MAX_LEN`] (or its parts are not allowed).
#[must_use]
pub fn encode(tag: &str, inner: Option<&str>, skip: u32) -> Option<String> {
    if !tag_ok(tag) || skip > MAX_SKIP {
        return None;
    }
    let inner = inner.unwrap_or("");
    if !inner_ok(inner) {
        return None;
    }
    let payload = format!("{tag}\n{skip}\n{inner}");
    let encoded_len = payload.len().div_ceil(3) * 4;
    if PREFIX.len() + encoded_len > MAX_LEN + 2 {
        // Cheap early out; the exact check follows.
        return None;
    }
    let cursor = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(payload));
    (cursor.len() <= MAX_LEN).then_some(cursor)
}

/// Read a cursor that `tag` made.
///
/// # Errors
///
/// [`CursorError`] for anything that is not exactly what [`encode`] makes for `tag`.
pub fn decode(cursor: &str, tag: &str) -> Result<Wrapped, CursorError> {
    if cursor.len() > MAX_LEN || !tag_ok(tag) {
        return Err(CursorError);
    }
    let body = cursor.strip_prefix(PREFIX).ok_or(CursorError)?;
    let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| CursorError)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| CursorError)?;
    let mut parts = text.splitn(3, '\n');
    let (Some(found_tag), Some(skip), Some(inner)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(CursorError);
    };
    if found_tag != tag {
        return Err(CursorError);
    }
    let skip: u32 = skip.parse().map_err(|_| CursorError)?;
    let inner = (!inner.is_empty()).then(|| inner.to_owned());
    // One spelling for each value: re-encoding must give back the cursor that was read.
    if encode(tag, inner.as_deref(), skip).as_deref() != Some(cursor) {
        return Err(CursorError);
    }
    Ok(Wrapped {
        tag: found_tag.to_owned(),
        inner,
        skip,
    })
}
