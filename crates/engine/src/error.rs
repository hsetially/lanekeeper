//! Error values (S21). A rejected value may be a secret, so no error carries input: each one is a small `Copy` enum that
//! says what was wrong, and `Display` and `Debug` print nothing else. The caller maps it to a problem response at the edge.

/// Why `decode_text` refused a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The file is longer than `Limits::input_bytes`.
    #[error("the input is larger than the size limit")]
    TooLarge,
    /// The file starts with a UTF-8 byte order mark and is not valid UTF-8.
    #[error("the file starts with a UTF-8 byte order mark but is not valid UTF-8")]
    InvalidUtf8,
    /// The file starts with a UTF-16 byte order mark and is not valid UTF-16 (an odd number of bytes, or a lone surrogate).
    #[error("the file starts with a UTF-16 byte order mark but is not valid UTF-16")]
    InvalidUtf16,
}

/// Why `encode_text` refused to write text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// The result would be longer than `Limits::input_bytes`.
    #[error("the text is larger than the size limit")]
    TooLarge,
    /// The file is Latin-1 and the text has a character that Latin-1 cannot hold. Writing it would change the text.
    #[error("the text has a character that the file's encoding cannot hold")]
    Unrepresentable,
    /// UTF-16 without a byte order mark (it could not be read back as UTF-16), or a byte order mark on Latin-1.
    #[error("the format asks for a byte order mark that does not fit the encoding")]
    BadFormat,
}

/// Why a list of deny globs was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DenyError {
    /// More globs than `DenyMatcher::MAX_EXTRA_GLOBS`.
    #[error("too many deny globs")]
    TooManyGlobs,
    /// A glob longer than `DenyMatcher::MAX_GLOB_BYTES`.
    #[error("a deny glob is too long")]
    GlobTooLong,
    /// A glob is not valid, or the list is too complex to compile.
    #[error("a deny glob could not be compiled")]
    InvalidGlob,
}
