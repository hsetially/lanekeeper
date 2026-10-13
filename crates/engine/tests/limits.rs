//! T1 (S11, S21): the engine's size limits, in one struct. Exceeding one gives an error that names the limit and never
//! the input; the tests that exercise each limit at its boundary come with the code that enforces it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{SettingPath, TextQuery};
use engine::Limits;

#[test]
fn defaults_are_the_documented_values() {
    let l = Limits::default();
    assert_eq!(l.input_bytes, 2 * 1024 * 1024);
    assert_eq!(l.nesting_depth, 64);
    assert_eq!(l.nodes, 500_000);
    assert_eq!(l.settings, 200_000);
    assert_eq!(l.scalar_bytes, 64 * 1024);
    assert_eq!(l.setting_path_bytes, 1_024);
    assert_eq!(l.warnings, 1_000);
    assert_eq!(l.diff_lines, 50_000);
    assert_eq!(l.inline_line_bytes, 4 * 1024);
    assert_eq!(l.regex_pattern_bytes, 512);
    assert_eq!(l.regex_size_limit, 1024 * 1024);
    assert_eq!(l.regex_dfa_size_limit, 8 * 1024 * 1024);
    assert_eq!(l.regex_nest_limit, 50);
    assert_eq!(l.hits_total, 500);
    assert_eq!(l.hits_per_file, 20);
    assert_eq!(l.locations_per_finding, 50);
}

#[test]
fn limits_that_the_contract_also_states_agree_with_it() {
    let l = Limits::default();
    // `SettingPath` is the type every setting path ends up in; a path the engine builds must fit.
    assert!(SettingPath::parse(&"a".repeat(l.setting_path_bytes)).is_ok());
    assert!(SettingPath::parse(&"a".repeat(l.setting_path_bytes + 1)).is_err());
    assert_eq!(l.regex_pattern_bytes, TextQuery::MAX_PATTERN_BYTES);
    assert_eq!(l.hits_total, TextQuery::DEFAULT_MAX_TOTAL as usize);
    assert_eq!(l.hits_per_file, TextQuery::DEFAULT_MAX_PER_FILE as usize);
}

#[test]
fn every_default_is_nonzero_and_ordered_sensibly() {
    let l = Limits::default();
    assert!(l.scalar_bytes < l.input_bytes);
    assert!(l.hits_per_file <= l.hits_total);
    assert!(l.regex_size_limit <= l.regex_dfa_size_limit);
    for v in [
        l.nesting_depth,
        l.nodes,
        l.settings,
        l.warnings,
        l.diff_lines,
        l.locations_per_finding,
    ] {
        assert!(v > 0);
    }
}
