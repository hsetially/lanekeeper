//! T2 (S11): `.properties` parsing and flattening. The rules are Java's (`Properties.load`), which Spring Boot follows:
//! `=`, `:` or whitespace separates the key, `#` and `!` start a comment line, a trailing odd backslash continues the
//! line, `\uXXXX` and the usual escapes are decoded. A repeated key is a warning and the last value wins. Keys are used as
//! they are written (`docs/domain-model.md`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use engine::{ParseError, Syntax, parse};
use insta::assert_snapshot;
use support::{dump, limits, parse_warnings, parsed, properties};

#[test]
fn golden_properties_separators_and_comments() {
    let flat = properties(
        "\
# a comment
! another comment
a=1
b = 2
c:3
d : 4
e 5
f\t6
   indented = 7
empty=
no-value
spaced   =   value with  spaces\x20\x20
url=http://host:8080/path?x=y
colon.in.value: a:b=c
config.server.port=8080
list[0]=x
list[1]=y
log.level: INFO
",
    );
    assert_snapshot!("properties_basic", dump(&flat));
}

#[test]
fn golden_properties_escapes_and_continuations() {
    let flat = properties(
        "\
unicode=caf\\u00e9 \\u0041
escapes=tab\\there\\nnewline\\\\backslash
key\\ with\\ spaces=v
key\\=eq=v2
other\\:colon: v3
keep\\ =trailing
multi=one \\
      two \\
      three
after=1
comment-not-continued=x
# trailing backslash in a comment is not a continuation \\
next=2
not\\#comment=3
",
    );
    assert_snapshot!("properties_escapes", dump(&flat));
}

#[test]
fn properties_lines_cover_continuations() {
    let flat = properties("a=1\nb=x \\\n  y \\\n  z\nc=3\n");
    let lines: Vec<(String, u32, u32)> = flat
        .settings
        .iter()
        .map(|s| (s.path.to_string(), s.lines.start(), s.lines.end()))
        .collect();
    assert_eq!(
        lines,
        [
            ("a".to_owned(), 1, 1),
            ("b".to_owned(), 2, 4),
            ("c".to_owned(), 5, 5)
        ]
    );
}

#[test]
fn properties_values_are_strings_and_never_flagged() {
    let flat = properties("a=yes\nb=true\nc=42\nd=\n");
    for s in &flat.settings {
        assert_eq!(s.value.ty, domain::ValueType::String, "{}", s.path);
        assert!(s.value.flag.is_none(), "{}", s.path);
    }
    assert!(flat.warnings.is_empty());
}

#[test]
fn properties_duplicate_key_last_wins_with_warning() {
    let p = parsed(Syntax::Properties, "a=1\nb=2\na=3\na=4\n");
    assert_snapshot!("properties_duplicate_warnings", parse_warnings(&p));
    let flat = engine::flatten(&p, &limits()).unwrap();
    assert_snapshot!("properties_duplicate_flat", dump(&flat));
}

#[test]
fn properties_line_endings() {
    let lf = properties("a=1\nb=2\n\nc=3");
    let crlf = properties("a=1\r\nb=2\r\n\r\nc=3");
    let cr = properties("a=1\rb=2\r\rc=3");
    assert_eq!(support::pairs(&lf), support::pairs(&crlf));
    assert_eq!(support::pairs(&lf), support::pairs(&cr));
    let lines = |f: &engine::Flat| f.settings.iter().map(|s| s.lines.start()).collect::<Vec<_>>();
    assert_eq!(lines(&lf), [1, 2, 4]);
    assert_eq!(lines(&crlf), [1, 2, 4]);
    assert_eq!(lines(&cr), [1, 2, 4]);
}

#[test]
fn a_continuation_line_that_ends_the_file_is_accepted() {
    let flat = properties("a=1 \\");
    assert_eq!(flat.settings[0].value.text, "1 ");
}

#[test]
fn an_empty_key_is_skipped_with_a_warning() {
    let flat = properties("=orphan\nok=1\n");
    assert_snapshot!("properties_empty_key", dump(&flat));
}

#[test]
fn a_key_with_a_control_character_is_skipped_with_a_warning() {
    let flat = properties("bad\\u0001key=1\nok=2\n");
    assert_snapshot!("properties_control_key", dump(&flat));
}

#[test]
fn a_malformed_unicode_escape_is_a_syntax_error_on_its_line() {
    for text in ["a=\\u12\n", "a=\\uZZZZ\n", "x=1\nb\\u00=2\n"] {
        let err = parse(text, Syntax::Properties, &limits()).unwrap_err();
        assert!(
            matches!(
                err,
                ParseError::Syntax {
                    syntax: Syntax::Properties,
                    ..
                }
            ),
            "{text:?}: {err:?}"
        );
    }
    let err = parse("x=1\ny=2\nb=\\u00\n", Syntax::Properties, &limits()).unwrap_err();
    assert!(matches!(err, ParseError::Syntax { line: 3, .. }), "{err:?}");
}

#[test]
fn a_lone_surrogate_escape_is_a_syntax_error() {
    let err = parse("a=\\ud800\n", Syntax::Properties, &limits()).unwrap_err();
    assert!(matches!(err, ParseError::Syntax { .. }), "{err:?}");
    // A pair is fine.
    let flat = properties("a=\\ud83d\\ude00\n");
    assert_eq!(flat.settings[0].value.text, "\u{1F600}");
}

#[test]
fn an_empty_file_has_no_settings() {
    assert!(properties("").settings.is_empty());
    assert!(properties("# only a comment\n\n").settings.is_empty());
}

#[test]
fn a_leading_byte_order_mark_is_ignored() {
    let flat = properties("\u{feff}a=1\n");
    assert_eq!(flat.settings[0].path.as_str(), "a");
}
