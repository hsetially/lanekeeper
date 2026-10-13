//! `index_settings` (T3): a blob as rows of the settings index.

use domain::SettingPath;

use super::yaml_ts::{self, Outcome};
use super::{LocationSource, SettingRow, SettingsIndex};
use crate::error::ParseError;
use crate::limits::Limits;
use crate::parse::{Body, ParseWarning, Syntax, combine_warnings, decode_for_parse, flatten, parse};

/// One row for every setting of the file `data`, read as `syntax`, in file order.
///
/// The bytes are decoded (a byte order mark is honoured, Latin-1 is the fallback), parsed and flattened under `limits`, so
/// every limit and every syntax rule of `validate` applies. For YAML the lines are then confirmed by tree-sitter, which
/// is given only a text that the parser has already accepted (see the module documentation).
///
/// # Errors
///
/// The errors of `decode_text`, `parse` and `flatten`. A file that parses has an index: tree-sitter failing, or disagreeing
/// with the parser, is a warning on the rows and never an error.
pub fn index_settings(data: &[u8], syntax: Syntax, limits: &Limits) -> Result<SettingsIndex, ParseError> {
    let decoded = decode_for_parse(data, limits)?;
    let parsed = parse(&decoded.text, syntax, limits)?;
    let flat = flatten(&parsed, limits)?;
    let (mut warnings, mut dropped) = combine_warnings(
        parsed.warnings,
        parsed.warnings_dropped,
        flat.warnings,
        flat.warnings_dropped,
        limits.warnings,
    );

    let mut located_by = LocationSource::Parser;
    let mut note = |path: Option<SettingPath>, warnings: &mut Vec<ParseWarning>| {
        if warnings.len() < limits.warnings {
            warnings.push(ParseWarning::LocationMismatch { path });
        } else {
            dropped += 1;
        }
    };
    if let (Syntax::Yaml, Body::Documents(documents)) = (syntax, &parsed.body) {
        match yaml_ts::confirm(&decoded.text, documents.len(), &flat.settings, limits) {
            Outcome::Confirmed(agreed) => {
                located_by = LocationSource::TreeSitter;
                for (setting, agrees) in flat.settings.iter().zip(&agreed) {
                    if !agrees {
                        note(Some(setting.path.clone()), &mut warnings);
                    }
                }
            }
            Outcome::Unreadable => note(None, &mut warnings),
        }
    }

    let rows = flat
        .settings
        .into_iter()
        .map(|setting| SettingRow {
            path: setting.path,
            value: setting.value,
            lines: setting.lines,
        })
        .collect();
    Ok(SettingsIndex {
        rows,
        warnings,
        warnings_dropped: dropped,
        located_by,
    })
}
