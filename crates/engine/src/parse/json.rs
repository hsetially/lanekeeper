//! JSON: json-event-parser's tokens into the tree builder (T2).
//!
//! The tokeniser is strict RFC 8259 and gives no positions, so the engine counts lines in the bytes it consumes. Each
//! call consumes the separators in front of a token (white space, `,` and `:`) and then the token, and a token itself
//! never holds a line break (a raw one in a string is a syntax error), so the token's line is the line of its first byte
//! after the separators. A line break is `\n`, `\r\n` or a lone `\r`, as everywhere in the engine. The column is not
//! tracked (0).

use std::borrow::Cow;

use json_event_parser::{JsonEvent, LowLevelJsonParser, LowLevelJsonParserResult};

use super::Syntax;
use super::build::{TreeBuilder, clamp32};
use super::node::{Position, ScalarStyle, ScalarTag};
use crate::error::ParseError;
use crate::limits::Limits;

pub(super) fn read(text: &str, limits: &Limits, builder: &mut TreeBuilder<'_>) -> Result<(), ParseError> {
    // The engine's own depth check answers first; the tokeniser's stack limit is a second wall behind it.
    let mut parser = LowLevelJsonParser::new().with_max_stack_size(limits.nesting_depth.saturating_add(2));
    let bytes = text.as_bytes();
    let mut offset = 0;
    let mut lines = Lines::default();
    builder.start_document();
    loop {
        let LowLevelJsonParserResult {
            consumed_bytes,
            event,
        } = parser.parse_next(&bytes[offset..], true);
        let region = &bytes[offset..offset + consumed_bytes.min(bytes.len() - offset)];
        offset += region.len();
        let token_line = lines.advance(region);
        let at = Position {
            line: token_line,
            column: 0,
        };
        let Some(event) = event else {
            if region.is_empty() {
                // Nothing consumed and nothing said: the tokeniser is stuck, which only damaged input does.
                return Err(builder.syntax_error(token_line, 0));
            }
            continue;
        };
        let event = event.map_err(|e| {
            let start = e.location().start;
            ParseError::Syntax {
                syntax: Syntax::Json,
                line: clamp32(
                    usize::try_from(start.line)
                        .unwrap_or(usize::MAX)
                        .saturating_add(1),
                ),
                column: clamp32(
                    usize::try_from(start.column)
                        .unwrap_or(usize::MAX)
                        .saturating_add(1),
                ),
            }
        })?;
        match event {
            JsonEvent::StartObject => builder.open_mapping(0, true, at)?,
            JsonEvent::StartArray => builder.open_sequence(0, true, at)?,
            JsonEvent::EndObject | JsonEvent::EndArray => builder.close(token_line)?,
            JsonEvent::ObjectKey(key) | JsonEvent::String(key) => {
                builder.scalar(key, ScalarStyle::Quoted, ScalarTag::None, 0, at, token_line)?;
            }
            JsonEvent::Number(n) => {
                builder.scalar(n, ScalarStyle::Plain, ScalarTag::None, 0, at, token_line)?;
            }
            JsonEvent::Boolean(b) => {
                let text = if b { "true" } else { "false" };
                builder.scalar(
                    Cow::Borrowed(text),
                    ScalarStyle::Plain,
                    ScalarTag::None,
                    0,
                    at,
                    token_line,
                )?;
            }
            JsonEvent::Null => {
                builder.scalar(
                    Cow::Borrowed("null"),
                    ScalarStyle::Plain,
                    ScalarTag::None,
                    0,
                    at,
                    token_line,
                )?;
            }
            JsonEvent::Eof => break,
        }
    }
    builder.end_document(lines.line())
}

/// Counts lines across the regions the tokeniser consumes.
struct Lines {
    line: u32,
    after_cr: bool,
}

impl Default for Lines {
    fn default() -> Self {
        Self {
            line: 1,
            after_cr: false,
        }
    }
}

impl Lines {
    fn line(&self) -> u32 {
        self.line
    }

    /// Moves over `region` and returns the line its token starts on: that of the first byte that is not a separator, or
    /// the line the region ends on if it is all separators.
    fn advance(&mut self, region: &[u8]) -> u32 {
        let mut token_line = None;
        for &b in region {
            if token_line.is_none() && !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b',' | b':') {
                token_line = Some(self.line);
            }
            match b {
                b'\n' => {
                    if !self.after_cr {
                        self.line = self.line.saturating_add(1);
                    }
                    self.after_cr = false;
                }
                b'\r' => {
                    self.line = self.line.saturating_add(1);
                    self.after_cr = true;
                }
                _ => self.after_cr = false,
            }
        }
        token_line.unwrap_or(self.line)
    }
}
