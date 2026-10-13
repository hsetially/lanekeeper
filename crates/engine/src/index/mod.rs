//! The settings index and `locate` (T3, D54, D65, S11, S22).
//!
//! `index_settings` is what the hub writes to `settings_index` once for every unique structured blob: one row for each
//! setting, with its path, its value and type, and the lines it is written on. Settings search, the grid and feature status
//! are built on those rows, so this is a hot path (P6, P7) and it reads hostile input (S22).
//!
//! # Where the ranges come from
//!
//! - **YAML:** tree-sitter, with the YAML grammar. A second, independent reader of the same text, which has to agree with
//!   the parser on where every setting is (`yaml_ts.rs`). Where it does, the row has that range. Where it does not, or
//!   when it cannot read the file, the row keeps the parser's range and a `LocationMismatch` warning says so; nothing
//!   panics and nothing is lost.
//! - **JSON:** the parser's positions. Its tokeniser gives the line of every token.
//! - **`.properties`:** the parser's lines. A key is on a line of its own, and a value that continues over more lines
//!   extends the range to the last of them.
//!
//! # Hostile input
//!
//! The Rust parser reads the bytes first, under every limit of `Limits`, and only a text it accepted is handed to
//! tree-sitter, which is C code. That reader then runs on a work budget (a count of its own progress callbacks, not a
//! clock), so its output is the same on a busy machine and on an idle one.

mod locate;
mod settings;
mod yaml_ts;

use domain::{LineRange, SettingPath, SettingValue};

use crate::parse::ParseWarning;

pub use self::locate::locate;
pub use self::settings::index_settings;

/// One row of the settings index: a setting, its value and where it is written.
#[derive(Clone, PartialEq, Eq)]
pub struct SettingRow {
    pub path: SettingPath,
    pub value: SettingValue,
    /// From the line of the key to the last line of the value, 1-based and inclusive. A setting that came in through a
    /// YAML alias or a merge key is where its value is written, in the anchor.
    pub lines: LineRange,
}

impl std::fmt::Debug for SettingRow {
    // A value may be a secret: the row's `Debug` shows its size, never its text (S13, S21).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingRow")
            .field("path", &self.path)
            .field("value_bytes", &self.value.text.len())
            .field("ty", &self.value.ty)
            .field("lines", &self.lines)
            .finish()
    }
}

/// Which reader the ranges of an index are from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationSource {
    /// YAML, read by tree-sitter and checked against the parser's marks. A row where the two disagree has a warning.
    TreeSitter,
    /// The parser's own positions: JSON and `.properties`, and a YAML file that tree-sitter could not read (its
    /// `LocationMismatch` warning with no path says so).
    Parser,
}

/// The result of `index_settings`.
#[derive(Clone, PartialEq, Eq)]
pub struct SettingsIndex {
    /// Every leaf of the file, in file order, each path once.
    pub rows: Vec<SettingRow>,
    /// The warnings of parsing and flattening, then the `LocationMismatch` ones, at most `Limits::warnings` in all.
    pub warnings: Vec<ParseWarning>,
    /// Warnings that were left out because of the cap.
    pub warnings_dropped: usize,
    pub located_by: LocationSource,
}

impl std::fmt::Debug for SettingsIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsIndex")
            .field("rows", &self.rows.len())
            .field("warnings", &self.warnings.len())
            .field("warnings_dropped", &self.warnings_dropped)
            .field("located_by", &self.located_by)
            .finish()
    }
}

impl SettingsIndex {
    /// The range of `path`, or `None` when the file has no such setting. A container is not a setting: only leaves are
    /// indexed. Rows are in file order and a path is unique, so this is a scan; a caller that asks many times builds a map.
    pub fn locate(&self, path: &SettingPath) -> Option<LineRange> {
        self.rows
            .iter()
            .find(|row| row.path == *path)
            .map(|row| row.lines)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::yaml_ts::READS;
    use super::*;
    use crate::error::{LimitKind, ParseError};
    use crate::limits::Limits;
    use crate::parse::Syntax;

    /// A hostile input, and what the refusal has to be.
    type Case = (&'static str, Vec<u8>, fn(&ParseError) -> bool);

    fn reads() -> usize {
        READS.with(std::cell::Cell::get)
    }

    /// S22: tree-sitter is C code. It reads a text only after the Rust parser has accepted it under every limit, so
    /// whatever the parser refuses (a syntax error, an alias bomb, a deep or huge file, a value over the limit, bytes that
    /// are not text) never reaches it.
    #[test]
    fn index_settings_never_hands_rejected_input_to_tree_sitter() {
        let limits = Limits {
            input_bytes: 4_096,
            nesting_depth: 8,
            nodes: 200,
            settings: 50,
            scalar_bytes: 200,
            setting_path_bytes: 64,
            ..Limits::default()
        };
        let deep = format!("{}1{}", "[".repeat(20), "]".repeat(20));
        let bomb = "a: &a [x, x, x, x, x, x, x, x]\nb: &b [*a, *a, *a, *a, *a, *a, *a, *a]\nc: &c [*b, *b, *b, *b, *b, *b, *b, *b]\n";
        let mut many = String::new();
        for i in 0..60 {
            writeln!(many, "k{i}: v").expect("writing to a string");
        }
        let huge_value = format!("a: {}\n", "x".repeat(300));
        let huge = "a: 1\n".repeat(2_000);
        let long_key = format!("{}: 1\n", "k".repeat(100));
        let rejected: Vec<Case> = vec![
            ("syntax", b"a: [1\n".to_vec(), |e| {
                matches!(e, ParseError::Syntax { .. })
            }),
            ("unknown alias", b"a: *nope\n".to_vec(), |e| {
                matches!(e, ParseError::UnknownAlias { .. })
            }),
            ("depth", deep.into_bytes(), |e| {
                matches!(e, ParseError::Limit(LimitKind::Depth))
            }),
            ("alias bomb", bomb.as_bytes().to_vec(), |e| {
                matches!(e, ParseError::Limit(LimitKind::Nodes))
            }),
            ("settings", many.into_bytes(), |e| {
                matches!(e, ParseError::Limit(LimitKind::Settings))
            }),
            ("scalar", huge_value.into_bytes(), |e| {
                matches!(e, ParseError::Limit(LimitKind::ScalarBytes))
            }),
            ("input size", huge.into_bytes(), |e| {
                matches!(e, ParseError::Limit(LimitKind::InputBytes))
            }),
            ("key length", long_key.into_bytes(), |e| {
                matches!(e, ParseError::Limit(LimitKind::SettingPathBytes))
            }),
            ("decode", b"\xEF\xBB\xBFa: \xFF\n".to_vec(), |e| {
                matches!(e, ParseError::Decode(_))
            }),
        ];
        for (name, data, expected) in rejected {
            let before = reads();
            let result = index_settings(&data, Syntax::Yaml, &limits);
            let error = result.expect_err(name);
            assert!(expected(&error), "{name}: {error:?}");
            assert_eq!(
                reads(),
                before,
                "{name}: tree-sitter was given a text the parser refused"
            );
        }
        // What the parser accepts is read once, and only once.
        let before = reads();
        let index = index_settings(b"a: 1\nb:\n  c: 2\n", Syntax::Yaml, &limits).expect("valid");
        assert_eq!(index.located_by, LocationSource::TreeSitter);
        assert_eq!(reads(), before + 1);
        // JSON and .properties never involve it.
        index_settings(b"{\"a\": 1}", Syntax::Json, &limits).expect("valid json");
        index_settings(b"a=1\n", Syntax::Properties, &limits).expect("valid properties");
        assert_eq!(reads(), before + 1);
    }

    /// The rows' `Debug` shows the size of a value and never the value (S13, S21).
    #[test]
    fn debug_output_of_rows_never_shows_a_value() {
        let secret = "s3cr3t-value-0xDEADBEEF";
        let text = format!("password: {secret}\n");
        let index = index_settings(text.as_bytes(), Syntax::Yaml, &Limits::default()).expect("valid");
        let shown = format!("{index:?} {:?}", index.rows);
        assert!(!shown.contains(secret), "{shown}");
        assert!(shown.contains("value_bytes"), "{shown}");
    }
}
