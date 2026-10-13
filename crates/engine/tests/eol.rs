//! T1: line endings (D17, rule 11). `normalise_eol` makes every terminator LF so that two files that differ only in line
//! endings compare equal; `apply_eol` writes them back in the original style. UTF-16 files (which have a BOM) are read
//! in two-byte units, so a `0D 0A` byte pair inside a character is never taken for a line break.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytes::Bytes;
use domain::EolStyle;
use engine::{Newline, apply_eol, detect_format, dominant_eol, normalise_eol};
use proptest::prelude::*;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const UTF16LE_BOM: [u8; 2] = [0xFF, 0xFE];
const UTF16BE_BOM: [u8; 2] = [0xFE, 0xFF];

fn b(data: &[u8]) -> Bytes {
    Bytes::copy_from_slice(data)
}

#[derive(Debug, Clone, Copy)]
enum Layout {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

fn encode(text: &str, layout: Layout) -> Vec<u8> {
    match layout {
        Layout::Utf8 => text.as_bytes().to_vec(),
        Layout::Utf8Bom => [UTF8_BOM.as_slice(), text.as_bytes()].concat(),
        Layout::Utf16Le => [
            UTF16LE_BOM.to_vec(),
            text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat(),
        Layout::Utf16Be => [
            UTF16BE_BOM.to_vec(),
            text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        ]
        .concat(),
    }
}

/// `lines` joined by `term`, with a closing terminator when `trailing` is set. Returns the text and how many terminators it has.
fn build(lines: &[String], term: &str, trailing: bool) -> (String, usize) {
    let mut out = lines.join(term);
    let mut count = lines.len().saturating_sub(1);
    if trailing && !lines.is_empty() {
        out.push_str(term);
        count += 1;
    }
    (out, count)
}

fn layout() -> impl Strategy<Value = Layout> {
    prop_oneof![
        Just(Layout::Utf8),
        Just(Layout::Utf8Bom),
        Just(Layout::Utf16Le),
        Just(Layout::Utf16Be)
    ]
}

fn lines() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[^\\r\\n]{0,24}", 0..8)
}

proptest! {
    /// The T1 verify line: `apply_eol(normalise_eol(x), style(x)) == x` for CRLF and LF files, with or without a BOM
    /// (UTF-8 BOM and both UTF-16 BOMs).
    #[test]
    fn eol_roundtrip_prop(
        lines in lines(),
        crlf in any::<bool>(),
        trailing in any::<bool>(),
        layout in layout(),
    ) {
        let term = if crlf { "\r\n" } else { "\n" };
        let (text, terminators) = build(&lines, term, trailing);
        let x = b(&encode(&text, layout));

        let style = dominant_eol(&x);
        let expected = if crlf && terminators > 0 { Newline::Crlf } else { Newline::Lf };
        prop_assert_eq!(style, expected);
        prop_assert_ne!(detect_format(&x).eol, EolStyle::Mixed);

        let back = apply_eol(&normalise_eol(&x), style);
        prop_assert_eq!(back, x);
    }

    /// The same for bytes that are not valid text: Latin-1 style files keep every byte.
    #[test]
    fn eol_roundtrip_raw_bytes_prop(
        lines in prop::collection::vec(prop::collection::vec(any::<u8>().prop_map(|v| if v == 13 || v == 10 { b'x' } else { v }), 0..16), 0..8),
        crlf in any::<bool>(),
        trailing in any::<bool>(),
    ) {
        // A line that starts with a UTF-16 BOM would make the whole file UTF-16, which the layout property covers.
        prop_assume!(!lines.first().is_some_and(|l| l.starts_with(&UTF16LE_BOM) || l.starts_with(&UTF16BE_BOM)));
        let term: &[u8] = if crlf { b"\r\n" } else { b"\n" };
        let mut x = lines.join(term);
        if trailing && !lines.is_empty() {
            x.extend_from_slice(term);
        }
        let x = Bytes::from(x);
        let style = dominant_eol(&x);
        prop_assert_eq!(apply_eol(&normalise_eol(&x), style), x);
    }

    /// Two files that differ only in their line endings are equal once normalised.
    #[test]
    fn normalise_hides_eol_only_difference_prop(lines in lines(), trailing in any::<bool>(), layout in layout()) {
        let (lf, _) = build(&lines, "\n", trailing);
        let (crlf, _) = build(&lines, "\r\n", trailing);
        let lf = b(&encode(&lf, layout));
        let crlf = b(&encode(&crlf, layout));
        prop_assert_eq!(normalise_eol(&crlf), normalise_eol(&lf));
        prop_assert_eq!(normalise_eol(&crlf), lf);
    }

    /// For any bytes at all: normalising twice changes nothing, and `apply_eol` is idempotent and leaves the normalised
    /// form unchanged. The alphabet is small so that CR, LF, both BOMs and NUL meet often.
    #[test]
    fn eol_functions_are_total_and_idempotent_prop(
        x in prop::collection::vec(prop::sample::select(vec![0x0D_u8, 0x0A, 0x41, 0x00, 0xFF, 0xFE, 0xEF, 0xBB, 0xBF]), 0..64),
        newline in prop_oneof![Just(Newline::Lf), Just(Newline::Crlf)],
    ) {
        let x = Bytes::from(x);
        let n = normalise_eol(&x);
        prop_assert_eq!(normalise_eol(&n), n.clone());
        let a = apply_eol(&x, newline);
        prop_assert_eq!(apply_eol(&a, newline), a.clone());
        // Line endings are the only thing either function changes.
        prop_assert_eq!(normalise_eol(&a), n);
        prop_assert!(a.len() <= x.len() * 2 + 2);
    }
}

#[test]
fn normalise_hides_eol_only_difference() {
    let lf = b(b"a: 1\nb: 2\n");
    let crlf = b(b"a: 1\r\nb: 2\r\n");
    let cr = b(b"a: 1\rb: 2\r");
    assert_eq!(normalise_eol(&crlf), lf);
    assert_eq!(normalise_eol(&cr), lf);
    assert_ne!(crlf, lf);
}

#[test]
fn normalise_table() {
    let rows: [(&[u8], &[u8]); 9] = [
        (b"", b""),
        (b"no terminator", b"no terminator"),
        (b"\r", b"\n"),
        (b"\r\n", b"\n"),
        // A CR in front of a CRLF is its own terminator.
        (b"\r\r\n", b"\n\n"),
        (b"a\r\n\r\nb", b"a\n\nb"),
        (b"a\rb\nc\r\n", b"a\nb\nc\n"),
        // The BOM is content, not a line ending.
        (b"\xEF\xBB\xBFa\r\n", b"\xEF\xBB\xBFa\n"),
        (b"\xC3\xA9\r\n", b"\xC3\xA9\n"),
    ];
    for (input, expected) in rows {
        assert_eq!(normalise_eol(&b(input)), b(expected), "input {input:?}");
    }
}

#[test]
fn apply_eol_table() {
    let rows: [(&[u8], Newline, &[u8]); 8] = [
        (b"a\nb\n", Newline::Lf, b"a\nb\n"),
        (b"a\nb\n", Newline::Crlf, b"a\r\nb\r\n"),
        // Whatever the input has is brought to the requested style, once.
        (b"a\r\nb\n", Newline::Crlf, b"a\r\nb\r\n"),
        (b"a\r\nb\n", Newline::Lf, b"a\nb\n"),
        (b"a\rb", Newline::Crlf, b"a\r\nb"),
        (b"\n\n", Newline::Crlf, b"\r\n\r\n"),
        (b"", Newline::Crlf, b""),
        (b"\xEF\xBB\xBFa\n", Newline::Crlf, b"\xEF\xBB\xBFa\r\n"),
    ];
    for (input, newline, expected) in rows {
        assert_eq!(
            apply_eol(&b(input), newline),
            b(expected),
            "input {input:?} as {newline:?}"
        );
    }
}

#[test]
fn utf16_is_read_in_code_units() {
    // "a\r\nb" in UTF-16LE and BE.
    let le = b(b"\xFF\xFEa\x00\r\x00\n\x00b\x00");
    let be = b(b"\xFE\xFF\x00a\x00\r\x00\n\x00b");
    assert_eq!(normalise_eol(&le), b(b"\xFF\xFEa\x00\n\x00b\x00"));
    assert_eq!(normalise_eol(&be), b(b"\xFE\xFF\x00a\x00\n\x00b"));
    assert_eq!(apply_eol(&normalise_eol(&le), dominant_eol(&le)), le);
    assert_eq!(apply_eol(&normalise_eol(&be), dominant_eol(&be)), be);
    assert_eq!(dominant_eol(&le), Newline::Crlf);
    assert_eq!(dominant_eol(&be), Newline::Crlf);
}

#[test]
fn utf16_unit_that_looks_like_crlf_is_untouched() {
    // U+0A0D is one character; in UTF-16LE its bytes are `0D 0A`. It is not a line break.
    let le = b(b"\xFF\xFE\x0D\x0A");
    assert_eq!(normalise_eol(&le), le);
    assert_eq!(apply_eol(&le, Newline::Crlf), le);
    assert_eq!(dominant_eol(&le), Newline::Lf);
    assert_eq!(detect_format(&le).eol, EolStyle::Lf);
    // Same for BE: U+0D0A is `0D 0A`.
    let be = b(b"\xFE\xFF\x0D\x0A");
    assert_eq!(normalise_eol(&be), be);
}

#[test]
fn utf16_odd_trailing_byte_is_kept() {
    let le = b(b"\xFF\xFEa\x00\r\x00\n\x00Z");
    assert_eq!(normalise_eol(&le), b(b"\xFF\xFEa\x00\n\x00Z"));
    assert_eq!(apply_eol(&normalise_eol(&le), Newline::Crlf), le);
}

#[test]
fn dominant_eol_table() {
    let rows: [(&[u8], Newline); 8] = [
        (b"", Newline::Lf),
        (b"no terminator", Newline::Lf),
        (b"a\nb\n", Newline::Lf),
        (b"a\r\nb\r\n", Newline::Crlf),
        // Mixed: the more frequent of CRLF and LF wins, and a tie goes to LF.
        (b"a\r\nb\r\nc\r\nd\n", Newline::Crlf),
        (b"a\r\nb\nc\nd\n", Newline::Lf),
        (b"a\r\nb\n", Newline::Lf),
        // A lone CR counts on the LF side.
        (b"a\r\nb\r\nc\r\nd\re\r", Newline::Crlf),
    ];
    for (input, expected) in rows {
        assert_eq!(dominant_eol(&b(input)), expected, "input {input:?}");
    }
    assert_eq!(dominant_eol(&b(b"a\r\nb\r\nc\rd\re\r")), Newline::Lf);
}

#[test]
fn nothing_to_change_is_the_same_buffer() {
    // No copy when the bytes are already in the requested style: the result shares the input's allocation.
    let lf = Bytes::from_static(b"a: 1\nb: 2\n");
    assert_eq!(normalise_eol(&lf).as_ptr(), lf.as_ptr());
    assert_eq!(apply_eol(&lf, Newline::Lf).as_ptr(), lf.as_ptr());
    let utf16 = Bytes::from_static(b"\xFF\xFEa\x00\n\x00");
    assert_eq!(normalise_eol(&utf16).as_ptr(), utf16.as_ptr());
    let empty = Bytes::new();
    assert!(normalise_eol(&empty).is_empty());
}
