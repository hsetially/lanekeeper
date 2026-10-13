//! The engine's size limits, in one struct (S11, S21).
//!
//! Every limit is on a size, never on time: a wall-clock timeout would make the output depend on how busy the machine is,
//! and the engine's output must be byte-identical for identical input. What a function does when it meets a limit depends on
//! what it is for, and the function says which in its documentation:
//!
//! - the functions that read or write a file (`parse`, `flatten`, `validate`, `index_settings`, `render`, `secret_scan`,
//!   `encode_text`) return an error value that names the limit and never the input;
//! - a text diff, which is shown to a person, gives the outcome `too_large` and no hunks (`diff_lines`, `diff_cells`);
//! - a search skips a blob that is over the file limit and counts it in the outcome, and stops at the caps on hits and
//!   says that the result was cut (`hits_total`, `hits_per_file`);
//! - a check lists at most `locations_per_finding` of anything and carries the full count.
//!
//! The defaults are the plan's (`plans/04-fact-engine.md`, "Limits"). Every field can be overridden, and the code that
//! enforces a limit has a test at its boundary: exactly at the limit passes, one over is refused, skipped or cut as above.

/// Size limits for parsing, indexing, diffing and searching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// The largest file, in bytes. The agent and uploads stop at the same size. The functions that take a file's bytes
    /// (`validate`, `index_settings`, `render_file`) hold the bytes to it; the text of a Latin-1 or UTF-16 file can be longer
    /// than the file, up to twice as long, and is not refused for that. The functions that take text (`parse`, `render`,
    /// `secret_scan`) hold the text to it.
    pub input_bytes: usize,
    /// The deepest nesting of maps and lists in a parsed file.
    pub nesting_depth: usize,
    /// Nodes in a parsed file after aliases are expanded. This is the defence against an alias bomb: expansion is
    /// counted and stopped at the limit, and the expanded tree is never built beyond it.
    pub nodes: usize,
    /// Settings (leaf values) in one file.
    pub settings: usize,
    /// The longest scalar value, in bytes.
    pub scalar_bytes: usize,
    /// The longest flattened setting path, in bytes. It is the length of `domain::SettingPath`.
    pub setting_path_bytes: usize,
    /// Parse warnings kept per file; the rest are counted and dropped.
    pub warnings: usize,
    /// Lines on one side of a text diff. A bigger file gets a "too large to diff" outcome.
    pub diff_lines: usize,
    /// The work a text diff may do, in cells. Aligning two texts costs about the square of half the combined length of each
    /// stretch between the lines that both texts have exactly once (the product of the lengths, when the sides are equally
    /// long), summed over the stretches (the lines that only one text has, and the common beginning and end, are set aside
    /// first). Two files with the same keys are cheap however long; two files
    /// with a shared vocabulary and no shared skeleton are not, and this is what keeps a hostile pair of 50,000-line files
    /// from holding a thread. It is a count of cells, not a time, so the outcome does not depend on how busy the machine is.
    pub diff_cells: usize,
    /// The longest line that gets word-level (inline) change ranges, in bytes.
    pub inline_line_bytes: usize,
    /// The longest search pattern, in bytes. It is `domain::TextQuery::MAX_PATTERN_BYTES`.
    pub regex_pattern_bytes: usize,
    /// The compiled size limit of a regex, in bytes.
    pub regex_size_limit: usize,
    /// The lazy DFA's cache limit, in bytes.
    pub regex_dfa_size_limit: usize,
    /// The deepest nesting of groups in a regex.
    pub regex_nest_limit: u32,
    /// Hits in a text search result. It is `domain::TextQuery::DEFAULT_MAX_TOTAL`.
    pub hits_total: usize,
    /// Hits from one file in a text search result. It is `domain::TextQuery::DEFAULT_MAX_PER_FILE`.
    pub hits_per_file: usize,
    /// The longest line text in a search hit, in bytes. A longer line is cut on a character boundary.
    pub hit_line_bytes: usize,
    /// Locations listed in one finding.
    pub locations_per_finding: usize,
}

impl Limits {
    /// The defaults, as a constant.
    pub const DEFAULT: Limits = Limits {
        input_bytes: 2 * 1024 * 1024,
        nesting_depth: 64,
        nodes: 500_000,
        settings: 200_000,
        scalar_bytes: 64 * 1024,
        setting_path_bytes: 1_024,
        warnings: 1_000,
        diff_lines: 50_000,
        diff_cells: 20_000_000,
        inline_line_bytes: 4 * 1024,
        regex_pattern_bytes: 512,
        regex_size_limit: 1024 * 1024,
        regex_dfa_size_limit: 8 * 1024 * 1024,
        regex_nest_limit: 50,
        hits_total: 500,
        hits_per_file: 20,
        hit_line_bytes: 512,
        locations_per_finding: 50,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
