//! T1 (S21): an error names what went wrong and never the input that caused it. A rejected value may be a secret.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{EolStyle, FileFormat, NfsPath, TextEncoding};
use engine::{DecodeError, DenyError, DenyMatcher, EncodeError, Limits, decode_text, encode_text};

const MARKER: &str = "SECRET-MARKER-5c1e";

fn assert_clean(what: &str, text: &str) {
    assert!(!text.contains(MARKER), "{what} echoes the input: {text}");
}

#[test]
fn display_and_debug_never_echo_input() {
    let limits = Limits {
        input_bytes: 64,
        ..Limits::default()
    };

    // Every way `decode_text` can fail, with the marker in the bytes.
    let utf8_bom_bad = [b"\xEF\xBB\xBF".as_slice(), MARKER.as_bytes(), b"\xFF"].concat();
    let utf16_odd = [b"\xFF\xFE".as_slice(), MARKER.as_bytes(), b"!"].concat();
    let too_large = MARKER.repeat(8).into_bytes();
    let cases = [
        (utf8_bom_bad, DecodeError::InvalidUtf8),
        (utf16_odd, DecodeError::InvalidUtf16),
        (too_large, DecodeError::TooLarge),
    ];
    for (input, expected) in cases {
        let err = decode_text(&input, &limits).unwrap_err();
        assert_eq!(err, expected);
        assert_clean("DecodeError display", &err.to_string());
        assert_clean("DecodeError debug", &format!("{err:?}"));
    }

    // Every way `encode_text` can fail.
    let latin1 = FileFormat {
        eol: EolStyle::Lf,
        encoding: TextEncoding::Latin1,
        bom: false,
    };
    let bare16 = FileFormat {
        encoding: TextEncoding::Utf16Le,
        ..latin1
    };
    let utf8 = FileFormat {
        encoding: TextEncoding::Utf8,
        ..latin1
    };
    let unrepresentable = format!("{MARKER}\u{20AC}");
    let big = MARKER.repeat(8);
    let cases = [
        (unrepresentable.as_str(), latin1, EncodeError::Unrepresentable),
        (MARKER, bare16, EncodeError::BadFormat),
        (big.as_str(), utf8, EncodeError::TooLarge),
    ];
    for (text, format, expected) in cases {
        let err = encode_text(text, format, &limits).unwrap_err();
        assert_eq!(err, expected);
        assert_clean("EncodeError display", &err.to_string());
        assert_clean("EncodeError debug", &format!("{err:?}"));
    }

    // Every way a deny glob list can be refused: the glob text is the input here, and globset's own error repeats it.
    let bad = format!("{MARKER}[");
    let long = MARKER.repeat(60);
    let many: Vec<String> = (0..=1_000).map(|i| format!("{MARKER}{i}")).collect();
    let cases = [
        (vec![bad.as_str()], DenyError::InvalidGlob),
        (vec![long.as_str()], DenyError::GlobTooLong),
        (many.iter().map(String::as_str).collect(), DenyError::TooManyGlobs),
    ];
    for (globs, expected) in cases {
        let err = DenyMatcher::with_globs(globs).unwrap_err();
        assert_eq!(err, expected);
        assert_clean("DenyError display", &err.to_string());
        assert_clean("DenyError debug", &format!("{err:?}"));
    }

    // The matcher itself never prints the globs it holds.
    let matcher = DenyMatcher::with_globs([format!("{MARKER}*").as_str()]).unwrap();
    assert_clean("DenyMatcher debug", &format!("{matcher:?}"));
    assert!(matcher.is_denied(&NfsPath::parse(&format!("a/{MARKER}x")).unwrap()));
}

#[test]
fn errors_are_small_copyable_values() {
    // No payload means nothing to leak and nothing to allocate on a hostile input.
    fn is_copy<T: Copy + Send + Sync + 'static + std::error::Error>() {}
    is_copy::<DecodeError>();
    is_copy::<EncodeError>();
    is_copy::<DenyError>();
}
