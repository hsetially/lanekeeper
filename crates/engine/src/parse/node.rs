//! The parsed tree and what comes with it (T2): nodes in an arena, the warnings, and the result types of `parse`.
//!
//! A file is parsed into documents. Each document is a flat arena of nodes that refer to each other by `NodeId`, so
//! there is no recursion to overflow and nothing to drop deeply. An alias is a node of its own that points at the
//! node it stands for (a shared node, never a copy): the tree is a DAG, and expanding it is `flatten`'s job, counted
//! against `Limits::nodes` so that an alias bomb is refused instead of built.
//!
//! The `Debug` output of every type that holds file content shows sizes and shapes, never values: a config can hold a
//! password, and a stray `{:?}` in a log line must not copy it (S13, S21).

use std::fmt;

use domain::{SettingPath, SettingValue};

use super::Syntax;

/// A place in the text. Lines and columns are 1-based; a column counts characters. A column of 0 means the syntax does not
/// track it (JSON, `.properties`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

/// A node of one document. Only valid for the document that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(pub(super) u32);

impl NodeId {
    pub(super) fn index(self) -> usize {
        self.0 as usize
    }
}

/// How a scalar was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarStyle {
    /// No quotes. Only a plain scalar is read as a number, a boolean or null.
    Plain,
    /// Single or double quotes (or a JSON string): always text.
    Quoted,
    /// A YAML literal or folded block scalar: always text.
    Block,
}

/// An explicit YAML tag on a scalar (`!!int 7`), reduced to what decides its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarTag {
    None,
    Str,
    Int,
    Float,
    Bool,
    Null,
    /// Any other tag: the value is not one of the core types.
    Other,
}

/// A scalar: its decoded text (quotes and escapes resolved, folded lines joined), and how it was written.
#[derive(Clone, PartialEq, Eq)]
pub struct Scalar {
    pub(super) text: Box<str>,
    pub style: ScalarStyle,
    pub tag: ScalarTag,
}

impl Scalar {
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl fmt::Debug for Scalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scalar")
            .field("bytes", &self.text.len())
            .field("style", &self.style)
            .field("tag", &self.tag)
            .finish()
    }
}

/// One `key: value` pair of a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub key: NodeId,
    pub value: NodeId,
    /// A later pair has the same key: this one does not count (last wins). `parse` reported it as a warning.
    pub overridden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Scalar(Scalar),
    Sequence(Vec<NodeId>),
    Mapping(Vec<Entry>),
    /// A YAML alias: this node stands for the one it points at.
    Alias(NodeId),
}

/// A node and where it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    /// The first line and the column the node starts in.
    pub line: u32,
    pub column: u32,
    /// The last line the node covers (for a block scalar, the last line of its text).
    pub end_line: u32,
}

/// One YAML or JSON document.
#[derive(Clone, PartialEq, Eq)]
pub struct Document {
    pub(super) nodes: Vec<Node>,
    pub(super) root: NodeId,
}

impl Document {
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The node, or `None` for an id that is not from this document.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    /// The number of nodes, aliases included (each alias counts once here, however much it stands for).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every node with its id, in the order they were finished: a node comes after everything it refers to.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (NodeId(u32::try_from(i).unwrap_or(u32::MAX)), node))
    }

    /// Inside the crate every id comes from the builder of this document, so it is in range.
    pub(super) fn at(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }
}

impl fmt::Debug for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("nodes", &self.nodes.len())
            .field("root", &self.root)
            .finish()
    }
}

/// One `key=value` pair of a `.properties` file, with its escapes decoded.
#[derive(Clone, PartialEq, Eq)]
pub struct PropertyEntry {
    pub(super) key: Box<str>,
    pub(super) value: Box<str>,
    /// The first and the last physical line of the pair (a value can continue over several).
    pub line: u32,
    pub end_line: u32,
    /// A later pair has the same key: this one does not count (last wins).
    pub overridden: bool,
}

impl PropertyEntry {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for PropertyEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyEntry")
            .field("key", &self.key)
            .field("value_bytes", &self.value.len())
            .field("line", &self.line)
            .field("end_line", &self.end_line)
            .field("overridden", &self.overridden)
            .finish()
    }
}

/// What was parsed: documents for YAML and JSON, a list of pairs for `.properties`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Documents(Vec<Document>),
    Properties(Vec<PropertyEntry>),
}

/// Something worth telling the caller that does not stop the file from being read. `#[non_exhaustive]`: later tasks add
/// kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseWarning {
    /// A mapping has the same key twice. The last one wins (check C8). `path` is the setting that is defined twice;
    /// `None` when it cannot be written as a setting path. `first` is the first occurrence and `second` the one this
    /// warning is about; a key that appears three times gives two warnings.
    DuplicateKey {
        path: Option<SettingPath>,
        first: Position,
        second: Position,
    },
    /// A plain `yes`, `no`, `on` or `off`: a boolean in YAML 1.1 and a string in YAML 1.2, so services may read it
    /// differently (`docs/domain-model.md`). Found by `flatten`.
    BooleanLikeValue { path: SettingPath, line: u32 },
    /// A mapping key that is not a scalar (`? [a, b]`). A setting path cannot name it, so the pair is left out.
    ComplexKeySkipped { at: Position },
    /// A `<<` key whose value is not a mapping or a list of mappings. It is kept as an ordinary key.
    InvalidMergeKey { at: Position },
    /// A `.properties` key that cannot be a setting path (empty, or with a control character). The pair is left out.
    UnusableKeySkipped { line: u32 },
    /// Found by `index_settings` (T3): tree-sitter and the parser do not agree on where a YAML setting is, or tree-sitter
    /// could not read the file at all (`path` is `None`). The row keeps the parser's range, which is the one every other
    /// function of the engine uses, and this says that the second reader did not confirm it.
    LocationMismatch { path: Option<SettingPath> },
}

/// The result of `parse`.
#[derive(Clone, PartialEq, Eq)]
pub struct Parsed {
    pub syntax: Syntax,
    pub body: Body,
    /// Warnings found while parsing, in file order, at most `Limits::warnings`.
    pub warnings: Vec<ParseWarning>,
    /// Warnings that were left out because of the cap.
    pub warnings_dropped: usize,
}

impl fmt::Debug for Parsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Parsed");
        d.field("syntax", &self.syntax);
        match &self.body {
            Body::Documents(docs) => d.field("documents", docs),
            Body::Properties(entries) => d.field("properties", entries),
        };
        d.field("warnings", &self.warnings.len())
            .field("warnings_dropped", &self.warnings_dropped)
            .finish()
    }
}

/// A leaf of a file as the grid, the diff and the checks see it.
#[derive(Clone, PartialEq, Eq)]
pub struct FlatSetting {
    pub path: SettingPath,
    pub value: SettingValue,
    /// From the line of the key to the last line of the value (for a list item, the lines of the item). For a setting
    /// that came in through an alias, the lines of the value where it is written, in the anchor.
    pub lines: domain::LineRange,
    /// The setting came in through a YAML alias or a merge key: it is not written where its path says.
    pub via_alias: bool,
}

impl fmt::Debug for FlatSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlatSetting")
            .field("path", &self.path)
            .field("value_bytes", &self.value.text.len())
            .field("ty", &self.value.ty)
            .field("flag", &self.value.flag)
            .field("lines", &self.lines)
            .field("via_alias", &self.via_alias)
            .finish()
    }
}

/// The result of `flatten`: every leaf of the file, in file order, each path once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flat {
    pub settings: Vec<FlatSetting>,
    /// Warnings found while flattening, in the order the settings are in, at most `Limits::warnings`.
    pub warnings: Vec<ParseWarning>,
    pub warnings_dropped: usize,
}

/// The result of `validate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    /// YAML or JSON documents; 0 for `.properties`.
    pub documents: usize,
    pub settings: usize,
    /// The warnings of parsing, then those of flattening, at most `Limits::warnings` in all.
    pub warnings: Vec<ParseWarning>,
    pub warnings_dropped: usize,
}
