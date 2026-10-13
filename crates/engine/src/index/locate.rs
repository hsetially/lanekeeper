//! `locate` (T3): the lines of one setting.

use domain::{LineRange, SettingPath};

use super::settings::index_settings;
use crate::error::ParseError;
use crate::limits::Limits;
use crate::parse::Syntax;

/// The lines of the setting `path` in the file `data`, or `None` when the file has no such setting.
///
/// It reads the file the way `index_settings` does, under the same limits and with the same agreement between the two
/// readers, so a range it gives is always the range of the row the index holds. A file that cannot be read is an error and
/// not a guess: a caller that links to lines must know that there are none.
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
