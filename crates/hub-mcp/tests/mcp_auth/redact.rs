//! S14b: what the redactor does to text, on its own. The tools' use of it is proved by their tests and by `planted_key` (T6).

use engine::{Limits, ScanConfig, secret_scan};
use hub_mcp::output::envelope::{Parts, Shape, fit_lines};
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

// ---------------------------------------------------------------------------------------------------------------------
// `RedactedText::window`: the part of a text that a result can show (round 2, review item 6)
// ---------------------------------------------------------------------------------------------------------------------

fn redacted(text: &str) -> hub_mcp::output::RedactedText {
    Redactor::new().text(text).unwrap().text
}

#[test]
fn a_window_is_whole_lines_from_the_top_within_both_limits() {
    for (text, max_lines, max_bytes, expected, kept) in [
        ("", 5, 100, "", 0),
        ("a", 5, 100, "a", 1),
        ("a\n", 5, 100, "a", 1),
        ("a\nb\nc\n", 2, 100, "a\nb", 2),
        ("a\nb\nc", 5, 100, "a\nb\nc", 3),
        // The byte limit counts what is kept: `a\nb` is 3 bytes, `a\nb\nc` is 5.
        ("a\nb\nc\n", 5, 4, "a\nb", 2),
        ("a\nb\nc\n", 5, 3, "a\nb", 2),
        ("a\nb\nc\n", 5, 2, "a", 1),
        ("a\nb\nc\n", 5, 0, "", 0),
        // A first line that does not fit leaves nothing: lines are not cut in two.
        ("abcdef\ng\n", 5, 5, "", 0),
        // CRLF: the `\r` stays with its line, as `first_lines` leaves it.
        ("a\r\nb\r\nc\r\n", 2, 100, "a\r\nb\r", 2),
        // A lone `\r` ends a line, as the count of lines has it.
        ("a\rb\rc", 2, 100, "a\rb", 2),
        // Empty lines count.
        ("a\n\nb\n", 2, 100, "a\n", 2),
        ("a\n\nb\n", 3, 100, "a\n\nb", 3),
    ] {
        let (window, lines) = redacted(text).window(max_lines, max_bytes);
        assert_eq!(
            (window.as_str(), lines),
            (expected, kept),
            "{text:?} {max_lines} {max_bytes}"
        );
    }
}

#[test]
fn a_window_of_a_two_mib_text_is_as_small_as_the_limits_say() {
    // What the tool hands to its fitting loop: never more than 200 lines and 16 KB, however large the file.
    const TWO_MIB: usize = 2 * 1024 * 1024;
    let many_short: String = (0..)
        .map(|n| format!("k{n}: v\n"))
        .scan(0, |len, line| {
            *len += line.len();
            (*len <= TWO_MIB).then_some(line)
        })
        .collect();
    let few_long: String = (0..200)
        .map(|n| format!("k{n}: {}\n", "x".repeat(10_000)))
        .collect();
    let one_line = "y".repeat(TWO_MIB);
    for (name, text) in [
        ("many short", many_short),
        ("few long", few_long),
        ("one line", one_line),
    ] {
        assert!(text.len() > 1_900_000, "{name}: {}", text.len());
        let (window, lines) = redacted(&text).window(200, 16 * 1024);
        assert!(lines <= 200, "{name}: {lines}");
        assert!(window.len() <= 16 * 1024, "{name}: {} bytes", window.len());
        // The work of fitting it, counted and not timed: how many candidates the search renders, and the most text any one of them
        // cuts and serialises. Both are bounded by the limits and have nothing to do with the 2 MiB.
        let shape = Shape {
            mode: hub_mcp::config::ContentMode::default(),
            cap: 16 * 1024,
            text_bearing: true,
        };
        let (mut steps, mut most) = (0_u32, 0_usize);
        let _ = fit_lines(&shape, lines, |k| {
            let shown = window.first_lines(k as usize);
            steps += 1;
            most = most.max(shown.len());
            Parts {
                summary: format!("{k} lines"),
                data: serde_json::json!({ "text": shown.as_str() }),
                truncated: false,
                next: None,
                web_link: None,
            }
        });
        assert!(steps <= 10, "{name}: {steps} candidates for {lines} lines");
        assert!(most <= 16 * 1024, "{name}: a candidate of {most} bytes");
    }
    let (window, lines) = redacted(&"y".repeat(TWO_MIB)).window(200, 16 * 1024);
    assert_eq!(
        (window.len(), lines),
        (0, 0),
        "a line over the limit is not cut in two"
    );
}

proptest! {
    /// Against `first_lines`, which the tool used before: for texts with `\n` and `\r\n` line ends, the first `k` lines of the window
    /// are the first `k` lines of the text, for every `k` the window has, and the window holds as many lines as the limits allow.
    #[test]
    fn the_first_lines_of_a_window_are_the_first_lines_of_the_text(
        lines in proptest::collection::vec("[ -~]{0,12}", 0..30),
        crlf in proptest::bool::ANY,
        trailing in proptest::bool::ANY,
        max_lines in 0..40_usize,
        max_bytes in 0..200_usize,
    ) {
        let end = if crlf { "\r\n" } else { "\n" };
        let mut text = lines.join(end);
        if trailing && !text.is_empty() {
            text.push_str(end);
        }
        let text = redacted(&text);
        let (window, kept) = text.window(max_lines, max_bytes);
        let kept_usize = kept as usize;
        prop_assert!(kept_usize <= max_lines);
        prop_assert!(kept <= line_count(text.as_str()));
        prop_assert!(window.len() <= max_bytes);
        for k in 0..=kept_usize {
            let (mine, theirs) = (window.first_lines(k), text.first_lines(k));
            prop_assert_eq!(mine.as_str(), theirs.as_str(), "k = {}", k);
        }
        // It is the most lines that the limits allow: one more would pass a limit.
        if kept < line_count(text.as_str()) && kept_usize < max_lines {
            let (next, more) = text.window(kept_usize + 1, usize::MAX);
            prop_assert_eq!(more, kept + 1);
            prop_assert!(next.len() > max_bytes, "{} lines of {} bytes fit {}", more, next.len(), max_bytes);
        }
    }
}
