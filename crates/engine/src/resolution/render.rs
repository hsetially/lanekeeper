//! Rendering a resource the way the config-server does (D82, D86): every `${KEY}` whose key is in the property view is
//! replaced with its value. Any other `${...}` stays exactly as written; the service resolves it later from its own
//! environment, and the engine never reads an environment (C10 knows environment variable *names* only).
//!
//! What counts as a placeholder is narrow on purpose (plan decision A10): `${`, a key of `[A-Za-z0-9_.\-\[\]]+`, `}`, with
//! no `$` right before the opener. `${KEY:default}`, `${${KEY}}`, `$${KEY}` and an opener that never closes are left as
//! written, as a whole, and are not placeholders for C10 either. A value that is inserted is not expanded again.
//!
//! The scan is one pass and bounded: an opener that is not a plain placeholder looks at most `MAX_EXPRESSION_BYTES` ahead for
//! its closing bracket (an expression longer than that is not skipped as a whole, only its opener is left alone), so the
//! work is at most that many steps per byte of input.

use bytes::Bytes;
use domain::{ContentHash, FileClass};

use super::property_view::PropertyView;
use crate::bytes::{decode_text, encode_text};
use crate::error::{LimitKind, ResolveError};
use crate::limits::Limits;
use crate::tree_hash::normalised_hash;

/// The longest `${...}` expression that is skipped as a whole when it is not a plain placeholder.
const MAX_EXPRESSION_BYTES: usize = 256;

/// A placeholder in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub key: String,
    /// The line it is on, from 1. A line ends at `\n`, `\r\n` or a lone `\r`.
    pub line: u32,
}

/// The result of rendering a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// How many placeholders were replaced.
    pub replaced: usize,
    /// The keys that were left as written, each once, at the line where it first appears, sorted by key.
    pub unresolved: Vec<Placeholder>,
}

/// A rendered file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFile {
    /// What the service gets: the file's encoding, byte order mark and line endings are kept.
    pub bytes: Bytes,
    pub replaced: usize,
    pub unresolved: Vec<Placeholder>,
    /// `normalised_hash` of `bytes`: two renderings are the same content exactly when this matches.
    pub hash: ContentHash,
}

fn is_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'[' | b']')
}

/// One step of the scan.
enum Step {
    /// A placeholder at `start..end` (the whole `${KEY}`), with its key at `key`.
    Placeholder { end: usize, key: (usize, usize) },
    /// Something that is not a placeholder; continue at `next`.
    Literal { next: usize },
}

/// What is at `at`, where `bytes[at..]` starts with `${`.
fn step(bytes: &[u8], at: usize) -> Step {
    let key_start = at + 2;
    let mut end = key_start;
    while end < bytes.len() && is_key_byte(bytes[end]) {
        end += 1;
    }
    if end > key_start && bytes.get(end) == Some(&b'}') {
        return Step::Placeholder {
            end: end + 1,
            key: (key_start, end),
        };
    }
    // Not a plain placeholder. If it is a balanced `${ ... }` expression, skip the whole of it, so that a default or a nested
    // form is left alone and its inside is not read as a placeholder.
    let limit = bytes.len().min(at + MAX_EXPRESSION_BYTES);
    let mut depth = 0usize;
    let mut i = at;
    while i < limit {
        match bytes[i] {
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                depth += 1;
                i += 2;
                continue;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Step::Literal { next: i + 1 };
                }
            }
            _ => {}
        }
        i += 1;
    }
    Step::Literal { next: at + 2 }
}

/// Walks the text and calls `f(start, end, key, line)` for each placeholder, in order.
fn walk(text: &str, mut f: impl FnMut(usize, usize, &str, u32)) {
    let bytes = text.as_bytes();
    let mut line = 1u32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => line = line.saturating_add(1),
            b'\r' => {
                if bytes.get(i + 1) != Some(&b'\n') {
                    line = line.saturating_add(1);
                }
            }
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                if i > 0 && bytes[i - 1] == b'$' {
                    // `$${`: an escaped opener.
                    i += 2;
                    continue;
                }
                match step(bytes, i) {
                    Step::Placeholder { end, key } => {
                        f(i, end, &text[key.0..key.1], line);
                        i = end;
                        continue;
                    }
                    Step::Literal { next } => {
                        // Lines inside a skipped expression still count.
                        for &b in &bytes[i..next] {
                            if b == b'\n' {
                                line = line.saturating_add(1);
                            }
                        }
                        i = next;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Every placeholder of `text`, in order, with its line.
pub fn scan_placeholders(text: &str) -> Vec<Placeholder> {
    let mut found = Vec::new();
    walk(text, |_, _, key, line| {
        found.push(Placeholder {
            key: key.to_owned(),
            line,
        });
    });
    found
}

/// Replaces every placeholder whose key is in `view`.
///
/// # Errors
///
/// `Limit(InputBytes)` when `text`, or what it renders to, is longer than `Limits::input_bytes`.
pub fn render(text: &str, view: &PropertyView, limits: &Limits) -> Result<Rendered, ResolveError> {
    if text.len() > limits.input_bytes {
        return Err(ResolveError::Limit(LimitKind::InputBytes));
    }
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut replaced = 0;
    let mut unresolved: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    let mut too_long = false;
    walk(text, |start, end, key, line| {
        if too_long {
            return;
        }
        match view.get(key) {
            Some(setting) => {
                out.push_str(&text[copied..start]);
                out.push_str(&setting.value.text);
                copied = end;
                replaced += 1;
                too_long = out.len() > limits.input_bytes;
            }
            None => {
                unresolved.entry(key.to_owned()).or_insert(line);
            }
        }
    });
    if too_long {
        return Err(ResolveError::Limit(LimitKind::InputBytes));
    }
    out.push_str(&text[copied..]);
    if out.len() > limits.input_bytes {
        return Err(ResolveError::Limit(LimitKind::InputBytes));
    }
    Ok(Rendered {
        text: out,
        replaced,
        unresolved: unresolved
            .into_iter()
            .map(|(key, line)| Placeholder { key, line })
            .collect(),
    })
}

/// Renders a file's bytes. A `binary` or `denied` file is returned as it is (the same buffer): it is not text, and a byte
/// sequence that looks like a placeholder in an image is not one. A text or structured file is decoded, rendered and
/// written back in the encoding, byte order mark and line endings it has.
///
/// # Errors
///
/// `Decode` for bytes that are not the text they declare, `Encode` for a rendering that the encoding cannot hold (a value
/// with a character that Latin-1 has no code for), and `Limit` as for `render`.
pub fn render_file(
    data: &Bytes,
    class: FileClass,
    view: &PropertyView,
    limits: &Limits,
) -> Result<RenderedFile, ResolveError> {
    if matches!(class, FileClass::Binary | FileClass::Denied) {
        return Ok(RenderedFile {
            bytes: data.clone(),
            replaced: 0,
            unresolved: Vec::new(),
            hash: normalised_hash(data, class),
        });
    }
    let decoded = decode_text(data, limits)?;
    let rendered = render(&decoded.text, view, limits)?;
    let bytes = if rendered.replaced == 0 {
        // Nothing changed: the very same bytes, not a re-encoding of them.
        data.clone()
    } else {
        encode_text(&rendered.text, decoded.format, limits)?
    };
    Ok(RenderedFile {
        hash: normalised_hash(&bytes, class),
        bytes,
        replaced: rendered.replaced,
        unresolved: rendered.unresolved,
    })
}
