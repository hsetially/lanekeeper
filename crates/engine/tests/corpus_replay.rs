//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never
//! drift apart. The fuzz crate is the engine's own (`crates/engine/fuzz/`, plan decision A2): the root `fuzz/` crate holds
//! the domain's targets and is replayed by `crates/domain/tests/corpus_replay.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use engine::{LimitKind, LocationSource, ParseError, Syntax, flatten, index_settings, parse};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn dir_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn seed(target: &str, name: &str) -> Vec<u8> {
    fs::read(fuzz_dir().join("corpus").join(target).join(name))
        .unwrap_or_else(|e| panic!("{target}/{name}: {e}"))
}

fn replay(target: &str, check: invariants::Check) {
    let dir = fuzz_dir().join("corpus").join(target);
    let names = dir_names(&dir);
    assert!(
        names.len() >= invariants::MIN_SEEDS,
        "corpus for {target} has {} seeds, need at least {}",
        names.len(),
        invariants::MIN_SEEDS
    );
    for name in names {
        let bytes = fs::read(dir.join(&name)).unwrap();
        if let Err(why) = check(&bytes) {
            panic!("seed {target}/{name} violates an invariant: {why}");
        }
    }
}

#[test]
fn corpus_replay_all_engine_targets() {
    replay("engine_bytes", invariants::engine_bytes);
    replay("engine_yaml", invariants::engine_yaml);
    replay("engine_json", invariants::engine_json);
    replay("engine_properties", invariants::engine_properties);
    replay("engine_flatten", invariants::engine_flatten);
    replay("engine_index_settings", invariants::engine_index_settings);
    replay("engine_path_mapping", invariants::engine_path_mapping);
    replay("engine_regex", invariants::engine_regex);
    replay("engine_secret_scan", invariants::engine_secret_scan);
    replay("engine_render", invariants::engine_render);
}

#[test]
fn every_target_has_a_fuzz_binary_a_corpus_and_a_manifest_entry() {
    let targets: BTreeSet<String> = invariants::TARGETS.iter().map(|(t, _)| (*t).to_owned()).collect();
    let binaries: BTreeSet<String> = dir_names(&fuzz_dir().join("fuzz_targets"))
        .into_iter()
        .map(|f| f.trim_end_matches(".rs").to_owned())
        .collect();
    let corpora = dir_names(&fuzz_dir().join("corpus"));
    assert_eq!(
        targets, binaries,
        "fuzz/fuzz_targets must hold exactly the targets in fuzz/invariants.rs"
    );
    assert_eq!(
        targets, corpora,
        "fuzz/corpus must hold exactly the targets in fuzz/invariants.rs"
    );
    let manifest = fs::read_to_string(fuzz_dir().join("Cargo.toml")).unwrap();
    for target in &targets {
        assert!(
            manifest.contains(&format!("path = \"fuzz_targets/{target}.rs\"")),
            "fuzz/Cargo.toml has no [[bin]] for {target}"
        );
        // Every fuzz binary calls the check of the same name, so a target cannot assert nothing.
        let source =
            fs::read_to_string(fuzz_dir().join("fuzz_targets").join(format!("{target}.rs"))).unwrap();
        assert!(
            source.contains(&format!("invariants::{target}(data)")),
            "{target} does not run its check"
        );
    }
}

/// The replay is not blind: the checks do notice the harm they exist to find.
#[test]
fn the_checks_catch_a_planted_violation() {
    invariants::planted_harm_is_detected().unwrap();
}

/// The hostile seeds mean something only while they do what they are named for under the limits the fuzzer uses.
#[test]
fn the_hostile_seeds_hit_the_limits_they_are_named_for() {
    let limits = invariants::fuzz_limits();
    let text = |target: &str, name: &str| String::from_utf8(seed(target, name)).unwrap();
    let limit = |target: &str, name: &str, syntax: Syntax| match parse(&text(target, name), syntax, &limits) {
        Err(ParseError::Limit(kind)) => kind,
        other => panic!("{target}/{name}: expected a limit, got {:?}", other.map(|_| ())),
    };
    assert_eq!(limit("engine_yaml", "alias_bomb", Syntax::Yaml), LimitKind::Nodes);
    assert_eq!(limit("engine_yaml", "merge_bomb", Syntax::Yaml), LimitKind::Nodes);
    assert_eq!(
        limit("engine_yaml", "oversized", Syntax::Yaml),
        LimitKind::InputBytes
    );
    assert_eq!(limit("engine_yaml", "deep_flow", Syntax::Yaml), LimitKind::Depth);
    assert_eq!(limit("engine_yaml", "deep_block", Syntax::Yaml), LimitKind::Depth);
    assert_eq!(
        limit("engine_json", "oversized", Syntax::Json),
        LimitKind::InputBytes
    );
    assert_eq!(
        limit("engine_json", "deep_arrays", Syntax::Json),
        LimitKind::Depth
    );
    assert_eq!(
        limit("engine_json", "deep_objects", Syntax::Json),
        LimitKind::Depth
    );
    assert_eq!(
        limit("engine_properties", "oversized", Syntax::Properties),
        LimitKind::InputBytes
    );
    // The key longer than the path limit.
    assert_eq!(
        limit("engine_properties", "hostile_keys", Syntax::Properties),
        LimitKind::SettingPathBytes
    );
    assert!(matches!(
        parse(&text("engine_yaml", "self_reference"), Syntax::Yaml, &limits),
        Err(ParseError::UnknownAlias { .. })
    ));
    // Seeds that are meant to be good parse and give settings.
    for (target, name, syntax) in [
        ("engine_yaml", "valid_nested", Syntax::Yaml),
        ("engine_yaml", "valid_anchor_merge", Syntax::Yaml),
        ("engine_yaml", "valid_multidoc", Syntax::Yaml),
        ("engine_json", "valid_object", Syntax::Json),
        ("engine_json", "valid_multiline", Syntax::Json),
        ("engine_properties", "valid", Syntax::Properties),
        ("engine_properties", "continuation", Syntax::Properties),
    ] {
        let parsed = parse(&text(target, name), syntax, &limits).unwrap();
        assert!(
            !flatten(&parsed, &limits).unwrap().settings.is_empty(),
            "{target}/{name}"
        );
    }
}

#[test]
fn the_huge_scalar_seeds_hit_the_scalar_limit() {
    let limits = invariants::fuzz_limits();
    for (target, name, syntax) in [
        ("engine_yaml", "huge_scalar", Syntax::Yaml),
        ("engine_yaml", "huge_quoted_scalar", Syntax::Yaml),
        ("engine_json", "huge_scalar", Syntax::Json),
        ("engine_properties", "huge_scalar", Syntax::Properties),
    ] {
        let text = String::from_utf8(seed(target, name)).unwrap();
        assert_eq!(
            parse(&text, syntax, &limits).map(|_| ()),
            Err(ParseError::Limit(LimitKind::ScalarBytes)),
            "{target}/{name}"
        );
    }
    let key = String::from_utf8(seed("engine_yaml", "huge_key")).unwrap();
    assert_eq!(
        parse(&key, Syntax::Yaml, &limits).map(|_| ()),
        Err(ParseError::Limit(LimitKind::SettingPathBytes))
    );
}

/// The index seeds are not blind either: the hostile ones are refused under the fuzzer's limits, the good ones are indexed
/// (a YAML file by tree-sitter, with nothing to report), and the selector byte picks the syntax.
#[test]
fn the_index_seeds_are_refused_or_indexed_as_named() {
    let limits = invariants::fuzz_limits();
    let run = |name: &str| {
        let data = seed("engine_index_settings", name);
        let syntax = invariants::syntax_of_selector(data[0]);
        index_settings(&data[1..], syntax, &limits)
    };
    for (name, kind) in [
        ("yaml_alias_bomb", LimitKind::Nodes),
        ("yaml_merge_bomb", LimitKind::Nodes),
        ("yaml_deep_flow", LimitKind::Depth),
        ("yaml_deep_block", LimitKind::Depth),
        ("yaml_oversized", LimitKind::InputBytes),
        ("yaml_huge_scalar", LimitKind::ScalarBytes),
        ("json_deep_arrays", LimitKind::Depth),
        ("properties_oversized", LimitKind::InputBytes),
    ] {
        assert!(
            matches!(run(name), Err(ParseError::Limit(got)) if got == kind),
            "{name}: expected the limit on {kind}, got {:?}",
            run(name).map(|i| i.rows.len())
        );
    }
    for name in [
        "yaml_truncated_flow",
        "yaml_unknown_alias",
        "yaml_self_reference",
        "json_truncated_object",
    ] {
        assert!(run(name).is_err(), "{name} should be refused");
    }
    for name in [
        "yaml_valid_nested",
        "yaml_valid_anchor_merge",
        "yaml_valid_multidoc",
        "yaml_block_scalar_then_indented_sibling",
        "yaml_block_scalar_in_sequence",
        "yaml_block_scalar_empty",
        "yaml_flow_pairs_in_sequence",
        "yaml_empty_items_and_values",
        "yaml_anchor_redefined",
        "yaml_merge_chain",
        "yaml_document_markers",
        "yaml_utf16le",
        "yaml_utf16be",
    ] {
        let index = run(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!index.rows.is_empty(), "{name}");
        assert_eq!(index.located_by, LocationSource::TreeSitter, "{name}");
        assert!(
            !index
                .warnings
                .iter()
                .any(|w| matches!(w, engine::ParseWarning::LocationMismatch { .. })),
            "{name}: tree-sitter disagreed with the parser: {:?}",
            index.warnings
        );
    }
    for name in [
        "json_valid_object",
        "json_nested_multiline",
        "properties_valid",
        "properties_continued_values",
    ] {
        let index = run(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!index.rows.is_empty(), "{name}");
        assert_eq!(index.located_by, LocationSource::Parser, "{name}");
    }
}

/// The path-mapping seeds are not blind either: each is refused, mapped or read as the name says, so a seed cannot go on
/// passing after the code it was written for has changed.
#[test]
fn the_path_mapping_seeds_reach_what_they_are_named_for() {
    use domain::{AppName, NfsPath, PathMappingRule, RepoPath, ReverseMapping, TenantId, TenantSet};
    use engine::reverse_map;

    let pieces = |name: &str| -> Vec<String> {
        String::from_utf8_lossy(&seed("engine_path_mapping", name))
            .split('\0')
            .map(str::to_owned)
            .collect()
    };
    let rule = PathMappingRule::default();
    let tenants =
        |text: &str| TenantSet::new(text.split_whitespace().filter_map(|t| TenantId::parse(t).ok())).unwrap();
    let read = |name: &str| {
        let p = pieces(name);
        let nfs = NfsPath::parse(&p[0]).unwrap_or_else(|e| panic!("{name}: {e}"));
        reverse_map(&rule, &nfs, &tenants(&p[1]))
    };

    // Read as a tenant file, with the name the seed is named for.
    for name in [
        "valid_tenant_file",
        "hyphenated_names",
        "extensionless_suffix",
        "multi_dot_name",
        "dotfile_name",
        "longest_tenant_wins",
    ] {
        assert!(
            matches!(read(name), Ok(ReverseMapping::Tenant { .. })),
            "{name} is not read as a tenant file"
        );
    }
    // Read as a base file: no suffix, an unknown suffix, or nothing before the suffix.
    for name in [
        "valid_base_file",
        "orphan_unknown_suffix",
        "suffix_with_nothing_before",
    ] {
        assert!(
            matches!(read(name), Ok(ReverseMapping::Base { .. })),
            "{name} is not read as a base file"
        );
    }
    // Refused before the mapping: not a plain relative path, or a `.git` component.
    for name in ["traversal", "absolute_and_backslash", "empty", "oversized_path"] {
        assert!(
            NfsPath::parse(&pieces(name)[0]).is_err(),
            "{name}: the NFS path should be refused"
        );
    }
    assert!(read("git_metadata").is_err());
    for name in [
        "traversal",
        "git_metadata",
        "root_only_repo_path",
        "repo_path_outside_root",
    ] {
        let repo = &pieces(name)[2];
        let mapped = RepoPath::parse(repo)
            .ok()
            .and_then(|r| engine::map_path(&rule, &engine::RepoSource::Base, &r).ok());
        assert!(
            mapped.is_none(),
            "{name}: the repository path should not map to a base file"
        );
    }
    // More tenants than a swimlane can have.
    let many = &pieces("many_tenants")[1];
    assert!(TenantSet::new(many.split_whitespace().filter_map(|t| TenantId::parse(t).ok())).is_err());
    // The application names after the repository path: the hostile ones are refused by `AppName`, so they never reach
    // `file_role`; the plain ones do.
    let names = |name: &str| pieces(name)[3..].to_vec();
    assert!(names("bad_globs").iter().all(|n| AppName::parse(n).is_err()));
    assert!(names("long_glob").iter().all(|n| AppName::parse(n).is_err()));
    assert!(names("glob_depth").iter().all(|n| AppName::parse(n).is_err()));
}

/// The regex seeds are not blind either: each is refused, searched or skipped as the name says, so a seed cannot go on
/// passing after the code it was written for has changed.
#[test]
fn the_regex_seeds_reach_what_they_are_named_for() {
    use domain::{FileClass, NfsPath, SwimlaneId, TextQuery};
    use engine::{
        CancelFlag, DenyMatcher, SearchBlob, SearchCorpus, SearchError, SearchLocation, normalised_hash,
        search_text,
    };

    let run = |name: &str| {
        let data = seed("engine_regex", name);
        let flags = data[0];
        let nul = data[1..].iter().position(|&b| b == 0).unwrap_or(data.len() - 1);
        let pattern = String::from_utf8(data[1..=nul].to_vec()).unwrap();
        let hay = data.get(nul + 2..).unwrap_or_default().to_vec();
        let query = TextQuery {
            pattern,
            regex: flags & 1 != 0,
            case_sensitive: flags & 2 != 0,
            swimlanes: Vec::new(),
            path_glob: None,
            max_total: if flags & 4 != 0 {
                3
            } else {
                TextQuery::DEFAULT_MAX_TOTAL
            },
            max_per_file: if flags & 8 != 0 {
                2
            } else {
                TextQuery::DEFAULT_MAX_PER_FILE
            },
        };
        let content = bytes::Bytes::from(hay);
        let corpus = SearchCorpus::new(vec![SearchBlob {
            hash: normalised_hash(&content, FileClass::Binary),
            content,
            locations: vec![SearchLocation {
                swimlane: SwimlaneId::parse("sit1").unwrap(),
                path: NfsPath::parse("a.txt").unwrap(),
            }],
        }])
        .unwrap();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let limits = invariants::search_limits();
        search_text(
            &corpus,
            &query,
            &DenyMatcher::default(),
            &limits,
            &pool,
            &CancelFlag::new(),
        )
    };
    assert_eq!(
        run("pattern_over_512_bytes").unwrap_err(),
        SearchError::PatternTooLong
    );
    for name in [
        "nested_groups_over_limit",
        "size_limit_exceeded",
        "line_break_in_pattern",
        "unclosed_group",
    ] {
        assert_eq!(run(name).unwrap_err(), SearchError::InvalidPattern, "{name}");
    }
    assert_eq!(run("empty_pattern").unwrap_err(), SearchError::EmptyPattern);
    for (name, hits) in [
        ("literal_valid", 2),
        ("literal_dot_is_a_dot", 2),
        ("literal_case_insensitive", 2),
        ("regex_anchors", 1),
        ("regex_alternation_word_boundary", 2),
        ("crlf_haystack_anchor", 3),
        ("lone_cr_lines", 3),
        ("catastrophic_backtracking", 0),
        ("many_matches_in_a_line", 1),
        ("long_line_cut_inside_a_character", 1),
    ] {
        let found = run(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(found.results.hits.len(), hits, "{name}");
        assert_eq!(found.stats.blobs_searched, 1, "{name}");
    }
    let capped = run("caps_total_and_per_file").unwrap();
    assert_eq!(capped.results.hits.len(), 2);
    assert!(capped.results.truncated);
    let binary = run("binary_haystack_nul").unwrap();
    assert!(binary.results.hits.is_empty());
    assert_eq!(binary.stats.blobs_binary, 1);
    // The cut falls inside a two-byte letter, and the line is cut before it.
    let cut = run("long_line_cut_inside_a_character").unwrap();
    assert_eq!(cut.results.hits[0].text.len(), 63);
}

/// And the scan seeds: each finds, or leaves alone, what its name says.
#[test]
fn the_secret_scan_seeds_reach_what_they_are_named_for() {
    use engine::{ScanConfig, ScanError, SecretRule, secret_scan};

    let scan = |name: &str, config: &ScanConfig| {
        let data = seed("engine_secret_scan", name);
        let text = String::from_utf8(data[1..].to_vec()).unwrap();
        secret_scan(&text, config, &invariants::fuzz_limits())
    };
    let plain = ScanConfig::default();
    let rules =
        |name: &str| -> Vec<SecretRule> { scan(name, &plain).unwrap().iter().map(|m| m.rule).collect() };
    assert_eq!(
        rules("private_key_block"),
        [
            SecretRule::PrivateKey,
            SecretRule::PrivateKey,
            SecretRule::PrivateKey
        ]
    );
    assert_eq!(rules("private_key_unterminated").len(), 3);
    assert_eq!(rules("private_key_inline_json"), [SecretRule::PrivateKey]);
    for quiet in [
        "public_key_and_certificate",
        "names_and_placeholders",
        "words_and_paths",
        "hex_digest_and_uuid",
        "truncated_input",
    ] {
        assert!(rules(quiet).is_empty(), "{quiet}: {:?}", rules(quiet));
    }
    assert_eq!(
        rules("github_tokens"),
        [
            SecretRule::GithubToken,
            SecretRule::GithubToken,
            SecretRule::GithubToken
        ]
    );
    assert_eq!(
        rules("aws_and_other_prefixes"),
        [
            SecretRule::AwsAccessKey,
            SecretRule::SlackToken,
            SecretRule::GitlabToken,
            SecretRule::StripeKey
        ]
    );
    assert_eq!(rules("jwt"), [SecretRule::Jwt]);
    assert_eq!(
        rules("random_token_values"),
        [SecretRule::HighEntropy, SecretRule::HighEntropy]
    );
    assert_eq!(rules("non_ascii_before_token").len(), 2);
    // Settings change what is found.
    let hex = ScanConfig {
        hex_is_random: true,
        ..plain.clone()
    };
    assert_eq!(scan("hex_digest_flagged_when_asked", &hex).unwrap().len(), 1);
    assert!(scan("hex_digest_flagged_when_asked", &plain).unwrap().is_empty());
    let loose = ScanConfig {
        min_random_len: 16,
        min_entropy_millibits: 3_000,
        ..plain.clone()
    };
    assert_eq!(scan("looser_thresholds", &loose).unwrap().len(), 1);
    assert!(scan("looser_thresholds", &plain).unwrap().is_empty());
    // Past the cap there is an error and no list.
    let capped = ScanConfig {
        max_matches: 64,
        ..plain.clone()
    };
    assert_eq!(
        scan("many_matches_over_the_cap", &capped).unwrap_err(),
        ScanError::TooManyMatches
    );
    assert_eq!(
        scan("one_character_lines_in_a_key", &capped).unwrap_err(),
        ScanError::TooManyMatches
    );
}
