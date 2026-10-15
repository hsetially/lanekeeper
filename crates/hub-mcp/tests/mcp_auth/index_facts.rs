//! The settings index the file cache keeps (P9): its outline and its lookup equal what a slow, obvious reading of the engine's index
//! gives, on files whose top-level keys are the hard cases (a key that is the start of another, a key with a dot, a document after
//! the first, a sequence at the top, a key that starts with `#`).

use std::collections::HashMap;

use domain::{LineRange, SettingLocation, SettingPath};
use engine::{Limits, Syntax, decode_setting_path, encode_setting_path, index_settings};
use hub_mcp::tools::cache::{FileCache, MAX_OUTLINE};

/// The outline as the first version of the cache computed it: every row taken apart.
fn reference(text: &str) -> (Vec<SettingLocation>, Vec<(SettingPath, LineRange)>) {
    let index = index_settings(text.as_bytes(), Syntax::Yaml, &Limits::DEFAULT).unwrap();
    let mut order: Vec<String> = Vec::new();
    let mut spans: HashMap<String, (u32, u32)> = HashMap::new();
    let mut rows = Vec::new();
    for row in &index.rows {
        rows.push((row.path.clone(), row.lines));
        let top = decode_setting_path(&row.path)
            .and_then(|d| {
                d.segments
                    .first()
                    .cloned()
                    .map(|first| encode_setting_path(d.document, &[first]))
            })
            .unwrap_or_else(|| row.path.as_str().to_owned());
        let entry = spans.entry(top.clone()).or_insert_with(|| {
            order.push(top);
            (row.lines.start(), row.lines.end())
        });
        entry.0 = entry.0.min(row.lines.start());
        entry.1 = entry.1.max(row.lines.end());
    }
    let outline = order
        .into_iter()
        .take(MAX_OUTLINE)
        .map(|top| {
            let (start, end) = spans[&top];
            SettingLocation {
                path: SettingPath::parse(&top).unwrap(),
                lines: LineRange::new(start, end).unwrap(),
            }
        })
        .collect();
    (outline, rows)
}

fn check(text: &str) {
    let (want_outline, want_rows) = reference(text);
    let cache = FileCache::new(0);
    let key = ports::content_hash(text.as_bytes());
    let (facts, hit) = cache.index(&key, Syntax::Yaml, text.as_bytes()).unwrap();
    assert!(!hit);
    assert_eq!(facts.outline(), want_outline.as_slice(), "outline of {text:?}");
    for (path, lines) in &want_rows {
        assert_eq!(facts.locate(path), Some(*lines), "{path} in {text:?}");
    }
    assert_eq!(
        facts.locate(&SettingPath::parse("no.such.setting").unwrap()),
        None
    );
}

#[test]
fn outline_and_lookup_equal_the_reference() {
    for text in [
        "a:\n  b: 1\n  c: 2\nab:\n  d: 3\n",
        // `x.y` is one key; `x` is another that is the start of its text.
        "\"x.y\":\n  z: 1\nx:\n  y: 2\n  w: 3\n",
        "list:\n  - 1\n  - 2\nlistb: 3\nlist2:\n  - k: v\n",
        "a: 1\n---\na: 2\nb:\n  c: 3\n",
        "\"#c\": 1\nd:\n  e: 2\n",
        "one: 1\n",
        "a:\n  b:\n    c: 1\n  d: 2\na_b: 5\na2: 6\n",
        "k:\n  - a: 1\n    b: 2\n  - a: 3\nk2: 4\n",
    ] {
        check(text);
    }
}

#[test]
fn the_outline_is_at_most_the_contract_maximum() {
    let text: String = (0..MAX_OUTLINE + 50)
        .map(|n| format!("key{n}:\n  v: {n}\n"))
        .collect();
    let cache = FileCache::new(0);
    let (facts, _) = cache
        .index(
            &ports::content_hash(text.as_bytes()),
            Syntax::Yaml,
            text.as_bytes(),
        )
        .unwrap();
    assert_eq!(facts.outline().len(), MAX_OUTLINE);
    assert_eq!(facts.outline()[0].path.as_str(), "key0");
    assert_eq!(facts.outline()[0].lines, LineRange::new(2, 2).unwrap());
    // A lookup still finds a setting past the outline.
    let late = SettingPath::parse(&format!("key{}.v", MAX_OUTLINE + 40)).unwrap();
    assert!(facts.locate(&late).is_some());
}

#[test]
fn a_cached_index_is_returned_on_the_second_ask() {
    let text = "a:\n  b: 1\n";
    let cache = FileCache::new(1 << 20);
    let key = ports::content_hash(text.as_bytes());
    assert!(!cache.index(&key, Syntax::Yaml, text.as_bytes()).unwrap().1);
    assert!(cache.index(&key, Syntax::Yaml, text.as_bytes()).unwrap().1);
}
