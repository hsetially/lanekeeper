//! YAML locations from tree-sitter (T3, D54, S22).
//!
//! tree-sitter reads the text a second time, with the YAML grammar and none of the parser's code, and its tree is turned
//! into the same kind of document tree the parser builds (through the same `TreeBuilder`, so the alias, merge key and
//! duplicate key rules are not written twice). `flatten` then gives the settings, with the lines tree-sitter puts them on,
//! and the index checks them against the settings of the parser, path by path. Two readers that agree are strong evidence
//! that the ranges are right; a disagreement is reported, never hidden and never a panic.
//!
//! # What tree-sitter is given
//!
//! Only a text that the Rust parser has accepted under every limit, so the C code never sees a file that was refused for
//! its size, depth, alias expansion or a value. It runs on a work budget counted in its own progress callbacks (one per
//! about a hundred of its steps), never on a clock, so the answer does not depend on how busy the machine is. Line breaks
//! are made `\n` first (tree-sitter counts rows by `\n` only, the engine counts `\r\n` and a lone `\r` once each).
//!
//! # What is compared
//!
//! Lines, and only lines: a scalar's text is decoded for keys and for anchored scalars (a path is made of keys, and an
//! alias can stand for a key), and left empty otherwise. Values and types come from the parser.
//!
//! The walk is iterative, with a stack of tasks: how deep the tree is decides how much memory it uses, never how much of
//! the call stack.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::ControlFlow;

use tree_sitter::{Language, Node, ParseOptions, ParseState, Parser, Point, Tree, TreeCursor};

use crate::limits::Limits;
use crate::parse::{
    FlatSetting, Parsed, Position, ScalarStyle, ScalarTag, SourceLines, Syntax, TreeBuilder, Warnings,
    assemble, clamp32, flatten,
};

/// tree-sitter's progress callbacks that every text is allowed, whatever its size.
const CALLS_BASE: usize = 4_096;
/// And one more for every this many bytes of text. The fixture files, small and 6,000 lines long, need one callback for
/// every 40 to 190 bytes (measured on the small and the target scale); the budget is five times the worst of that, so only
/// a text that sends tree-sitter into error recovery can reach it.
const BYTES_PER_CALL: usize = 8;

#[cfg(test)]
thread_local! {
    /// How many times tree-sitter was asked to read a text on this thread (tests only).
    pub(super) static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What the second reader says about a file.
pub(super) enum Outcome {
    /// tree-sitter read the file. One entry for each setting of the parser, in order: does it put the setting on the same
    /// lines?
    Confirmed(Vec<bool>),
    /// tree-sitter could not read the file, or read it as a different shape (another number of documents, an error node).
    Unreadable,
}

/// Reads `text` with tree-sitter and compares what it finds with `settings`, the settings the parser found in the same
/// text. `documents` is the number of YAML documents the parser read.
pub(super) fn confirm(text: &str, documents: usize, settings: &[FlatSetting], limits: &Limits) -> Outcome {
    #[cfg(test)]
    READS.with(|reads| reads.set(reads.get() + 1));
    match read(text, documents, limits) {
        Some(found) => Outcome::Confirmed(compare(settings, &found)),
        None => Outcome::Unreadable,
    }
}

/// One entry for each setting of `parser`: is it in `found` with the same lines?
fn compare(parser: &[FlatSetting], found: &[FlatSetting]) -> Vec<bool> {
    // The common case: the same paths in the same order.
    if parser.len() == found.len() && parser.iter().zip(found).all(|(a, b)| a.path == b.path) {
        return parser
            .iter()
            .zip(found)
            .map(|(a, b)| a.lines == b.lines)
            .collect();
    }
    let by_path: BTreeMap<&str, domain::LineRange> =
        found.iter().map(|s| (s.path.as_str(), s.lines)).collect();
    parser
        .iter()
        .map(|s| by_path.get(s.path.as_str()) == Some(&s.lines))
        .collect()
}

/// The settings tree-sitter finds in `text`, or `None` when it cannot read it as the parser did.
fn read(text: &str, documents: usize, limits: &Limits) -> Option<Vec<FlatSetting>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let unified;
    let text = if has_lone_cr(text) {
        unified = text.replace("\r\n", "\n").replace('\r', "\n");
        unified.as_str()
    } else {
        text
    };
    let tree = parse_tree(text, call_budget(text.len()))?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let parsed = Translator::new(text, limits, root)?.run(root, documents)?;
    flatten(&parsed, limits).ok().map(|flat| flat.settings)
}

/// A `\r` that does not start a `\r\n`.
fn has_lone_cr(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
}

/// The progress callbacks tree-sitter may use to read a text of `len` bytes.
fn call_budget(len: usize) -> usize {
    CALLS_BASE.saturating_add(len / BYTES_PER_CALL)
}

/// The syntax tree of `text`, or `None` when tree-sitter gives up or is stopped after `budget` progress callbacks.
fn parse_tree(text: &str, budget: usize) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&Language::from(tree_sitter_yaml::LANGUAGE))
        .ok()?;
    let mut calls = 0_usize;
    let mut progress = |_: &ParseState| {
        calls += 1;
        if calls > budget {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let bytes = text.as_bytes();
    parser.parse_with_options(
        &mut |offset, _: Point| bytes.get(offset..).unwrap_or_default(),
        None,
        Some(options),
    )
}

fn position(point: Point) -> Position {
    Position {
        line: clamp32(point.row.saturating_add(1)),
        column: clamp32(point.column.saturating_add(1)),
    }
}

/// The 1-based line of the last character of a node: a node that ends at the very start of a line (a block scalar, up to
/// the next token) ends on the line before.
fn last_line(node: Node<'_>) -> u32 {
    let (start, end) = (node.start_position(), node.end_position());
    if end.column == 0 && end.row > start.row {
        clamp32(end.row)
    } else {
        clamp32(end.row.saturating_add(1))
    }
}

/// What is left to do, on a stack.
#[derive(Clone, Copy)]
enum Task<'tree> {
    /// A value or an item: a `block_node` or a `flow_node`.
    Visit(Node<'tree>),
    /// The same, as the key of a mapping: its text is decoded.
    Key(Node<'tree>),
    /// `key: value`: a `block_mapping_pair` or a `flow_pair`.
    Pair(Node<'tree>),
    /// A key with no value: a `flow_node` in a flow mapping (`{a, b: 1}`).
    BareKey(Node<'tree>),
    /// One item of a block sequence.
    Item(Node<'tree>),
    /// A `flow_pair` in a flow sequence: a mapping of one pair.
    SinglePair(Node<'tree>),
    /// An empty value, key or item, at a place.
    Null(Position, u32),
    Close(u32),
}

/// The numbers tree-sitter gives the node kinds and fields the translation looks at. Comparing numbers is much cheaper than
/// comparing names, and there is a node for every token of a 6,000-line file.
struct Kinds {
    document: u16,
    block_node: u16,
    flow_node: u16,
    anchor: u16,
    tag: u16,
    comment: u16,
    plain_scalar: u16,
    single_quote_scalar: u16,
    double_quote_scalar: u16,
    block_scalar: u16,
    alias: u16,
    block_mapping: u16,
    flow_mapping: u16,
    block_sequence: u16,
    flow_sequence: u16,
    block_mapping_pair: u16,
    flow_pair: u16,
    block_sequence_item: u16,
    /// The `:` token (not a named node).
    colon: u16,
    key_field: u16,
    value_field: u16,
}

impl Kinds {
    fn new(language: &Language) -> Option<Self> {
        let named = |kind: &str| language.id_for_node_kind(kind, true);
        Some(Self {
            document: named("document"),
            block_node: named("block_node"),
            flow_node: named("flow_node"),
            anchor: named("anchor"),
            tag: named("tag"),
            comment: named("comment"),
            plain_scalar: named("plain_scalar"),
            single_quote_scalar: named("single_quote_scalar"),
            double_quote_scalar: named("double_quote_scalar"),
            block_scalar: named("block_scalar"),
            alias: named("alias"),
            block_mapping: named("block_mapping"),
            flow_mapping: named("flow_mapping"),
            block_sequence: named("block_sequence"),
            flow_sequence: named("flow_sequence"),
            block_mapping_pair: named("block_mapping_pair"),
            flow_pair: named("flow_pair"),
            block_sequence_item: named("block_sequence_item"),
            colon: language.id_for_node_kind(":", false),
            key_field: language.field_id_for_name("key")?.get(),
            value_field: language.field_id_for_name("value")?.get(),
        })
    }

    fn is_node(&self, kind: u16) -> bool {
        kind == self.block_node || kind == self.flow_node
    }
}

struct Translator<'t, 'l> {
    text: &'t str,
    lines: SourceLines<'t>,
    builder: TreeBuilder<'l>,
    limits: &'l Limits,
    kinds: Kinds,
    /// One cursor for the whole walk: a cursor is a heap allocation, and there is a node for every token.
    cursor: TreeCursor<'t>,
    /// Anchor names to the numbers the builder knows them by. A name defined again means the later one from there on.
    anchors: BTreeMap<&'t str, usize>,
    next_anchor: usize,
}

impl<'t, 'l> Translator<'t, 'l> {
    fn new(text: &'t str, limits: &'l Limits, stream: Node<'t>) -> Option<Self> {
        let kinds = Kinds::new(&Language::from(tree_sitter_yaml::LANGUAGE))?;
        Some(Self {
            text,
            lines: SourceLines::new(text),
            builder: TreeBuilder::new(limits, Syntax::Yaml),
            limits,
            kinds,
            cursor: stream.walk(),
            anchors: BTreeMap::new(),
            next_anchor: 0,
        })
    }

    fn run(mut self, stream: Node<'t>, documents: usize) -> Option<Parsed> {
        let mut found = 0_usize;
        let documents_in_stream: Vec<Node<'t>> = stream
            .named_children(&mut self.cursor)
            .filter(|n| n.kind_id() == self.kinds.document)
            .collect();
        for node in documents_in_stream {
            found += 1;
            self.document(node)?;
        }
        if found != documents {
            return None;
        }
        Some(assemble(
            Syntax::Yaml,
            self.builder,
            Warnings::new(self.limits.warnings),
        ))
    }

    fn document(&mut self, document: Node<'t>) -> Option<()> {
        self.builder.start_document();
        self.anchors.clear();
        let kinds = &self.kinds;
        let root = document
            .named_children(&mut self.cursor)
            .find(|n| kinds.is_node(n.kind_id()));
        let mut stack = vec![match root {
            Some(root) => Task::Visit(root),
            None => Task::Null(position(document.start_position()), last_line(document)),
        }];
        while let Some(task) = stack.pop() {
            self.step(task, &mut stack)?;
        }
        self.builder.end_document(last_line(document)).ok()
    }

    fn step(&mut self, task: Task<'t>, stack: &mut Vec<Task<'t>>) -> Option<()> {
        match task {
            Task::Visit(node) => self.node(node, false, stack),
            Task::Key(node) => self.node(node, true, stack),
            Task::Null(at, end_line) => self.null(at, end_line),
            Task::Close(end_line) => self.builder.close(end_line).ok(),
            Task::Pair(pair) => {
                // Pushed in reverse: the key comes off the stack first.
                stack.push(match pair.child_by_field_id(self.kinds.value_field) {
                    Some(value) => Task::Visit(value),
                    None => Task::Null(position(pair.end_position()), last_line(pair)),
                });
                stack.push(self.key_task(pair));
                Some(())
            }
            Task::BareKey(key) => {
                stack.push(Task::Null(position(key.end_position()), last_line(key)));
                stack.push(Task::Key(key));
                Some(())
            }
            Task::Item(item) => {
                let kinds = &self.kinds;
                let content = item
                    .named_children(&mut self.cursor)
                    .find(|n| kinds.is_node(n.kind_id()));
                stack.push(match content {
                    Some(content) => Task::Visit(content),
                    None => Task::Null(position(item.end_position()), last_line(item)),
                });
                Some(())
            }
            Task::SinglePair(pair) => {
                self.builder
                    .open_mapping(0, true, position(pair.start_position()))
                    .ok()?;
                stack.push(Task::Close(last_line(pair)));
                stack.push(Task::Pair(pair));
                Some(())
            }
        }
    }

    /// The key of a pair, or an empty key. An empty key (`: v`, or `?` and then `: v`) sits at the `:`, as the parser puts it.
    fn key_task(&mut self, pair: Node<'t>) -> Task<'t> {
        if let Some(key) = pair.child_by_field_id(self.kinds.key_field) {
            return Task::Key(key);
        }
        let colon_kind = self.kinds.colon;
        let colon = pair
            .children(&mut self.cursor)
            .find(|c| c.kind_id() == colon_kind);
        let at = colon.unwrap_or(pair).start_position();
        Task::Null(position(at), clamp32(at.row.saturating_add(1)))
    }

    fn null(&mut self, at: Position, end_line: u32) -> Option<()> {
        self.builder
            .scalar(
                Cow::Borrowed(""),
                ScalarStyle::Plain,
                ScalarTag::None,
                0,
                at,
                end_line,
            )
            .ok()
    }

    /// A `block_node` or a `flow_node`: properties (an anchor, a tag) and the content.
    fn node(&mut self, node: Node<'t>, is_key: bool, stack: &mut Vec<Task<'t>>) -> Option<()> {
        let kinds = &self.kinds;
        if !kinds.is_node(node.kind_id()) {
            return None;
        }
        let mut anchor_node = None;
        let mut tagged = false;
        let mut content = None;
        for child in node.named_children(&mut self.cursor) {
            let kind = child.kind_id();
            if kind == kinds.anchor {
                anchor_node = Some(child);
            } else if kind == kinds.tag {
                tagged = true;
            } else if kind != kinds.comment && content.is_none() {
                content = Some(child);
            }
        }
        let anchor = match anchor_node {
            Some(anchor_node) => {
                let name = self.anchor_name(anchor_node)?;
                self.next_anchor += 1;
                self.anchors.insert(name, self.next_anchor);
                self.next_anchor
            }
            None => 0,
        };
        let tag = if tagged { ScalarTag::Other } else { ScalarTag::None };
        let Some(content) = content else {
            // `a: !!null` or `a: &x`: properties and no content, which is an empty value.
            let (at, end) = (position(node.start_position()), last_line(node));
            return self
                .builder
                .scalar(Cow::Borrowed(""), ScalarStyle::Plain, tag, anchor, at, end)
                .ok();
        };
        let at = position(content.start_position());
        let kind = content.kind_id();
        let kinds = &self.kinds;
        if kind == kinds.alias {
            let name = self.alias_name(content)?;
            let target = *self.anchors.get(name)?;
            self.builder.alias(target, at).ok()
        } else if kind == kinds.block_mapping || kind == kinds.flow_mapping {
            self.builder
                .open_mapping(anchor, kind == kinds.flow_mapping, at)
                .ok()?;
            self.push_mapping_children(content, stack);
            Some(())
        } else if kind == kinds.block_sequence || kind == kinds.flow_sequence {
            self.builder
                .open_sequence(anchor, kind == kinds.flow_sequence, at)
                .ok()?;
            self.push_sequence_children(content, stack);
            Some(())
        } else {
            self.scalar(content, is_key || anchor != 0, tag, anchor, at)
        }
    }

    /// A scalar of any style. Its text is decoded when `decode` (a key, or something an alias can stand for) and left empty
    /// otherwise: the lines are what is compared.
    fn scalar(
        &mut self,
        content: Node<'t>,
        decode: bool,
        tag: ScalarTag,
        anchor: usize,
        at: Position,
    ) -> Option<()> {
        let kinds = &self.kinds;
        let kind = content.kind_id();
        let (style, name) = if kind == kinds.plain_scalar {
            (ScalarStyle::Plain, "plain_scalar")
        } else if kind == kinds.block_scalar {
            (ScalarStyle::Block, "block_scalar")
        } else if kind == kinds.single_quote_scalar {
            (ScalarStyle::Quoted, "single_quote_scalar")
        } else if kind == kinds.double_quote_scalar {
            (ScalarStyle::Quoted, "double_quote_scalar")
        } else {
            return None;
        };
        let mut end = last_line(content);
        if style == ScalarStyle::Block {
            // The text of a block scalar ends on its last line that has anything on it.
            while end > at.line && self.lines.is_blank(end as usize) {
                end -= 1;
            }
        }
        let text = if decode {
            scalar_text(name, self.text.get(content.byte_range())?)
        } else {
            Cow::Borrowed("")
        };
        self.builder.scalar(text, style, tag, anchor, at, end).ok()
    }

    /// Closes the mapping after its pairs, and puts the pairs (and the keys with no value) on the stack in order.
    fn push_mapping_children(&mut self, mapping: Node<'t>, stack: &mut Vec<Task<'t>>) {
        stack.push(Task::Close(last_line(mapping)));
        let mark = stack.len();
        let kinds = &self.kinds;
        for child in mapping.named_children(&mut self.cursor) {
            let kind = child.kind_id();
            if kind == kinds.block_mapping_pair || kind == kinds.flow_pair {
                stack.push(Task::Pair(child));
            } else if kind == kinds.flow_node {
                stack.push(Task::BareKey(child));
            }
        }
        stack[mark..].reverse();
    }

    /// Closes the sequence after its items, and puts the items on the stack in order.
    fn push_sequence_children(&mut self, sequence: Node<'t>, stack: &mut Vec<Task<'t>>) {
        stack.push(Task::Close(last_line(sequence)));
        let mark = stack.len();
        let kinds = &self.kinds;
        for child in sequence.named_children(&mut self.cursor) {
            let kind = child.kind_id();
            if kind == kinds.block_sequence_item {
                stack.push(Task::Item(child));
            } else if kind == kinds.flow_node {
                stack.push(Task::Visit(child));
            } else if kind == kinds.flow_pair {
                stack.push(Task::SinglePair(child));
            }
        }
        stack[mark..].reverse();
    }

    fn anchor_name(&self, anchor: Node<'t>) -> Option<&'t str> {
        let text = self.text.get(anchor.byte_range())?;
        Some(text.strip_prefix('&').unwrap_or(text))
    }

    fn alias_name(&self, alias: Node<'t>) -> Option<&'t str> {
        let text = self.text.get(alias.byte_range())?;
        Some(text.strip_prefix('*').unwrap_or(text))
    }
}

/// The text a scalar stands for, as far as a key needs it: quotes taken off, escapes resolved, line breaks folded. A
/// scalar this does not decode exactly (a block scalar used as a key, an escape it does not know) comes out as written,
/// which makes the key differ from the parser's, and the row says so with a warning.
fn scalar_text<'a>(kind: &str, raw: &'a str) -> Cow<'a, str> {
    match kind {
        "plain_scalar" => fold(Cow::Borrowed(raw)),
        "single_quote_scalar" => {
            let inner = raw
                .strip_prefix('\'')
                .and_then(|r| r.strip_suffix('\''))
                .unwrap_or(raw);
            let unescaped = if inner.contains("''") {
                Cow::Owned(inner.replace("''", "'"))
            } else {
                Cow::Borrowed(inner)
            };
            fold(unescaped)
        }
        "double_quote_scalar" => {
            let inner = raw
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .unwrap_or(raw);
            match unescape(inner) {
                Some(text) => fold(text),
                None => Cow::Borrowed(raw),
            }
        }
        _ => Cow::Borrowed(raw),
    }
}

/// Folds line breaks the way a flow scalar does: a break and the white space around it become one space, and each empty
/// line after it becomes a line feed.
fn fold(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.contains('\n') {
        return text;
    }
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let last = lines.len() - 1;
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0_usize;
    for (i, line) in lines.iter().enumerate() {
        let mut line = *line;
        if i > 0 {
            line = line.trim_start_matches([' ', '\t']);
        }
        if i < last {
            line = line.trim_end_matches([' ', '\t']);
        }
        if i > 0 && line.is_empty() && i < last {
            blank_run += 1;
            continue;
        }
        if i > 0 {
            if blank_run > 0 {
                out.extend(std::iter::repeat_n('\n', blank_run));
            } else {
                out.push(' ');
            }
        }
        blank_run = 0;
        out.push_str(line);
    }
    Cow::Owned(out)
}

/// The escapes of a double-quoted scalar, or `None` for one that is not a YAML escape (or is an escaped line break, which
/// folds differently from an unescaped one).
fn unescape(inner: &str) -> Option<Cow<'_, str>> {
    if !inner.contains('\\') {
        return Some(Cow::Borrowed(inner));
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            '0' => '\0',
            'a' => '\u{7}',
            'b' => '\u{8}',
            't' | '\t' => '\t',
            'n' => '\n',
            'v' => '\u{b}',
            'f' => '\u{c}',
            'r' => '\r',
            'e' => '\u{1b}',
            ' ' => ' ',
            '"' => '"',
            '/' => '/',
            '\\' => '\\',
            'N' => '\u{85}',
            '_' => '\u{a0}',
            'L' => '\u{2028}',
            'P' => '\u{2029}',
            'x' => hex_char(&mut chars, 2)?,
            'u' => hex_char(&mut chars, 4)?,
            'U' => hex_char(&mut chars, 8)?,
            _ => return None,
        });
    }
    Some(Cow::Owned(out))
}

fn hex_char(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<char> {
    let mut value = 0_u32;
    for _ in 0..digits {
        value = value.checked_mul(16)?.checked_add(chars.next()?.to_digit(16)?)?;
    }
    char::from_u32(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_budget_stops_tree_sitter_and_a_fair_one_does_not() {
        let text = "a: 1\n".repeat(5_000);
        assert!(
            parse_tree(&text, 2).is_none(),
            "two callbacks cannot read 25,000 bytes"
        );
        assert!(parse_tree(&text, call_budget(text.len())).is_some());
    }

    #[test]
    fn the_budget_grows_with_the_text_and_is_bounded_by_it() {
        assert_eq!(call_budget(0), CALLS_BASE);
        assert!(call_budget(2 * 1024 * 1024) > call_budget(1024));
        assert!(call_budget(usize::MAX) >= CALLS_BASE, "no overflow");
    }

    #[test]
    fn keys_are_decoded_the_way_the_parser_decodes_them() {
        let text = |kind, raw| scalar_text(kind, raw).into_owned();
        assert_eq!(text("plain_scalar", "plain key"), "plain key");
        assert_eq!(text("plain_scalar", "multi\n  line\n\n  key"), "multi line\nkey");
        assert_eq!(text("single_quote_scalar", "'it''s'"), "it's");
        assert_eq!(text("double_quote_scalar", "\"a\\tb\\\\c\\\"d\""), "a\tb\\c\"d");
        assert_eq!(text("double_quote_scalar", "\"\\x41\\u00e9\\U0001F600\""), "Aé😀");
        assert_eq!(text("double_quote_scalar", "\"one\n  two\""), "one two");
        // An escape this does not know comes out as written, so that the key differs and the row warns.
        assert_eq!(text("double_quote_scalar", "\"\\q\""), "\"\\q\"");
        assert_eq!(text("double_quote_scalar", "\"\\u12\""), "\"\\u12\"");
        assert_eq!(text("block_scalar", "|\n  x"), "|\n  x");
    }

    #[test]
    fn a_lone_cr_is_found_and_a_crlf_is_not() {
        assert!(has_lone_cr("a\rb"));
        assert!(has_lone_cr("a\r"));
        assert!(!has_lone_cr("a\r\nb\r\n"));
        assert!(!has_lone_cr("a\nb"));
        assert!(has_lone_cr("a\r\n\rb"));
    }
}
