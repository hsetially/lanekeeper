//! The properties the fuzz target `mcp_redactor` asserts (S14b, S22): arbitrary text through the redactor, the window, and the final
//! sweep of a result.
//!
//! Included, with `#[path]`, by `fuzz_targets/mcp_redactor.rs` and by `tests/mcp_auth/corpus_replay.rs`. Both depend on the `hub-mcp`
//! crate, so this file names it by its path, `hub_mcp::`.
//!
//! # The input
//!
//! Three control bytes and then the text. The first is a set of kits of secrets, one bit each, which the check builds at run time from
//! parts and splices into the text on lines of their own (a corpus file must not hold a key, A18, and the fuzzer cannot be expected
//! to find a private key header by chance); the second and third are the first and last line of a window; the rest, read as UTF-8 with
//! the bad bytes replaced, is the text. A file shorter than three bytes is padded with zeros.
//!
//! # What must hold
//!
//! | Check | Why |
//! |---|---|
//! | `Redactor::text` never panics, gives the same answer twice, and keeps the text as it was when the scan finds nothing | the scan sits in front of every tool |
//! | what it returns has nothing left for the scan to find, and redacting it again changes nothing | idempotent, and complete |
//! | it never changes the number of lines, and the output is at most 48 bytes per replaced value longer than the input | the line-oriented tools (`diff_hunk`) cut by lines; bounded growth |
//! | `Redactor::window` of a window never panics, shows exactly the lines asked for, and has nothing left for the scan to find | a private key cut by a window is still hidden |
//! | a kit that was spliced in is not in the output (unless the text has a `-----` block marker, inside which the scan skips public blocks by design) | every shape of secret the scan knows is taken out |
//! | the sweep never panics, changes only strings (the cursor and the link not at all), and what it returns has nothing left for the scan to find, in the data and in `content` | the last look at a result |
//! | the sweep of a result with nothing in it to replace is that result, byte for byte, with `late == 0` | it adds nothing and costs nothing in the normal case |
//! | the sweep is idempotent, and its size is the size of what it returns | a second sweep finds nothing; the cap is counted on the right bytes |
//! | the sweep and the redactor agree: the text the sweep returns for a string is what the redactor returns for it | one scan, one answer |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use hub_mcp::config::ContentMode;
use hub_mcp::output::envelope::{Parts, Rendered, Shape, measure, render};
use hub_mcp::output::sweep::sweep;
use hub_mcp::output::{Redactor, line_count};
use serde_json::{Value, json};

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// How many kits there are: the bits of the first control byte that mean something.
pub const KITS: u8 = 6;

/// The longest a marker is: `[REDACTED: N chars, rule]` with the longest rule name and a count of ten digits.
const MARKER_MAX: usize = 48;

/// A secret of kind `n`, built at run time from parts so that no literal in the repository is one (A18).
pub fn kit(n: u8) -> String {
    match n {
        0 => format!("{}{}", "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
        1 => format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE"),
        2 => format!("{}{}", "xoxb-", "123456789012-abcdefghijklmnop"),
        3 => format!(
            "{}.{}.{}",
            "eyJhbGciOiJIUzI1NiJ9",
            "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
            "dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        ),
        4 => {
            let begin = format!("-----BEGIN {} KEY-----", "RSA PRIVATE");
            let end = format!("-----END {} KEY-----", "RSA PRIVATE");
            let mut lines = vec![begin];
            for i in 0..4 {
                lines.push(format!(
                    "{}{i}{}",
                    "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC", "x7Yz"
                ));
            }
            lines.push(end);
            lines.join("\n")
        }
        _ => "Zq8Rm3Tn7Wx2Lk9Vb4Hs6Jd1Fg5Pa0Ce+Yu/Io2Ab".to_owned(),
    }
}

/// The part of a kit that a reader could use: its text without the prefix that names its kind.
fn body(n: u8) -> String {
    let whole = kit(n);
    match n {
        0 => whole["ghp_".len()..].to_owned(),
        1 => whole["AKIA".len()..].to_owned(),
        2 => whole["xoxb-".len()..].to_owned(),
        4 => whole.lines().nth(2).unwrap_or_default().to_owned(),
        _ => whole,
    }
}

/// The input, taken apart.
struct Input {
    mask: u8,
    first: u32,
    last: u32,
    text: String,
    /// The text the kits were spliced into had no block marker, so every kit must be gone from the output.
    plain: bool,
}

fn input(data: &[u8]) -> Input {
    let mut head = [0_u8; 3];
    for (slot, byte) in head.iter_mut().zip(data) {
        *slot = *byte;
    }
    let rest = String::from_utf8_lossy(data.get(3..).unwrap_or_default()).into_owned();
    let mut text = rest.clone();
    let mut inserted = 0_usize;
    for n in 0..KITS {
        if head[0] & (1 << n) == 0 {
            continue;
        }
        // On lines of its own, at a place that depends on the kit and the text: the start of a line, so that a kit is never split by
        // another one (a key cut in two is not a key).
        let starts: Vec<usize> = std::iter::once(0)
            .chain(
                text.char_indices()
                    .filter(|&(_, c)| c == '\n' || c == '\r')
                    .map(|(i, c)| i + c.len_utf8()),
            )
            .collect();
        let at = starts[(usize::from(n) * 7919 + text.len() / 2) % starts.len()];
        text.insert_str(at, &format!("\n{}\n", kit(n)));
        inserted += 1;
    }
    let first = u32::from(head[1] % 16) + 1;
    let last = first + u32::from(head[2] % 32);
    Input {
        mask: head[0],
        first,
        last,
        plain: inserted > 0 && !rest.contains("-----"),
        text,
    }
}

/// The text of an input after the kits are spliced in (for the tests of the seeds).
pub fn expanded(data: &[u8]) -> String {
    input(data).text
}

fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        Value::Object(map) => map.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

/// The two forms of a value with the same shape: the same keys, the same array lengths, the same kinds of leaf.
fn same_shape(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_shape(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same_shape(v, w))
        }
        (Value::String(_), Value::String(_)) | (Value::Number(_), Value::Number(_)) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

/// `mcp_redactor`.
pub fn mcp_redactor(data: &[u8]) -> Result<(), String> {
    let Input {
        mask,
        first,
        last,
        text,
        plain,
    } = input(data);
    let redactor = Redactor::new();

    // The redactor.
    let Ok(redacted) = redactor.text(&text) else {
        // The text is small: a scan that cannot finish on it is a defect.
        return Err("the scan could not finish on a small text".to_owned());
    };
    let again = redactor
        .text(&text)
        .map_err(|_| "the second scan failed".to_owned())?;
    if again.text != redacted.text || again.redacted != redacted.redacted {
        return Err("redacting the same text twice gave different answers".to_owned());
    }
    let out = redacted.text.as_str();
    let found = redactor.scan(&text).map_err(|_| "the scan failed".to_owned())?;
    if found.is_empty() && out != text {
        return Err("a text with nothing in it for the scan was changed".to_owned());
    }
    if usize::try_from(redacted.redacted).ok() != Some(found.len()) {
        return Err("the count of replaced values is not the count of matches".to_owned());
    }
    if !redactor
        .scan(out)
        .map_err(|_| "the scan failed on the output".to_owned())?
        .is_empty()
    {
        return Err("the scan finds a secret in the redacted text".to_owned());
    }
    let second = redactor
        .text(out)
        .map_err(|_| "the scan failed on the output".to_owned())?;
    if second.redacted != 0 || second.text.as_str() != out {
        return Err("redacting the redacted text changed it".to_owned());
    }
    if line_count(out) != line_count(&text) {
        return Err(format!(
            "the number of lines changed from {} to {}",
            line_count(&text),
            line_count(out)
        ));
    }
    let allowed = text.len() + MARKER_MAX * found.len();
    if out.len() > allowed {
        return Err(format!(
            "the output is {} bytes for {} in and {} replaced",
            out.len(),
            text.len(),
            found.len()
        ));
    }
    if plain {
        for n in 0..KITS {
            if mask & (1 << n) != 0 && out.contains(&body(n)) {
                return Err(format!("the body of kit {n} is in the output"));
            }
        }
    }

    // The window: the lines first..=last, with the matches of the scan of the whole text. The callers of `window` hold it to the
    // lines the text has (`1 <= first <= last <= line_count`), so the check does too.
    let total = line_count(&text);
    if total > 0 {
        let (first, last) = ((first - 1) % total + 1, last.clamp(1, total));
        let last = last.max(first);
        let window = redactor.window(&text, &found, first, last);
        let shown = window.text.as_str().split('\n').count();
        if usize::try_from(last - first + 1).ok() != Some(shown) {
            return Err(format!(
                "the window of lines {first} to {last} shows {shown} lines"
            ));
        }
        if !redactor
            .scan(window.text.as_str())
            .map_err(|_| "the scan failed on the window".to_owned())?
            .is_empty()
            && !text.contains("-----")
        {
            return Err("the scan finds a secret in the window".to_owned());
        }
    }

    // The sweep, over a result that holds the text in several places.
    let mode = if mask & 0x80 == 0 {
        ContentMode::Summary
    } else {
        ContentMode::DataBlock
    };
    let shape = Shape {
        mode,
        cap: usize::MAX,
        text_bearing: true,
    };
    let depth = usize::from(mask >> 1 & 0x1f);
    let mut nested = json!({"deep": text});
    for _ in 0..depth {
        nested = json!([nested, {"again": "plain"}]);
    }
    let before = render(
        &shape,
        Parts {
            summary: "A result.".to_owned(),
            data: json!({
                "hits": [{"path": "a/b.yml", "line": 3, "text": text}],
                "nested": nested,
                "count": 3,
                "flag": true,
            }),
            truncated: false,
            next: Some(
                "m1.eyJ0IjoiYSIsImkiOiJRMjNwV3RGOHZMa1R6eFlSN2JOY1U1ZFhhSGpNd1BzIiwicyI6M30".to_owned(),
            ),
            web_link: Some("https://hub.example.test/x?q=Ab3dE6gH9jK2mN5pQ8sT1vW4yZ7bC0eF".to_owned()),
        },
    );
    let swept = sweep(&redactor, mode, before.clone())
        .map_err(|_| "the sweep failed on a small result".to_owned())?;
    check_swept(&redactor, &before, &swept.rendered, swept.late)?;
    // The sweep and the redactor agree on the text.
    let sweep_text = swept.rendered.structured["data"]["hits"][0]["text"]
        .as_str()
        .ok_or("the text of the hit is not a string after the sweep")?;
    if sweep_text != out {
        return Err("the sweep and the redactor disagree on the same text".to_owned());
    }
    // The cursor and the link are the server's: not swept.
    if swept.rendered.structured["next"] != before.structured["next"]
        || swept.rendered.structured["web_link"] != before.structured["web_link"]
    {
        return Err("the sweep changed the cursor or the link".to_owned());
    }
    // A result that is already swept is a fixed point.
    let twice =
        sweep(&redactor, mode, swept.rendered.clone()).map_err(|_| "the second sweep failed".to_owned())?;
    if twice.late != 0
        || twice.rendered.structured != swept.rendered.structured
        || twice.rendered.content != swept.rendered.content
    {
        return Err("sweeping a swept result changed it".to_owned());
    }
    Ok(())
}

fn check_swept(redactor: &Redactor, before: &Rendered, after: &Rendered, late: u32) -> Result<(), String> {
    if !same_shape(&before.structured, &after.structured) {
        return Err("the sweep changed the shape of the result".to_owned());
    }
    if after.bytes != measure(&after.structured, &after.content) {
        return Err("the size of the swept result is not its size".to_owned());
    }
    let mut all = Vec::new();
    // The cursor and the link are not swept and look random by design; everything else must be clean.
    strings(&after.structured["data"], &mut all);
    all.push(
        after.structured["summary"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    );
    all.push(after.content.clone());
    for text in &all {
        if !redactor
            .scan(text)
            .map_err(|_| "the scan failed on the swept result".to_owned())?
            .is_empty()
        {
            return Err("the scan finds a secret in the swept result".to_owned());
        }
    }
    if late == 0 {
        if after.structured != before.structured
            || after.content != before.content
            || after.bytes != before.bytes
        {
            return Err("the sweep changed a result and said that it replaced nothing".to_owned());
        }
    } else if after.structured == before.structured && after.content == before.content {
        return Err("the sweep said that it replaced something and changed nothing".to_owned());
    }
    Ok(())
}
