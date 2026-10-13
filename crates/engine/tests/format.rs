//! T1: encoding, BOM and line-ending style (`detect_format`), and text that goes in and out of bytes without changing
//! them (`decode_text`, `encode_text`; D17, rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::borrow::Cow;

use domain::{EolStyle, FileFormat, TextEncoding};
use engine::{DecodeError, EncodeError, Limits, decode_text, detect_format, encode_text};
use proptest::prelude::*;

fn fmt(eol: EolStyle, encoding: TextEncoding, bom: bool) -> FileFormat {
    FileFormat { eol, encoding, bom }
}

#[test]
fn detect_format_table() {
    use EolStyle::{Crlf, Lf, Mixed};
    use TextEncoding::{Latin1, Utf8, Utf16Be, Utf16Le};
    let rows: [(&str, &[u8], FileFormat); 17] = [
        ("empty", b"", fmt(Lf, Utf8, false)),
        ("lf", b"a: 1\nb: 2\n", fmt(Lf, Utf8, false)),
        ("crlf", b"a: 1\r\nb: 2\r\n", fmt(Crlf, Utf8, false)),
        ("no terminator", b"a: 1", fmt(Lf, Utf8, false)),
        ("crlf and lf", b"a\r\nb\n", fmt(Mixed, Utf8, false)),
        ("lone cr", b"a\rb", fmt(Mixed, Utf8, false)),
        ("only a cr", b"\r", fmt(Mixed, Utf8, false)),
        ("lone cr next to lf", b"a\rb\nc\n", fmt(Mixed, Utf8, false)),
        ("utf-8 bom, crlf", b"\xEF\xBB\xBFa\r\n", fmt(Crlf, Utf8, true)),
        ("utf-8 bom only", b"\xEF\xBB\xBF", fmt(Lf, Utf8, true)),
        ("utf-8 text", "caf\u{e9}\n".as_bytes(), fmt(Lf, Utf8, false)),
        ("latin-1 byte", b"caf\xE9\n", fmt(Lf, Latin1, false)),
        ("single high byte", b"\xFF", fmt(Lf, Latin1, false)),
        (
            "utf-16le crlf",
            b"\xFF\xFEa\x00\r\x00\n\x00",
            fmt(Crlf, Utf16Le, true),
        ),
        ("utf-16be lf", b"\xFE\xFF\x00a\x00\n", fmt(Lf, Utf16Be, true)),
        ("utf-16 bom only", b"\xFF\xFE", fmt(Lf, Utf16Le, true)),
        // A BOM is a declaration: the body is checked when the text is decoded, not here.
        ("utf-8 bom, bad body", b"\xEF\xBB\xBF\xFF", fmt(Lf, Utf8, true)),
    ];
    for (name, data, expected) in rows {
        assert_eq!(detect_format(data), expected, "{name}");
    }
}

#[test]
fn decode_text_table() {
    let limits = Limits::default();
    let rows: [(&str, &[u8], &str, FileFormat); 6] = [
        (
            "utf-8",
            "caf\u{e9}\r\n".as_bytes(),
            "caf\u{e9}\r\n",
            detect_format("caf\u{e9}\r\n".as_bytes()),
        ),
        (
            "utf-8 bom is not in the text",
            b"\xEF\xBB\xBFa: 1\n",
            "a: 1\n",
            detect_format(b"\xEF\xBB\xBF"),
        ),
        ("latin-1", b"caf\xE9", "caf\u{e9}", detect_format(b"caf\xE9")),
        (
            "utf-16le",
            b"\xFF\xFEa\x00\xE9\x00",
            "a\u{e9}",
            detect_format(b"\xFF\xFE"),
        ),
        // U+1F600 is a surrogate pair in UTF-16.
        (
            "utf-16be pair",
            b"\xFE\xFF\xD8\x3D\xDE\x00",
            "\u{1F600}",
            detect_format(b"\xFE\xFF"),
        ),
        ("empty", b"", "", detect_format(b"")),
    ];
    for (name, data, text, format) in rows {
        let decoded = decode_text(data, &limits).unwrap();
        assert_eq!(decoded.text, text, "{name}");
        assert_eq!(decoded.format, format, "{name}");
    }
}

#[test]
fn utf8_without_bom_is_not_copied() {
    let data = b"a: 1\n";
    let plain = decode_text(data, &Limits::default()).unwrap();
    assert!(matches!(plain.text, Cow::Borrowed(s) if s.as_ptr() == data.as_ptr()));
    let with_bom = b"\xEF\xBB\xBFa: 1\n";
    let decoded = decode_text(with_bom, &Limits::default()).unwrap();
    assert!(matches!(decoded.text, Cow::Borrowed(_)));
}

#[test]
fn decode_text_rejects_what_the_bom_does_not_describe() {
    let limits = Limits::default();
    assert_eq!(
        decode_text(b"\xEF\xBB\xBF\xFF", &limits).unwrap_err(),
        DecodeError::InvalidUtf8
    );
    // Odd length.
    assert_eq!(
        decode_text(b"\xFF\xFEa", &limits).unwrap_err(),
        DecodeError::InvalidUtf16
    );
    // A lone high surrogate.
    assert_eq!(
        decode_text(b"\xFF\xFE\x3D\xD8", &limits).unwrap_err(),
        DecodeError::InvalidUtf16
    );
    // A lone low surrogate, big-endian.
    assert_eq!(
        decode_text(b"\xFE\xFF\xDE\x00", &limits).unwrap_err(),
        DecodeError::InvalidUtf16
    );
}

#[test]
fn input_size_limit_is_enforced_at_the_boundary() {
    let limits = Limits {
        input_bytes: 8,
        ..Limits::default()
    };
    let at = [b'a'; 8];
    let over = [b'a'; 9];
    assert!(decode_text(&at, &limits).is_ok());
    assert_eq!(decode_text(&over, &limits).unwrap_err(), DecodeError::TooLarge);
    let format = fmt(EolStyle::Lf, TextEncoding::Utf8, false);
    assert!(encode_text("aaaaaaaa", format, &limits).is_ok());
    assert_eq!(
        encode_text("aaaaaaaaa", format, &limits).unwrap_err(),
        EncodeError::TooLarge
    );
}

#[test]
fn encode_text_table() {
    let limits = Limits::default();
    let rows: [(&str, &str, FileFormat, &[u8]); 6] = [
        (
            "utf-8",
            "a\u{e9}",
            fmt(EolStyle::Lf, TextEncoding::Utf8, false),
            "a\u{e9}".as_bytes(),
        ),
        (
            "utf-8 bom",
            "a",
            fmt(EolStyle::Lf, TextEncoding::Utf8, true),
            b"\xEF\xBB\xBFa",
        ),
        (
            "latin-1",
            "caf\u{e9}",
            fmt(EolStyle::Lf, TextEncoding::Latin1, false),
            b"caf\xE9",
        ),
        (
            "utf-16le",
            "a\u{e9}",
            fmt(EolStyle::Lf, TextEncoding::Utf16Le, true),
            b"\xFF\xFEa\x00\xE9\x00",
        ),
        (
            "utf-16be pair",
            "\u{1F600}",
            fmt(EolStyle::Lf, TextEncoding::Utf16Be, true),
            b"\xFE\xFF\xD8\x3D\xDE\x00",
        ),
        (
            "empty",
            "",
            fmt(EolStyle::Lf, TextEncoding::Utf16Le, true),
            b"\xFF\xFE",
        ),
    ];
    for (name, text, format, expected) in rows {
        assert_eq!(
            encode_text(text, format, &limits).unwrap().as_ref(),
            expected,
            "{name}"
        );
    }
}

#[test]
fn encode_text_refuses_what_cannot_be_written_back() {
    let limits = Limits::default();
    let latin1 = fmt(EolStyle::Lf, TextEncoding::Latin1, false);
    // U+20AC is not in Latin-1: writing it would change the text.
    assert_eq!(
        encode_text("\u{20AC}", latin1, &limits).unwrap_err(),
        EncodeError::Unrepresentable
    );
    // UTF-16 is recognised by its BOM alone, so a file without one could never be read back.
    let bare16 = fmt(EolStyle::Lf, TextEncoding::Utf16Le, false);
    assert_eq!(
        encode_text("a", bare16, &limits).unwrap_err(),
        EncodeError::BadFormat
    );
    // A BOM means nothing in Latin-1.
    let bom_latin1 = fmt(EolStyle::Lf, TextEncoding::Latin1, true);
    assert_eq!(
        encode_text("a", bom_latin1, &limits).unwrap_err(),
        EncodeError::BadFormat
    );
}

fn interesting_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Small alphabet: BOMs, CR, LF, NUL, surrogate halves and ASCII meet often.
        prop::collection::vec(
            prop::sample::select(vec![
                0x0D_u8, 0x0A, 0x41, 0x00, 0xFF, 0xFE, 0xEF, 0xBB, 0xBF, 0xD8, 0xDC, 0xE9
            ]),
            0..48
        ),
        prop::collection::vec(any::<u8>(), 0..96),
        // Real text, in each BOM layout.
        "[^\\r\\n]{0,30}".prop_map(|s| [b"\xEF\xBB\xBF".as_slice(), s.as_bytes()].concat()),
        "[ -~\\u{e9}\\u{20AC}\\u{1F600}]{0,30}".prop_map(|s| {
            [
                vec![0xFF, 0xFE],
                s.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            ]
            .concat()
        }),
        "[ -~\\u{e9}\\u{20AC}\\u{1F600}]{0,30}".prop_map(|s| {
            [
                vec![0xFE, 0xFF],
                s.encode_utf16().flat_map(u16::to_be_bytes).collect(),
            ]
            .concat()
        }),
    ]
}

proptest! {
    /// Text that decodes is written back to the very same bytes, whatever the encoding and BOM (rule 11).
    #[test]
    fn decode_encode_roundtrip_prop(x in interesting_bytes()) {
        let limits = Limits::default();
        if let Ok(decoded) = decode_text(&x, &limits) {
            let back = encode_text(&decoded.text, decoded.format, &limits).unwrap();
            prop_assert_eq!(back.as_ref(), x.as_slice());
        }
    }

    /// Nothing in this module panics on any bytes, and `detect_format` and `decode_text` agree on the format.
    #[test]
    fn arbitrary_bytes_never_panic_prop(x in interesting_bytes()) {
        let limits = Limits::default();
        let format = detect_format(&x);
        if let Ok(decoded) = decode_text(&x, &limits) {
            prop_assert_eq!(decoded.format, format);
        }
        let owned = bytes::Bytes::from(x.clone());
        let _ = engine::normalise_eol(&owned);
        let _ = engine::apply_eol(&owned, engine::Newline::Lf);
        let _ = engine::apply_eol(&owned, engine::Newline::Crlf);
        let _ = engine::dominant_eol(&x);
        let deny = engine::DenyMatcher::default();
        for name in ["a/b", "a/b.yml", "a/b.xml", "a/b.png", "a/b.pem"] {
            let _ = engine::classify(&domain::NfsPath::parse(name).unwrap(), Some(&x), &deny);
        }
    }
}
