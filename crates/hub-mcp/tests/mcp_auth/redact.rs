//! S14b: what the redactor does to text, on its own. The tools' use of it is proved by their tests and by `planted_key` (T6).

use engine::{Limits, ScanConfig, secret_scan};
use hub_mcp::output::{Redactor, line_count};
use proptest::prelude::*;

use crate::fix::fake_github_token;

fn private_key_block() -> Vec<String> {
    // Built at run time from parts: no literal in the repository is a key (A18).
    let begin = format!("-----BEGIN {} KEY-----", "RSA PRIVATE");
    let end = format!("-----END {} KEY-----", "RSA PRIVATE");
    let mut lines = vec![begin];
    for n in 0..6 {
        lines.push(format!(
            "{}{n}{}",
            "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC", "x7Yz"
        ));
    }
    lines.push(end);
    lines
}

#[test]
fn redaction_marker_format_matches_engine() {
    let secret = fake_github_token();
    let text = format!("token: {secret}\nname: plain\n");
    let redactor = Redactor::new();
    let matches = secret_scan(&text, &ScanConfig::default(), &Limits::DEFAULT).unwrap();
    assert_eq!(matches.len(), 1);
    let redacted = redactor.text(&text).unwrap();
    // The text a person reads is the engine's own marker, the same one the write previews print.
    assert_eq!(
        redacted.text.as_str(),
        format!("token: {}\nname: plain\n", matches[0].marker())
    );
    assert_eq!(redacted.redacted, 1);
    assert!(
        redacted
            .text
            .as_str()
            .contains("[REDACTED: 40 chars, github_token]")
    );
    assert!(!redacted.text.as_str().contains(&secret));
}

#[test]
fn private_key_block_split_by_a_line_window_is_still_hidden() {
    let mut lines = vec!["a: 1".to_owned(), "b: 2".to_owned()];
    let key = private_key_block();
    lines.extend(key.clone());
    lines.push("c: 3".to_owned());
    let text = format!("{}\n", lines.join("\n"));
    let redactor = Redactor::new();
    let matches = redactor.scan(&text).unwrap();
    // A window that starts in the middle of the block, so that its header is above it and its footer below.
    let window = redactor.window(&text, &matches, 5, 6);
    for body_line in &key[2..4] {
        assert!(
            !window.text.as_str().contains(body_line.as_str()),
            "a line of the key is in the window"
        );
        assert!(!window.text.as_str().contains("MIIEvQIBADANBg"));
    }
    assert!(
        window.text.as_str().contains("private_key"),
        "{}",
        window.text.as_str()
    );
    assert_eq!(window.redacted, 2);
    // Scanning only the window would not have seen the header: that is why the scan is of the whole file.
    let only_window = key[2..4].join("\n");
    let alone = redactor.text(&only_window).unwrap();
    assert!(alone.redacted <= 2);
    // The whole text through the window path.
    let whole = redactor.window(&text, &matches, 1, line_count(&text));
    assert!(!whole.text.as_str().contains("MIIEvQIBADANBg"));
    assert!(whole.text.as_str().starts_with("a: 1\nb: 2\n"));
    assert!(whole.text.as_str().ends_with("c: 3"));
}

#[test]
fn scan_error_fails_closed() {
    let redactor = Redactor::new();
    let too_big = "a".repeat(2 * 1024 * 1024 + 1);
    assert!(
        redactor.text(&too_big).is_err(),
        "a text the scan cannot take is not passed through"
    );
    assert!(redactor.scan(&too_big).is_err());
    // At the limit it is scanned.
    assert!(redactor.text(&"a".repeat(2 * 1024 * 1024)).is_ok());
}

#[test]
fn text_without_a_secret_is_unchanged_and_counts_zero() {
    let redactor = Redactor::new();
    for text in [
        "",
        "a: 1",
        "a: 1\n",
        "\n\n",
        "line one\r\nline two\r\n",
        "tab\tseparated\tvalues\n",
        "unicode \u{20ac}\u{1f600}",
    ] {
        let redacted = redactor.text(text).unwrap();
        assert_eq!(redacted.text.as_str(), text);
        assert_eq!(redacted.redacted, 0);
    }
}

#[test]
fn the_secret_goes_whatever_the_line_ending() {
    let secret = fake_github_token();
    let redactor = Redactor::new();
    for ending in ["\n", "\r\n", "\r"] {
        let text = format!("a: 1{ending}token: {secret}{ending}b: 2{ending}");
        let redacted = redactor.text(&text).unwrap();
        assert!(!redacted.text.as_str().contains(&secret), "{ending:?}");
        assert!(
            redacted
                .text
                .as_str()
                .starts_with(&format!("a: 1{ending}token: [REDACTED")),
            "{ending:?}"
        );
        assert!(
            redacted.text.as_str().ends_with(&format!("b: 2{ending}")),
            "{ending:?}"
        );
    }
}

#[test]
fn line_count_is_how_a_reader_counts() {
    for (text, lines) in [
        ("", 0),
        ("a", 1),
        ("a\n", 1),
        ("a\nb", 2),
        ("a\nb\n", 2),
        ("\n", 1),
        ("\n\n", 2),
        ("a\r\nb\r\n", 2),
        ("a\rb", 2),
    ] {
        assert_eq!(line_count(text), lines, "{text:?}");
    }
}

#[test]
fn a_cut_text_cannot_show_a_secret_the_cut_would_have_split() {
    // The value is redacted first and cut after, so a cut in the middle of a secret leaves a marker, never half of the secret.
    let secret = fake_github_token();
    let redactor = Redactor::new();
    let text = format!("{}{secret}", "x".repeat(10));
    let redacted = redactor.text(&text).unwrap();
    for max in [5, 10, 15, 20, 30] {
        let cut = redacted.text.truncated(max);
        assert!(!cut.as_str().contains("ghp_A1b2"), "{max}: {}", cut.as_str());
        assert!(cut.as_str().chars().count() <= max);
    }
}

proptest! {
    #[test]
    fn redaction_is_idempotent_and_leaves_nothing_for_the_scan(
        lines in proptest::collection::vec("[ -~]{0,60}", 0..12),
        with_secret in proptest::bool::ANY,
        at in 0..12_usize,
    ) {
        let mut lines = lines;
        if with_secret {
            let at = at.min(lines.len());
            lines.insert(at, format!("token: {}", fake_github_token()));
        }
        let text = lines.join("\n");
        let redactor = Redactor::new();
        let once = redactor.text(&text).unwrap();
        let twice = redactor.text(once.text.as_str()).unwrap();
        prop_assert_eq!(once.text.as_str(), twice.text.as_str());
        prop_assert_eq!(twice.redacted, 0);
        let left = secret_scan(once.text.as_str(), &ScanConfig::default(), &Limits::DEFAULT).unwrap();
        prop_assert!(left.is_empty(), "{left:?}");
        prop_assert!(!once.text.as_str().contains("ghp_A1b2C3d4"));
    }
}
