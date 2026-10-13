//! T2: the setting path syntax (`docs/domain-model.md`, "Flattening rules"): `a.b.c`, `a.b[0]`, `a["x.y"]`, and the
//! `#<document>/` prefix. The encoder and the decoder are inverses (property tests), and a YAML file and the same data as
//! JSON flatten to the same settings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use domain::SettingPath;
use engine::{DecodedPath, PathSegment, Syntax, decode_setting_path, encode_setting_path};
use proptest::prelude::*;
use serde_json::{Map, Value};
use support::{flat, pairs};

fn key(s: &str) -> PathSegment {
    PathSegment::Key(s.to_owned())
}

fn enc(document: Option<u32>, segments: &[PathSegment]) -> String {
    encode_setting_path(document, segments)
}

#[test]
fn encoding_table() {
    let idx = PathSegment::Index;
    let cases: Vec<(Option<u32>, Vec<PathSegment>, &str)> = vec![
        (None, vec![key("a"), key("b"), key("c")], "a.b.c"),
        (None, vec![key("a"), key("b"), idx(0)], "a.b[0]"),
        (None, vec![key("a"), idx(12), key("b")], "a[12].b"),
        (None, vec![idx(0)], "[0]"),
        (None, vec![idx(2), key("x")], "[2].x"),
        (None, vec![idx(0), idx(1)], "[0][1]"),
        (None, vec![key("x.y")], "[\"x.y\"]"),
        (None, vec![key("a"), key("x.y")], "a[\"x.y\"]"),
        (None, vec![key("a"), key("x.y"), key("z")], "a[\"x.y\"].z"),
        (None, vec![key("a"), key("x.y"), idx(3)], "a[\"x.y\"][3]"),
        (None, vec![key("c[0]")], "[\"c[0]\"]"),
        (None, vec![key("a"), key("e]f")], "a[\"e]f\"]"),
        (None, vec![key("")], "[\"\"]"),
        (None, vec![key("a"), key("")], "a[\"\"]"),
        // A quote or a backslash inside a key that needs no quoting stays as it is.
        (None, vec![key("q\"uote")], "q\"uote"),
        (None, vec![key("back\\slash")], "back\\slash"),
        // Inside the quotes they are escaped.
        (None, vec![key("a.\"b\\")], "[\"a.\\\"b\\\\\"]"),
        // Control characters are written as escapes, so a path never holds one.
        (None, vec![key("tab\there")], "[\"tab\\u{9}here\"]"),
        (None, vec![key("line\nbreak")], "[\"line\\u{a}break\"]"),
        // A first key that looks like the document prefix or the root marker is quoted; later keys are not.
        (None, vec![key("#0/x")], "[\"#0/x\"]"),
        (None, vec![key("a"), key("#0/x")], "a.#0/x"),
        (None, vec![key("$")], "[\"$\"]"),
        (None, vec![key("a"), key("$")], "a.$"),
        (None, vec![key("with space"), key("ünï")], "with space.ünï"),
        (None, vec![], "$"),
        (Some(0), vec![], "#0/$"),
        (Some(1), vec![key("a")], "#1/a"),
        (Some(12), vec![idx(0), key("b")], "#12/[0].b"),
        (Some(3), vec![key("x.y")], "#3/[\"x.y\"]"),
    ];
    for (doc, segments, expected) in cases {
        let encoded = enc(doc, &segments);
        assert_eq!(encoded, expected, "{segments:?}");
        let decoded = decode_setting_path(&SettingPath::parse(&encoded).unwrap()).unwrap();
        assert_eq!(
            decoded,
            DecodedPath {
                document: doc,
                segments
            },
            "{encoded}"
        );
    }
}

#[test]
fn decoding_refuses_what_the_flattener_never_writes() {
    for bad in [
        "a..b",
        ".a",
        "a.",
        "a[",
        "a[]",
        "a[x]",
        "a[-1]",
        "a[99999999999]",
        "a[\"x]",
        "a[\"x\"",
        "a[\"x\"]b",
        "a[\"\\q\"]",
        "a[\"\\u{zz}\"]",
        "a[\"\\u{110000}\"]",
        "a[\"x\"y]",
        "a.[0]",
        "a]",
        "a.b]",
        "#/a",
        "#x/a",
        "#1",
        "#1/",
        "#99999999999/a",
        "$.a",
        "a.$x[",
    ] {
        let path = SettingPath::parse(bad).unwrap();
        assert!(decode_setting_path(&path).is_none(), "{bad}");
    }
}

#[test]
fn decoding_is_lenient_about_quotes_it_would_not_have_written() {
    // A person (or a tool) may quote a key that needs no quoting; it means the same key.
    let path = SettingPath::parse("a[\"b\"].c").unwrap();
    let decoded = decode_setting_path(&path).unwrap();
    assert_eq!(decoded.segments, [key("a"), key("b"), key("c")]);
    assert_eq!(enc(None, &decoded.segments), "a.b.c");
}

fn segment() -> impl Strategy<Value = PathSegment> {
    prop_oneof![
        3 => any::<String>().prop_map(|s| PathSegment::Key(s.chars().take(12).collect())),
        2 => "[a-z.\\[\\]\"\\\\#$ ]{0,6}".prop_map(PathSegment::Key),
        2 => any::<u32>().prop_map(PathSegment::Index),
    ]
}

proptest! {
    #[test]
    fn flatten_key_roundtrip_prop(
        document in proptest::option::of(any::<u32>()),
        segments in proptest::collection::vec(segment(), 0..6),
    ) {
        let encoded = encode_setting_path(document, &segments);
        prop_assert!(!encoded.chars().any(char::is_control), "control character in {encoded:?}");
        let path = SettingPath::parse(&encoded).unwrap();
        let decoded = decode_setting_path(&path).expect("what the encoder wrote can be decoded");
        prop_assert_eq!(decoded.document, document);
        prop_assert_eq!(&decoded.segments, &segments);
        // Writing it again gives the same text.
        prop_assert_eq!(encode_setting_path(decoded.document, &decoded.segments), encoded);
    }

    #[test]
    fn decode_never_panics_and_what_it_accepts_re_encodes(text in "\\PC{1,40}") {
        if let Ok(path) = SettingPath::parse(&text) {
            if let Some(decoded) = decode_setting_path(&path) {
                let again = encode_setting_path(decoded.document, &decoded.segments);
                let back = decode_setting_path(&SettingPath::parse(&again).unwrap()).unwrap();
                prop_assert_eq!(back, decoded);
            }
        }
    }
}

// --- JSON that is a subset of YAML flattens the same way ---

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(|n| Value::from(i64::from(n))),
        "[ -~]{0,8}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::btree_map("[ -~]{0,6}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

proptest! {
    #[test]
    fn json_subset_flattens_like_yaml_prop(value in json_value()) {
        let text = serde_json::to_string(&value).unwrap();
        let as_json = flat(Syntax::Json, &text);
        let as_yaml = flat(Syntax::Yaml, &text);
        prop_assert_eq!(pairs(&as_json), pairs(&as_yaml), "{}", text);
        let mut seen = std::collections::BTreeSet::new();
        for s in &as_json.settings {
            prop_assert!(seen.insert(s.path.to_string()), "a path appears twice: {}", s.path);
        }
    }
}

#[test]
fn json_and_yaml_agree_on_a_fixed_example() {
    let text = r#"{"a": {"b": [1, "two", null, true], "x.y": 1.5}, "c": []}"#;
    assert_eq!(pairs(&flat(Syntax::Json, text)), pairs(&flat(Syntax::Yaml, text)));
}
