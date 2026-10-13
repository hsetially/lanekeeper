//! Parsing, validation and flattening (T2, S11, S22): files as the engine reads them.
//!
//! `parse` reads YAML, JSON or `.properties` text under the limits of `Limits` and returns the documents (or the pairs)
//! with every position and a list of warnings; `flatten` turns them into one setting per leaf with the path syntax of
//! `docs/domain-model.md`; `validate` does both from bytes and says whether a file is acceptable.
//!
//! Hostile input is the point of this module. A parse never panics and never builds more than the limits allow: an alias
//! bomb is refused by counting what an alias stands for (see `build.rs`), nesting and sizes are checked as the events
//! arrive, and every error is a small value with a line and a column but none of the text.

mod build;
mod flatten;
mod json;
mod key;
mod node;
mod properties;
mod scalar;
mod validate;
mod yaml;

use std::fmt;

use domain::NfsPath;

pub(crate) use self::build::{TreeBuilder, Warnings, clamp32};
pub use self::flatten::flatten;
pub use self::key::{DecodedPath, PathSegment, decode_setting_path, encode_setting_path};
pub use self::node::{
    Body, Document, Entry, Flat, FlatSetting, Node, NodeId, NodeKind, ParseWarning, Parsed, Position,
    PropertyEntry, Scalar, ScalarStyle, ScalarTag, Validation,
};
pub use self::validate::validate;
pub(crate) use self::validate::{combine_warnings, decode_for_parse};
pub(crate) use self::yaml::SourceLines;
use crate::bytes::file_extension;
use crate::error::{LimitKind, ParseError};
use crate::limits::Limits;

/// How a structured file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Syntax {
    Yaml,
    Json,
    Properties,
}

impl Syntax {
    /// The syntax of a file by its extension (`.yml`, `.yaml`, `.json`, `.properties`, in any case), or `None` for a file
    /// that is not structured.
    pub fn of_path(path: &NfsPath) -> Option<Self> {
        let ext = file_extension(path.file_name())?;
        if ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml") {
            Some(Self::Yaml)
        } else if ext.eq_ignore_ascii_case("json") {
            Some(Self::Json)
        } else if ext.eq_ignore_ascii_case("properties") {
            Some(Self::Properties)
        } else {
            None
        }
    }
}

impl fmt::Display for Syntax {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Yaml => "YAML",
            Self::Json => "JSON",
            Self::Properties => ".properties",
        })
    }
}

/// The `Parsed` of documents that some other reader fed to a `TreeBuilder` (the tree-sitter reader of the settings index,
/// T3). It has no warnings of its own: the builder's go into `warnings`.
pub(crate) fn assemble(syntax: Syntax, builder: TreeBuilder<'_>, mut warnings: Warnings) -> Parsed {
    let documents = builder.finish(&mut warnings);
    Parsed {
        syntax,
        body: Body::Documents(documents),
        warnings: warnings.items,
        warnings_dropped: warnings.dropped,
    }
}

/// Reads `text` as `syntax`. A leading byte order mark is ignored. Lines count `\n`, `\r\n` and a lone `\r` once each, as
/// `normalise_eol` does.
///
/// # Errors
///
/// `Syntax` for text that is not valid (with its line and column), `UnknownAlias` for an alias that names nothing yet, and
/// `Limit` when the text is over `Limits::input_bytes`, nests deeper than `Limits::nesting_depth` (counting what aliases
/// bring in), stands for more than `Limits::nodes` nodes once aliases are expanded, or holds a value longer than
/// `Limits::scalar_bytes` or a key longer than `Limits::setting_path_bytes`.
pub fn parse(text: &str, syntax: Syntax, limits: &Limits) -> Result<Parsed, ParseError> {
    if text.len() > limits.input_bytes {
        return Err(ParseError::Limit(LimitKind::InputBytes));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut warnings = Warnings::new(limits.warnings);
    let body = match syntax {
        Syntax::Properties => Body::Properties(properties::read(text, limits, &mut warnings)?),
        Syntax::Yaml | Syntax::Json => {
            let mut builder = TreeBuilder::new(limits, syntax);
            if syntax == Syntax::Yaml {
                yaml::read(text, limits, &mut builder)?;
            } else {
                json::read(text, limits, &mut builder)?;
            }
            Body::Documents(builder.finish(&mut warnings))
        }
    };
    Ok(Parsed {
        syntax,
        body,
        warnings: warnings.items,
        warnings_dropped: warnings.dropped,
    })
}
