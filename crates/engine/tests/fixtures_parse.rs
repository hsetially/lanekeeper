//! T2 (S11, acceptance 2): the parsers on real-shaped files. Every structured file in the small synthetic fixtures (YAML,
//! JSON, `.properties`; CRLF and LF; anchors, merge keys, 6,000-line files, planted duplicate keys) must parse and flatten,
//! and the generator's manifest is the oracle for the duplicate keys (C8).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use engine::{Limits, ParseWarning, Syntax, decode_text, flatten, parse, validate};
use support::{files_below, small_fixtures};

fn syntax_of(name: &str) -> Option<Syntax> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "yml" | "yaml" => Some(Syntax::Yaml),
        "json" => Some(Syntax::Json),
        "properties" => Some(Syntax::Properties),
        _ => None,
    }
}

#[test]
fn every_structured_fixture_file_parses_and_flattens() {
    let fx = small_fixtures();
    let mut counts = [0_usize; 3];
    let mut settings = 0;
    for (rel, path) in files_below(&fx.root.join("nfs")) {
        let Some(syntax) = syntax_of(&rel) else { continue };
        let bytes = std::fs::read(&path).unwrap();
        let v = validate(&bytes, syntax, &Limits::default()).unwrap_or_else(|e| panic!("{rel}: {e}"));
        settings += v.settings;
        counts[match syntax {
            Syntax::Yaml => 0,
            Syntax::Json => 1,
            Syntax::Properties => 2,
        }] += 1;
    }
    assert!(
        counts.iter().all(|&n| n > 0),
        "the fixtures hold every syntax: {counts:?}"
    );
    assert!(settings > 1_000, "{settings} settings");
}

#[test]
fn the_planted_duplicate_keys_are_found() {
    let fx = small_fixtures();
    assert!(!fx.manifest.planted.duplicate_keys.is_empty());
    for planted in &fx.manifest.planted.duplicate_keys {
        let path = fx.root.join("nfs/sl-01").join(&planted.path);
        let bytes = std::fs::read(&path).unwrap();
        let text = decode_text(&bytes, &Limits::default()).unwrap().text.into_owned();
        let parsed = parse(&text, Syntax::Yaml, &Limits::default()).unwrap();
        let found: BTreeSet<String> = parsed
            .warnings
            .iter()
            .filter_map(|w| match w {
                ParseWarning::DuplicateKey { path: Some(p), .. } => {
                    Some(p.as_str().rsplit('.').next().unwrap().to_owned())
                }
                _ => None,
            })
            .collect();
        let expected: BTreeSet<String> = planted.keys.iter().cloned().collect();
        assert_eq!(found, expected, "{}", planted.path);
    }
}

#[test]
fn the_big_yaml_files_flatten_to_one_setting_per_leaf_in_line_order() {
    let fx = small_fixtures();
    let (_, path) = files_below(&fx.root.join("nfs/sl-01"))
        .into_iter()
        .find(|(rel, _)| rel.ends_with("catalog-big-01.yml"))
        .unwrap();
    let text = String::from_utf8(std::fs::read(path).unwrap()).unwrap();
    assert!(text.lines().count() > 5_900);
    let parsed = parse(&text, Syntax::Yaml, &Limits::default()).unwrap();
    let flat = flatten(&parsed, &Limits::default()).unwrap();
    assert!(flat.settings.len() > 5_000);
    // Merge keys bring `timeoutMs` and `retries` into every item, through the alias.
    let merged = flat
        .settings
        .iter()
        .filter(|s| s.path.as_str().ends_with(".timeoutMs") && s.via_alias)
        .count();
    assert!(merged > 1_000, "{merged}");
    // Paths are unique, and every line range is inside the file.
    let mut seen = BTreeSet::new();
    let lines = u32::try_from(text.lines().count()).unwrap();
    for s in &flat.settings {
        assert!(seen.insert(s.path.as_str()), "{} appears twice", s.path);
        assert!(s.lines.end() <= lines + 1, "{}: {:?}", s.path, s.lines);
    }
}

#[test]
fn crlf_and_lf_files_flatten_to_the_same_settings() {
    let fx = small_fixtures();
    let mut compared = 0;
    for (rel, path) in files_below(&fx.root.join("nfs/sl-01")) {
        let Some(syntax) = syntax_of(&rel) else { continue };
        let bytes = std::fs::read(&path).unwrap();
        let text = decode_text(&bytes, &Limits::default()).unwrap().text.into_owned();
        let lf = text.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let a = flatten(
            &parse(&lf, syntax, &Limits::default()).unwrap(),
            &Limits::default(),
        )
        .unwrap();
        let b = flatten(
            &parse(&crlf, syntax, &Limits::default()).unwrap(),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(support::dump(&a), support::dump(&b), "{rel}");
        compared += 1;
    }
    assert!(compared > 10);
}
