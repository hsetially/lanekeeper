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
                let (at, end_line) = if style == ScalarStyle::Block {
                    let (header, last) = block_scalar_lines(&mut source, &span, value.is_empty());
                    (Position { line: header, ..at }, last)
                } else {
                    // A span that ends at column 0 ends in the line before.
                    let end = span.end;
                    let last = if end.col() == 0 && end.line() > span.start.line() {
                        end.line() - 1
                    } else {
                        end.line()
                    };
                    (at, clamp32(last))
                };
                builder.scalar(value, style, scalar_tag, anchor, at, end_line)?;
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

/// The first and the last line of a block scalar (`|` or `>`), 1-based.
///
/// The parser's span for it covers the text only: it starts at the first line that has text (or, for an empty scalar, at
/// the start of the next token), and ends in front of the next token, which is at column 0 or after that token's
/// indentation. The scalar as a person sees it starts on the line of its indicator (`- |`, `key: |`), which is the last
/// line before the text that has anything on it, and ends on the last line with text.
fn block_scalar_lines(source: &mut SourceLines<'_>, span: &Span, empty: bool) -> (u32, u32) {
    let (start, end) = (span.start, span.end);
    // An empty scalar at the very end of the text has a span that covers its indicator, and then the indicator's line is the
    // one it starts on. (Text that happens to begin with `|` or `>` is not empty.)
    let on_indicator = empty && matches!(source.char_at(start.line(), start.col()), Some('|' | '>'));
    let mut header = if on_indicator {
        start.line()
    } else {
        start.line().saturating_sub(1).max(1)
    };
    while !on_indicator && header > 1 && source.is_blank(header) {
        header -= 1;
    }
    let mut last = if end.col() == 0 || source.prefix_is_blank(end.line(), end.col()) {
        end.line().saturating_sub(1)
    } else {
        end.line()
    };
    // The span includes the blank lines that follow the text; they are not part of it.
    while last > header && source.is_blank(last) {
        last -= 1;
    }
    (clamp32(header), clamp32(last.max(header)))
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
pub(crate) struct SourceLines<'a> {
    text: &'a str,
    starts: Option<Vec<usize>>,
}

impl<'a> SourceLines<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self { text, starts: None }
    }

    /// True when the 1-based `line` holds nothing but blanks.
    pub(crate) fn is_blank(&mut self, line: usize) -> bool {
        self.line_text(line).is_some_and(|l| l.trim().is_empty())
    }

    /// True when the first `columns` characters of the 1-based `line` are blanks: what is at that column is the first thing
    /// on its line.
    pub(crate) fn prefix_is_blank(&mut self, line: usize, columns: usize) -> bool {
        self.line_text(line)
            .is_some_and(|l| l.chars().take(columns).all(char::is_whitespace))
    }

    /// The character at the 0-based `column` of the 1-based `line`.
    pub(crate) fn char_at(&mut self, line: usize, column: usize) -> Option<char> {
        self.line_text(line)?.chars().nth(column)
    }

    fn line_text(&mut self, line: usize) -> Option<&'a str> {
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
        let from = *starts.get(line.wrapping_sub(1))?;
        let to = starts.get(line).copied().unwrap_or(text.len());
        text.get(from..to)
    }
}
