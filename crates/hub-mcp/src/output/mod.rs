//! What a tool answers with: the envelope, the byte cap, cursors, web links and redacted text (S14b, S15, D58).

pub mod cap;
pub mod cursor;
pub mod envelope;
pub mod links;
pub mod redact;

pub use redact::{CUT_MARK, RedactError, Redacted, RedactedText, Redactor, cut_with_mark, line_count};

/// Lower-case hexadecimal of `bytes`.
#[must_use]
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}
