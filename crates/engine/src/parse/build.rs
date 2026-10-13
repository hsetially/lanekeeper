//! Builds documents from a stream of events (T2). The YAML and the JSON readers both feed this, so the limits, the alias
//! rules and the duplicate-key rule are written once.
//!
//! What it guarantees:
//!
//! - **Aliases are shared, never copied.** An alias node points at the node it stands for. Every node knows how many
//!   nodes it stands for once its aliases are expanded (`expanded`) and how deep that is (`height`), computed when the
//!   node is finished. A node over `Limits::nodes`, or deeper than `Limits::nesting_depth` where it sits, is refused at
//!   that moment, so the expansion of an alias bomb is never built or walked.
//! - **An anchor is known when its node is finished**, not before. An alias inside its own anchor (`&a [*a]`) names
//!   nothing yet and is an error: the engine never builds a cycle, so everything that walks a document ends.
//! - **A repeated key is a warning, and the earlier pairs do not count** (last wins).

use std::borrow::Cow;

use super::Syntax;
use super::key::{PathSegment, encode_setting_path};
use super::node::{
    Document, Entry, Node, NodeId, NodeKind, ParseWarning, Position, Scalar, ScalarStyle, ScalarTag,
};
use crate::error::{LimitKind, ParseError};
use crate::limits::Limits;

/// `n` as a line or column number: saturates instead of wrapping.
pub(super) fn clamp32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Warnings kept up to a cap; the rest are counted.
#[derive(Debug)]
pub(super) struct Warnings {
    pub(super) items: Vec<ParseWarning>,
    pub(super) dropped: usize,
    cap: usize,
}

impl Warnings {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            items: Vec::new(),
            dropped: 0,
            cap,
        }
    }

    pub(super) fn has_room(&self) -> bool {
        self.items.len() < self.cap
    }

    pub(super) fn push(&mut self, warning: ParseWarning) {
        if self.has_room() {
            self.items.push(warning);
        } else {
            self.dropped += 1;
        }
    }
}

/// A duplicate key, before its path can be written: a file with a second document gets `#<n>/` in front of every path,
/// and that is not known until the last document has been read.
struct PendingDuplicate {
    document: usize,
    relative: Option<String>,
    first: Position,
    second: Position,
}

enum FrameKind {
    Sequence(Vec<NodeId>),
    Mapping {
        entries: Vec<Entry>,
        /// The key whose value is being read.
        key: Option<NodeId>,
    },
}

struct Frame {
    kind: FrameKind,
    anchor: usize,
    flow: bool,
    line: u32,
    column: u32,
    /// The last line of the children so far.
    last_line: u32,
    /// One for the container, plus its children so far.
    expanded: usize,
    height: u32,
}

#[derive(Clone, Copy)]
struct Meta {
    expanded: usize,
    height: u32,
}

pub(super) struct TreeBuilder<'l> {
    limits: &'l Limits,
    syntax: Syntax,
    nodes: Vec<Node>,
    meta: Vec<Meta>,
    anchors: Vec<Option<NodeId>>,
    stack: Vec<Frame>,
    root: Option<NodeId>,
    /// Nodes (aliases expanded) in the documents finished so far.
    file_expanded: usize,
    documents: Vec<Document>,
    pending: Vec<PendingDuplicate>,
    dropped: usize,
}

impl<'l> TreeBuilder<'l> {
    pub(super) fn new(limits: &'l Limits, syntax: Syntax) -> Self {
        Self {
            limits,
            syntax,
            nodes: Vec::new(),
            meta: Vec::new(),
            anchors: Vec::new(),
            stack: Vec::new(),
            root: None,
            file_expanded: 0,
            documents: Vec::new(),
            pending: Vec::new(),
            dropped: 0,
        }
    }

    pub(super) fn syntax_error(&self, line: u32, column: u32) -> ParseError {
        ParseError::Syntax {
            syntax: self.syntax,
            line,
            column,
        }
    }

    pub(super) fn start_document(&mut self) {
        self.nodes.clear();
        self.meta.clear();
        self.anchors.clear();
        self.stack.clear();
        self.root = None;
    }

    pub(super) fn end_document(&mut self, line: u32) -> Result<(), ParseError> {
        if !self.stack.is_empty() {
            return Err(self.syntax_error(line, 0));
        }
        let Some(root) = self.root.take() else {
            return Err(self.syntax_error(line, 0));
        };
        self.file_expanded = self
            .file_expanded
            .saturating_add(self.meta[root.index()].expanded);
        if self.file_expanded > self.limits.nodes {
            return Err(ParseError::Limit(LimitKind::Nodes));
        }
        self.documents.push(Document {
            nodes: std::mem::take(&mut self.nodes),
            root,
        });
        Ok(())
    }

    /// The documents, and the duplicate-key warnings with their final paths.
    pub(super) fn finish(mut self, warnings: &mut Warnings) -> Vec<Document> {
        let multiple = self.documents.len() > 1;
        // File order: by document, then by where the repeated key is written. (Inner mappings finish first.)
        self.pending
            .sort_by_key(|d| (d.document, d.second.line, d.second.column));
        for dup in self.pending {
            let path = dup.relative.and_then(|relative| {
                let text = if multiple {
                    format!("#{}/{relative}", dup.document)
                } else {
                    relative
                };
                domain::SettingPath::parse(&text).ok()
            });
            warnings.push(ParseWarning::DuplicateKey {
                path,
                first: dup.first,
                second: dup.second,
            });
        }
        warnings.dropped += self.dropped;
        self.documents
    }

    pub(super) fn scalar(
        &mut self,
        text: Cow<'_, str>,
        style: ScalarStyle,
        tag: ScalarTag,
        anchor: usize,
        start: Position,
        end_line: u32,
    ) -> Result<(), ParseError> {
        if text.len() > self.limits.scalar_bytes {
            return Err(ParseError::Limit(LimitKind::ScalarBytes));
        }
        let scalar = Scalar {
            text: text.into_owned().into_boxed_str(),
            style,
            tag,
        };
        let id = self.push_node(
            NodeKind::Scalar(scalar),
            start,
            end_line.max(start.line),
            anchor,
            Meta {
                expanded: 1,
                height: 0,
            },
        )?;
        self.attach(id)
    }

    pub(super) fn alias(&mut self, anchor: usize, at: Position) -> Result<(), ParseError> {
        let target = self
            .anchors
            .get(anchor)
            .copied()
            .flatten()
            .ok_or(ParseError::UnknownAlias { line: at.line })?;
        let meta = self.meta[target.index()];
        let id = self.push_node(NodeKind::Alias(target), at, at.line, 0, meta)?;
        self.attach(id)
    }

    pub(super) fn open_sequence(
        &mut self,
        anchor: usize,
        flow: bool,
        at: Position,
    ) -> Result<(), ParseError> {
        self.open(FrameKind::Sequence(Vec::new()), anchor, flow, at)
    }

    pub(super) fn open_mapping(&mut self, anchor: usize, flow: bool, at: Position) -> Result<(), ParseError> {
        self.open(
            FrameKind::Mapping {
                entries: Vec::new(),
                key: None,
            },
            anchor,
            flow,
            at,
        )
    }

    fn open(&mut self, kind: FrameKind, anchor: usize, flow: bool, at: Position) -> Result<(), ParseError> {
        if self.stack.len() >= self.limits.nesting_depth {
            return Err(ParseError::Limit(LimitKind::Depth));
        }
        self.stack.push(Frame {
            kind,
            anchor,
            flow,
            line: at.line,
            column: at.column,
            last_line: at.line,
            expanded: 1,
            height: 1,
        });
        Ok(())
    }

    /// Finishes the innermost container. `end_line` is the line of its closing bracket; only a flow container has one.
    pub(super) fn close(&mut self, end_line: u32) -> Result<(), ParseError> {
        let Some(frame) = self.stack.pop() else {
            return Err(self.syntax_error(end_line, 0));
        };
        let end = if frame.flow {
            end_line.max(frame.last_line)
        } else {
            frame.last_line
        };
        let meta = Meta {
            expanded: frame.expanded,
            height: frame.height,
        };
        let start = Position {
            line: frame.line,
            column: frame.column,
        };
        let kind = match frame.kind {
            FrameKind::Sequence(items) => NodeKind::Sequence(items),
            FrameKind::Mapping { mut entries, key } => {
                if key.is_some() {
                    // A key without a value: the readers never produce this.
                    return Err(self.syntax_error(end, 0));
                }
                self.mark_duplicates(&mut entries);
                NodeKind::Mapping(entries)
            }
        };
        let id = self.push_node(kind, start, end, frame.anchor, meta)?;
        self.attach(id)
    }

    /// Adds a finished node to the arena. This is where the limits that depend on the whole node are checked.
    fn push_node(
        &mut self,
        kind: NodeKind,
        start: Position,
        end_line: u32,
        anchor: usize,
        meta: Meta,
    ) -> Result<NodeId, ParseError> {
        if meta.expanded > self.limits.nodes {
            return Err(ParseError::Limit(LimitKind::Nodes));
        }
        // The node sits under `stack.len()` containers, and has `height` levels of containers in it.
        let depth = self.stack.len().saturating_add(meta.height as usize);
        if depth > self.limits.nesting_depth {
            return Err(ParseError::Limit(LimitKind::Depth));
        }
        let id = NodeId(u32::try_from(self.nodes.len()).map_err(|_| ParseError::Limit(LimitKind::Nodes))?);
        self.nodes.push(Node {
            kind,
            line: start.line,
            column: start.column,
            end_line,
        });
        self.meta.push(meta);
        if anchor != 0 {
            // The anchors are numbered one after the other by the reader, and every one is on a node.
            if anchor > self.limits.nodes.saturating_add(1) {
                return Err(ParseError::Limit(LimitKind::Nodes));
            }
            if self.anchors.len() <= anchor {
                self.anchors.resize(anchor + 1, None);
            }
            self.anchors[anchor] = Some(id);
        }
        Ok(id)
    }

    /// Puts a finished node where it belongs: the root, an item, a key or a value.
    fn attach(&mut self, id: NodeId) -> Result<(), ParseError> {
        let meta = self.meta[id.index()];
        let (line, end_line) = {
            let node = &self.nodes[id.index()];
            (node.line, node.end_line)
        };
        let Some(frame) = self.stack.last_mut() else {
            if self.root.is_some() {
                // A second value where only one is allowed (JSON: `{} {}`).
                return Err(self.syntax_error(line, 0));
            }
            self.root = Some(id);
            return Ok(());
        };
        frame.expanded = frame.expanded.saturating_add(meta.expanded);
        if frame.expanded > self.limits.nodes {
            return Err(ParseError::Limit(LimitKind::Nodes));
        }
        frame.height = frame.height.max(meta.height.saturating_add(1));
        frame.last_line = frame.last_line.max(end_line);
        match &mut frame.kind {
            FrameKind::Sequence(items) => items.push(id),
            FrameKind::Mapping { entries, key } => match key.take() {
                None => {
                    // A key becomes part of a setting path, which has a limit.
                    if let NodeKind::Scalar(s) = &self.nodes[id.index()].kind {
                        if s.text.len() > self.limits.setting_path_bytes {
                            return Err(ParseError::Limit(LimitKind::SettingPathBytes));
                        }
                    }
                    *key = Some(id);
                }
                Some(k) => entries.push(Entry {
                    key: k,
                    value: id,
                    overridden: false,
                }),
            },
        }
        Ok(())
    }

    /// Marks every pair whose key comes again later, and records a warning for each later one. Keys are compared by
    /// their text; a key that is not a scalar is never a duplicate.
    fn mark_duplicates(&mut self, entries: &mut [Entry]) {
        if entries.len() < 2 {
            return;
        }
        // Pairs of (first occurrence, a later one), and the pairs that do not count.
        let (mut dups, overridden) = {
            let nodes = &self.nodes;
            let mut keyed: Vec<(&str, usize)> = entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| match &nodes[e.key.index()].kind {
                    NodeKind::Scalar(s) => Some((&*s.text, i)),
                    _ => None,
                })
                .collect();
            keyed.sort_unstable();
            let mut dups: Vec<(usize, usize)> = Vec::new();
            let mut overridden: Vec<usize> = Vec::new();
            let mut start = 0;
            while start < keyed.len() {
                let mut end = start + 1;
                while end < keyed.len() && keyed[end].0 == keyed[start].0 {
                    end += 1;
                }
                if end - start > 1 {
                    let first = keyed[start].1;
                    dups.extend(keyed[start + 1..end].iter().map(|&(_, later)| (first, later)));
                    overridden.extend(keyed[start..end - 1].iter().map(|&(_, i)| i));
                }
                start = end;
            }
            (dups, overridden)
        };
        for i in overridden {
            entries[i].overridden = true;
        }
        if dups.is_empty() {
            return;
        }
        dups.sort_unstable_by_key(|&(_, later)| later);
        // The path of the mapping is made of the keys and indexes of the containers around it.
        let parent = self.parent_segments();
        let position = |nodes: &[Node], entry: &Entry| {
            let n = &nodes[entry.key.index()];
            Position {
                line: n.line,
                column: n.column,
            }
        };
        for (first, later) in dups {
            if self.pending.len() >= self.limits.warnings {
                self.dropped += 1;
                continue;
            }
            let relative = match (&parent, &self.nodes[entries[later].key.index()].kind) {
                (Some(segments), NodeKind::Scalar(key)) => {
                    let mut all = segments.clone();
                    all.push(PathSegment::Key(key.text.to_string()));
                    Some(encode_setting_path(None, &all))
                }
                _ => None,
            };
            self.pending.push(PendingDuplicate {
                document: self.documents.len(),
                relative,
                first: position(&self.nodes, &entries[first]),
                second: position(&self.nodes, &entries[later]),
            });
        }
    }

    /// The segments from the root of the document to the container that was just finished, or `None` when one of the
    /// keys on the way is not a scalar.
    fn parent_segments(&self) -> Option<Vec<PathSegment>> {
        let mut segments = Vec::with_capacity(self.stack.len());
        for frame in &self.stack {
            match &frame.kind {
                FrameKind::Sequence(items) => segments.push(PathSegment::Index(clamp32(items.len()))),
                FrameKind::Mapping { key, .. } => {
                    let key = (*key)?;
                    match &self.nodes[key.index()].kind {
                        NodeKind::Scalar(s) => segments.push(PathSegment::Key(s.text.to_string())),
                        _ => return None,
                    }
                }
            }
        }
        Some(segments)
    }
}
