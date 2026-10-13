//! `validate` (T2, S11): is this file acceptable? Bytes in, a verdict out.

use super::node::{Body, Validation};
use super::{Syntax, flatten, parse};
use crate::bytes::decode_text;
use crate::error::{DecodeError, LimitKind, ParseError};
use crate::limits::Limits;

/// Decodes the bytes, parses them as `syntax` and flattens the result, so that every limit and every syntax rule is
/// checked. Used before a file is written (S11) and by anything that has to decide whether a file can be read.
///
/// # Errors
///
/// The errors of `decode_text`, `parse` and `flatten`; a file over `Limits::input_bytes` is `Limit(InputBytes)`.
pub fn validate(data: &[u8], syntax: Syntax, limits: &Limits) -> Result<Validation, ParseError> {
    let decoded = decode_text(data, limits).map_err(|e| match e {
        DecodeError::TooLarge => ParseError::Limit(LimitKind::InputBytes),
        other => ParseError::Decode(other),
    })?;
    let parsed = parse(&decoded.text, syntax, limits)?;
    let flat = flatten(&parsed, limits)?;
    let documents = match &parsed.body {
        Body::Documents(documents) => documents.len(),
        Body::Properties(_) => 0,
    };
    // The parse warnings come first, and the cap applies to all of them together.
    let mut warnings = parsed.warnings;
    let mut dropped = parsed.warnings_dropped + flat.warnings_dropped;
    for warning in flat.warnings {
        if warnings.len() < limits.warnings {
            warnings.push(warning);
        } else {
            dropped += 1;
        }
    }
    Ok(Validation {
        documents,
        settings: flat.settings.len(),
        warnings,
        warnings_dropped: dropped,
    })
}
