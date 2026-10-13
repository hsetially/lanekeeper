//! T2 (S11, S21, S22): every hard limit is enforced at its boundary (exactly at the limit passes, one over fails with an
//! error that names the limit), and hostile input returns an error value, never a panic or a hang.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::format_push_string,
    clippy::format_collect
)]

mod support;

use std::time::{Duration, Instant};

use engine::{LimitKind, Limits, ParseError, Syntax, flatten, parse};
use support::limits;

fn with(f: impl FnOnce(&mut Limits)) -> Limits {
    let mut l = limits();
    f(&mut l);
    l
}

fn parse_ok(text: &str, syntax: Syntax, limits: &Limits) {
    if let Err(e) = parse(text, syntax, limits) {
        panic!("{syntax} should parse ({e}): {}", &text[..text.len().min(80)]);
    }
}

fn parse_limit(text: &str, syntax: Syntax, limits: &Limits) -> LimitKind {
    match parse(text, syntax, limits) {
        Err(ParseError::Limit(kind)) => kind,
        other => panic!("{syntax} should hit a limit: {:?}", other.map(|_| ())),
    }
}

#[test]
fn input_size_limit_is_enforced_at_boundary() {
    let l = with(|l| l.input_bytes = 64);
    let yaml = |n: usize| format!("a: {}\n", "x".repeat(n - 4));
    parse_ok(&yaml(64), Syntax::Yaml, &l);
    assert_eq!(parse_limit(&yaml(65), Syntax::Yaml, &l), LimitKind::InputBytes);
    let json = |n: usize| format!("[\"{}\"]", "x".repeat(n - 4));
    parse_ok(&json(64), Syntax::Json, &l);
    assert_eq!(parse_limit(&json(65), Syntax::Json, &l), LimitKind::InputBytes);
    let props = |n: usize| format!("a={}", "x".repeat(n - 2));
    parse_ok(&props(64), Syntax::Properties, &l);
    assert_eq!(
        parse_limit(&props(65), Syntax::Properties, &l),
        LimitKind::InputBytes
    );
}

#[test]
fn depth_limit_is_enforced_at_boundary() {
    let l = with(|l| l.nesting_depth = 8);
    let flow = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
    parse_ok(&flow(8), Syntax::Yaml, &l);
    assert_eq!(parse_limit(&flow(9), Syntax::Yaml, &l), LimitKind::Depth);
    parse_ok(&flow(8), Syntax::Json, &l);
    assert_eq!(parse_limit(&flow(9), Syntax::Json, &l), LimitKind::Depth);
    // Block maps: `a:` then one more level each time.
    let block = |n: usize| {
        let mut s = String::new();
        for i in 0..n {
            s.push_str(&"  ".repeat(i));
            s.push_str(&format!("k{i}:\n"));
        }
        s.push_str(&"  ".repeat(n));
        s.push_str("leaf: 1\n");
        s
    };
    parse_ok(&block(7), Syntax::Yaml, &l);
    assert_eq!(parse_limit(&block(8), Syntax::Yaml, &l), LimitKind::Depth);
    let objects = |n: usize| format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n));
    parse_ok(&objects(8), Syntax::Json, &l);
    assert_eq!(parse_limit(&objects(9), Syntax::Json, &l), LimitKind::Depth);
}

#[test]
fn depth_counts_what_an_alias_brings_in() {
    // `deep` is 6 levels; used at depth 3 it would make 9 levels once expanded.
    let l = with(|l| l.nesting_depth = 8);
    let text = "deep: &d [[[[[[1]]]]]]\nuse:\n  x:\n    y: *d\n";
    assert_eq!(parse_limit(text, Syntax::Yaml, &l), LimitKind::Depth);
    let ok = "deep: &d [[[[[[1]]]]]]\nuse: *d\n";
    parse_ok(ok, Syntax::Yaml, &l);
}

#[test]
fn nodes_limit_is_enforced_at_boundary() {
    // `[1, 2, 3]` is four nodes: the list and three scalars.
    let l = with(|l| l.nodes = 11);
    let list = |n: usize| format!("[{}]", vec!["1"; n].join(","));
    parse_ok(&list(10), Syntax::Yaml, &l);
    assert_eq!(parse_limit(&list(11), Syntax::Yaml, &l), LimitKind::Nodes);
    parse_ok(&list(10), Syntax::Json, &l);
    assert_eq!(parse_limit(&list(11), Syntax::Json, &l), LimitKind::Nodes);
    // An alias counts for everything it stands for.
    let aliased = "a: &a [1, 2, 3]\nb: *a\n";
    // root map 1 + key a 1 + list 4 + key b 1 + alias (4) = 11
    let l = with(|l| l.nodes = 11);
    parse_ok(aliased, Syntax::Yaml, &l);
    let l = with(|l| l.nodes = 10);
    assert_eq!(parse_limit(aliased, Syntax::Yaml, &l), LimitKind::Nodes);
}

#[test]
fn nodes_limit_counts_across_documents() {
    let l = with(|l| l.nodes = 5);
    parse_ok("a: 1\n---\nb: 2\n", Syntax::Yaml, &with(|l| l.nodes = 6));
    assert_eq!(
        parse_limit("a: 1\n---\nb: 2\n", Syntax::Yaml, &l),
        LimitKind::Nodes
    );
}

#[test]
fn scalar_limit_is_enforced_at_boundary() {
    let l = with(|l| l.scalar_bytes = 16);
    for (syntax, wrap) in [
        (Syntax::Yaml, "a: {}\n"),
        (Syntax::Yaml, "a: \"{}\"\n"),
        (Syntax::Yaml, "a: |\n  {}\n"),
        (Syntax::Json, "{\"a\": \"{}\"}"),
        (Syntax::Properties, "a={}\n"),
    ] {
        let at = wrap.replace("{}", &"x".repeat(16));
        let over = wrap.replace("{}", &"x".repeat(17));
        // A block scalar keeps its final newline, so it is one byte longer than what is written.
        if wrap.contains('|') {
            let at = wrap.replace("{}", &"x".repeat(15));
            parse_ok(&at, syntax, &l);
            assert_eq!(parse_limit(&over, syntax, &l), LimitKind::ScalarBytes, "{wrap}");
            continue;
        }
        parse_ok(&at, syntax, &l);
        assert_eq!(parse_limit(&over, syntax, &l), LimitKind::ScalarBytes, "{wrap}");
    }
}

#[test]
fn settings_limit_is_enforced_at_boundary() {
    let l = with(|l| l.settings = 5);
    let map = |n: usize| (0..n).map(|i| format!("k{i}: {i}\n")).collect::<String>();
    let p = parse(&map(5), Syntax::Yaml, &l).unwrap();
    assert_eq!(flatten(&p, &l).unwrap().settings.len(), 5);
    let p = parse(&map(6), Syntax::Yaml, &l).unwrap();
    assert_eq!(
        flatten(&p, &l).unwrap_err(),
        ParseError::Limit(LimitKind::Settings)
    );
    let props = |n: usize| (0..n).map(|i| format!("k{i}={i}\n")).collect::<String>();
    let p = parse(&props(5), Syntax::Properties, &l).unwrap();
    assert_eq!(flatten(&p, &l).unwrap().settings.len(), 5);
    let p = parse(&props(6), Syntax::Properties, &l).unwrap();
    assert_eq!(
        flatten(&p, &l).unwrap_err(),
        ParseError::Limit(LimitKind::Settings)
    );
    let items = |n: usize| format!("[{}]", vec!["1"; n].join(","));
    let p = parse(&items(5), Syntax::Json, &l).unwrap();
    assert_eq!(flatten(&p, &l).unwrap().settings.len(), 5);
    let p = parse(&items(6), Syntax::Json, &l).unwrap();
    assert_eq!(
        flatten(&p, &l).unwrap_err(),
        ParseError::Limit(LimitKind::Settings)
    );
}

#[test]
fn key_length_limit_is_enforced_at_boundary() {
    let l = with(|l| l.setting_path_bytes = 32);
    // One key: the path is the key.
    let at = format!("{}: 1\n", "k".repeat(32));
    let over = format!("{}: 1\n", "k".repeat(33));
    let p = parse(&at, Syntax::Yaml, &l).unwrap();
    assert_eq!(flatten(&p, &l).unwrap().settings.len(), 1);
    assert_eq!(parse_limit(&over, Syntax::Yaml, &l), LimitKind::SettingPathBytes);
    let json_at = format!("{{\"{}\": 1}}", "k".repeat(32));
    let json_over = format!("{{\"{}\": 1}}", "k".repeat(33));
    let p = parse(&json_at, Syntax::Json, &l).unwrap();
    assert_eq!(flatten(&p, &l).unwrap().settings.len(), 1);
    assert_eq!(
        parse_limit(&json_over, Syntax::Json, &l),
        LimitKind::SettingPathBytes
    );
    let props_at = format!("{}=1\n", "k".repeat(32));
    let props_over = format!("{}=1\n", "k".repeat(33));
    let p = parse(&props_at, Syntax::Properties, &l).unwrap();
    assert_eq!(flatten(&p, &l).unwrap().settings.len(), 1);
    assert_eq!(
        parse_limit(&props_over, Syntax::Properties, &l),
        LimitKind::SettingPathBytes
    );
}

#[test]
fn the_whole_path_has_to_fit_not_just_each_key() {
    let l = with(|l| l.setting_path_bytes = 32);
    // 10 + 1 + 10 + 1 + 10 = 32 fits; one more byte does not.
    let k = "k".repeat(10);
    let fits = format!("{k}:\n  {k}:\n    {k}: 1\n");
    let p = parse(&fits, Syntax::Yaml, &l).unwrap();
    assert_eq!(flatten(&p, &l).unwrap().settings.len(), 1);
    let over = format!("{k}:\n  {k}:\n    {k}x: 1\n");
    let p = parse(&over, Syntax::Yaml, &l).unwrap();
    assert_eq!(
        flatten(&p, &l).unwrap_err(),
        ParseError::Limit(LimitKind::SettingPathBytes)
    );
    // A key that needs quoting is longer once written: it counts as written.
    let quoted = format!("\"{}\": 1\n", ".".repeat(29));
    let p = parse(&quoted, Syntax::Yaml, &l).unwrap();
    assert_eq!(
        flatten(&p, &l).unwrap_err(),
        ParseError::Limit(LimitKind::SettingPathBytes)
    );
}

#[test]
fn warnings_are_capped_and_the_rest_counted() {
    let l = with(|l| l.warnings = 3);
    let text: String = (0..10).map(|_| "k: 1\n").collect();
    let p = parse(&text, Syntax::Yaml, &l).unwrap();
    assert_eq!(p.warnings.len(), 3);
    assert_eq!(p.warnings_dropped, 6);
}

#[test]
fn alias_bomb_returns_limit_error_fast() {
    let mut bomb = String::from("a: &a [x, x, x, x, x, x, x, x, x, x]\n");
    for (name, from) in "bcdefghij".chars().zip("abcdefghi".chars()) {
        let refs = vec![format!("*{from}"); 10].join(", ");
        bomb.push_str(&format!("{name}: &{name} [{refs}]\n"));
    }
    let started = Instant::now();
    assert_eq!(parse_limit(&bomb, Syntax::Yaml, &limits()), LimitKind::Nodes);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // The same through merge keys, which expand maps.
    let mut merge = String::from("m0: &m0 {a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9, j: 10}\n");
    for i in 1..12 {
        let refs = vec![format!("*m{}", i - 1); 10].join(", ");
        merge.push_str(&format!("m{i}: &m{i} {{<<: [{refs}], own{i}: 1}}\n"));
    }
    let started = Instant::now();
    assert_eq!(parse_limit(&merge, Syntax::Yaml, &limits()), LimitKind::Nodes);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_wide_alias_fan_out_is_stopped_by_the_settings_limit() {
    // 300,000 aliases of one scalar: within the node limit (500,000 with the list), over the settings limit.
    let text = format!("a: &a 1\nb: [{}]\n", vec!["*a"; 300_000].join(","));
    assert!(text.len() < Limits::default().input_bytes);
    let started = Instant::now();
    let p = parse(&text, Syntax::Yaml, &limits()).unwrap();
    assert_eq!(
        flatten(&p, &limits()).unwrap_err(),
        ParseError::Limit(LimitKind::Settings)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn hostile_input_returns_error_not_panic() {
    let big = Limits::default();
    let inputs: Vec<(&str, Syntax, String)> = vec![
        ("200k open brackets", Syntax::Yaml, "[".repeat(200_000)),
        ("200k open brackets", Syntax::Json, "[".repeat(200_000)),
        ("200k open objects", Syntax::Json, "{\"a\":".repeat(200_000)),
        ("200k open flow maps", Syntax::Yaml, "{a: ".repeat(200_000)),
        (
            "200k nested block seq",
            Syntax::Yaml,
            (0..20_000)
                .map(|i| format!("{}- \n", " ".repeat(i % 80)))
                .collect(),
        ),
        ("anchors everywhere", Syntax::Yaml, "&a ".repeat(100_000)),
        ("aliases everywhere", Syntax::Yaml, "*a ".repeat(100_000)),
        ("tags everywhere", Syntax::Yaml, "!!str ".repeat(100_000)),
        ("many documents", Syntax::Yaml, "---\n".repeat(500_000)),
        ("many empty keys", Syntax::Yaml, ":\n".repeat(100_000)),
        ("many question marks", Syntax::Yaml, "? ".repeat(100_000)),
        ("directive flood", Syntax::Yaml, "%YAML 1.2\n".repeat(50_000)),
        (
            "a million-character key",
            Syntax::Yaml,
            format!("{}: 1\n", "k".repeat(1_000_000)),
        ),
        (
            "a million-character scalar",
            Syntax::Yaml,
            format!("k: {}\n", "v".repeat(1_000_000)),
        ),
        ("NUL bytes", Syntax::Yaml, "a:\0\0\0 b\0\n".repeat(1000)),
        (
            "control characters",
            Syntax::Json,
            "\u{1}\u{2}\u{7f}".repeat(1000),
        ),
        (
            "huge exponent",
            Syntax::Json,
            "[1e999999999999, -1e-999999999999]".to_owned(),
        ),
        ("long number", Syntax::Json, format!("[{}]", "9".repeat(100_000))),
        (
            "unterminated string",
            Syntax::Json,
            format!("\"{}", "a".repeat(1_000_000)),
        ),
        ("many commas", Syntax::Json, format!("[{}]", ",".repeat(100_000))),
        ("many keys", Syntax::Properties, "k=v\n".repeat(400_000)),
        ("many continuations", Syntax::Properties, "a=\\\n".repeat(400_000)),
        (
            "escape flood",
            Syntax::Properties,
            format!("a={}", "\\u0041".repeat(100_000)),
        ),
        ("backslash flood", Syntax::Properties, "\\".repeat(1_000_000)),
        ("separator flood", Syntax::Properties, "=".repeat(1_000_000)),
    ];
    let started = Instant::now();
    for (what, syntax, text) in &inputs {
        match parse(text, *syntax, &big) {
            Ok(p) => {
                // Whatever parsed also flattens or fails cleanly.
                if let Err(e) = flatten(&p, &big) {
                    assert!(matches!(e, ParseError::Limit(_)), "{what}: {e:?}");
                }
            }
            Err(ParseError::Syntax { .. } | ParseError::Limit(_) | ParseError::UnknownAlias { .. }) => {}
            Err(other) => panic!("{what} ({syntax}): {other:?}"),
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "hostile inputs took {:?}",
        started.elapsed()
    );
}

#[test]
fn work_is_bounded_by_the_input_not_by_what_it_says() {
    // The same hostile shapes with small limits finish in a blink: nothing is proportional to a claimed size.
    let tiny = with(|l| {
        l.input_bytes = 4096;
        l.nodes = 100;
        l.nesting_depth = 8;
        l.settings = 50;
    });
    let started = Instant::now();
    for text in ["[".repeat(4000), "a: &a [1,2,3]\nb: [*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a,*a]\n".to_owned()] {
        let r = parse(&text, Syntax::Yaml, &tiny);
        assert!(matches!(r, Err(ParseError::Limit(_) | ParseError::Syntax { .. })), "{:?}", r.map(|_| ()));
    }
    assert!(started.elapsed() < Duration::from_secs(1));
}
