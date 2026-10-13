//! T1 (S21): an error names what went wrong and never the input that caused it. A rejected value may be a secret.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{EolStyle, FileFormat, NfsPath, TextEncoding};
use engine::{
    DecodeError, DenyError, DenyMatcher, EncodeError, LimitKind, Limits, ParseError, Syntax, decode_text,
    encode_text, flatten, parse, validate,
};

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

/// T2: the parsers are where hostile text comes in. Every way they can refuse a file, with the marker in the input.
#[test]
fn parse_errors_never_echo_input() {
    let tiny = |f: fn(&mut Limits)| {
        let mut l = Limits::default();
        f(&mut l);
        l
    };
    let big_scalar = format!("{MARKER}{}", "x".repeat(40));
    let cases: Vec<(&str, Syntax, String, Limits)> = vec![
        (
            "yaml syntax",
            Syntax::Yaml,
            format!("{MARKER}: [unclosed\nnext: {MARKER}\n"),
            Limits::default(),
        ),
        (
            "yaml unknown alias",
            Syntax::Yaml,
            format!("a: *{MARKER}\n"),
            Limits::default(),
        ),
        (
            "yaml scalar limit",
            Syntax::Yaml,
            format!("a: {big_scalar}\n"),
            tiny(|l| l.scalar_bytes = 16),
        ),
        (
            "yaml key limit",
            Syntax::Yaml,
            format!("{big_scalar}: 1\n"),
            tiny(|l| l.setting_path_bytes = 16),
        ),
        (
            "yaml depth",
            Syntax::Yaml,
            format!("{}{MARKER}{}", "[".repeat(5), "]".repeat(5)),
            tiny(|l| l.nesting_depth = 2),
        ),
        (
            "yaml nodes",
            Syntax::Yaml,
            format!("[{MARKER}, {MARKER}, {MARKER}]"),
            tiny(|l| l.nodes = 2),
        ),
        (
            "yaml input",
            Syntax::Yaml,
            format!("a: {MARKER}{MARKER}\n"),
            tiny(|l| l.input_bytes = 16),
        ),
        (
            "json syntax",
            Syntax::Json,
            format!("{{\"{MARKER}\": }}"),
            Limits::default(),
        ),
        (
            "json trailing",
            Syntax::Json,
            format!("1 {MARKER}"),
            Limits::default(),
        ),
        (
            "json scalar limit",
            Syntax::Json,
            format!("[\"{big_scalar}\"]"),
            tiny(|l| l.scalar_bytes = 16),
        ),
        (
            "json depth",
            Syntax::Json,
            format!("{}\"{MARKER}\"{}", "[".repeat(5), "]".repeat(5)),
            tiny(|l| l.nesting_depth = 2),
        ),
        (
            "properties escape",
            Syntax::Properties,
            format!("{MARKER}=\\u12\n"),
            Limits::default(),
        ),
        (
            "properties scalar limit",
            Syntax::Properties,
            format!("a={big_scalar}\n"),
            tiny(|l| l.scalar_bytes = 16),
        ),
    ];
    for (what, syntax, text, limits) in cases {
        let err = parse(&text, syntax, &limits).expect_err(what);
        assert_clean(&format!("{what}: display"), &err.to_string());
        assert_clean(&format!("{what}: debug"), &format!("{err:?}"));
    }
}

/// T2: the limits that `flatten` finds, and the decode errors that come through `validate`.
#[test]
fn flatten_and_validate_errors_never_echo_input() {
    let tiny = |f: fn(&mut Limits)| {
        let mut l = Limits::default();
        f(&mut l);
        l
    };
    let many = format!("{MARKER}1: 1\n{MARKER}2: 2\n{MARKER}3: 3\n");
    let l = tiny(|l| l.settings = 2);
    let err = flatten(&parse(&many, Syntax::Yaml, &l).unwrap(), &l).unwrap_err();
    assert_eq!(err, ParseError::Limit(LimitKind::Settings));
    assert_clean("settings display", &err.to_string());
    let deep = format!("{MARKER}:\n  {MARKER}:\n    {MARKER}: 1\n");
    let l = tiny(|l| l.setting_path_bytes = 40);
    let err = flatten(&parse(&deep, Syntax::Yaml, &l).unwrap(), &l).unwrap_err();
    assert_eq!(err, ParseError::Limit(LimitKind::SettingPathBytes));
    assert_clean("path display", &err.to_string());

    // `validate` starts from bytes: the decode errors come through it.
    let utf8_bom_bad = [b"\xEF\xBB\xBF".as_slice(), MARKER.as_bytes(), b"\xFF"].concat();
    let err = validate(&utf8_bom_bad, Syntax::Yaml, &Limits::default()).unwrap_err();
    assert!(
        matches!(err, ParseError::Decode(DecodeError::InvalidUtf8)),
        "{err:?}"
    );
    assert_clean("decode display", &err.to_string());
    assert_clean("decode debug", &format!("{err:?}"));
}

/// T2 (S13, S21): a debug print of a parsed or flattened file shows its shape, never its values. A config can hold a
/// password, and a stray `{:?}` in a log line must not copy it.
#[test]
fn debug_output_never_contains_values() {
    for (syntax, text) in [
        (Syntax::Yaml, format!("password: {MARKER}\nlist:\n  - {MARKER}\n")),
        (Syntax::Yaml, format!("a: |\n  {MARKER}\nb: &x {MARKER}\nc: *x\n")),
        (
            Syntax::Json,
            format!("{{\"token\": \"{MARKER}\", \"l\": [\"{MARKER}\"]}}"),
        ),
        (Syntax::Properties, format!("db.password={MARKER}\n")),
    ] {
        let parsed = parse(&text, syntax, &Limits::default()).unwrap();
        assert_clean(&format!("{syntax} Parsed debug"), &format!("{parsed:?}"));
        assert_clean(&format!("{syntax} Parsed pretty debug"), &format!("{parsed:#?}"));
        let flat = flatten(&parsed, &Limits::default()).unwrap();
        assert!(!flat.settings.is_empty());
        assert_clean(&format!("{syntax} Flat debug"), &format!("{flat:?}"));
        assert_clean(&format!("{syntax} Flat pretty debug"), &format!("{flat:#?}"));
        for s in &flat.settings {
            assert_clean(&format!("{syntax} FlatSetting debug"), &format!("{s:?}"));
        }
    }
}

#[test]
fn parse_errors_are_small_copyable_values() {
    fn is_copy<T: Copy + Send + Sync + 'static + std::error::Error>() {}
    is_copy::<ParseError>();
}

/// T7 (S21): a search pattern or a scanned text may be a pasted secret. The errors of both name what was wrong and nothing
/// of the input.
#[test]
fn search_and_scan_errors_never_echo_input() {
    fn is_copy<T: Copy + Send + Sync + 'static + std::error::Error>() {}
    use bytes::Bytes;
    use domain::{FileClass, NfsPath, SwimlaneId, TextQuery};
    use engine::{
        CancelFlag, ScanConfig, ScanError, SearchBlob, SearchCorpus, SearchError, SearchLocation,
        normalised_hash, search_text, secret_scan,
    };

    let content = Bytes::from_static(b"nothing\n");
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
    let query = |pattern: String, regex: bool, glob: Option<&str>| TextQuery {
        pattern,
        regex,
        case_sensitive: true,
        swimlanes: Vec::new(),
        path_glob: glob.map(|g| domain::ShortText::parse(g).unwrap()),
        max_total: 10,
        max_per_file: 10,
    };
    let cases = [
        (
            "too long",
            query(MARKER.repeat(60), false, None),
            SearchError::PatternTooLong,
        ),
        (
            "not a regex",
            query(format!("({MARKER}"), true, None),
            SearchError::InvalidPattern,
        ),
        (
            "line break",
            query(format!("{MARKER}\n"), false, None),
            SearchError::InvalidPattern,
        ),
        (
            "bad glob",
            query("x".into(), false, Some(&format!("{MARKER}[a-"))),
            SearchError::InvalidGlob,
        ),
    ];
    for (what, q, expected) in cases {
        let err = search_text(
            &corpus,
            &q,
            &DenyMatcher::default(),
            &Limits::default(),
            &pool,
            &CancelFlag::new(),
        )
        .unwrap_err();
        assert_eq!(err, expected, "{what}");
        assert_clean(&format!("{what}: display"), &err.to_string());
        assert_clean(&format!("{what}: debug"), &format!("{err:?}"));
    }

    let tiny = Limits {
        input_bytes: 8,
        ..Limits::default()
    };
    let err = secret_scan(&format!("{MARKER}{MARKER}"), &ScanConfig::default(), &tiny).unwrap_err();
    assert_eq!(err, ScanError::TooLarge);
    assert_clean("scan too large", &format!("{err} {err:?}"));
    let many = ScanConfig {
        max_matches: 0,
        ..ScanConfig::default()
    };
    let err = secret_scan(
        "ghp_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dE3fG5",
        &many,
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(err, ScanError::TooManyMatches);
    assert_clean("scan too many", &format!("{err} {err:?}"));

    is_copy::<SearchError>();
    is_copy::<ScanError>();
}
