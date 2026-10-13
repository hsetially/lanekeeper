//! T2 (S11): `validate` is the one question "is this file acceptable?" for bytes: decode, parse, flatten.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::NfsPath;
use engine::{DecodeError, LimitKind, Limits, ParseError, ParseWarning, Syntax, validate};

fn utf16le(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

#[test]
fn syntax_follows_the_extension_in_any_case() {
    let of = |p: &str| Syntax::of_path(&NfsPath::parse(p).unwrap());
    assert_eq!(of("a/b.yml"), Some(Syntax::Yaml));
    assert_eq!(of("a/b.YAML"), Some(Syntax::Yaml));
    assert_eq!(of("b.json"), Some(Syntax::Json));
    assert_eq!(of("x/y/app.Properties"), Some(Syntax::Properties));
    assert_eq!(of("a/b.xml"), None);
    assert_eq!(of("a/b.xsl"), None);
    assert_eq!(of("a/noextension"), None);
    assert_eq!(of("a/.profile"), None);
    assert_eq!(of("a/b.yml.bak"), None);
    assert_eq!(of("a/b.png"), None);
}

#[test]
fn a_good_file_gives_counts_and_warnings() {
    let text = "a: yes\nb:\n  c: 1\n  c: 2\n---\nd: 3\n";
    let v = validate(text.as_bytes(), Syntax::Yaml, &Limits::default()).unwrap();
    assert_eq!(v.documents, 2);
    assert_eq!(v.settings, 3);
    assert_eq!(v.warnings.len(), 2);
    assert!(matches!(v.warnings[0], ParseWarning::DuplicateKey { .. }));
    assert!(matches!(v.warnings[1], ParseWarning::BooleanLikeValue { .. }));
    assert_eq!(v.warnings_dropped, 0);
}

#[test]
fn properties_have_no_documents() {
    let v = validate(b"a=1\nb=2\n", Syntax::Properties, &Limits::default()).unwrap();
    assert_eq!((v.documents, v.settings), (0, 2));
}

#[test]
fn the_encoding_is_read_as_the_file_declares_it() {
    let limits = Limits::default();
    // UTF-16 with a byte order mark, CRLF.
    let v = validate(&utf16le("a: 1\r\nb: 2\r\n"), Syntax::Yaml, &limits).unwrap();
    assert_eq!(v.settings, 2);
    // UTF-8 with a byte order mark.
    let v = validate(b"\xEF\xBB\xBF{\"a\": 1}", Syntax::Json, &limits).unwrap();
    assert_eq!(v.settings, 1);
    // Latin-1: no mark, not UTF-8.
    let v = validate(b"caf\xE9=1\n", Syntax::Properties, &limits).unwrap();
    assert_eq!(v.settings, 1);
}

#[test]
fn bytes_that_are_not_text_of_their_declared_encoding_are_refused() {
    let limits = Limits::default();
    let err = validate(b"\xEF\xBB\xBF\xFF", Syntax::Yaml, &limits).unwrap_err();
    assert_eq!(err, ParseError::Decode(DecodeError::InvalidUtf8));
    let err = validate(b"\xFF\xFEa", Syntax::Yaml, &limits).unwrap_err();
    assert_eq!(err, ParseError::Decode(DecodeError::InvalidUtf16));
}

#[test]
fn a_file_over_the_size_limit_is_a_limit_error_whichever_step_finds_it() {
    let limits = Limits {
        input_bytes: 32,
        ..Limits::default()
    };
    let big = format!("a: {}\n", "x".repeat(40));
    let err = validate(big.as_bytes(), Syntax::Yaml, &limits).unwrap_err();
    assert_eq!(err, ParseError::Limit(LimitKind::InputBytes));
}

#[test]
fn a_syntax_error_and_a_limit_come_through_unchanged() {
    let limits = Limits::default();
    assert!(matches!(
        validate(b"a: [1, 2", Syntax::Yaml, &limits),
        Err(ParseError::Syntax {
            syntax: Syntax::Yaml,
            ..
        })
    ));
    let small = Limits {
        settings: 1,
        ..Limits::default()
    };
    assert_eq!(
        validate(b"a: 1\nb: 2\n", Syntax::Yaml, &small).unwrap_err(),
        ParseError::Limit(LimitKind::Settings)
    );
}

#[test]
fn warnings_are_capped_across_parsing_and_flattening() {
    let limits = Limits {
        warnings: 3,
        ..Limits::default()
    };
    // Two duplicate keys (parse warnings), three boolean-like values (flatten warnings).
    let text = "k: 1\nk: 2\nj: 1\nj: 2\na: yes\nb: no\nc: on\n";
    let v = validate(text.as_bytes(), Syntax::Yaml, &limits).unwrap();
    assert_eq!(v.warnings.len(), 3);
    assert_eq!(v.warnings_dropped, 2);
}
