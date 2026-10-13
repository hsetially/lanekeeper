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

/// Why a list of severity rules was refused. The error never contains the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SeverityError {
    /// More rules than `SeverityRules::MAX_RULES`.
    #[error("too many severity rules")]
    TooManyRules,
    /// Two rules have the same id.
    #[error("two severity rules have the same id")]
    DuplicateId,
    /// A glob longer than `SeverityRules::MAX_GLOB_BYTES`.
    #[error("a severity rule glob is too long")]
    GlobTooLong,
    /// A glob is not valid, or the rules are too complex to compile.
    #[error("a severity rule glob could not be compiled")]
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

/// Why a batch function refused its input (T8). Small values that name the problem and never the input, like every error here.
/// The checks are made before any parallel work starts, so the same input gives the same error however many threads ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BatchError {
    /// More swimlanes than `MAX_BATCH_SWIMLANES`.
    #[error("the batch has more swimlanes than the limit")]
    TooManySwimlanes,
    /// A swimlane with more files than `MAX_LANE_FILES`.
    #[error("a swimlane has more files than the limit")]
    TooManyFiles,
    /// More branches than `MAX_ADOPTION_CANDIDATES` to rank.
    #[error("there are more branches to rank than the limit")]
    TooManyCandidates,
    /// The same swimlane listed twice.
    #[error("a swimlane is listed twice")]
    DuplicateSwimlane,
    /// The same path listed twice in one swimlane.
    #[error("a path is listed twice in one swimlane")]
    DuplicatePath,
    /// `recompute_paths` was asked about a swimlane that has no earlier state, or no current files.
    #[error("a swimlane has no earlier state or no current files")]
    UnknownSwimlane,
    /// The effective files of a swimlane do not form a tree: a name is both a file and a directory.
    #[error("the effective files of a swimlane do not form a tree")]
    InconsistentTree,
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

/// Why `search_text` refused a query. The error never contains the pattern or the glob: a pattern may be a pasted secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    /// The pattern is empty (it would match every line).
    #[error("the search pattern is empty")]
    EmptyPattern,
    /// The pattern is longer than `Limits::regex_pattern_bytes`.
    #[error("the search pattern is longer than the limit")]
    PatternTooLong,
    /// The pattern is not a valid regex, holds a line break (a search reads one line at a time), or is over the size or
    /// nesting limit.
    #[error("the search pattern is not valid, or is too large or too complex")]
    InvalidPattern,
    /// The path filter is not a valid glob.
    #[error("the path filter is not a valid glob")]
    InvalidGlob,
    /// More blobs or places than the search can index.
    #[error("the set of files to search is too large")]
    CorpusTooLarge,
    /// The cancel flag was set. No partial answer is given.
    #[error("the search was cancelled")]
    Cancelled,
}

/// Why `secret_scan` refused a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// The text is longer than `Limits::input_bytes`.
    #[error("the text is larger than the size limit")]
    TooLarge,
    /// More matches than `ScanConfig::max_matches`. A short list would leave the rest unredacted, so there is none: the
    /// caller treats the text as unsafe to show.
    #[error("the text holds more secrets than can be listed")]
    TooManyMatches,
}

/// Why the config-server's resolution (T10) refused an input: a property view that is too big, or a file that cannot be
/// rendered. The error never contains the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// A size limit was exceeded: the property view holds more settings than `Limits::settings`, or the text to render, or
    /// the text it renders to, is longer than `Limits::input_bytes`.
    #[error("the {} limit was exceeded", .0.describe())]
    Limit(LimitKind),
    /// The file's bytes are not text of the encoding it declares.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The rendered text cannot be written back in the file's encoding.
    #[error(transparent)]
    Encode(#[from] EncodeError),
}
