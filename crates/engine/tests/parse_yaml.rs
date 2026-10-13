//! T2 (S11): YAML parsing and flattening, as golden snapshots. Each snapshot is the text form in `support::dump`: one line per
//! setting (path, decoded text, type, flag, line range, whether it came through an alias), then the warnings. Read them as
//! the expected behaviour, not as output to accept: a change to a snapshot is a change to what the grid, the diff and the
//! checks show.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use engine::{ParseError, ParseWarning, Syntax, parse};
use insta::assert_snapshot;
use support::{documents, dump, limits, parse_warnings, parsed, warning, yaml};

#[test]
fn golden_yaml_nested_maps_and_sequences() {
    let flat = yaml(
        "\
server:
  port: 8080
  host: example.org
  tags:
    - alpha
    - beta
  limits: {cpu: 2, mem: 512}
list:
  - name: first
    value: 1
  - name: second
    value: 2.5
matrix:
  - [1, 2]
  - [3, 4]
",
    );
    assert_snapshot!("yaml_nested", dump(&flat));
}

#[test]
fn golden_yaml_anchors_aliases_and_merge_keys() {
    let flat = yaml(
        "\
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
service-c:
  name: c
  <<: *defaults
list: &items
  - x
  - y
copy: *items
base1: &b1
  k: from-b1
  only1: 1
base2: &b2
  k: from-b2
  only2: 2
merged:
  <<: [*b1, *b2]
  own: yes
",
    );
    assert_snapshot!("yaml_anchors_merge", dump(&flat));
}

#[test]
fn golden_yaml_multi_document() {
    let flat = yaml("a: 1\n---\nb: 2\n...\n---\n- x\n- y\n---\nplain\n");
    assert_snapshot!("yaml_multi_document", dump(&flat));
    let p = parsed(Syntax::Yaml, "a: 1\n---\nb: 2\n");
    assert_eq!(documents(&p), 2);
}

#[test]
fn single_document_has_no_prefix() {
    let flat = yaml("---\na: 1\n...\n");
    assert_eq!(flat.settings.len(), 1);
    assert_eq!(flat.settings[0].path.as_str(), "a");
}

#[test]
fn golden_yaml_quoted_and_special_keys() {
    let flat = yaml(
        "\
'a.b': 1
\"c[0]\": 2
d:
  'x.y': 3
  \"e]f\": 4
  \"q\\\"uote\": 5
  'back\\slash': 6
  \"tab\\there\": 7
  '': 8
'#0/looks-like-a-document': 9
'$': 10
plain key with spaces: 11
mid\"quote: 12
",
    );
    assert_snapshot!("yaml_special_keys", dump(&flat));
}

#[test]
fn golden_yaml_scalar_types() {
    let flat = yaml(
        "\
int: 42
negative: -7
hex: 0x1F
octal12: 0o17
leading-zero: 0755
float: 3.14
exp: 1e3
inf: .inf
nan: .NaN
bool-true: true
bool-capital: True
bool-upper: FALSE
null-tilde: ~
null-word: null
null-empty:
yes: yes
no-value: no
on-value: On
off-value: OFF
quoted-yes: 'yes'
quoted-int: \"42\"
quoted-bool: 'true'
empty-quoted: ''
string: hello world
url: https://example.org/a?b=c#d
tagged-str: !!str 123
tagged-int: !!int '7'
tagged-other: !custom value
placeholder: ${ENV_NAME}
placeholder-default: ${HOST:localhost}:${PORT}
template: '{{ var }}'
",
    );
    assert_snapshot!("yaml_scalar_types", dump(&flat));
}

#[test]
fn golden_yaml_positions() {
    let flat = yaml(
        "\
# comment first
literal: |
  line one
  line two

folded: >-
  folded
  text
quoted: \"one
  two\"
plain: a
  b
  c
flow: [1, 2,
  3]
map: {x: 1,
  y: 2}
nested:
  deep:
    leaf: v
after: 1
",
    );
    assert_snapshot!("yaml_positions", dump(&flat));
}

#[test]
fn golden_yaml_line_endings_are_counted_once() {
    let crlf = yaml("a: 1\r\nb:\r\n  c: |\r\n    x\r\n    y\r\nd: 2\r\n");
    let lf = yaml("a: 1\nb:\n  c: |\n    x\n    y\nd: 2\n");
    let lines = |f: &engine::Flat| -> Vec<(String, u32, u32)> {
        f.settings
            .iter()
            .map(|s| (s.path.to_string(), s.lines.start(), s.lines.end()))
            .collect()
    };
    assert_eq!(lines(&crlf), lines(&lf));
    let cr = yaml("a: 1\rb: 2\rc: 3\r");
    let starts: Vec<u32> = cr.settings.iter().map(|s| s.lines.start()).collect();
    assert_eq!(
        starts,
        [1, 2, 3],
        "a lone CR ends a line, as `normalise_eol` treats it"
    );
}

#[test]
fn golden_yaml_empty_collections_and_root_values() {
    for (name, text) in [
        ("empty_map_and_list", "a: {}\nb: []\nc:\n  d: []\n"),
        ("root_scalar", "just a scalar\n"),
        ("root_sequence", "- a\n- b: 1\n  c: 2\n- [x]\n"),
        ("empty_file", ""),
        ("only_comments", "# nothing here\n\n# or here\n"),
        ("document_marker_only", "---\n"),
    ] {
        assert_snapshot!(format!("yaml_{name}"), dump(&yaml(text)));
    }
}

#[test]
fn duplicate_key_last_wins_with_warning() {
    let p = parsed(
        Syntax::Yaml,
        "\
a: 1
b: 2
a: 3
nested:
  x: 1
  y: 2
  x: 5
  x: 6
  deeper:
    k: 1
    k: 2
",
    );
    assert_snapshot!("yaml_duplicate_warnings", parse_warnings(&p));
    let flat = engine::flatten(&p, &limits()).unwrap();
    assert_snapshot!("yaml_duplicate_flat", dump(&flat));
    // The last value is the one that counts, and a path is listed once.
    let a: Vec<_> = flat.settings.iter().filter(|s| s.path.as_str() == "a").collect();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].value.text, "3");
    assert_eq!(a[0].lines.start(), 3);
}

#[test]
fn duplicate_keys_in_a_later_document_are_named_with_the_document_prefix() {
    let p = parsed(Syntax::Yaml, "a: 1\n---\nk: 1\nk: 2\n");
    let w: Vec<String> = p.warnings.iter().map(warning).collect();
    assert_eq!(w, ["duplicate key #1/k first at 3:1 again at 4:1"]);
}

#[test]
fn a_duplicated_value_replaces_the_whole_earlier_value() {
    let flat = yaml("a:\n  x: 1\n  y: 2\na:\n  z: 3\n");
    let paths: Vec<&str> = flat.settings.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        ["a.z"],
        "the later `a` replaces the earlier one, it is not merged into it"
    );
}

#[test]
fn explicit_keys_beat_merged_keys_without_a_duplicate_warning() {
    let p = parsed(Syntax::Yaml, "d: &d\n  k: 1\ns:\n  <<: *d\n  k: 2\n");
    assert!(p.warnings.is_empty(), "{}", parse_warnings(&p));
    let flat = engine::flatten(&p, &limits()).unwrap();
    let k = flat.settings.iter().find(|s| s.path.as_str() == "s.k").unwrap();
    assert_eq!(k.value.text, "2");
    assert!(!k.via_alias);
}

#[test]
fn merged_settings_say_they_came_through_an_alias() {
    let flat = yaml("d: &d\n  k: 1\ns:\n  <<: *d\n");
    let k = flat.settings.iter().find(|s| s.path.as_str() == "s.k").unwrap();
    assert!(k.via_alias);
    assert_eq!(
        k.lines.start(),
        2,
        "the line is where the value is written, in the anchor"
    );
    let own = flat.settings.iter().find(|s| s.path.as_str() == "d.k").unwrap();
    assert!(!own.via_alias);
}

#[test]
fn a_boolean_like_value_is_flagged_and_warned_about() {
    let flat = yaml("a: yes\nb: true\nc: 'no'\nd: off\n");
    let flagged: Vec<&str> = flat
        .settings
        .iter()
        .filter(|s| s.value.flag.is_some())
        .map(|s| s.path.as_str())
        .collect();
    assert_eq!(flagged, ["a", "d"]);
    assert_snapshot!("yaml_boolean_like_warnings", dump(&flat));
}

#[test]
fn complex_keys_are_skipped_with_a_warning() {
    let flat = yaml("? [a, b]\n: v\nok: 1\n");
    assert_snapshot!("yaml_complex_key", dump(&flat));
}

#[test]
fn a_merge_key_that_is_not_a_map_is_warned_about_and_kept_as_a_plain_key() {
    let flat = yaml("a:\n  <<: 5\n  k: 1\n");
    assert_snapshot!("yaml_invalid_merge", dump(&flat));
}

#[test]
fn syntax_errors_name_the_line_and_never_the_text() {
    let err = parse("a: [1, 2\nb: 3\n", Syntax::Yaml, &limits()).unwrap_err();
    match err {
        ParseError::Syntax { syntax, line, .. } => {
            assert_eq!(syntax, Syntax::Yaml);
            assert!((1..=3).contains(&line));
        }
        other => panic!("expected a syntax error, got {other:?}"),
    }
    let err = parse("key: \"unterminated\nnext: 1\n", Syntax::Yaml, &limits()).unwrap_err();
    assert!(matches!(err, ParseError::Syntax { .. }));
    let err = parse("a: 1\n\tb: 2\n", Syntax::Yaml, &limits()).unwrap_err();
    assert!(matches!(err, ParseError::Syntax { line: 1..=2, .. }), "{err:?}");
}

#[test]
fn an_alias_to_an_undefined_or_still_open_anchor_is_an_error() {
    let err = parse("a: *nope\n", Syntax::Yaml, &limits()).unwrap_err();
    assert!(
        matches!(err, ParseError::UnknownAlias { .. } | ParseError::Syntax { .. }),
        "{err:?}"
    );
    // `&x [*x]` refers to a list that is not finished: a cycle, which the engine never builds.
    let err = parse("a: &x [*x]\n", Syntax::Yaml, &limits()).unwrap_err();
    assert!(matches!(err, ParseError::UnknownAlias { line: 1 }), "{err:?}");
}

#[test]
fn an_alias_in_the_next_document_cannot_see_an_earlier_anchor() {
    let err = parse("a: &x 1\n---\nb: *x\n", Syntax::Yaml, &limits()).unwrap_err();
    assert!(
        matches!(err, ParseError::UnknownAlias { .. } | ParseError::Syntax { .. }),
        "{err:?}"
    );
}

#[test]
fn a_leading_byte_order_mark_is_ignored() {
    let flat = yaml("\u{feff}a: 1\n");
    assert_eq!(flat.settings[0].path.as_str(), "a");
}

#[test]
fn positions_in_duplicate_warnings_are_one_based_lines_and_columns() {
    let p = parsed(Syntax::Yaml, "top:\n  k: 1\n  k: 2\n");
    match &p.warnings[..] {
        [ParseWarning::DuplicateKey { path, first, second }] => {
            assert_eq!(path.as_ref().unwrap().as_str(), "top.k");
            assert_eq!((first.line, first.column), (2, 3));
            assert_eq!((second.line, second.column), (3, 3));
        }
        other => panic!("{other:?}"),
    }
}
