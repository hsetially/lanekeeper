//! YAML: granit-parser's events into the tree builder (T2, D90).
//!
//! The parser gives events with a position each and knows nothing of trees: anchors and aliases arrive as numbers, so the
//! builder can count what an alias stands for without ever expanding it. The parser also has its own nesting limits; they
//! are set one above the engine's, so the engine's check answers first and names the limit, and the parser's is a second
//! wall behind it.
//!
//! Two things in the parser's output are translated:
//!
//! - an empty value (`a:`) arrives as the text `~` with a span of no width, where the real `~` has one; the engine keeps
//!   the empty text, which is what is written;
//! - a block scalar's span ends at the start of the next token, so its last line is the one before.

use granit_parser::{ErrorKind, Event, Parser, ScalarStyle as YamlStyle, ScanError, Span, StructureStyle};

use super::Syntax;
use super::build::{TreeBuilder, clamp32};
use super::node::{Position, ScalarStyle, ScalarTag};
use crate::error::{LimitKind, ParseError};
use crate::limits::Limits;

pub(super) fn read(text: &str, limits: &Limits, builder: &mut TreeBuilder<'_>) -> Result<(), ParseError> {
    let depth = limits.nesting_depth.saturating_add(1);
    let options = granit_parser::options! {
        emit_comments: false,
        flow_nesting_limit: depth,
        block_nesting_limit: depth,
    };
    let mut parser = Parser::new_from_str_with_options(text, options);
    let mut source = SourceLines::new(text);
    while let Some(next) = parser.next_event() {
        let (event, span) = next.map_err(|e| translate(&e))?;
        let at = start_of(&span);
        match event {
            Event::DocumentStart(..) => builder.start_document(),
            Event::DocumentEnd => builder.end_document(at.line)?,
            Event::Scalar(value, style, anchor, tag) => {
                let implicit_null =
                    style == YamlStyle::Plain && &*value == "~" && span.start.index() == span.end.index();
                let value = if implicit_null { "".into() } else { value };
                let scalar_tag = tag.as_deref().map_or(ScalarTag::None, |t| match t.core_suffix() {
                    Some("str") => ScalarTag::Str,
                    Some("int") => ScalarTag::Int,
                    Some("float") => ScalarTag::Float,
                    Some("bool") => ScalarTag::Bool,
                    Some("null") => ScalarTag::Null,
                    _ => ScalarTag::Other,
                });
                let style = match style {
                    YamlStyle::Plain => ScalarStyle::Plain,
                    YamlStyle::SingleQuoted | YamlStyle::DoubleQuoted => ScalarStyle::Quoted,
                    _ => ScalarStyle::Block,
                };
                // A span that ends at column 0 ends in the line before (a block scalar, up to the next token).
                let end = span.end;
                let mut end_line = if end.col() == 0 && end.line() > span.start.line() {
                    end.line() - 1
                } else {
                    end.line()
                };
                // A block scalar's span includes the blank lines that follow its text; they are not part of it.
                if style == ScalarStyle::Block {
                    while end_line > span.start.line() && source.is_blank(end_line) {
                        end_line -= 1;
                    }
                }
                builder.scalar(value, style, scalar_tag, anchor, at, clamp32(end_line))?;
            }
            Event::Alias(id) => builder.alias(id, at)?,
            Event::SequenceStart(style, anchor, _) => {
                builder.open_sequence(anchor, style == StructureStyle::Flow, at)?;
            }
            Event::MappingStart(style, anchor, _) => {
                builder.open_mapping(anchor, style == StructureStyle::Flow, at)?;
            }
            Event::SequenceEnd | Event::MappingEnd => builder.close(clamp32(span.end.line()))?,
            // The stream's own start and end, and comments (switched off).
            _ => {}
        }
    }
    Ok(())
}

fn start_of(span: &Span) -> Position {
    Position {
        line: clamp32(span.start.line()),
        column: clamp32(span.start.col().saturating_add(1)),
    }
}

/// An error of the parser as the engine reports it: where, and never what.
fn translate(error: &ScanError) -> ParseError {
    let marker = error.marker();
    let line = clamp32(marker.line());
    match error.kind() {
        ErrorKind::RecursionLimitExceeded => ParseError::Limit(LimitKind::Depth),
        ErrorKind::UnknownAnchor => ParseError::UnknownAlias { line },
        _ => ParseError::Syntax {
            syntax: Syntax::Yaml,
            line,
            column: clamp32(marker.col().saturating_add(1)),
        },
    }
}

/// Which lines of the text are blank, found without a pass over the text unless a block scalar asks.
struct SourceLines<'a> {
    text: &'a str,
    starts: Option<Vec<usize>>,
}

impl<'a> SourceLines<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, starts: None }
    }

    /// True when the 1-based `line` holds nothing but blanks.
    fn is_blank(&mut self, line: usize) -> bool {
        let text = self.text;
        let starts = self.starts.get_or_insert_with(|| {
            let bytes = text.as_bytes();
            let mut starts = vec![0];
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                        i += 2;
                        starts.push(i);
                    }
                    b'\n' | b'\r' => {
                        i += 1;
                        starts.push(i);
                    }
                    _ => i += 1,
                }
            }
            starts
        });
        let Some(&from) = starts.get(line.wrapping_sub(1)) else {
            return false;
        };
        let to = starts.get(line).copied().unwrap_or(text.len());
        text[from..to].trim().is_empty()
    }
}
