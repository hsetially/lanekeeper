//! The setting path syntax (`docs/domain-model.md`, "Flattening rules"), written and read.
//!
//! - Mappings give `a.b.c`, sequences give `a.b[0]`.
//! - A key that holds `.`, `[` or `]` is written in brackets and quotes: `a["x.y"]`. So is an empty key and a key with a
//!   control character, because a setting path never holds one. Inside the quotes `\` and `"` are escaped with a
//!   backslash and a control character is written `\u{hex}`.
//! - A first key that starts with `#` or is `$` is quoted too, because `#<n>/` starts a path in a later document of a
//!   multi-document file, and `$` is the path of a file that is a single value.
//! - A file with several YAML documents gets `#<n>/` in front (n counts from 0).
//!
//! `encode_setting_path` and `decode_setting_path` are inverses: whatever the first writes, the second reads back to the
//! same segments. The second also reads a few forms the first never writes (a key in quotes that needs none), so that a
//! person can type a path.

use domain::SettingPath;

/// One step of a setting path. Ordered (keys before indexes, then by value) so that the settings below a path are
/// contiguous in a sorted map, which the merge of `effective` relies on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathSegment {
    /// A key of a mapping.
    Key(String),
    /// An item of a sequence.
    Index(u32),
}

/// A setting path taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedPath {
    /// The document, for a file with several YAML documents.
    pub document: Option<u32>,
    /// Empty for a file that is a single value.
    pub segments: Vec<PathSegment>,
}

/// The path of the value at `segments` in `document` (the document index, when the file has more than one).
pub fn encode_setting_path(document: Option<u32>, segments: &[PathSegment]) -> String {
    let mut out = String::new();
    if let Some(n) = document {
        push_document_prefix(&mut out, n);
    }
    if segments.is_empty() {
        out.push('$');
        return out;
    }
    let base = out.len();
    for segment in segments {
        match segment {
            PathSegment::Key(key) => {
                let first = out.len() == base;
                push_key(&mut out, key, first);
            }
            PathSegment::Index(i) => push_index(&mut out, *i),
        }
    }
    out
}

/// Takes a path apart. `None` for text that `encode_setting_path` could not have written and that is not one of the
/// few forms it accepts as well.
pub fn decode_setting_path(setting: &SettingPath) -> Option<DecodedPath> {
    decode(setting.as_str())
}

pub(super) fn push_document_prefix(out: &mut String, document: u32) {
    use std::fmt::Write;
    // Writing to a `String` cannot fail.
    let _ = write!(out, "#{document}/");
}

/// Appends `[n]`.
pub(super) fn push_index(out: &mut String, index: u32) {
    use std::fmt::Write;
    let _ = write!(out, "[{index}]");
}

/// Appends a key to a path. `first` is true when the path so far is empty (or only the document prefix).
pub(super) fn push_key(out: &mut String, key: &str, first: bool) {
    if needs_quotes(key, first) {
        out.push_str("[\"");
        for c in key.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                c if c.is_control() => {
                    use std::fmt::Write;
                    let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
                }
                c => out.push(c),
            }
        }
        out.push_str("\"]");
    } else {
        if !first {
            out.push('.');
        }
        out.push_str(key);
    }
}

fn needs_quotes(key: &str, first: bool) -> bool {
    key.is_empty()
        || key
            .chars()
            .any(|c| matches!(c, '.' | '[' | ']') || c.is_control())
        || (first && (key.starts_with('#') || key == "$"))
}

fn decode(text: &str) -> Option<DecodedPath> {
    let mut rest = text;
    let mut document = None;
    if let Some(after_hash) = rest.strip_prefix('#') {
        let (digits, after) = after_hash.split_once('/')?;
        document = Some(parse_u32(digits)?);
        rest = after;
    }
    if rest == "$" {
        return Some(DecodedPath {
            document,
            segments: Vec::new(),
        });
    }
    if rest.is_empty() {
        return None;
    }
    let mut segments = Vec::new();
    while !rest.is_empty() {
        let first = segments.is_empty();
        if let Some(inner) = rest.strip_prefix('[') {
            let (segment, after) = if let Some(quoted) = inner.strip_prefix('"') {
                let (key, after_quote) = unquote(quoted)?;
                (PathSegment::Key(key), after_quote.strip_prefix(']')?)
            } else {
                let (digits, after) = inner.split_once(']')?;
                (PathSegment::Index(parse_u32(digits)?), after)
            };
            segments.push(segment);
            rest = after;
        } else {
            let plain = if first { rest } else { rest.strip_prefix('.')? };
            let end = plain.find(['.', '[']).unwrap_or(plain.len());
            let key = &plain[..end];
            if key.is_empty()
                || key.contains(']')
                || key.chars().any(char::is_control)
                || (first && (key.starts_with('#') || key == "$"))
            {
                return None;
            }
            segments.push(PathSegment::Key(key.to_owned()));
            rest = &plain[end..];
        }
    }
    Some(DecodedPath { document, segments })
}

/// All ASCII digits, and a `u32`.
fn parse_u32(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Reads a quoted key whose opening quote is already consumed. Returns the key and what follows the closing quote.
fn unquote(text: &str) -> Option<(String, &str)> {
    let mut key = String::new();
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((key, &text[i + 1..])),
            '\\' => match chars.next()?.1 {
                '\\' => key.push('\\'),
                '"' => key.push('"'),
                'u' => {
                    if chars.next()?.1 != '{' {
                        return None;
                    }
                    let mut hex = String::new();
                    loop {
                        let (_, h) = chars.next()?;
                        if h == '}' {
                            break;
                        }
                        if !h.is_ascii_hexdigit() || hex.len() >= 6 {
                            return None;
                        }
                        hex.push(h);
                    }
                    key.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                _ => return None,
            },
            c => key.push(c),
        }
    }
    None
}
