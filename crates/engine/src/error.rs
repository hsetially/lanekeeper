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

/// Why a list of merge rules was refused (interim: T10 deletes `MergeRules`, and this with it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MergeRulesError {
    /// More rules than `MergeRules::MAX_RULES`.
    #[error("too many merge rules")]
    TooManyRules,
    /// A glob longer than `MergeRules::MAX_GLOB_BYTES`.
    #[error("a merge rule glob is too long")]
    GlobTooLong,
    /// A glob is not valid, or the list is too complex to compile.
    #[error("a merge rule glob could not be compiled")]
    InvalidGlob,
}

/// Why a list of host rules (check C6) was refused. The error never contains the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostRulesError {
    /// More rules than `HostRules::MAX_RULES`.
    #[error("too many host rules")]
    TooManyRules,
    /// A rule with nothing to match.
    #[error("a host rule is empty")]
    EmptyNeedle,
    /// A rule longer than a host name can be (`HostRules::MAX_NEEDLE_BYTES`).
    #[error("a host rule is too long")]
    NeedleTooLong,
    /// A rule with a character that a host name does not have (only ASCII letters, digits, `.`, `-` and `_` do).
    #[error("a host rule has a character that a host name cannot have")]
    InvalidNeedle,
}

/// Why `effective_tree_hash` refused its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TreeHashError {
    /// The same logical file twice, or a path that is a file in one entry and a directory in another. A tree has one
    /// answer for a path, so the input is wrong, not the tree.
    #[error("the files do not form a tree: a path appears twice, or as both a file and a directory")]
    Inconsistent,
}

/// Which of the engine's size limits was met (`Limits`). Naming it is all an error does: the input stays out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// `Limits::input_bytes`.
    InputBytes,
    /// `Limits::nesting_depth`, counting what aliases bring in.
    Depth,
    /// `Limits::nodes`, counting what aliases expand to (the alias-bomb defence).
    Nodes,
    /// `Limits::settings`.
    Settings,
    /// `Limits::scalar_bytes`.
    ScalarBytes,
    /// `Limits::setting_path_bytes`, for a key or for a whole path as it is written.
    SettingPathBytes,
}

impl LimitKind {
    /// What the limit is on, for messages.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::InputBytes => "file size",
            Self::Depth => "nesting depth",
            Self::Nodes => "node count",
            Self::Settings => "setting count",
            Self::ScalarBytes => "value size",
            Self::SettingPathBytes => "setting path length",
        }
    }
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.describe())
    }
}

/// Why `parse`, `flatten` or `validate` refused a file. A small `Copy` value: a line and a column at most, never any of
/// the text (S21), because a rejected file may hold a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The bytes are not text of the encoding the file declares, or are over the size limit.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The text is not valid for its syntax. The line and the column are 1-based.
    #[error("the file is not valid {syntax} (line {line}, column {column})")]
    Syntax {
        syntax: crate::parse::Syntax,
        line: u32,
        column: u32,
    },
    /// An alias names an anchor that is not defined, or is not finished, where the alias is.
    #[error("an alias on line {line} names an anchor that is not defined before it")]
    UnknownAlias { line: u32 },
    /// A size limit was met.
    #[error("the file is over the limit on {0}")]
    Limit(LimitKind),
}
