//! Deterministic fact engine: classification, parsing, property view, rendering, drift, diffs, grid, checks C1-C11, text search.
//!
//! Owned by prompt 04. Read `AGENTS.md` and `prompts/04*.md` before changing this crate.
//!
//! Every view in the UI and every MCP answer is built from these functions (D33). They are pure: no I/O, no clock, no
//! async, no global state, and no unordered maps, so that identical input gives byte-identical output. A function takes
//! typed inputs (`NfsPath`, `LogicalFile`, `ContentHash`, `Bytes`), not text that nobody has checked (S11), and returns
//! an error value that never contains the input (S21). Sizes are bounded by [`Limits`].
//!
//! Callers run the heavy functions from a blocking context (`spawn_blocking`); the batch functions take the rayon pool
//! they run on.
#![forbid(unsafe_code)]
// One-line validating functions do not need a `# Errors` section each: the error enums are the documentation.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

mod bytes;
mod error;
mod index;
mod limits;
mod parse;

pub use self::bytes::{
    DEFAULT_DENY_GLOBS, Decoded, DenyMatcher, Newline, apply_eol, classify, decode_text, detect_format,
    dominant_eol, encode_text, normalise_eol,
};
pub use self::error::{DecodeError, DenyError, EncodeError, LimitKind, ParseError};
pub use self::index::{LocationSource, SettingRow, SettingsIndex, index_settings, locate};
pub use self::limits::Limits;
pub use self::parse::{
    Body, DecodedPath, Document, Entry, Flat, FlatSetting, Node, NodeId, NodeKind, ParseWarning, Parsed,
    PathSegment, Position, PropertyEntry, Scalar, ScalarStyle, ScalarTag, Syntax, Validation,
    decode_setting_path, encode_setting_path, flatten, parse, validate,
};
