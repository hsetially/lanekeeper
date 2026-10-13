//! Deterministic fact engine: classification, parsing, property view, rendering, drift, diffs, grid, checks C1-C11, text search.
//!
//! Owned by prompt 04. Read `AGENTS.md` and `prompts/04*.md` before changing this crate.
//!
//! Every view in the UI and every MCP answer is built from these functions (D33). They are pure: no I/O, no clock, no
//! async, no global state, and no unordered maps, so that identical input gives byte-identical output. A function takes
//! typed inputs (`NfsPath`, `LogicalFile`, `ContentHash`, `Bytes`), not text that nobody has checked (S11), and returns
//! an error value that never contains the input (S21). Sizes are bounded by [`Limits`].
//!
//! Callers run the heavy functions from a blocking context (`spawn_blocking`); the batch functions take the rayon pool
//! they run on.
#![forbid(unsafe_code)]
// One-line validating functions do not need a `# Errors` section each: the error enums are the documentation.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

mod batch;
mod bytes;
mod checks;
mod diff;
mod drift;
mod effect;
mod effective;
mod error;
mod grid;
mod index;
mod limits;
mod parse;
mod paths;
mod resolution;
mod search;
mod secret_scan;
mod severity;
mod tree_hash;

pub use self::batch::{
    AdoptionCandidate, AdoptionScore, BatchRules, FileState, LaneFile, LaneHistory, LaneInput, LaneState,
    MAX_ADOPTION_CANDIDATES, MAX_BATCH_SWIMLANES, MAX_LANE_FILES, NfsContent, Similarity, SwimlaneFindings,
    adoption_rank, findings_for_swimlanes, recompute_all, recompute_paths,
};
pub use self::bytes::{
    DEFAULT_DENY_GLOBS, Decoded, DenyMatcher, Newline, apply_eol, classify, decode_text, detect_format,
    dominant_eol, encode_text, normalise_eol,
};
pub use self::checks::{
    BaseCommit, BaseHistory, BaseVersion, ChangedSetting, ChannelsYml, CheckDetail, CheckFile, CheckInput,
    CheckResult, ConfigServerFacts, DuplicatedKey, EnvNames, EqualBy, FolderSeen, HostMatcher, HostRule,
    HostRules, MismatchedHost, OrphanReason, Overwrite, PlaceholderFile, RestartReason, Structured,
    TenantView, UnresolvedKey, c1_missed_base_changes, c2_redundant_tenant_file, c3_uneven_tenant_files,
    c4_orphan_file, c5_drift, c6_environment_host_mismatch, c7_overwritten_nfs_change, c8_duplicate_keys,
    c9_ambiguous_file_names, c10_unresolved_placeholders, c11_config_server_restart_required,
    checks_c1_to_c8,
};
pub use self::diff::{
    BinaryDiff, BinaryFacts, DIFF_CONTEXT_LINES, SettingView, TextDiff, binary_compare, semantic_diff,
    text_diff,
};
pub use self::drift::{Baseline, Drift, DriftInput, DriftLabel, IntentionalMark, drift_hash, drift_state};
pub use self::effect::{
    EffectChange, EffectInput, EffectPreview, EffectScope, EffectWarning, effect_preview,
};
pub use self::effective::{Effective, EffectiveValue, Layer, Layers, Origin, Source, effective};
pub use self::error::{
    BatchError, DecodeError, DenyError, EncodeError, HostRulesError, LimitKind, ParseError, ResolveError,
    ScanError, SearchError, SeverityError, TreeHashError,
};
pub use self::grid::{
    GridColumn, GridEntry, GridError, GridOptions, GridResult, GridValue, MAX_GRID_COLUMNS, grid,
};
pub use self::index::{LocationSource, SettingRow, SettingsIndex, index_settings, locate};
pub use self::limits::Limits;
pub use self::parse::{
    Body, DecodedPath, Document, Entry, Flat, FlatSetting, Node, NodeId, NodeKind, ParseWarning, Parsed,
    PathSegment, Position, PropertyEntry, Scalar, ScalarStyle, ScalarTag, Syntax, Validation,
    decode_setting_path, encode_setting_path, flatten, parse, validate,
};
pub use self::paths::{FileFacts, RepoSource, classify_path, map_path, reverse_map};
pub use self::resolution::{
    ClientSettings, DEFAULT_CACHE_TTL_SECS, Pickup, PickupChange, PickupInput, Placeholder, PropertyFile,
    PropertyView, Rendered, RenderedFile, ResourceChoice, SearchLocations, ViewSetting, file_role,
    is_channel_dir, logical_file_role, merge_mode, pickup_state, property_view, render, render_file,
    resolve_resource, scan_placeholders, search_locations,
};
pub use self::search::{
    CancelFlag, SearchBlob, SearchCorpus, SearchLocation, SearchOutcome, SearchStats, search_text,
};
pub use self::secret_scan::{ScanConfig, SecretMatch, SecretRule, secret_scan};
pub use self::severity::{
    ChangeContext, SeverityMatch, SeverityOutcome, SeverityRule, SeverityRules, default_rules, severity,
};
pub use self::tree_hash::{TreeHashes, effective_tree_hash, normalised_hash, swimlane_hash};
