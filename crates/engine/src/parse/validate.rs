//! `validate` (T2, S11): is this file acceptable? Bytes in, a verdict out.

use super::node::{Body, ParseWarning, Validation};
use super::{Syntax, flatten, parse};
use crate::bytes::{Decoded, decode_text};
use crate::error::{DecodeError, LimitKind, ParseError};
use crate::limits::Limits;

/// The bytes of a file as text, with the errors the way the parsing functions report them: a file over
/// `Limits::input_bytes` is `Limit(InputBytes)`, and a byte order mark that does not fit the bytes is `Decode`.
pub(crate) fn decode_for_parse<'a>(data: &'a [u8], limits: &Limits) -> Result<Decoded<'a>, ParseError> {
    decode_text(data, limits).map_err(|e| match e {
        DecodeError::TooLarge => ParseError::Limit(LimitKind::InputBytes),
        other => ParseError::Decode(other),
    })
}

/// Two lists of warnings as one: the first list, then the second, at most `cap` in all. What does not fit is counted with
/// the ones the earlier steps dropped.
pub(crate) fn combine_warnings(
    first: Vec<ParseWarning>,
    first_dropped: usize,
    second: Vec<ParseWarning>,
    second_dropped: usize,
    cap: usize,
) -> (Vec<ParseWarning>, usize) {
    let mut warnings = first;
    let mut dropped = first_dropped + second_dropped;
    for warning in second {
        if warnings.len() < cap {
            warnings.push(warning);
        } else {
            dropped += 1;
        }
    }
    (warnings, dropped)
}

/// Decodes the bytes, parses them as `syntax` and flattens the result, so that every limit and every syntax rule is
/// checked. Used before a file is written (S11) and by anything that has to decide whether a file can be read.
///
/// # Errors
///
/// The errors of `decode_text`, `parse` and `flatten`; a file over `Limits::input_bytes` is `Limit(InputBytes)`.
pub fn validate(data: &[u8], syntax: Syntax, limits: &Limits) -> Result<Validation, ParseError> {
    let decoded = decode_for_parse(data, limits)?;
    let parsed = parse(&decoded.text, syntax, limits)?;
    let flat = flatten(&parsed, limits)?;
    let documents = match &parsed.body {
        Body::Documents(documents) => documents.len(),
        Body::Properties(_) => 0,
    };
    // The parse warnings come first, and the cap applies to all of them together.
    let (warnings, warnings_dropped) = combine_warnings(
        parsed.warnings,
        parsed.warnings_dropped,
        flat.warnings,
        flat.warnings_dropped,
        limits.warnings,
    );
    Ok(Validation {
        documents,
        settings: flat.settings.len(),
        warnings,
        warnings_dropped,
    })
}
