//! `flatten` (T2): a parsed file as a list of settings, one per leaf, with the path syntax of `docs/domain-model.md`.
//!
//! The walk is iterative (a stack of frames, not recursion), so the depth of a file never decides how much of the call
//! stack is used. Aliases are followed, and a merge key (`<<`) brings in the pairs of the mappings it names, exactly as
//! `SnakeYAML` does:
//!
//! - a pair written in the mapping itself beats a merged one, wherever the `<<` is;
//! - of several merged mappings (`<<: [*a, *b]`) the first wins;
//! - merged pairs sit where the `<<` is.
//!
//! Every step is counted against a budget derived from `Limits::nodes`: the nodes of a document already count what their
//! aliases expand to (see `build.rs`), so the walk cannot take longer than that, and the counter is a second wall in case
//! a document was made some other way.

use std::collections::BTreeSet;

use domain::{LineRange, SettingPath, SettingValue, ValueType};

use super::build::{Warnings, clamp32};
use super::key::{push_document_prefix, push_index, push_key};
use super::node::{
    Body, Document, Entry, Flat, FlatSetting, NodeId, NodeKind, ParseWarning, Parsed, Position,
    PropertyEntry, ScalarStyle, ScalarTag,
};
use super::scalar::classify;
use crate::error::{LimitKind, ParseError};
use crate::limits::Limits;

/// How deep merge keys may refer to merge keys.
const MAX_MERGE_DEPTH: usize = 256;

/// Every leaf of the file, in file order, each path once.
///
/// # Errors
///
/// `Limit(Settings)` when there are more leaves than `Limits::settings`, `Limit(SettingPathBytes)` when a path is longer
/// than `Limits::setting_path_bytes` as it is written, `Limit(Depth)` for merge keys nested too deeply, and
/// `Limit(Nodes)` if the walk took more steps than the document's size allows.
pub fn flatten(parsed: &Parsed, limits: &Limits) -> Result<Flat, ParseError> {
    let mut out = Out {
        limits,
        settings: Vec::new(),
        warnings: Warnings::new(limits.warnings),
        work: 0,
        work_limit: limits.nodes.saturating_mul(4).saturating_add(1_024),
    };
    match &parsed.body {
        Body::Documents(documents) => {
            let multiple = documents.len() > 1;
            for (index, document) in documents.iter().enumerate() {
                walk_document(document, multiple.then_some(clamp32(index)), &mut out)?;
            }
        }
        Body::Properties(entries) => flatten_properties(entries, &mut out)?,
    }
    Ok(Flat {
        settings: out.settings,
        warnings: out.warnings.items,
        warnings_dropped: out.warnings.dropped,
    })
}

struct Out<'l> {
    limits: &'l Limits,
    settings: Vec<FlatSetting>,
    warnings: Warnings,
    work: usize,
    work_limit: usize,
}

impl Out<'_> {
    fn tick(&mut self, steps: usize) -> Result<(), ParseError> {
        self.work = self.work.saturating_add(steps);
        if self.work > self.work_limit {
            return Err(ParseError::Limit(LimitKind::Nodes));
        }
        Ok(())
    }

    fn setting_path(&self, text: &str) -> Result<SettingPath, ParseError> {
        if text.len() > self.limits.setting_path_bytes {
            return Err(ParseError::Limit(LimitKind::SettingPathBytes));
        }
        SettingPath::parse(text).map_err(|_| ParseError::Limit(LimitKind::SettingPathBytes))
    }

    fn emit(
        &mut self,
        path: &str,
        value: SettingValue,
        start: u32,
        end: u32,
        via_alias: bool,
    ) -> Result<(), ParseError> {
        if self.settings.len() >= self.limits.settings {
            return Err(ParseError::Limit(LimitKind::Settings));
        }
        let path = self.setting_path(path)?;
        let start = start.max(1);
        let lines =
            LineRange::new(start, end.max(start)).map_err(|_| ParseError::Limit(LimitKind::Settings))?;
        self.settings.push(FlatSetting {
            path,
            value,
            lines,
            via_alias,
        });
        Ok(())
    }
}

fn flatten_properties(entries: &[PropertyEntry], out: &mut Out<'_>) -> Result<(), ParseError> {
    for entry in entries.iter().filter(|e| !e.overridden) {
        out.tick(1)?;
        let key = entry.key();
        if key.is_empty() || key.chars().any(char::is_control) {
            out.warnings
                .push(ParseWarning::UnusableKeySkipped { line: entry.line });
            continue;
        }
        let value = SettingValue {
            text: entry.value().to_owned(),
            ty: ValueType::String,
            flag: None,
        };
        out.emit(key, value, entry.line, entry.end_line, false)?;
    }
    Ok(())
}

/// A pair as the walk sees it: after duplicates and merge keys.
#[derive(Clone, Copy)]
struct Eff {
    key: NodeId,
    value: NodeId,
    via_alias: bool,
}

enum Items<'d> {
    Sequence(&'d [NodeId]),
    /// A mapping without a merge key: its own pairs.
    Direct(&'d [Entry]),
    /// A mapping with merge keys, resolved.
    Resolved(Vec<Eff>),
}

struct Frame<'d> {
    items: Items<'d>,
    next: usize,
    /// The length of the path of the container itself.
    base: usize,
    via_alias: bool,
}

/// What a frame hands out next.
struct Child {
    /// The key of a mapping pair; `None` for a sequence item.
    key: Option<NodeId>,
    index: u32,
    value: NodeId,
    via_alias: bool,
}

impl Frame<'_> {
    fn next_child(&mut self) -> Option<Child> {
        loop {
            let at = self.next;
            self.next += 1;
            return match &self.items {
                Items::Sequence(items) => items.get(at).map(|&value| Child {
                    key: None,
                    index: clamp32(at),
                    value,
                    via_alias: self.via_alias,
                }),
                Items::Direct(entries) => {
                    let entry = entries.get(at)?;
                    if entry.overridden {
                        continue;
                    }
                    Some(Child {
                        key: Some(entry.key),
                        index: 0,
                        value: entry.value,
                        via_alias: self.via_alias,
                    })
                }
                Items::Resolved(list) => list.get(at).map(|e| Child {
                    key: Some(e.key),
                    index: 0,
                    value: e.value,
                    via_alias: self.via_alias || e.via_alias,
                }),
            };
        }
    }
}

fn walk_document(document: &Document, index: Option<u32>, out: &mut Out<'_>) -> Result<(), ParseError> {
    let mut path = String::new();
    if let Some(index) = index {
        push_document_prefix(&mut path, index);
    }
    let prefix_len = path.len();
    let mut walk = Walk {
        document,
        prefix_len,
        path,
        stack: Vec::new(),
        out,
    };
    let root = document.root();
    walk.visit(root, document.at(root).line, false)?;
    walk.run()
}

/// The state of the walk over one document: the path being written, and the containers being walked.
struct Walk<'d, 'o, 'l> {
    document: &'d Document,
    /// The length of the document prefix (`#<n>/`), which the path starts with.
    prefix_len: usize,
    path: String,
    stack: Vec<Frame<'d>>,
    out: &'o mut Out<'l>,
}

impl<'d> Walk<'d, '_, '_> {
    fn run(&mut self) -> Result<(), ParseError> {
        let document = self.document;
        while let Some(frame) = self.stack.last_mut() {
            let base = frame.base;
            let Some(child) = frame.next_child() else {
                self.stack.pop();
                continue;
            };
            self.out.tick(1)?;
            self.path.truncate(base);
            let start_line;
            if let Some(key) = child.key {
                let Some(text) = key_text(document, key) else {
                    let at = document.at(key);
                    self.out.warnings.push(ParseWarning::ComplexKeySkipped {
                        at: Position {
                            line: at.line,
                            column: at.column,
                        },
                    });
                    continue;
                };
                start_line = document.at(key).line;
                push_key(&mut self.path, text, base == self.prefix_len);
            } else {
                start_line = document.at(child.value).line;
                push_index(&mut self.path, child.index);
            }
            if self.path.len() > self.out.limits.setting_path_bytes {
                return Err(ParseError::Limit(LimitKind::SettingPathBytes));
            }
            self.visit(child.value, start_line, child.via_alias)?;
        }
        Ok(())
    }

    /// Emits a leaf, or pushes the frame of a container. `start` is the line the value's setting starts on.
    fn visit(&mut self, id: NodeId, start: u32, via_alias: bool) -> Result<(), ParseError> {
        let document = self.document;
        self.out.tick(1)?;
        let mut id = id;
        let mut start = start;
        let mut via_alias = via_alias;
        // An alias is a stand-in for the node it names, which is never an alias itself.
        if let NodeKind::Alias(target) = &document.at(id).kind {
            id = *target;
            via_alias = true;
            start = document.at(id).line;
        }
        let node = document.at(id);
        let lines = (start, node.end_line);
        match &node.kind {
            NodeKind::Scalar(scalar) => {
                let (ty, flag) = classify(scalar);
                if flag.is_some() {
                    self.warn_boolean_like(start);
                }
                let value = SettingValue {
                    text: scalar.text().to_owned(),
                    ty,
                    flag,
                };
                self.leaf(value, lines, via_alias)
            }
            NodeKind::Sequence(items) => {
                if items.is_empty() {
                    return self.leaf(empty_container("[]"), lines, via_alias);
                }
                self.push_frame(Items::Sequence(items), via_alias);
                Ok(())
            }
            NodeKind::Mapping(entries) => {
                let items = if has_merge_key(document, entries) {
                    Items::Resolved(resolve(document, entries, 0, self.out)?)
                } else {
                    Items::Direct(entries)
                };
                let empty = match &items {
                    Items::Direct(entries) => entries.iter().all(|e| e.overridden),
                    Items::Resolved(list) => list.is_empty(),
                    Items::Sequence(_) => false,
                };
                if empty {
                    return self.leaf(empty_container("{}"), lines, via_alias);
                }
                self.push_frame(items, via_alias);
                Ok(())
            }
            NodeKind::Alias(_) => Err(ParseError::Limit(LimitKind::Nodes)),
        }
    }

    fn push_frame(&mut self, items: Items<'d>, via_alias: bool) {
        self.stack.push(Frame {
            items,
            next: 0,
            base: self.path.len(),
            via_alias,
        });
    }

    fn leaf(
        &mut self,
        value: SettingValue,
        (start, end): (u32, u32),
        via_alias: bool,
    ) -> Result<(), ParseError> {
        // A file that is a single value has no key to name it.
        if self.path.len() == self.prefix_len {
            self.path.push('$');
        }
        self.out.emit(&self.path, value, start, end, via_alias)
    }

    fn warn_boolean_like(&mut self, line: u32) {
        if !self.out.warnings.has_room() {
            self.out.warnings.dropped += 1;
        } else if let Ok(path) = SettingPath::parse(&self.path) {
            self.out
                .warnings
                .push(ParseWarning::BooleanLikeValue { path, line });
        }
    }
}

fn empty_container(text: &str) -> SettingValue {
    SettingValue {
        text: text.to_owned(),
        ty: ValueType::Other,
        flag: None,
    }
}

/// The text of a key that is a scalar, or an alias of one.
fn key_text(document: &Document, key: NodeId) -> Option<&str> {
    let mut node = document.at(key);
    if let NodeKind::Alias(target) = &node.kind {
        node = document.at(*target);
    }
    match &node.kind {
        NodeKind::Scalar(s) => Some(s.text()),
        _ => None,
    }
}

fn is_merge_key(document: &Document, key: NodeId) -> bool {
    match &document.at(key).kind {
        NodeKind::Scalar(s) => s.style == ScalarStyle::Plain && s.tag == ScalarTag::None && s.text() == "<<",
        _ => false,
    }
}

fn has_merge_key(document: &Document, entries: &[Entry]) -> bool {
    entries
        .iter()
        .any(|e| !e.overridden && is_merge_key(document, e.key))
}

/// What a `<<` names: one mapping, or a list of them. `None` when the value is anything else.
fn merge_sources(document: &Document, value: NodeId) -> Option<Vec<&[Entry]>> {
    let follow = |id: NodeId| match &document.at(id).kind {
        NodeKind::Alias(target) => *target,
        _ => id,
    };
    match &document.at(follow(value)).kind {
        NodeKind::Mapping(entries) => Some(vec![entries.as_slice()]),
        NodeKind::Sequence(items) => items
            .iter()
            .map(|&item| match &document.at(follow(item)).kind {
                NodeKind::Mapping(entries) => Some(entries.as_slice()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

enum Plan<'d> {
    Pair(&'d Entry),
    Merge(Vec<&'d [Entry]>),
}

/// The pairs of a mapping that has merge keys, in order, with the merged ones in place and the shadowed ones left out.
fn resolve<'d>(
    document: &'d Document,
    entries: &'d [Entry],
    depth: usize,
    out: &mut Out<'_>,
) -> Result<Vec<Eff>, ParseError> {
    if depth > MAX_MERGE_DEPTH.min(out.limits.nesting_depth.max(1)) {
        return Err(ParseError::Limit(LimitKind::Depth));
    }
    out.tick(entries.len())?;
    let mut plans: Vec<Plan<'d>> = Vec::new();
    let mut seen: BTreeSet<&'d str> = BTreeSet::new();
    for entry in entries.iter().filter(|e| !e.overridden) {
        if is_merge_key(document, entry.key) {
            if let Some(sources) = merge_sources(document, entry.value) {
                plans.push(Plan::Merge(sources));
                continue;
            }
            let at = document.at(entry.key);
            out.warnings.push(ParseWarning::InvalidMergeKey {
                at: Position {
                    line: at.line,
                    column: at.column,
                },
            });
        }
        if let Some(text) = key_text(document, entry.key) {
            seen.insert(text);
        }
        plans.push(Plan::Pair(entry));
    }
    let mut list = Vec::new();
    for plan in plans {
        match plan {
            Plan::Pair(entry) => list.push(Eff {
                key: entry.key,
                value: entry.value,
                via_alias: false,
            }),
            Plan::Merge(sources) => {
                for source in sources {
                    let merged = if has_merge_key(document, source) {
                        resolve(document, source, depth + 1, out)?
                    } else {
                        out.tick(source.len())?;
                        source
                            .iter()
                            .filter(|e| !e.overridden)
                            .map(|e| Eff {
                                key: e.key,
                                value: e.value,
                                via_alias: false,
                            })
                            .collect()
                    };
                    for eff in merged {
                        let keep = match key_text(document, eff.key) {
                            Some(text) => seen.insert(text),
                            None => true,
                        };
                        if keep {
                            list.push(Eff {
                                via_alias: true,
                                ..eff
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(list)
}
