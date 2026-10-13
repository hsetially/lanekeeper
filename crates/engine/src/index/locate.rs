//! `locate` (T3): the lines of one setting.

use domain::{LineRange, SettingPath};

use super::settings::index_settings;
use crate::error::ParseError;
use crate::limits::Limits;
use crate::parse::Syntax;

/// The lines of the setting `path` in the file `data`, or `None` when the file has no such setting.
///
/// It reads the file the way `index_settings` does, under the same limits and with the same agreement between the two
/// readers, so a range it gives is always the range of the row the index holds. A file that the parser cannot read (a syntax
/// error, a limit, bytes that are not text) is an error and not a guess: a caller that links to lines must know that there
/// are none. A YAML file that the parser reads and tree-sitter, the second reader, does not confirm is not an error: the row
/// keeps the parser's range, which is the range every other function of the engine uses, and `index_settings` says so with a
/// `LocationMismatch` warning that `locate` does not return. A caller that wants the warning asks for the index.
///
/// # Errors
///
/// The errors of `index_settings`.
pub fn locate(
    data: &[u8],
    syntax: Syntax,
    path: &SettingPath,
    limits: &Limits,
) -> Result<Option<LineRange>, ParseError> {
    Ok(index_settings(data, syntax, limits)?.locate(path))
}
