//! Encoding, BOM and text (D17, rule 11): what a file's bytes say about how to read them, and text that goes in and out of
//! bytes without changing them.
//!
//! The rule is the usual one. A byte order mark is a declaration: a file that starts with a UTF-8 or UTF-16 mark is read
//! as that, and if it then turns out not to be valid, `decode_text` says so and does not guess. A file with no mark is
//! UTF-8 when it is valid UTF-8, and Latin-1 otherwise (every byte is a Latin-1 character, so this never fails). Because
//! of that, any bytes that decode are written back by `encode_text` to the very same bytes.

use std::borrow::Cow;

use bytes::Bytes;
use domain::{FileFormat, TextEncoding};

use super::eol::eol_style;
use crate::error::{DecodeError, EncodeError};
use crate::limits::Limits;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const UTF16LE_BOM: [u8; 2] = [0xFF, 0xFE];
const UTF16BE_BOM: [u8; 2] = [0xFE, 0xFF];

/// A file's bytes read as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded<'a> {
    /// The text without its byte order mark. Line endings are as they are in the file; `normalise_eol` the bytes first
    /// to get LF. UTF-8 text is borrowed from the input, not copied.
    pub text: Cow<'a, str>,
    /// What `encode_text` needs to write the text back.
    pub format: FileFormat,
}

/// What a file's first bytes declare, with the body that follows the byte order mark.
enum Sniffed<'a> {
    /// UTF-8, declared by a mark or found valid. `text` is `None` when the body is not valid UTF-8, which only a file with
    /// a mark can be.
    Utf8 {
        bom: bool,
        text: Option<&'a str>,
    },
    Utf16 {
        big_endian: bool,
        body: &'a [u8],
    },
    Latin1,
}

fn sniff(data: &[u8]) -> Sniffed<'_> {
    if let Some(body) = data.strip_prefix(&UTF8_BOM) {
        return Sniffed::Utf8 {
            bom: true,
            text: std::str::from_utf8(body).ok(),
        };
    }
    if let Some(body) = data.strip_prefix(&UTF16LE_BOM) {
        return Sniffed::Utf16 {
            big_endian: false,
            body,
        };
    }
    if let Some(body) = data.strip_prefix(&UTF16BE_BOM) {
        return Sniffed::Utf16 {
            big_endian: true,
            body,
        };
    }
    match std::str::from_utf8(data) {
        Ok(text) => Sniffed::Utf8 {
            bom: false,
            text: Some(text),
        },
        Err(_) => Sniffed::Latin1,
    }
}

fn format_of(sniffed: &Sniffed<'_>, data: &[u8]) -> FileFormat {
    let (encoding, bom) = match sniffed {
        Sniffed::Utf8 { bom, .. } => (TextEncoding::Utf8, *bom),
        Sniffed::Utf16 {
            big_endian: false, ..
        } => (TextEncoding::Utf16Le, true),
        Sniffed::Utf16 { big_endian: true, .. } => (TextEncoding::Utf16Be, true),
        Sniffed::Latin1 => (TextEncoding::Latin1, false),
    };
    FileFormat {
        eol: eol_style(data),
        encoding,
        bom,
    }
}

/// The encoding, byte order mark and line-ending style of a file.
///
/// The format of a file whose bytes are not valid for what its mark declares is still the declared one; `decode_text`
/// is where that is reported.
pub fn detect_format(data: &[u8]) -> FileFormat {
    format_of(&sniff(data), data)
}

/// Read a file as text. The text is borrowed from `data` when it is UTF-8.
///
/// # Errors
///
/// `TooLarge` above `Limits::input_bytes`, and `InvalidUtf8` or `InvalidUtf16` when a byte order mark declares an
/// encoding that the bytes after it are not valid in. The error never contains any of the input.
pub fn decode_text<'a>(data: &'a [u8], limits: &Limits) -> Result<Decoded<'a>, DecodeError> {
    if data.len() > limits.input_bytes {
        return Err(DecodeError::TooLarge);
    }
    let sniffed = sniff(data);
    let format = format_of(&sniffed, data);
    let text = match sniffed {
        Sniffed::Utf8 { text, .. } => Cow::Borrowed(text.ok_or(DecodeError::InvalidUtf8)?),
        Sniffed::Utf16 { big_endian, body } => Cow::Owned(decode_utf16(body, big_endian)?),
        Sniffed::Latin1 => Cow::Owned(data.iter().map(|&b| char::from(b)).collect()),
    };
    Ok(Decoded { text, format })
}

fn decode_utf16(body: &[u8], big_endian: bool) -> Result<String, DecodeError> {
    if body.len() % 2 != 0 {
        return Err(DecodeError::InvalidUtf16);
    }
    let units = body.chunks_exact(2).map(|pair| {
        let pair = [pair[0], pair[1]];
        if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        }
    });
    let mut text = String::with_capacity(body.len() / 2);
    for decoded in char::decode_utf16(units) {
        text.push(decoded.map_err(|_| DecodeError::InvalidUtf16)?);
    }
    Ok(text)
}

/// Write text as bytes in the given format: the byte order mark the format asks for, then the text in its encoding.
///
/// Line endings are not changed here; use `apply_eol` on the result. `format.eol` is not used.
///
/// # Errors
///
/// - `TooLarge` when the result would be longer than `Limits::input_bytes`;
/// - `Unrepresentable` when the format is Latin-1 and the text has a character outside it;
/// - `BadFormat` for UTF-16 without a mark (it could not be told from UTF-8 when read back) and for a mark on Latin-1.
pub fn encode_text(text: &str, format: FileFormat, limits: &Limits) -> Result<Bytes, EncodeError> {
    // No text that fits once encoded is longer than this, so a longer one is refused before anything is allocated.
    if text.len() > limits.input_bytes.saturating_mul(2) {
        return Err(EncodeError::TooLarge);
    }
    let out = match (format.encoding, format.bom) {
        (TextEncoding::Utf8, bom) => {
            let mut out = Vec::with_capacity(UTF8_BOM.len() + text.len());
            if bom {
                out.extend_from_slice(&UTF8_BOM);
            }
            out.extend_from_slice(text.as_bytes());
            out
        }
        (TextEncoding::Latin1, false) => text
            .chars()
            .map(|c| u8::try_from(u32::from(c)).map_err(|_| EncodeError::Unrepresentable))
            .collect::<Result<Vec<u8>, _>>()?,
        (TextEncoding::Utf16Le, true) => encode_utf16(text, UTF16LE_BOM, u16::to_le_bytes),
        (TextEncoding::Utf16Be, true) => encode_utf16(text, UTF16BE_BOM, u16::to_be_bytes),
        (TextEncoding::Latin1, true) | (TextEncoding::Utf16Le | TextEncoding::Utf16Be, false) => {
            return Err(EncodeError::BadFormat);
        }
    };
    if out.len() > limits.input_bytes {
        return Err(EncodeError::TooLarge);
    }
    Ok(Bytes::from(out))
}

fn encode_utf16(text: &str, bom: [u8; 2], unit: fn(u16) -> [u8; 2]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bom.len() + text.len() * 2);
    out.extend_from_slice(&bom);
    for u in text.encode_utf16() {
        out.extend_from_slice(&unit(u));
    }
    out
}
