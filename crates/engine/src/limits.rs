//! The engine's size limits, in one struct (S11, S21).
//!
//! Every limit is on a size, never on time: a wall-clock timeout would make the output depend on how busy the machine is,
//! and the engine's output must be byte-identical for identical input. A function that meets a limit returns an error
//! value that names the limit and never the input.
//!
//! The defaults are the plan's (`plans/04-fact-engine.md`, "Limits"). Every field can be overridden, and the code that
//! enforces a limit has a test at its boundary: exactly at the limit passes, one over fails.

/// Size limits for parsing, indexing, diffing and searching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// The largest file, in bytes. The agent and uploads stop at the same size.
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
    /// The work a text diff may do, in cells. Aligning two texts costs about the product of the lengths of the stretches
    /// between the lines that both texts have exactly once, summed over the stretches (the lines that only one text has, and
    /// the common beginning and end, are set aside first). Two files with the same keys are cheap however long; two files
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
        locations_per_finding: 50,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
