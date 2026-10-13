//! T2 (S11): `validate` is the one question "is this file acceptable?" for bytes: decode, parse, flatten.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::NfsPath;
use engine::{DecodeError, LimitKind, Limits, ParseError, ParseWarning, Syntax, index_settings, validate};

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

#[test]
fn the_size_limit_is_on_the_file_and_not_on_what_decoding_makes_of_it() {
    // `input_bytes` is the size of a file. Latin-1 turns each byte over 0x7F into two bytes of text, and UTF-16 turns two
    // bytes into up to three, so a file within the limit can have a longer text: it is still within the limit.
    let limits = Limits {
        input_bytes: 64,
        ..Limits::default()
    };
    // Latin-1: 2 + 40 + 1 = 43 bytes, and 2 + 80 + 1 = 83 bytes of text.
    let mut latin1 = b"k=".to_vec();
    latin1.extend(std::iter::repeat_n(0xE9, 40));
    latin1.push(b'\n');
    assert!(latin1.len() <= limits.input_bytes);
    let v = validate(&latin1, Syntax::Properties, &limits).unwrap();
    assert_eq!(v.settings, 1);
    assert_eq!(
        index_settings(&latin1, Syntax::Properties, &limits)
            .unwrap()
            .rows
            .len(),
        1
    );

    // UTF-16: 2 + 28 * 2 = 58 bytes, and 2 + 25 * 3 + 1 = 78 bytes of text.
    let utf16 = utf16le(&format!("k={}\n", "\u{4e2d}".repeat(25)));
    assert!(utf16.len() <= limits.input_bytes);
    let v = validate(&utf16, Syntax::Properties, &limits).unwrap();
    assert_eq!(v.settings, 1);
    assert_eq!(
        index_settings(&utf16, Syntax::Properties, &limits)
            .unwrap()
            .rows
            .len(),
        1
    );

    // The file itself is still held to the limit, one byte over or not.
    let mut over = latin1.clone();
    over.extend(std::iter::repeat_n(b'x', 64 - over.len() + 1));
    assert_eq!(over.len(), limits.input_bytes + 1);
    for result in [
        validate(&over, Syntax::Properties, &limits).unwrap_err(),
        index_settings(&over, Syntax::Properties, &limits).unwrap_err(),
    ] {
        assert_eq!(result, ParseError::Limit(LimitKind::InputBytes));
    }
    // And a text handed to `parse` is held to it as it always was: it is the size of the text.
    let text = "k=".to_owned() + &"\u{e9}".repeat(40) + "\n";
    assert_eq!(
        engine::parse(&text, Syntax::Properties, &limits).unwrap_err(),
        ParseError::Limit(LimitKind::InputBytes)
    );
}
