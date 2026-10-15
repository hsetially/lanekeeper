//! Redaction (S14b, S15): the one place text from a file or a doc becomes text in a result.
//!
//! [`RedactedText`] has no public constructor. The only way to make one is through a [`Redactor`], and a [`Redactor`] runs the
//! engine's value-based secret scan on whatever it is given first. Every field of an output type that can hold file or doc text is a
//! [`RedactedText`], so a tool cannot return text that did not pass the scan: it does not compile.
//!
//! **What is replaced.** Each flagged value becomes `[REDACTED: N chars, rule]`, the engine's own marker
//! (`SecretMatch::marker`), so the MCP tools and the write previews of prompt 06 print the same text. A private key is flagged on
//! every line of its block, so it is hidden whole; a key whose header is outside the lines that are returned is hidden too, because a
//! file is scanned as a whole and the matches are then mapped onto the lines of the window ([`Redactor::window`]).
//!
//! **Fail closed.** If the scan cannot finish (a text over the engine's limit, more matches than it keeps) there is no
//! [`RedactedText`]: the caller gets [`RedactError`] and withholds the text, and never passes it through. No input asks for a
//! flagged value; revealing one is an audited action of the web UI (D79).
//!
//! **Nothing here logs text.** [`RedactedText`]'s `Debug` prints its length only (S10).

use std::fmt;

use engine::{Limits, ScanConfig, SecretMatch, secret_scan};
use serde::{Serialize, Serializer};

use super::cap::cut_chars;

/// Text that has been through the secret scan. The only way to get one is a [`Redactor`].
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedText(String);

impl RedactedText {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    /// Text the server wrote itself, with no file or doc text in it (a fixed message, a marker). Private to this module's users:
    /// callers inside the crate use it only through [`Redactor::fixed`], which says what it is for.
    fn from_server(text: &str) -> Self {
        Self(text.to_owned())
    }

    /// At most `max` characters of this text, with a marker when something was cut. Shortening scanned text shows less of it, so
    /// the result is as safe as the original.
    #[must_use]
    pub fn truncated(&self, max: usize) -> Self {
        Self(cut_with_mark(&self.0, max))
    }

    /// The first `k` lines of this text, joined with `\n`.
    #[must_use]
    pub fn first_lines(&self, k: usize) -> Self {
        if k == 0 {
            return Self(String::new());
        }
        match self.0.match_indices('\n').nth(k - 1) {
            Some((at, _)) => Self(self.0[..at].to_owned()),
            None => self.clone(),
        }
    }

    /// The first lines of this text that fit both limits: whole lines from the top, at most `max_lines` of them and at most
    /// `max_bytes` bytes in all. Returns that part and the number of lines in it. Lines end at `\n`, `\r\n` or `\r`, as
    /// [`line_count`] counts them, and the part ends where [`RedactedText::first_lines`] would cut after the last of them, so for
    /// every `k` up to the count, `window.first_lines(k)` is `self.first_lines(k)`. A first line over `max_bytes` leaves nothing:
    /// lines are not cut in two.
    ///
    /// The work is bounded by the limits and not by the text: it reads one byte past `max_bytes` at most, and copies what it
    /// keeps. A result that can show 200 lines or 16 KB of a 2 MiB text therefore fits and renders a part of at most that size.
    /// Shortening scanned text shows less of it, so the part is as safe as the whole.
    #[must_use]
    pub fn window(&self, max_lines: usize, max_bytes: usize) -> (Self, u32) {
        let bytes = self.0.as_bytes();
        let len = bytes.len();
        // A line is looked for only as far as `max_bytes` allows; one that runs past it does not fit.
        let reach = len.min(max_bytes.saturating_add(1));
        let (mut start, mut cut, mut kept) = (0_usize, 0_usize, 0_u32);
        while start < len && (kept as usize) < max_lines {
            let end = match bytes[start..reach].iter().position(|&b| b == b'\n' || b == b'\r') {
                Some(at) => start + at,
                None if reach == len => len,
                None => break,
            };
            // Where this line is cut, and where the next one starts: the `\r` of a `\r\n` stays with its line.
            let (stop, next) = match (bytes.get(end), bytes.get(end + 1)) {
                (None, _) => (len, len),
                (Some(b'\r'), Some(b'\n')) => (end + 1, end + 2),
                (Some(_), _) => (end, end + 1),
            };
            if stop > max_bytes {
                break;
            }
            kept += 1;
            cut = stop;
            start = next;
        }
        (Self(self.0[..cut].to_owned()), kept)
    }

    /// The lines of this text, split at `\n`. Parts of scanned text are scanned text.
    #[must_use]
    pub fn lines(&self) -> Vec<Self> {
        self.0.split('\n').map(|line| Self(line.to_owned())).collect()
    }
}

/// The marker that says a text was cut.
pub const CUT_MARK: &str = "[truncated]";

/// Cut `text` to `max` characters in all, the marker included.
#[must_use]
pub fn cut_with_mark(text: &str, max: usize) -> String {
    let (head, cut) = cut_chars(text, max);
    if !cut || max <= CUT_MARK.len() {
        // Nothing to cut, or no room for the marker: the first `max` characters and no more.
        return head.to_owned();
    }
    let keep = max - CUT_MARK.len();
    format!("{}{CUT_MARK}", cut_chars(head, keep).0)
}

impl fmt::Debug for RedactedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RedactedText({} bytes)", self.0.len())
    }
}

impl Serialize for RedactedText {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

/// The scan could not finish. Carries nothing of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the secret scan could not complete")]
pub struct RedactError;

/// Text after the scan, and how many values were replaced in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted {
    pub text: RedactedText,
    pub redacted: u32,
}

/// The engine's secret scan and the replacement of what it finds.
#[derive(Debug, Clone)]
pub struct Redactor {
    config: ScanConfig,
    limits: Limits,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

/// Where each line of `text` starts and ends (the end is before the terminator), numbered as the scan numbers them: a line ends at
/// `\n`, `\r\n` or `\r`, and a text that ends with a terminator has an empty last line.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0_usize;
    loop {
        let stop = bytes[start..].iter().position(|&b| b == b'\n' || b == b'\r');
        match stop {
            None => {
                spans.push((start, bytes.len()));
                return spans;
            }
            Some(p) => {
                let at = start + p;
                spans.push((start, at));
                start = if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                    at + 2
                } else {
                    at + 1
                };
            }
        }
    }
}

/// The number of lines of `text` as a reader counts them: `a\nb\n` has two, an empty text has none, `a` has one.
#[must_use]
pub fn line_count(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let spans = line_spans(text);
    let last_is_phantom = spans.last().is_some_and(|&(s, e)| s == e && s == text.len());
    u32::try_from(spans.len() - usize::from(last_is_phantom)).unwrap_or(u32::MAX)
}

/// `line` with the characters of each match replaced by its marker. `matches` are in column order and do not overlap.
fn replace_in_line(line: &str, matches: &[&SecretMatch]) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.char_indices();
    let mut taken = 0_u32;
    for m in matches {
        let from = m.column.saturating_sub(1);
        // Copy up to the match.
        while taken < from {
            match chars.next() {
                Some((_, c)) => {
                    out.push(c);
                    taken += 1;
                }
                None => break,
            }
        }
        // Skip its characters.
        let mut skipped = 0_u32;
        while skipped < m.length {
            if chars.next().is_none() {
                break;
            }
            skipped += 1;
            taken += 1;
        }
        out.push_str(&m.marker());
    }
    for (_, c) in chars {
        out.push(c);
    }
    out
}

impl Redactor {
    /// The default scan configuration: conservative, and the engine's (a false positive costs a blocked save, A18).
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ScanConfig::default(),
            limits: Limits::DEFAULT,
        }
    }

    /// Text the server wrote itself (a fixed message, never file or doc text). The type is the same, so the output types stay
    /// uniform; the name says that it did not come from a file.
    #[must_use]
    pub fn fixed(&self, text: &'static str) -> RedactedText {
        RedactedText::from_server(text)
    }

    /// Every secret in `text`.
    ///
    /// # Errors
    ///
    /// [`RedactError`] when the scan cannot finish.
    pub fn scan(&self, text: &str) -> Result<Vec<SecretMatch>, RedactError> {
        secret_scan(text, &self.config, &self.limits).map_err(|_| RedactError)
    }

    /// `raw` with every secret replaced.
    ///
    /// # Errors
    ///
    /// [`RedactError`] when the scan cannot finish.
    pub fn text(&self, raw: &str) -> Result<Redacted, RedactError> {
        let matches = self.scan(raw)?;
        if matches.is_empty() {
            return Ok(Redacted {
                text: RedactedText(raw.to_owned()),
                redacted: 0,
            });
        }
        let spans = line_spans(raw);
        let mut out = String::with_capacity(raw.len());
        for (index, &(start, end)) in spans.iter().enumerate() {
            let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let here: Vec<&SecretMatch> = matches.iter().filter(|m| m.line == number).collect();
            if index > 0 {
                // Keep the terminator the text had between the lines.
                out.push_str(raw.get(spans[index - 1].1..start).unwrap_or("\n"));
            }
            let line = raw.get(start..end).unwrap_or("");
            if here.is_empty() {
                out.push_str(line);
            } else {
                out.push_str(&replace_in_line(line, &here));
            }
        }
        Ok(Redacted {
            text: RedactedText(out),
            redacted: u32::try_from(matches.len()).unwrap_or(u32::MAX),
        })
    }

    /// Lines `first..=last` (1-based) of `text`, joined with `\n`, with the matches of a scan of the **whole** text applied. A
    /// private key whose header is above the window is still hidden, because the scan saw the header.
    #[must_use]
    pub fn window(&self, text: &str, matches: &[SecretMatch], first: u32, last: u32) -> Redacted {
        let spans = line_spans(text);
        let mut out = String::new();
        let mut count = 0_u32;
        for number in first..=last {
            let Some(&(start, end)) = spans.get(usize::try_from(number - 1).unwrap_or(usize::MAX)) else {
                break;
            };
            let here: Vec<&SecretMatch> = matches.iter().filter(|m| m.line == number).collect();
            if number > first {
                out.push('\n');
            }
            let line = text.get(start..end).unwrap_or("");
            if here.is_empty() {
                out.push_str(line);
            } else {
                count += u32::try_from(here.len()).unwrap_or(u32::MAX);
                out.push_str(&replace_in_line(line, &here));
            }
        }
        Redacted {
            text: RedactedText(out),
            redacted: count,
        }
    }
}
