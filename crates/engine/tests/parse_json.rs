//! T2 (S11): JSON parsing and flattening. JSON follows the same flattening rules as YAML (`docs/domain-model.md`); the
//! syntax is strict (RFC 8259), the lines come from the bytes the tokeniser consumed, and a repeated key is a warning with
//! the last value winning.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use engine::{ParseError, Syntax, parse};
use insta::assert_snapshot;
use support::{dump, json, limits, parse_warnings, parsed};

#[test]
fn golden_json_object_with_nesting() {
    let flat = json(
        "\
{
  \"server\": {
    \"port\": 8080,
    \"host\": \"example.org\",
    \"tags\": [\"alpha\", \"beta\"],
    \"enabled\": true,
    \"proxy\": null
  },
  \"ratio\": 0.75,
  \"big\": 12345678901234567890123,
  \"exp\": 1.5E+3,
  \"neg\": -4,
  \"empty-list\": [],
  \"empty-map\": {},
  \"list\": [
    {\"name\": \"first\"},
    {\"name\": \"second\"}
  ]
}
",
    );
    assert_snapshot!("json_nested", dump(&flat));
}

#[test]
fn golden_json_strings_keep_their_decoded_text() {
    let flat = json(
        r#"{"esc": "a\tb\n\"q\" \\ Aé 😀", "yes": "yes", "num-string": "42", "placeholder": "${HOST}", "dotted.key": 1, "[x]": 2}"#,
    );
    assert_snapshot!("json_strings", dump(&flat));
}

#[test]
fn json_lines_are_where_the_values_are_written() {
    let flat = json("{\n\n  \"a\":\n\n    1,\n  \"b\": [\n    2,\n\n    3\n  ]\n}\n");
    let lines: Vec<(String, u32, u32)> = flat
        .settings
        .iter()
        .map(|s| (s.path.to_string(), s.lines.start(), s.lines.end()))
        .collect();
    assert_eq!(
        lines,
        [
            ("a".to_owned(), 3, 5),
            ("b[0]".to_owned(), 7, 7),
            ("b[1]".to_owned(), 9, 9)
        ]
    );
}

#[test]
fn json_root_values_are_allowed() {
    assert_snapshot!("json_root_array", dump(&json("[1, {\"a\": 2}]")));
    assert_snapshot!("json_root_scalar", dump(&json("\"text\"")));
    assert_snapshot!("json_root_number", dump(&json("  42  ")));
}

#[test]
fn json_duplicate_key_last_wins_with_warning() {
    let p = parsed(
        Syntax::Json,
        "{\n  \"a\": 1,\n  \"b\": {\"x\": 1, \"x\": 2},\n  \"a\": 3\n}\n",
    );
    assert_snapshot!("json_duplicate_warnings", parse_warnings(&p));
    let flat = engine::flatten(&p, &limits()).unwrap();
    assert_snapshot!("json_duplicate_flat", dump(&flat));
}

#[test]
fn json_is_strict() {
    for (what, text) in [
        ("trailing comma in object", r#"{"a": 1,}"#),
        ("trailing comma in array", "[1, 2,]"),
        ("single quotes", "{'a': 1}"),
        ("unquoted key", "{a: 1}"),
        ("comment", "{\"a\": 1} // c"),
        ("leading zero", "[01]"),
        ("plus sign", "[+1]"),
        ("NaN", "[NaN]"),
        ("Infinity", "[Infinity]"),
        ("hex", "[0x10]"),
        ("two values", "{} {}"),
        ("trailing garbage", "{\"a\": 1} x"),
        ("empty", ""),
        ("only whitespace", "  \n "),
        ("unterminated string", "{\"a\": \"b}"),
        ("unterminated object", "{\"a\": 1"),
        ("raw newline in string", "{\"a\": \"b\nc\"}"),
        ("bad escape", r#"["\x"]"#),
        ("lone surrogate", r#"["\ud800"]"#),
        ("missing colon", r#"{"a" 1}"#),
        ("missing comma", "[1 2]"),
        ("mismatched brackets", "[1}"),
    ] {
        let result = parse(text, Syntax::Json, &limits());
        assert!(
            matches!(
                result,
                Err(ParseError::Syntax {
                    syntax: Syntax::Json,
                    ..
                })
            ),
            "{what}: {result:?}"
        );
    }
}

#[test]
fn json_syntax_errors_name_the_line() {
    let err = parse("{\n  \"a\": 1,\n  \"b\": ]\n}", Syntax::Json, &limits()).unwrap_err();
    assert!(matches!(err, ParseError::Syntax { line: 3, .. }), "{err:?}");
}

#[test]
fn a_leading_byte_order_mark_is_ignored() {
    let flat = json("\u{feff}{\"a\": 1}");
    assert_eq!(flat.settings[0].path.as_str(), "a");
}

#[test]
fn json_numbers_keep_their_source_text() {
    let flat = json("[1, 1.0, 1e2, -0, 0.10, 123456789012345678901234567890]");
    let texts: Vec<&str> = flat.settings.iter().map(|s| s.value.text.as_str()).collect();
    assert_eq!(
        texts,
        ["1", "1.0", "1e2", "-0", "0.10", "123456789012345678901234567890"]
    );
}
