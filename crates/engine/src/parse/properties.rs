//! `.properties` (T2): the rules of Java's `Properties.load`, which Spring Boot follows.
//!
//! - A line ends with `\n`, `\r\n` or a lone `\r`.
//! - A line whose first non-blank character is `#` or `!` is a comment, and a blank line is ignored.
//! - A line that ends with an odd number of backslashes goes on in the next line, whose leading blanks are dropped.
//! - The key runs to the first unescaped `=`, `:` or blank. Blanks, and at most one `=` or `:`, separate it from the
//!   value, which is the rest of the line, trailing blanks included.
//! - In both, `\t`, `\n`, `\r`, `\f` and `\uXXXX` are decoded (a surrogate pair gives one character), and a backslash
//!   before any other character is dropped. A malformed or lone-surrogate `\u` is a syntax error, as it is for Java.
//!
//! The config-server's own test data writes `LOG_LEVEL: INFO`, which is why `:` and blanks are separators too.

use super::Syntax;
use super::build::Warnings;
use super::node::{ParseWarning, Position, PropertyEntry};
use crate::error::{LimitKind, ParseError};
use crate::limits::Limits;

fn is_blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | 0x0c)
}

/// The physical lines of `text`, each with its 1-based number. `\r\n` is one terminator.
struct Lines<'a> {
    rest: &'a str,
    number: u32,
}

impl<'a> Iterator for Lines<'a> {
    type Item = (u32, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let bytes = self.rest.as_bytes();
        let end = bytes
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
            .unwrap_or(bytes.len());
        let line = &self.rest[..end];
        let terminator = match (bytes.get(end), bytes.get(end + 1)) {
            (Some(b'\r'), Some(b'\n')) => 2,
            (Some(_), _) => 1,
            (None, _) => 0,
        };
        self.rest = &self.rest[end + terminator..];
        let number = self.number;
        self.number = self.number.saturating_add(1);
        Some((number, line))
    }
}

pub(super) fn read(
    text: &str,
    limits: &Limits,
    warnings: &mut Warnings,
) -> Result<Vec<PropertyEntry>, ParseError> {
    let mut lines = Lines {
        rest: text,
        number: 1,
    };
    let mut entries: Vec<PropertyEntry> = Vec::new();
    while let Some((start, first)) = lines.next() {
        let trimmed = first.trim_start_matches([' ', '\t', '\u{c}']);
        if trimmed.is_empty() || trimmed.starts_with(['#', '!']) {
            continue;
        }
        // The logical line: the continuation lines are joined, without their leading blanks.
        let mut logical = String::from(trimmed);
        let mut end = start;
        while ends_with_odd_backslashes(&logical) {
            logical.pop();
            match lines.next() {
                Some((number, next)) => {
                    logical.push_str(next.trim_start_matches([' ', '\t', '\u{c}']));
                    end = number;
                }
                None => break,
            }
            if logical.len() > limits.input_bytes {
                return Err(ParseError::Limit(LimitKind::InputBytes));
            }
        }
        let (raw_key, raw_value) = split(&logical);
        let key = unescape(raw_key).ok_or(ParseError::Syntax {
            syntax: Syntax::Properties,
            line: start,
            column: 1,
        })?;
        let value = unescape(raw_value).ok_or(ParseError::Syntax {
            syntax: Syntax::Properties,
            line: start,
            column: 1,
        })?;
        if key.len() > limits.setting_path_bytes {
            return Err(ParseError::Limit(LimitKind::SettingPathBytes));
        }
        if value.len() > limits.scalar_bytes {
            return Err(ParseError::Limit(LimitKind::ScalarBytes));
        }
        entries.push(PropertyEntry {
            key: key.into_boxed_str(),
            value: value.into_boxed_str(),
            line: start,
            end_line: end,
            overridden: false,
        });
    }
    mark_duplicates(&mut entries, warnings);
    Ok(entries)
}

fn ends_with_odd_backslashes(line: &str) -> bool {
    line.bytes().rev().take_while(|&b| b == b'\\').count() % 2 == 1
}

/// Splits a logical line into the raw key and the raw value, the way `Properties.load` does.
fn split(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let mut key_end = bytes.len();
    let mut value_start = bytes.len();
    let mut has_separator = false;
    let mut escaped = false;
    for (i, &c) in bytes.iter().enumerate() {
        if !escaped && (c == b'=' || c == b':') {
            key_end = i;
            value_start = i + 1;
            has_separator = true;
            break;
        }
        if !escaped && is_blank(c) {
            key_end = i;
            value_start = i + 1;
            break;
        }
        escaped = c == b'\\' && !escaped;
    }
    while value_start < bytes.len() {
        let c = bytes[value_start];
        if !is_blank(c) {
            if !has_separator && (c == b'=' || c == b':') {
                has_separator = true;
            } else {
                break;
            }
        }
        value_start += 1;
    }
    (&line[..key_end], &line[value_start.min(line.len())..])
}

/// Decodes the escapes. `None` for a malformed `\u` or a surrogate that is not half of a pair.
fn unescape(raw: &str) -> Option<String> {
    if !raw.contains('\\') {
        return Some(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // A lone backslash at the very end: Java drops it.
            None => {}
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let unit = hex4(&mut chars)?;
                match unit {
                    0xD800..=0xDBFF => {
                        if chars.next()? != '\\' || chars.next()? != 'u' {
                            return None;
                        }
                        let low = hex4(&mut chars)?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return None;
                        }
                        let scalar = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                        out.push(char::from_u32(scalar)?);
                    }
                    0xDC00..=0xDFFF => return None,
                    _ => out.push(char::from_u32(unit)?),
                }
            }
            Some(other) => out.push(other),
        }
    }
    Some(out)
}

fn hex4(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    let mut value = 0_u32;
    for _ in 0..4 {
        value = value * 16 + chars.next()?.to_digit(16)?;
    }
    Some(value)
}

/// Marks every pair whose key comes again later, and warns about each later one.
fn mark_duplicates(entries: &mut [PropertyEntry], warnings: &mut Warnings) {
    if entries.len() < 2 {
        return;
    }
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_unstable_by(|&a, &b| entries[a].key.cmp(&entries[b].key).then(a.cmp(&b)));
    let mut found: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    while start < order.len() {
        let mut end = start + 1;
        while end < order.len() && entries[order[end]].key == entries[order[start]].key {
            end += 1;
        }
        if end - start > 1 {
            let first = order[start];
            found.extend(order[start + 1..end].iter().map(|&later| (first, later)));
            for &i in &order[start..end - 1] {
                entries[i].overridden = true;
            }
        }
        start = end;
    }
    found.sort_unstable_by_key(|&(_, later)| later);
    for (first, later) in found {
        if !warnings.has_room() {
            warnings.dropped += 1;
            continue;
        }
        // A properties key is its own setting path ("used as they are").
        warnings.push(ParseWarning::DuplicateKey {
            path: domain::SettingPath::parse(&entries[later].key).ok(),
            first: Position {
                line: entries[first].line,
                column: 1,
            },
            second: Position {
                line: entries[later].line,
                column: 1,
            },
        });
    }
}
