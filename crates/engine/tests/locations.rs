//! T3 (S11, S22, P6, P7): the settings index and `locate`.
//!
//! The ranges of a YAML file come from tree-sitter. The parser has its own marks for the same settings, and the two have
//! to agree: on every setting of every fixture file, and on a hand-written table of the YAML shapes that are easy to get
//! wrong (anchors, merge keys, block scalars, flow collections, empty values, several documents, line endings). Where
//! they disagree at run time the row keeps the parser's range and says so with a warning, so any warning in these tests
//! is a failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use domain::{LineRange, SettingPath};
use engine::{
    Limits, LocationSource, ParseError, ParseWarning, SettingsIndex, Syntax, decode_text, flatten,
    index_settings, locate, parse,
};
use proptest::prelude::*;
use support::{files_below, small_fixtures};

fn limits() -> Limits {
    Limits::default()
}

fn syntax_of(name: &str) -> Option<Syntax> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "yml" | "yaml" => Some(Syntax::Yaml),
        "json" => Some(Syntax::Json),
        "properties" => Some(Syntax::Properties),
        _ => None,
    }
}

fn index(syntax: Syntax, text: &str) -> SettingsIndex {
    index_settings(text.as_bytes(), syntax, &limits())
        .unwrap_or_else(|e| panic!("{syntax} did not index: {e}\n{text}"))
}

fn yaml(text: &str) -> SettingsIndex {
    index(Syntax::Yaml, text)
}

fn mismatches(index: &SettingsIndex) -> Vec<&ParseWarning> {
    index
        .warnings
        .iter()
        .filter(|w| matches!(w, ParseWarning::LocationMismatch { .. }))
        .collect()
}

/// `path = "text" Lstart-end`, one line per row: the form the tables below are written in.
fn dump(index: &SettingsIndex) -> String {
    let mut out = String::new();
    for row in &index.rows {
        writeln!(
            out,
            "{} = {:?} L{}-{}",
            row.path,
            row.value.text,
            row.lines.start(),
            row.lines.end()
        )
        .unwrap();
    }
    out
}

/// The rows must be exactly the settings of `parse` and `flatten`, with the same ranges, and a YAML file must have been
/// located by tree-sitter with nothing to report.
fn assert_agrees(label: &str, syntax: Syntax, bytes: &[u8]) -> usize {
    let limits = limits();
    let index = index_settings(bytes, syntax, &limits).unwrap_or_else(|e| panic!("{label}: {e}"));
    let text = decode_text(bytes, &limits).unwrap().text.into_owned();
    let flat = flatten(&parse(&text, syntax, &limits).unwrap(), &limits).unwrap();
    assert_eq!(
        index.rows.len(),
        flat.settings.len(),
        "{label}: number of settings"
    );
    for (row, setting) in index.rows.iter().zip(&flat.settings) {
        assert_eq!(row.path, setting.path, "{label}: order of settings");
        assert_eq!(row.value, setting.value, "{label}: {}", row.path);
        assert_eq!(
            row.lines, setting.lines,
            "{label}: {} is at {:?} for the index and at {:?} for the parser",
            row.path, row.lines, setting.lines
        );
    }
    assert!(
        mismatches(&index).is_empty(),
        "{label}: tree-sitter disagreed: {:?}",
        mismatches(&index)
    );
    let expected = if syntax == Syntax::Yaml {
        LocationSource::TreeSitter
    } else {
        LocationSource::Parser
    };
    assert_eq!(index.located_by, expected, "{label}");
    index.rows.len()
}

#[test]
fn tree_sitter_agrees_with_parser_on_every_fixture_setting() {
    let fx = small_fixtures();
    let (mut files, mut settings, mut yaml_files) = (0, 0, 0);
    for (rel, path) in files_below(&fx.root.join("nfs")) {
        let Some(syntax) = syntax_of(&rel) else { continue };
        let bytes = std::fs::read(&path).unwrap();
        settings += assert_agrees(&rel, syntax, &bytes);
        files += 1;
        yaml_files += usize::from(syntax == Syntax::Yaml);
    }
    assert!(files > 200, "{files} files");
    assert!(yaml_files > 200, "{yaml_files} YAML files");
    assert!(settings > 10_000, "{settings} settings");
}

/// The same at target scale (about 80,000 files, 472 MB): `cargo test -p engine --test locations -- --ignored`. Files with
/// the same content are checked once.
#[test]
#[ignore = "target scale: generates and reads about 80,000 files"]
fn tree_sitter_agrees_with_parser_on_every_fixture_setting_at_target_scale() {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let base = std::env::var_os("LK_FIXTURES_DIR").map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/target"),
        std::path::PathBuf::from,
    );
    if !base.join("manifest.json").is_file() {
        xtask::fixtures::generate(&xtask::fixtures::Config {
            scale: xtask::fixtures::Scale::Target,
            seed: xtask::fixtures::DEFAULT_SEED,
            out: base.clone(),
        })
        .unwrap();
    }
    let mut seen = BTreeSet::new();
    let (mut files, mut settings) = (0_usize, 0_usize);
    for (rel, path) in files_below(&base.join("nfs")) {
        let Some(syntax) = syntax_of(&rel) else { continue };
        let bytes = std::fs::read(&path).unwrap();
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        if !seen.insert((bytes.len(), hasher.finish())) {
            continue;
        }
        settings += assert_agrees(&rel, syntax, &bytes);
        files += 1;
    }
    assert!(
        files > 100 && settings > 100_000,
        "{files} files, {settings} settings"
    );
}

// --- YAML: the shapes that are easy to get wrong -------------------------------------------------------------------

#[test]
fn locations_plain_nested_and_sequences() {
    let text = "a: 1\nb:\n  c: 2\n  d:\n    - x\n    - y: 3\n      z: 4\n    - - p\n      - q\ne: end\n";
    let index = yaml(text);
    insta::assert_snapshot!("locations_nested", dump(&index));
    assert!(mismatches(&index).is_empty());
    assert_eq!(index.located_by, LocationSource::TreeSitter);
}

#[test]
fn locations_block_scalars_with_trailing_blank_lines() {
    let text = "\
literal: |
  line one
  line two

folded: >-
  folded
  text


keep: |+
  kept

after: 1
";
    let index = yaml(text);
    insta::assert_snapshot!("locations_block_scalars", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

#[test]
fn locations_multi_line_scalars_and_values_on_the_next_line() {
    let text = "\
quoted: \"one\n  two\"
plain: a
  b
  c
single: 'x

  y'
next:
  value on the next line
key with spaces: v
\"quoted key\": 1
'single key': 2
";
    let index = yaml(text);
    insta::assert_snapshot!("locations_multi_line", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

#[test]
fn locations_flow_collections() {
    let text = "\
flow: [1, 2,
  3]
map: {a: 1, b: 2,
  c: 3}
empty_list: []
empty_map: {}
nested: {x: [1, {y: 2}], z: []}
seq_of_pairs: [a: 1, b: 2]
";
    let index = yaml(text);
    insta::assert_snapshot!("locations_flow", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

#[test]
fn locations_empty_values() {
    let text = "a:\nb: ~\nc:   # comment\nd:\n  e:\n- ignored\n";
    // `- ignored` after a mapping is a syntax error, so use the valid part and a separate sequence.
    let valid = text.replace("- ignored\n", "");
    let index = yaml(&valid);
    insta::assert_snapshot!("locations_empty_values", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
    let seq = yaml("-\n- a\n-\n  - b\n- c:\n");
    insta::assert_snapshot!("locations_empty_items", dump(&seq));
    assert!(mismatches(&seq).is_empty(), "{:?}", seq.warnings);
}

#[test]
fn locations_anchors_aliases_and_merge_keys() {
    let text = "\
defaults: &defaults
  timeoutMs: 3000
  retries: 3
service-a:
  <<: *defaults
  name: a
service-b:
  <<: *defaults
  timeoutMs: 100
  name: b
scalar_anchor: &s hello
scalar_alias: *s
list: &list
  - x
  - y
copy: *list
multi:
  <<: [*defaults, {extra: 1}]
  own: yes
anchored_value: &deep
  inner: &leaf 5
alias_of_leaf: *leaf
";
    let index = yaml(text);
    insta::assert_snapshot!("locations_anchors_aliases", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
    // A setting that came through an alias is located where its value is written.
    let a = index
        .rows
        .iter()
        .find(|r| r.path.as_str() == "service-a.timeoutMs")
        .unwrap();
    assert_eq!(a.lines, LineRange::new(2, 2).unwrap());
}

#[test]
fn locations_anchors_and_tags_on_values_on_the_next_line() {
    let text = "\
a: &x
  hello
b: *x
c: !!str
  tagged
d: !!str 12
e: &y !!int 5
f: *y
";
    let index = yaml(text);
    insta::assert_snapshot!("locations_anchors_next_line", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

#[test]
fn locations_several_documents_and_markers() {
    let text = "a: 1\n---\nb: 2\n---\n- x\n- y\n---\nplain\n...\n---\n---\nlast: 1\n";
    let index = yaml(text);
    insta::assert_snapshot!("locations_documents", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
    let marker = yaml("---\n");
    assert!(mismatches(&marker).is_empty(), "{:?}", marker.warnings);
    assert_eq!(dump(&marker), "$ = \"\" L1-1\n");
    let directive = yaml("%YAML 1.2\n---\na: 1\n");
    assert!(mismatches(&directive).is_empty(), "{:?}", directive.warnings);
}

#[test]
fn locations_comments_and_blank_lines_do_not_move_ranges() {
    let text = "# head\n\na: 1 # trailing\n\n# between\nb:\n  # inside\n  c: 2\n\n\nd: |\n  text\n  # not a comment\n# tail\n";
    let index = yaml(text);
    insta::assert_snapshot!("locations_comments", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

#[test]
fn locations_special_keys_are_found_by_their_written_path() {
    let text = "\
\"a.b\": 1
\"c[0]\": 2
d:
  \"x.y\": 3
  \"e]f\": 4
  \"\": 5
  \"tab\\there\": 6
  'it''s': 7
\"#0/looks-like-a-document\": 8
\"$\": 9
? explicit
: 10
";
    let index = yaml(text);
    insta::assert_snapshot!("locations_special_keys", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

#[test]
fn locations_duplicate_keys_use_the_last_one() {
    let text = "a: 1\nb: 2\na: 3\nc:\n  d: 1\n  d: 2\n";
    let index = yaml(text);
    insta::assert_snapshot!("locations_duplicates", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

#[test]
fn locations_are_the_same_for_every_line_ending() {
    let lf = "a: 1\nb:\n  c: |\n    one\n    two\n  d: [1,\n    2]\ne: &x v\nf: *x\n";
    let expected = dump(&yaml(lf));
    assert_eq!(dump(&yaml(&lf.replace('\n', "\r\n"))), expected, "CRLF");
    assert_eq!(dump(&yaml(&lf.replace('\n', "\r"))), expected, "lone CR");
    let bom = format!("\u{feff}{lf}");
    assert_eq!(dump(&yaml(&bom)), expected, "BOM");
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in lf.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    let from_utf16 = index_settings(&utf16, Syntax::Yaml, &limits()).unwrap();
    assert_eq!(dump(&from_utf16), expected, "UTF-16");
    assert!(mismatches(&from_utf16).is_empty());
}

#[test]
fn locations_with_non_ascii_text_are_by_line_not_by_byte() {
    let text = "ключ: значение\nzh: \"你好\n  world\"\nemoji: 🙂\nlast: 1\n";
    let index = yaml(text);
    insta::assert_snapshot!("locations_non_ascii", dump(&index));
    assert!(mismatches(&index).is_empty(), "{:?}", index.warnings);
}

// --- where the two readers disagree ----------------------------------------------------------------------------------

/// tree-sitter reads a block scalar used as an explicit key as written, not decoded, so it does not find the path that
/// the parser found. The row is still there, with the parser's range, and a warning names it; the rows around it are
/// unaffected.
#[test]
fn a_disagreement_keeps_the_parsers_range_and_says_so_for_that_setting_only() {
    let text = "ok: 1\n? |\n  block key\n: 2\nafter: 3\n";
    let index = yaml(text);
    assert_eq!(index.located_by, LocationSource::TreeSitter);
    let flat = flatten(&parse(text, Syntax::Yaml, &limits()).unwrap(), &limits()).unwrap();
    assert_eq!(index.rows.len(), 3);
    for (row, setting) in index.rows.iter().zip(&flat.settings) {
        assert_eq!((&row.path, row.lines), (&setting.path, setting.lines));
    }
    let named: Vec<&SettingPath> = index
        .warnings
        .iter()
        .filter_map(|w| match w {
            ParseWarning::LocationMismatch { path: Some(p) } => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(named.len(), 1, "{:?}", index.warnings);
    assert_eq!(named[0].as_str(), index.rows[1].path.as_str());
    assert_eq!(index.rows[1].lines, LineRange::new(2, 4).unwrap());
    assert_eq!(index.locate(&path("after")), Some(LineRange::new(5, 5).unwrap()));
}

/// A file that tree-sitter cannot read at all is indexed from the parser, with one warning for the file.
#[test]
fn a_file_tree_sitter_cannot_read_is_indexed_from_the_parser_with_one_warning() {
    // A document end marker before anything: the parser starts a document after it, tree-sitter sees an error.
    let index = yaml("...\n--a,:");
    assert_eq!(index.located_by, LocationSource::Parser);
    assert_eq!(index.rows.len(), 1);
    assert_eq!(
        index
            .warnings
            .iter()
            .filter(|w| matches!(w, ParseWarning::LocationMismatch { path: None }))
            .count(),
        1,
        "{:?}",
        index.warnings
    );
}

/// The warning for a disagreement takes its place in the cap with the others, and is counted when it does not fit.
#[test]
fn location_warnings_count_against_the_warning_cap() {
    let tight = Limits {
        warnings: 1,
        ..Limits::default()
    };
    let index = index_settings(b"...\n--a,:", Syntax::Yaml, &tight).unwrap();
    assert_eq!(index.warnings.len(), 1);
    // And with a duplicate key taking the one place, the location warning is counted, not lost.
    let index = index_settings(b"...\na: 1\na: 2\n", Syntax::Yaml, &tight).unwrap();
    assert_eq!(
        index.warnings.len() + index.warnings_dropped,
        2,
        "{:?}",
        index.warnings
    );
}

// --- JSON and .properties ------------------------------------------------------------------------------------------

#[test]
fn json_and_properties_lines() {
    let json = index(
        Syntax::Json,
        "{\n  \"a\": 1,\n  \"b\": {\n    \"c\": \"x\",\n    \"d\": [\n      1,\n      2\n    ]\n  },\n  \"e\": [],\n  \"f\": \"s\"\n}\n",
    );
    insta::assert_snapshot!("locations_json", dump(&json));
    assert_eq!(json.located_by, LocationSource::Parser);

    let props = index(
        Syntax::Properties,
        "# comment\na=1\n\nb.c = two\nlong=first \\\n   second\nd:3\ne f\n! other comment\ng=\n",
    );
    insta::assert_snapshot!("locations_properties", dump(&props));
    assert_eq!(props.located_by, LocationSource::Parser);
    // Every key of a `.properties` file is on a line of its own, and a continued value runs on from there.
    for row in &props.rows {
        assert!(row.lines.end() >= row.lines.start());
    }
}

// --- locate --------------------------------------------------------------------------------------------------------

fn path(text: &str) -> SettingPath {
    SettingPath::parse(text).unwrap()
}

#[test]
fn locate_finds_a_setting_and_says_none_for_one_that_is_not_there() {
    let text = b"a: 1\nb:\n  c: |\n    x\n    y\n  d: 2\n";
    let l = limits();
    assert_eq!(
        locate(text, Syntax::Yaml, &path("b.c"), &l).unwrap(),
        Some(LineRange::new(3, 5).unwrap())
    );
    assert_eq!(
        locate(text, Syntax::Yaml, &path("b.d"), &l).unwrap(),
        Some(LineRange::new(6, 6).unwrap())
    );
    assert_eq!(locate(text, Syntax::Yaml, &path("b.nope"), &l).unwrap(), None);
    // The path of a container is not a setting: only leaves are indexed.
    assert_eq!(locate(text, Syntax::Yaml, &path("b"), &l).unwrap(), None);
    let json = b"{\n \"k\": {\n  \"v\": 1\n }\n}";
    assert_eq!(
        locate(json, Syntax::Json, &path("k.v"), &l).unwrap(),
        Some(LineRange::new(3, 3).unwrap())
    );
    let props = b"x=1\ny=2\n";
    assert_eq!(
        locate(props, Syntax::Properties, &path("y"), &l).unwrap(),
        Some(LineRange::new(2, 2).unwrap())
    );
}

#[test]
fn locate_reports_a_file_it_cannot_read_instead_of_a_guess() {
    let err = locate(b"a: [1\n", Syntax::Yaml, &path("a"), &limits()).unwrap_err();
    assert!(matches!(err, ParseError::Syntax { .. }), "{err:?}");
}

#[test]
fn index_settings_refuses_what_the_parser_refuses() {
    let l = limits();
    assert!(matches!(
        index_settings(b"a: [1\n", Syntax::Yaml, &l),
        Err(ParseError::Syntax { .. })
    ));
    assert!(matches!(
        index_settings(b"a: *nope\n", Syntax::Yaml, &l),
        Err(ParseError::UnknownAlias { .. })
    ));
    let tight = Limits {
        input_bytes: 8,
        ..Limits::default()
    };
    assert!(matches!(
        index_settings(b"a: 1\nb: 2\nc: 3\n", Syntax::Yaml, &tight),
        Err(ParseError::Limit(engine::LimitKind::InputBytes))
    ));
    let deep = format!("{}1{}", "[".repeat(100), "]".repeat(100));
    assert!(matches!(
        index_settings(deep.as_bytes(), Syntax::Yaml, &l),
        Err(ParseError::Limit(engine::LimitKind::Depth))
    ));
    // A UTF-8 byte order mark on bytes that are not UTF-8.
    assert!(matches!(
        index_settings(b"\xEF\xBB\xBFa: \xFF\n", Syntax::Yaml, &l),
        Err(ParseError::Decode(_))
    ));
}

#[test]
fn the_alias_bomb_is_refused_before_anything_reads_it_twice() {
    let mut bomb = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x]\n");
    for i in 1..12 {
        writeln!(
            bomb,
            "a{i}: &a{i} [*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p}]",
            p = i - 1
        )
        .unwrap();
    }
    let result = index_settings(bomb.as_bytes(), Syntax::Yaml, &limits());
    assert!(
        matches!(result, Err(ParseError::Limit(engine::LimitKind::Nodes))),
        "{:?}",
        result.map(|i| i.rows.len())
    );
}

#[test]
fn rows_hold_the_value_and_the_type_the_parser_reports() {
    use domain::{ValueFlag, ValueType};
    let index = yaml("n: 12\nf: 1.5\nb: true\ns: text\nq: \"12\"\nz: ~\ny: yes\n");
    let by = |p: &str| {
        index
            .rows
            .iter()
            .find(|r| r.path.as_str() == p)
            .unwrap_or_else(|| panic!("no row for {p}"))
    };
    assert_eq!(by("n").value.ty, ValueType::Int);
    assert_eq!(by("f").value.ty, ValueType::Float);
    assert_eq!(by("b").value.ty, ValueType::Bool);
    assert_eq!(by("s").value.ty, ValueType::String);
    assert_eq!(by("q").value.ty, ValueType::String);
    assert_eq!(by("q").value.text, "12");
    assert_eq!(by("z").value.ty, ValueType::Null);
    assert_eq!(by("y").value.flag, Some(ValueFlag::YamlBooleanLike));
    // The parser's warnings come with the index: the boolean-like value, and a duplicate key.
    assert!(
        index
            .warnings
            .iter()
            .any(|w| matches!(w, ParseWarning::BooleanLikeValue { .. }))
    );
    let dup = yaml("a: 1\na: 2\n");
    assert!(
        dup.warnings
            .iter()
            .any(|w| matches!(w, ParseWarning::DuplicateKey { .. }))
    );
    assert_eq!(dup.rows.len(), 1);
}

#[test]
fn warnings_are_capped_and_counted() {
    let mut text = String::new();
    for i in 0..30 {
        writeln!(text, "k{i}: yes").unwrap();
    }
    let tight = Limits {
        warnings: 5,
        ..Limits::default()
    };
    let index = index_settings(text.as_bytes(), Syntax::Yaml, &tight).unwrap();
    assert_eq!(index.warnings.len(), 5);
    assert_eq!(index.warnings_dropped, 25);
    assert_eq!(index.rows.len(), 30);
}

#[test]
fn a_file_with_no_settings_has_no_rows() {
    for (syntax, text) in [
        (Syntax::Yaml, ""),
        (Syntax::Yaml, "# only a comment\n"),
        (Syntax::Properties, "# nothing\n"),
    ] {
        let index = index(syntax, text);
        assert!(index.rows.is_empty(), "{syntax}: {text:?}");
        assert!(mismatches(&index).is_empty());
    }
}

// --- properties of the whole -------------------------------------------------------------------------------------

/// A node of a random YAML document.
#[derive(Debug, Clone)]
enum Tree {
    /// Written on the line of its key or its `-`.
    Inline(String),
    /// `|` or `>` and its text (one entry per line, may be empty).
    Block(char, Vec<String>),
    Seq(Vec<Tree>),
    /// `merge` puts `<<: *base` first.
    Map {
        merge: bool,
        pairs: Vec<(String, Tree)>,
    },
}

fn inline_scalar() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-z][a-z0-9]{0,6}".prop_map(String::from),
        2 => (0_u32..1000).prop_map(|n| n.to_string()),
        1 => Just("true".to_owned()),
        1 => Just("yes".to_owned()),
        1 => Just("~".to_owned()),
        1 => Just("\"quoted: value\"".to_owned()),
        1 => Just("'single ''quoted'".to_owned()),
        1 => Just("${PLACEHOLDER}".to_owned()),
        1 => Just("a long plain value with spaces".to_owned()),
        1 => Just("[1, 2, 3]".to_owned()),
        1 => Just("{k: v, j: [w, {z: 1}]}".to_owned()),
        1 => Just("[]".to_owned()),
        1 => Just("{}".to_owned()),
        1 => Just("*base".to_owned()),
        1 => Just("*list".to_owned()),
        1 => Just("&here anchored".to_owned()),
        1 => Just("!!str 12".to_owned()),
    ]
}

fn map_key() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[a-z][a-zA-Z0-9_-]{0,6}".prop_map(String::from),
        1 => Just("\"dotted.key\"".to_owned()),
        1 => Just("'with space'".to_owned()),
        1 => Just("\"esc\\tape\"".to_owned()),
    ]
}

fn tree() -> impl Strategy<Value = Tree> {
    let leaf = prop_oneof![
        5 => inline_scalar().prop_map(Tree::Inline),
        1 => (
            prop_oneof![Just('|'), Just('>')],
            prop::collection::vec("([a-z][a-z ]{0,7})?", 0..4),
        )
            .prop_map(|(c, lines)| Tree::Block(c, lines)),
    ];
    leaf.prop_recursive(5, 96, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..5).prop_map(Tree::Seq),
            (any::<bool>(), prop::collection::vec((map_key(), inner), 1..6))
                .prop_map(|(merge, pairs)| Tree::Map { merge, pairs }),
        ]
    })
}

/// Pads the next line, and sometimes puts a comment or a blank line in front of it. `noise` is a small generator state, so a
/// document is a function of the strategy's seed.
fn line_start(out: &mut String, noise: &mut u32, pad: &str, skip_pad: bool) {
    *noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    if skip_pad {
        return;
    }
    match (*noise >> 16) % 7 {
        0 => writeln!(out, "{pad}# a comment").unwrap(),
        1 => out.push('\n'),
        _ => {}
    }
    out.push_str(pad);
}

/// The lines of `tree` at `indent`, each ending in a line feed. The first line has no padding when `compact` (it follows
/// `- ` on the same line).
fn render(tree: &Tree, indent: usize, compact: bool, out: &mut String, noise: &mut u32) {
    let pad = " ".repeat(indent);
    match tree {
        Tree::Inline(text) => {
            out.push_str(text);
            out.push('\n');
        }
        Tree::Block(style, lines) => {
            out.push(*style);
            out.push('\n');
            for line in lines {
                out.push_str(&" ".repeat(indent + 2));
                out.push_str(line.trim_end());
                out.push('\n');
            }
            if lines.iter().all(|l| l.trim().is_empty()) {
                // A block scalar with no text still needs a following line to be unambiguous.
                out.push_str(&" ".repeat(indent + 2));
                out.push_str("text\n");
            }
        }
        Tree::Seq(items) => {
            for (i, item) in items.iter().enumerate() {
                line_start(out, noise, &pad, compact && i == 0);
                out.push_str("- ");
                render_child(item, indent + 2, out, noise, true);
            }
        }
        Tree::Map { merge, pairs } => {
            let mut first = compact;
            if *merge {
                line_start(out, noise, &pad, first);
                first = false;
                out.push_str("<<: *base\n");
            }
            for (k, v) in pairs {
                line_start(out, noise, &pad, first);
                first = false;
                out.push_str(k);
                out.push(':');
                render_child(v, indent + 2, out, noise, false);
            }
        }
    }
}

/// What follows `key:` or `- `.
fn render_child(tree: &Tree, indent: usize, out: &mut String, noise: &mut u32, after_dash: bool) {
    match tree {
        Tree::Inline(_) | Tree::Block(..) => {
            if !after_dash {
                out.push(' ');
            }
            // The text of a block scalar is indented under the key or the dash, not under the text.
            render(tree, indent - 2, false, out, noise);
        }
        Tree::Seq(_) | Tree::Map { .. } if after_dash => render(tree, indent, true, out, noise),
        Tree::Seq(_) | Tree::Map { .. } => {
            out.push('\n');
            render(tree, indent, false, out, noise);
        }
    }
}

/// A random YAML document: nested maps and sequences, scalars in every style, block scalars, flow collections, comments,
/// blank lines, an anchored map and list that later values refer to (aliases and merge keys). Nearly every document it makes
/// is valid, so each one is a chance for tree-sitter to disagree with the parser.
fn yaml_text() -> impl Strategy<Value = String> {
    (tree(), any::<u32>(), any::<bool>()).prop_map(|(tree, seed, with_prelude)| {
        let mut out = String::new();
        let mut noise = seed;
        if with_prelude {
            out.push_str("base: &base\n  timeoutMs: 3000\n  retries: 3\nlist: &list\n  - 1\n  - 2\n");
        } else {
            // The names must exist when the body uses them.
            out.push_str("base: &base\n  timeoutMs: 1\nlist: &list [1]\n");
        }
        out.push_str("body:\n");
        // The body is a map or a sequence, indented under its key; anything else is wrapped in a map.
        match &tree {
            Tree::Seq(_) | Tree::Map { .. } => render(&tree, 2, false, &mut out, &mut noise),
            leaf => {
                out.push_str("  only: ");
                render(leaf, 2, false, &mut out, &mut noise);
            }
        }
        out
    })
}

/// The random documents are mostly valid, so the properties below compare tree-sitter with the parser on real settings
/// and not on refusals. (A generator that drifted into making only invalid text would keep them green and mean nothing.)
#[test]
fn the_random_documents_are_mostly_valid_and_varied() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;
    let strategy = yaml_text();
    let mut runner = TestRunner::deterministic();
    let (mut valid, mut settings, mut aliased, mut block, mut merged) =
        (0_usize, 0_usize, 0_usize, 0_usize, 0_usize);
    for _ in 0..300 {
        let text = strategy.new_tree(&mut runner).unwrap().current();
        let Ok(parsed) = parse(&text, Syntax::Yaml, &limits()) else {
            continue;
        };
        valid += 1;
        let flat = flatten(&parsed, &limits()).unwrap();
        settings += flat.settings.len();
        aliased += usize::from(flat.settings.iter().any(|s| s.via_alias));
        block += usize::from(
            text.contains(": |") || text.contains("- |") || text.contains(": >") || text.contains("- >"),
        );
        merged += usize::from(text.contains("<<: *base"));
    }
    assert!(
        valid >= 290,
        "only {valid} of 300 random documents are valid YAML"
    );
    assert!(settings >= 3_000, "{settings} settings in {valid} documents");
    assert!(block >= 100, "{block} documents with a block scalar");
    assert!(
        aliased >= 150,
        "{aliased} documents with a setting that came through an alias"
    );
    assert!(merged >= 100, "{merged} documents with a merge key");
}

proptest! {
    /// Whatever valid YAML the strategy makes, tree-sitter and the parser agree on every setting.
    #[test]
    fn tree_sitter_and_parser_agree_on_random_documents(text in yaml_text()) {
        let Ok(parsed) = parse(&text, Syntax::Yaml, &limits()) else {
            // The strategy can make a document the parser refuses; then there is nothing to compare.
            return Ok(());
        };
        let flat = flatten(&parsed, &limits()).unwrap();
        let index = index_settings(text.as_bytes(), Syntax::Yaml, &limits()).unwrap();
        prop_assert!(mismatches(&index).is_empty(), "{:?}\n{}", mismatches(&index), text);
        prop_assert_eq!(index.rows.len(), flat.settings.len());
        for (row, setting) in index.rows.iter().zip(&flat.settings) {
            prop_assert_eq!(&row.path, &setting.path);
            prop_assert_eq!(row.lines, setting.lines, "{}\n{}", row.path, text);
        }
    }

    /// `locate` finds every path the index lists, at the range the index gives.
    #[test]
    fn locate_returns_range_for_every_indexed_path_prop(text in yaml_text()) {
        let Ok(index) = index_settings(text.as_bytes(), Syntax::Yaml, &limits()) else {
            return Ok(());
        };
        for row in index.rows.iter().take(20) {
            prop_assert_eq!(
                locate(text.as_bytes(), Syntax::Yaml, &row.path, &limits()).unwrap(),
                Some(row.lines)
            );
            prop_assert_eq!(index.locate(&row.path), Some(row.lines));
        }
    }

    /// Indexing never panics on arbitrary bytes, says the same thing twice, and keeps every range inside the text.
    #[test]
    fn index_settings_on_arbitrary_bytes_is_total_and_deterministic(
        data in prop::collection::vec(any::<u8>(), 0..400),
        syntax in prop_oneof![Just(Syntax::Yaml), Just(Syntax::Json), Just(Syntax::Properties)],
    ) {
        let l = limits();
        let first = index_settings(&data, syntax, &l);
        let second = index_settings(&data, syntax, &l);
        prop_assert_eq!(&first, &second);
        if let Ok(index) = first {
            let lines = u32::try_from(data.iter().filter(|&&b| b == b'\n' || b == b'\r').count()).unwrap() + 2;
            for row in &index.rows {
                prop_assert!(row.lines.end() <= lines, "{}: {:?} in {} lines", row.path, row.lines, lines);
            }
        }
    }
}
