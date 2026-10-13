//! Bytes: classes, encodings and line endings (T1). Everything here works on one file's bytes and nothing else.

mod classify;
mod decode;
mod eol;

pub use classify::{DEFAULT_DENY_GLOBS, DenyMatcher, classify};
pub use decode::{Decoded, decode_text, detect_format, encode_text};
pub use eol::{Newline, apply_eol, dominant_eol, normalise_eol};
