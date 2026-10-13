//! Diffs (T5, D65, D76, P7): what differs between two versions of a file, in the form each class of file needs.
//!
//! - **text**: hunks for a virtualised view, from Patience over lines, with line endings normalised first. A difference in
//!   line endings alone is hidden and flagged (`eol_only`). A text too big to diff within the limits is reported as such.
//! - **semantic**: the settings that differ, for a `structured` file, sorted by path, each with the lines it is written on.
//! - **binary**: hash and size.
//!
//! Every function is a function of its arguments: the same two inputs give the same output, byte for byte.

mod align;
mod binary;
mod semantic;
mod text;

pub use self::binary::{BinaryDiff, BinaryFacts, binary_compare};
pub use self::semantic::{SettingView, semantic_diff};
pub use self::text::{DIFF_CONTEXT_LINES, TextDiff, text_diff};
