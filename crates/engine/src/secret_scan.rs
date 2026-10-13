//! The value-based secret scan (T7, S13, S14b, S21).
//!
//! `secret_scan` reads a text and says where the secrets in it are. It judges **values**, never names: a key called `password`
//! with nothing after it is nothing, and a long random string is a secret whatever it is called. What it looks for:
//!
//! - **private keys:** a `-----BEGIN ... PRIVATE KEY-----` header and every line of the block after it, so that hiding the
//!   matches hides the whole key. A public key or certificate block is not a secret and is skipped whole (its body would
//!   otherwise look random);
//! - **known token prefixes:** GitHub (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`), AWS access key ids (`AKIA`,
//!   `ASIA`), Slack (`xox[abprs]-`), GitLab (`glpat-`), Google API keys (`AIza`) and Stripe live keys (`sk_live_`, `rk_live_`),
//!   each with the length that makes it a token and not a word;
//! - **JWT-shaped strings:** three base64url parts, the first two starting `eyJ` (a base64 JSON object);
//! - **random-looking strings:** a token of letters, digits and `+/_-` (and base64 padding) at least
//!   [`ScanConfig::min_random_len`] long, with at least [`ScanConfig::min_classes`] of the kinds lower case, upper case,
//!   digit and symbol, whose Shannon entropy is at least [`ScanConfig::min_entropy_millibits`] bits per character.
//!
//! It never flags a **placeholder** (`${NAME}`), a token in key position (a YAML key, or the name before `=` in a
//! `.properties` file), a hexadecimal digest or a UUID (unless [`ScanConfig::hex_is_random`]), a base64 JSON object that is
//! not a whole JWT, and a token that is mostly words (a path, a class name, a camel-case identifier). These exemptions are the
//! false positives that a blocked save would cost (A18), and every number is configuration to tune with the real files (Q20).
//!
//! # What a match holds
//!
//! A [`SecretMatch`] is a line, a column, a length and a rule. It holds **no value**, in memory or in any output, so a caller
//! that redacts or reports has the place and the size and nothing to leak. [`SecretMatch::marker`] is the one text for a
//! redaction: `[REDACTED: N chars, rule]`.
//!
//! Lines are 1-based and end at `\n`, `\r\n` or `\r`. Columns and lengths count characters (Unicode scalar values), and a
//! column is 1-based. Matches come in order and do not overlap.
//!
//! # Bounds and determinism
//!
//! The text is limited to `Limits::input_bytes`, the scan is one pass, and the number of matches is limited by
//! [`ScanConfig::max_matches`]: over it the scan returns an error, never a short list, because a caller that hides what it was
//! given would leave the rest in the clear. The entropy is computed in integers (a fixed-point logarithm), so the answer does
//! not depend on the platform's floating point.

use std::fmt;

use crate::error::ScanError;
use crate::limits::Limits;

/// The kind of secret a match is. The names are in answers that people and other components read, and do not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretRule {
    PrivateKey,
    GithubToken,
    AwsAccessKey,
    SlackToken,
    GitlabToken,
    GoogleApiKey,
    StripeKey,
    Jwt,
    HighEntropy,
}

impl SecretRule {
    /// The rule's name, as it is written in a marker.
    pub const fn name(self) -> &'static str {
        match self {
            Self::PrivateKey => "private_key",
            Self::GithubToken => "github_token",
            Self::AwsAccessKey => "aws_access_key",
            Self::SlackToken => "slack_token",
            Self::GitlabToken => "gitlab_token",
            Self::GoogleApiKey => "google_api_key",
            Self::StripeKey => "stripe_key",
            Self::Jwt => "jwt",
            Self::HighEntropy => "high_entropy",
        }
    }
}

impl fmt::Display for SecretRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Where a secret is. It holds the place and the size, never the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretMatch {
    /// 1-based.
    pub line: u32,
    /// 1-based, in characters.
    pub column: u32,
    /// In characters.
    pub length: u32,
    pub rule: SecretRule,
}

impl SecretMatch {
    /// The text that stands for the secret where it was: `[REDACTED: N chars, rule]` (S14b).
    pub fn marker(&self) -> String {
        format!("[REDACTED: {} chars, {}]", self.length, self.rule)
    }
}

/// The thresholds of the scan. The defaults are conservative: a false positive blocks a save (S13, A18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanConfig {
    /// The shortest token judged by entropy, in characters.
    pub min_random_len: usize,
    /// The least Shannon entropy of such a token, in thousandths of a bit per character. Random base64 is about 4,500 at 32
    /// characters and 5,000 at 64; words and paths stay under 4,000.
    pub min_entropy_millibits: u32,
    /// The least number of kinds (lower case, upper case, digit, symbol) such a token has.
    pub min_classes: u8,
    /// A token with this share of its characters (in percent) in runs of three or more lower-case letters is made of words
    /// and is not flagged.
    pub max_wordlike_percent: u8,
    /// Flag hexadecimal tokens of the minimum length too. They are mostly digests and identifiers, so this is off by default.
    pub hex_is_random: bool,
    /// The most matches in one text; more is an error.
    pub max_matches: usize,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            min_random_len: 32,
            min_entropy_millibits: 4_000,
            min_classes: 3,
            max_wordlike_percent: 60,
            hex_is_random: false,
            max_matches: 100_000,
        }
    }
}

const BEGIN: &str = "-----BEGIN ";
const END: &str = "-----END ";
const DASHES: &str = "-----";

/// Scan a text.
///
/// # Errors
///
/// `TooLarge` above `Limits::input_bytes`, and `TooManyMatches` above `ScanConfig::max_matches`. Neither contains any of the
/// text.
pub fn secret_scan(text: &str, config: &ScanConfig, limits: &Limits) -> Result<Vec<SecretMatch>, ScanError> {
    if text.len() > limits.input_bytes {
        return Err(ScanError::TooLarge);
    }
    let mut scanner = Scanner {
        config,
        out: Vec::new(),
        lead: 0,
    };
    let mut block = Block::None;
    let bytes = text.as_bytes();
    let (mut start, mut line_no) = (0_usize, 0_u32);
    while start <= bytes.len() {
        let rest = &bytes[start..];
        let stop = rest.iter().position(|&b| b == b'\n' || b == b'\r');
        let end = stop.map_or(bytes.len(), |p| start + p);
        line_no = line_no.saturating_add(1);
        // A line starts and ends at ASCII bytes, so the slice is on character boundaries.
        scanner.line(line_no, &text[start..end], &mut block)?;
        match stop {
            None => break,
            Some(p) => {
                let at = start + p;
                start = if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                    at + 2
                } else {
                    at + 1
                };
            }
        }
    }
    Ok(scanner.out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    /// Inside a private key: every line is part of the secret.
    Private,
    /// Inside a public key or certificate: nothing in it is looked at.
    Public,
}

/// Converts byte offsets of one line to character columns, moving forward only.
struct Columns {
    ascii: bool,
    byte: usize,
    chars: u32,
}

impl Columns {
    fn new(line: &str) -> Self {
        Self {
            ascii: line.is_ascii(),
            byte: 0,
            chars: 0,
        }
    }

    /// The number of characters before byte offset `to`.
    fn before(&mut self, line: &str, to: usize) -> u32 {
        if self.ascii {
            return u32::try_from(to).unwrap_or(u32::MAX);
        }
        if to < self.byte {
            self.byte = 0;
            self.chars = 0;
        }
        let n = line[self.byte..to].chars().count();
        self.chars = self.chars.saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
        self.byte = to;
        self.chars
    }
}

struct Scanner<'a> {
    config: &'a ScanConfig,
    out: Vec<SecretMatch>,
    /// Where the first token of the current line can start: after its indentation and list dashes.
    lead: usize,
}

/// The next occurrence of a marker in a line, remembered. A line can hold many markers (nothing stops a file from being one
/// line of them), and looking for the next one from scratch each time would make the walk quadratic.
struct Next {
    needle: &'static str,
    /// The search start that `found` was computed for.
    from: Option<usize>,
    found: Option<usize>,
}

impl Next {
    const fn new(needle: &'static str) -> Self {
        Self {
            needle,
            from: None,
            found: None,
        }
    }

    /// The first occurrence at or after `from`.
    fn at(&mut self, line: &str, from: usize) -> Option<usize> {
        if self.from.is_some_and(|searched| searched <= from) {
            // Nothing lies between the search start and the remembered position, so it is still the first one; and if there
            // was none from the search start on, there is none from here on.
            match self.found {
                None => return None,
                Some(position) if position >= from => return Some(position),
                Some(_) => {}
            }
        }
        let found = line[from..].find(self.needle).map(|i| from + i);
        self.from = Some(from);
        self.found = found;
        found
    }
}

/// Both markers of one line.
struct Markers {
    begin: Next,
    end: Next,
}

impl Scanner<'_> {
    fn push(
        &mut self,
        columns: &mut Columns,
        line_no: u32,
        line: &str,
        span: (usize, usize),
        rule: SecretRule,
    ) -> Result<(), ScanError> {
        if self.out.len() >= self.config.max_matches {
            return Err(ScanError::TooManyMatches);
        }
        let before = columns.before(line, span.0);
        let upto = columns.before(line, span.1);
        self.out.push(SecretMatch {
            line: line_no,
            column: before.saturating_add(1),
            length: upto.saturating_sub(before),
            rule,
        });
        Ok(())
    }

    /// One line: a walk from its start, in which begin and end markers (there can be several in one line, JSON puts a whole
    /// key in one) change what the text after them is.
    fn line(&mut self, line_no: u32, line: &str, block: &mut Block) -> Result<(), ScanError> {
        let mut columns = Columns::new(line);
        let mut markers = Markers {
            begin: Next::new(BEGIN),
            end: Next::new(END),
        };
        self.lead = content_start(line.as_bytes());
        let mut from = 0;
        loop {
            let next = if *block == Block::None {
                self.outside(&mut columns, &mut markers, line_no, line, from, block)?
            } else {
                self.inside(&mut columns, &mut markers, line_no, line, from, block)?
            };
            match next {
                Some(resume) if resume < line.len() => from = resume,
                _ => return Ok(()),
            }
        }
    }

    /// `line[from..]` outside any block. Returns where to go on, or `None` when the rest of the line is dealt with.
    fn outside(
        &mut self,
        columns: &mut Columns,
        markers: &mut Markers,
        line_no: u32,
        line: &str,
        from: usize,
        block: &mut Block,
    ) -> Result<Option<usize>, ScanError> {
        let (begin, end) = (markers.begin.at(line, from), markers.end.at(line, from));
        // An end marker with no begin before it (the begin was cut off, or is in another line) is skipped, not read as a
        // token, so that the text after it is read as it is after the end of a block.
        if let Some(at) = end.filter(|&e| begin.is_none_or(|b| e < b)) {
            self.tokens(columns, line_no, line, from, at)?;
            return Ok(Some(marker_end(line, at + END.len())));
        }
        let Some(pos) = begin else {
            self.tokens(columns, line_no, line, from, line.len())?;
            return Ok(None);
        };
        self.tokens(columns, line_no, line, from, pos)?;
        let label_from = pos + BEGIN.len();
        let label_to = line[label_from..]
            .find(DASHES)
            .map_or(line.len(), |i| label_from + i);
        let private = line[label_from..label_to].contains("PRIVATE KEY");
        let header_end = (label_to + DASHES.len()).min(line.len());
        // The end marker, when the whole block is in this line.
        let closing = markers
            .end
            .at(line, header_end)
            .map(|at| marker_end(line, at + END.len()));
        if private {
            let span_end = closing.unwrap_or_else(|| line.trim_end().len());
            self.push(columns, line_no, line, (pos, span_end), SecretRule::PrivateKey)?;
        }
        if closing.is_some() {
            return Ok(closing);
        }
        *block = if private { Block::Private } else { Block::Public };
        Ok(None)
    }

    /// `line[from..]` inside a block. Everything before an end marker is the block's: key, if the block is a private key.
    fn inside(
        &mut self,
        columns: &mut Columns,
        markers: &mut Markers,
        line_no: u32,
        line: &str,
        from: usize,
        block: &mut Block,
    ) -> Result<Option<usize>, ScanError> {
        let marker = markers.end.at(line, from);
        if *block == Block::Private {
            let body = &line[from..marker.unwrap_or(line.len())];
            let trimmed = body.trim();
            if !trimmed.is_empty() {
                let start = from + (body.len() - body.trim_start().len());
                self.push(
                    columns,
                    line_no,
                    line,
                    (start, start + trimmed.len()),
                    SecretRule::PrivateKey,
                )?;
            }
        }
        Ok(marker.map(|at| {
            *block = Block::None;
            marker_end(line, at + END.len())
        }))
    }

    /// The tokens of `line[from..to]`, in order.
    fn tokens(
        &mut self,
        columns: &mut Columns,
        line_no: u32,
        line: &str,
        from: usize,
        to: usize,
    ) -> Result<(), ScanError> {
        let b = line.as_bytes();
        let mut i = from;
        while i < to {
            if !is_token(b[i]) {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < to && is_token(b[j]) {
                j += 1;
            }
            let next = self.token(columns, line_no, line, (i, j), to)?;
            // `next` is past `i`: a span is never empty.
            i = next.max(i + 1);
        }
        Ok(())
    }

    /// Judge the token `line[span]`. Returns the offset where scanning goes on.
    fn token(
        &mut self,
        columns: &mut Columns,
        line_no: u32,
        line: &str,
        span: (usize, usize),
        limit: usize,
    ) -> Result<usize, ScanError> {
        let (i, j) = span;
        let b = line.as_bytes();
        let seg = &b[i..j];
        if seg.starts_with(b"eyJ") {
            if let Some(end) = jwt_end(b, i, limit) {
                self.push(columns, line_no, line, (i, end), SecretRule::Jwt)?;
                return Ok(end);
            }
        }
        if let Some((rule, len)) = known_prefix(seg) {
            self.push(columns, line_no, line, (i, i + len), rule)?;
            return Ok(i + len);
        }
        if seg.len() >= self.config.min_random_len && self.looks_random(b, i, j) {
            // Base64 padding belongs to the value.
            let pad = b[j..limit].iter().take(2).take_while(|&&c| c == b'=').count();
            self.push(columns, line_no, line, (i, j + pad), SecretRule::HighEntropy)?;
            return Ok(j + pad);
        }
        Ok(j)
    }

    /// Whether the token `b[i..j]` is a random-looking value in value position.
    fn looks_random(&self, b: &[u8], i: usize, j: usize) -> bool {
        let seg = &b[i..j];
        // A base64 JSON object that is not a whole token, and a placeholder.
        if seg.starts_with(b"eyJ") || (i >= 2 && &b[i - 2..i] == b"${") {
            return false;
        }
        if i == self.lead && in_key_position(b, j) {
            return false;
        }
        if is_uuid(seg) {
            return false;
        }
        // A hexadecimal token is a digest or an identifier unless the setting says it is a key; as a key, its length is
        // all there is to judge it by (hex digits carry at most four bits each).
        if seg.iter().all(u8::is_ascii_hexdigit) {
            return self.config.hex_is_random;
        }
        if kinds(seg) < self.config.min_classes {
            return false;
        }
        if wordlike_percent(seg) >= u32::from(self.config.max_wordlike_percent) {
            return false;
        }
        entropy_q16(seg) * 1_000 >= u64::from(self.config.min_entropy_millibits) * 65_536
    }
}

/// Where the marker whose label starts at `label_from` ends: after its closing dashes, or at the end of the line.
fn marker_end(line: &str, label_from: usize) -> usize {
    line[label_from..]
        .find(DASHES)
        .map_or(line.len(), |j| label_from + j + DASHES.len())
}

/// Characters of a token: what a key, a password or a base64 value is written with. A full stop ends one.
fn is_token(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'+' | b'/')
}

fn is_base64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')
}

/// The end of a JWT that starts at `i`, or `None`: `eyJ...` `.` `eyJ...` `.` signature, in base64url.
fn jwt_end(b: &[u8], i: usize, limit: usize) -> Option<usize> {
    let part = |from: usize| {
        let len = b[from..limit].iter().take_while(|&&c| is_base64url(c)).count();
        (from + len, len)
    };
    let (e0, l0) = part(i);
    if l0 < 8 || b.get(e0) != Some(&b'.') || e0 + 1 >= limit {
        return None;
    }
    let second = e0 + 1;
    let (e1, l1) = part(second);
    if l1 < 8 || !b[second..e1].starts_with(b"eyJ") || b.get(e1) != Some(&b'.') || e1 + 1 >= limit {
        return None;
    }
    let (e2, l2) = part(e1 + 1);
    (l2 >= 8).then_some(e2)
}

/// A known token prefix at the start of `seg` and the length of the token, when what follows is long enough to be one.
fn known_prefix(seg: &[u8]) -> Option<(SecretRule, usize)> {
    let run = |from: usize, ok: fn(u8) -> bool| seg[from..].iter().take_while(|&&c| ok(c)).count();
    let alnum = |c: u8| c.is_ascii_alphanumeric();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let slug = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-');
    let dashed = |c: u8| c.is_ascii_alphanumeric() || c == b'-';
    if seg.len() > 4
        && seg.starts_with(b"gh")
        && matches!(seg[2], b'p' | b'o' | b'u' | b's' | b'r')
        && seg[3] == b'_'
    {
        let n = run(4, alnum);
        return (n >= 30).then_some((SecretRule::GithubToken, 4 + n));
    }
    if seg.starts_with(b"github_pat_") {
        let n = run(11, word);
        return (n >= 22).then_some((SecretRule::GithubToken, 11 + n));
    }
    if seg.starts_with(b"AKIA") || seg.starts_with(b"ASIA") {
        let n = run(4, |c| c.is_ascii_uppercase() || c.is_ascii_digit());
        let after_ok = seg.get(4 + n).is_none_or(|c| !c.is_ascii_alphanumeric());
        return (n == 16 && after_ok).then_some((SecretRule::AwsAccessKey, 20));
    }
    if seg.starts_with(b"xox")
        && seg.len() > 5
        && matches!(seg[3], b'a' | b'b' | b'p' | b'r' | b's')
        && seg[4] == b'-'
    {
        let n = run(5, dashed);
        return (n >= 10).then_some((SecretRule::SlackToken, 5 + n));
    }
    if seg.starts_with(b"glpat-") {
        let n = run(6, slug);
        return (n >= 20).then_some((SecretRule::GitlabToken, 6 + n));
    }
    if seg.starts_with(b"AIza") {
        let n = run(4, slug);
        return (n >= 35).then_some((SecretRule::GoogleApiKey, 4 + n));
    }
    if seg.starts_with(b"sk_live_") || seg.starts_with(b"rk_live_") {
        let n = run(8, alnum);
        return (n >= 24).then_some((SecretRule::StripeKey, 8 + n));
    }
    None
}

/// Where the first token of a line can start: after its indentation and list dashes (`- `).
fn content_start(line: &[u8]) -> usize {
    let mut at = 0;
    loop {
        while at < line.len() && matches!(line[at], b' ' | b'\t') {
            at += 1;
        }
        if line.get(at) == Some(&b'-') && line.get(at + 1) == Some(&b' ') {
            at += 2;
        } else {
            return at;
        }
    }
}

/// Whether the token that ends at `end` (and is the first of its line) is a name: it is followed by `:` or `=`.
fn in_key_position(line: &[u8], end: usize) -> bool {
    let mut after = end;
    while after < line.len() && matches!(line[after], b' ' | b'\t') {
        after += 1;
    }
    match line.get(after) {
        Some(b':') => line.get(after + 1).is_none_or(|c| matches!(c, b' ' | b'\t')),
        Some(b'=') => line.get(after + 1).is_some_and(|&c| c != b'='),
        _ => false,
    }
}

/// How many of lower case, upper case, digit and symbol the token has.
fn kinds(seg: &[u8]) -> u8 {
    let mut seen = [false; 4];
    for &c in seg {
        let k = if c.is_ascii_lowercase() {
            0
        } else if c.is_ascii_uppercase() {
            1
        } else if c.is_ascii_digit() {
            2
        } else {
            3
        };
        seen[k] = true;
    }
    u8::try_from(seen.iter().filter(|&&s| s).count()).unwrap_or(4)
}

fn is_uuid(seg: &[u8]) -> bool {
    seg.len() == 36
        && seg.iter().enumerate().all(|(i, &c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// The share (in percent, rounded down) of the token's characters that are in runs of three or more lower-case letters.
fn wordlike_percent(seg: &[u8]) -> u32 {
    let mut in_words = 0_usize;
    let mut run = 0_usize;
    for &c in seg {
        if c.is_ascii_lowercase() {
            run += 1;
        } else {
            if run >= 3 {
                in_words += run;
            }
            run = 0;
        }
    }
    if run >= 3 {
        in_words += run;
    }
    u32::try_from(in_words * 100 / seg.len().max(1)).unwrap_or(100)
}

/// `log2(x)` in Q16 fixed point, for `x >= 1`, with integers only: the whole part from the leading bit, then sixteen
/// fractional bits by repeated squaring of the mantissa.
fn log2_q16(x: u32) -> u64 {
    if x <= 1 {
        return 0;
    }
    let whole = 31 - x.leading_zeros();
    // The mantissa in [1, 2) as a Q31 number.
    let mut m = u64::from(x) << (31 - whole);
    let mut fraction = 0_u64;
    for _ in 0..16 {
        m = (m * m) >> 31;
        fraction <<= 1;
        if m >= 1 << 32 {
            m >>= 1;
            fraction |= 1;
        }
    }
    (u64::from(whole) << 16) | fraction
}

/// The Shannon entropy of the token's characters, in bits per character, in Q16 fixed point:
/// `log2(n) - (sum of c * log2(c)) / n` over the counts `c` of each distinct byte.
fn entropy_q16(seg: &[u8]) -> u64 {
    let n = u32::try_from(seg.len()).unwrap_or(u32::MAX);
    if n < 2 {
        return 0;
    }
    let mut counts = [0_u32; 256];
    for &c in seg {
        counts[usize::from(c)] += 1;
    }
    let weighted: u64 = counts
        .iter()
        .filter(|&&c| c > 1)
        .map(|&c| u64::from(c) * log2_q16(c))
        .sum();
    log2_q16(n).saturating_sub(weighted / u64::from(n))
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::{entropy_q16, log2_q16};

    #[test]
    fn the_fixed_point_logarithm_is_close() {
        for x in [2_u32, 3, 5, 7, 10, 33, 64, 100, 1_000, 65_535, 1_000_000] {
            let exact = (f64::from(x)).log2() * 65_536.0;
            let got = log2_q16(x) as f64;
            assert!((exact - got).abs() < 2.0, "log2({x}): {got} against {exact}");
        }
    }

    #[test]
    fn the_entropy_is_zero_for_one_symbol_and_log2_n_for_all_different() {
        assert_eq!(entropy_q16(b"aaaaaaaa"), 0);
        let distinct: Vec<u8> = (0..64).collect();
        let h = entropy_q16(&distinct);
        assert!((h as f64 - 6.0 * 65_536.0).abs() < 4.0, "{h}");
    }
}
