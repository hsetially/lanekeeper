//! Step 4: validation with the engine (D45, S13). Facts come from `crates/engine` (rule 7): this module calls it and turns what
//! it says into the issues a caller sees.
//!
//! - A file that does not parse blocks the save and reports the line, never the text.
//! - A duplicate key is a warning (check C8): the write goes through and the warning goes back with the outcome.
//! - The value-based secret scan blocks the save and reports line numbers and rule names, never the value (S13, S10).
//!
//! The audit diff is made here too, because it needs the same scan: a line of the old file that held a secret is redacted
//! before the diff is kept (`AuditEvent::diff` says flagged values must already be redacted).
//!
//! Everything here is CPU-bound and synchronous; callers run it on the blocking pool (rule 4).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::num::NonZeroU32;

use bytes::Bytes;
use domain::{
    DiffHunk, DiffLineKind, FileClass, IssueKind, NfsPath, RejectReason, ShortText, ValidationIssue,
};
use engine::{
    Limits, ParseError, ParseWarning, ScanConfig, SecretMatch, Syntax, decode_text, secret_scan, text_diff,
    validate,
};
use ports::AuditEvent;

/// What the file held before, as far as the writer could find out.
#[derive(Debug, Clone)]
pub(crate) enum Old {
    /// The request expects the file not to exist.
    Absent,
    Known(Bytes),
    /// The file the request expects could not be read back anywhere (not in the registry, the blob store or from the agent).
    Unknown,
}

/// The verdict on new content.
#[derive(Debug, Clone, Default)]
pub(crate) struct Checked {
    /// Set when the content is refused: the reason, and the places.
    pub reject: Option<(RejectReason, Vec<ValidationIssue>)>,
    /// Worth knowing, not blocking.
    pub warnings: Vec<ValidationIssue>,
    /// The diff to keep in the audit event: unified, with flagged values redacted, at most `AuditEvent::MAX_DIFF_BYTES`.
    pub diff: String,
}

fn issue(kind: IssueKind, line: u32, rule: Option<&str>) -> ValidationIssue {
    ValidationIssue {
        kind,
        line: NonZeroU32::new(line),
        rule: rule.and_then(|r| ShortText::parse(r).ok()),
    }
}

/// Validate `new` as the content of `path`.
pub(crate) fn check(path: &NfsPath, class: FileClass, new: &Bytes, old: &Old, limits: &Limits) -> Checked {
    if class == FileClass::Binary {
        return Checked {
            diff: binary_note(old, new.len()),
            ..Checked::default()
        };
    }

    let mut parse_issues: Vec<ValidationIssue> = Vec::new();
    let mut warnings: Vec<ValidationIssue> = Vec::new();
    let mut reject_reason: Option<RejectReason> = None;

    if let Some(syntax) = Syntax::of_path(path).filter(|_| class == FileClass::Structured) {
        match validate(new, syntax, limits) {
            Ok(v) => {
                for w in v.warnings {
                    if let ParseWarning::DuplicateKey { second, .. } = w {
                        warnings.push(issue(IssueKind::DuplicateKey, second.line, None));
                    }
                }
            }
            Err(ParseError::Syntax { line, .. } | ParseError::UnknownAlias { line }) => {
                parse_issues.push(issue(IssueKind::ParseError, line, None));
            }
            Err(ParseError::Limit(engine::LimitKind::InputBytes)) => {
                reject_reason = Some(RejectReason::TooLarge);
            }
            Err(_) => parse_issues.push(issue(IssueKind::ParseError, 0, None)),
        }
    }

    let mut secret_issues: Vec<ValidationIssue> = Vec::new();
    let mut new_flags: BTreeMap<u32, String> = BTreeMap::new();
    let mut new_text: Option<String> = None;
    match decode_text(new, limits) {
        Ok(decoded) => {
            match secret_scan(&decoded.text, &ScanConfig::default(), limits) {
                Ok(matches) => {
                    for m in &matches {
                        secret_issues.push(issue(IssueKind::SecretMatch, m.line, Some(m.rule.name())));
                    }
                    new_flags = flagged_lines(&matches);
                }
                // More matches than the scan reports is a secret-laden file, and a short list would hide the rest.
                Err(engine::ScanError::TooManyMatches) => {
                    reject_reason.get_or_insert(RejectReason::SecretDetected);
                }
                Err(_) => {
                    reject_reason.get_or_insert(RejectReason::InvalidContent);
                }
            }
            new_text = Some(decoded.text.into_owned());
        }
        // A structured file that does not decode has been reported by the parser above.
        Err(_) if !parse_issues.is_empty() => {}
        Err(_) => {
            reject_reason.get_or_insert(RejectReason::InvalidContent);
        }
    }

    let reject = if reject_reason == Some(RejectReason::TooLarge) {
        Some((RejectReason::TooLarge, Vec::new()))
    } else if !parse_issues.is_empty() {
        parse_issues.extend(secret_issues);
        Some((RejectReason::ParseError, parse_issues))
    } else if !secret_issues.is_empty() || reject_reason == Some(RejectReason::SecretDetected) {
        Some((RejectReason::SecretDetected, secret_issues))
    } else {
        reject_reason.map(|r| (r, Vec::new()))
    };
    let reject = reject.map(|(r, mut issues)| {
        issues.truncate(ValidationIssue::MAX_PER_OUTCOME);
        (r, issues)
    });
    warnings.truncate(ValidationIssue::MAX_PER_OUTCOME);

    let diff = match (&new_text, &reject) {
        (Some(text), None) => audit_diff(old, text, &new_flags, limits),
        // A refused write has no audit event, so no diff is made for it.
        _ => String::new(),
    };
    Checked {
        reject,
        warnings,
        diff,
    }
}

/// The diff an audit event keeps for a deleted file: every line removed, with the lines the scan flagged redacted. A binary file
/// is named, never shown.
pub(crate) fn delete_diff(class: FileClass, old: &Old, limits: &Limits) -> String {
    match (class, old) {
        (FileClass::Binary | FileClass::Denied, Old::Known(b)) => {
            format!("binary file deleted ({} bytes)", b.len())
        }
        (FileClass::Binary | FileClass::Denied, _) => "binary file deleted".to_owned(),
        _ => audit_diff(old, "", &BTreeMap::new(), limits),
    }
}

/// The marker for each line the scan flagged: the first match on the line stands for the line.
pub(crate) fn flagged_lines(matches: &[SecretMatch]) -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    for m in matches {
        out.entry(m.line).or_insert_with(|| m.marker());
    }
    out
}

fn binary_note(old: &Old, new_len: usize) -> String {
    match old {
        Old::Absent => format!("binary file added ({new_len} bytes)"),
        Old::Known(b) => format!("binary file changed ({} bytes to {new_len} bytes)", b.len()),
        Old::Unknown => format!("binary file written ({new_len} bytes)"),
    }
}

/// The unified diff of the old file against the new text, with every line the scan flagged on either side replaced by its
/// marker, and cut at the audit event's limit.
fn audit_diff(old: &Old, new_text: &str, new_flags: &BTreeMap<u32, String>, limits: &Limits) -> String {
    let old_bytes = match old {
        Old::Absent => Bytes::new(),
        Old::Known(b) => b.clone(),
        Old::Unknown => return "(previous content not available)".to_owned(),
    };
    let Ok(decoded) = decode_text(&old_bytes, limits) else {
        return "(previous content is not text)".to_owned();
    };
    // If the old file cannot be scanned, its lines cannot be shown: a diff with an unredacted secret is worse than none.
    let Ok(matches) = secret_scan(&decoded.text, &ScanConfig::default(), limits) else {
        return "(diff withheld: the previous content could not be scanned)".to_owned();
    };
    let old_flags = flagged_lines(&matches);
    let diff = text_diff(&decoded.text, new_text, limits);
    if diff.identical {
        return "(no change)".to_owned();
    }
    if diff.eol_only {
        return "(line endings only)".to_owned();
    }
    if diff.too_large {
        return "(diff too large to record)".to_owned();
    }
    let mut out = render(&diff.hunks, &old_flags, new_flags);
    cap(&mut out, AuditEvent::MAX_DIFF_BYTES);
    out
}

fn render(
    hunks: &[DiffHunk],
    old_flags: &BTreeMap<u32, String>,
    new_flags: &BTreeMap<u32, String>,
) -> String {
    let mut out = String::new();
    for h in hunks {
        let _ = writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        );
        let (mut o, mut n) = (h.old_start, h.new_start);
        for line in &h.lines {
            let (sign, flagged) = match line.kind {
                DiffLineKind::Context => {
                    let f = old_flags.get(&o).or_else(|| new_flags.get(&n));
                    o += 1;
                    n += 1;
                    (' ', f)
                }
                DiffLineKind::Removed => {
                    let f = old_flags.get(&o);
                    o += 1;
                    ('-', f)
                }
                DiffLineKind::Added => {
                    let f = new_flags.get(&n);
                    n += 1;
                    ('+', f)
                }
            };
            out.push(sign);
            out.push_str(flagged.map_or(line.text.as_str(), String::as_str));
            out.push('\n');
        }
    }
    out
}

/// Cut `s` to at most `max` bytes on a character boundary, and say so.
fn cap(s: &mut String, max: usize) {
    const NOTE: &str = "\n... diff cut\n";
    if s.len() <= max {
        return;
    }
    let mut end = max.saturating_sub(NOTE.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s.push_str(NOTE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> NfsPath {
        NfsPath::parse(s).unwrap()
    }

    fn run(path: &str, class: FileClass, new: &str, old: &Old) -> Checked {
        check(
            &p(path),
            class,
            &Bytes::copy_from_slice(new.as_bytes()),
            old,
            &Limits::DEFAULT,
        )
    }

    fn known(s: &str) -> Old {
        Old::Known(Bytes::copy_from_slice(s.as_bytes()))
    }

    #[test]
    fn a_cut_diff_stays_within_the_limit_and_on_a_character_boundary() {
        let mut s = "é".repeat(AuditEvent::MAX_DIFF_BYTES);
        cap(&mut s, AuditEvent::MAX_DIFF_BYTES);
        assert!(s.len() <= AuditEvent::MAX_DIFF_BYTES);
        assert!(s.ends_with("diff cut\n"));
        let mut short = "abc".to_owned();
        cap(&mut short, 10);
        assert_eq!(short, "abc");
    }

    #[test]
    fn a_changed_line_is_in_the_diff_and_an_unchanged_file_says_so() {
        let c = run("a.yml", FileClass::Structured, "x: 2\n", &known("x: 1\n"));
        assert!(c.reject.is_none());
        assert!(c.diff.contains("-x: 1") && c.diff.contains("+x: 2"), "{}", c.diff);
        let same = run("a.yml", FileClass::Structured, "x: 1\n", &known("x: 1\n"));
        assert_eq!(same.diff, "(no change)");
        let created = run("a.yml", FileClass::Structured, "x: 1\n", &Old::Absent);
        assert!(created.diff.contains("+x: 1"));
        let unknown = run("a.yml", FileClass::Structured, "x: 1\n", &Old::Unknown);
        assert_eq!(unknown.diff, "(previous content not available)");
    }

    #[test]
    fn an_old_secret_is_redacted_in_the_diff_and_the_new_text_is_not_blocked() {
        let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
        let c = run(
            "a.yml",
            FileClass::Structured,
            "ok: 1\n",
            &known(&format!("k: {key}\nok: 1\n")),
        );
        assert!(c.reject.is_none());
        assert!(!c.diff.contains(&key), "{}", c.diff);
        assert!(c.diff.contains("[REDACTED: "));
    }

    #[test]
    fn a_parse_error_reports_the_line_and_no_text() {
        let c = run(
            "a.yml",
            FileClass::Structured,
            "a: 1\nb: [oops-marker\n",
            &Old::Absent,
        );
        let (reason, issues) = c.reject.unwrap();
        assert_eq!(reason, RejectReason::ParseError);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].line.is_some());
        assert!(!format!("{issues:?}").contains("oops-marker"));
    }

    #[test]
    fn a_secret_reports_the_line_and_the_rule() {
        let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
        let c = run(
            "notes.txt",
            FileClass::Text,
            &format!("a\nb: {key}\n"),
            &Old::Absent,
        );
        let (reason, issues) = c.reject.unwrap();
        assert_eq!(reason, RejectReason::SecretDetected);
        assert_eq!(issues[0].line.map(NonZeroU32::get), Some(2));
        assert_eq!(
            issues[0].rule.as_ref().map(ShortText::as_str),
            Some("aws_access_key")
        );
        assert!(!format!("{issues:?}").contains(&key));
    }

    #[test]
    fn duplicate_keys_warn_and_binary_is_not_read() {
        let c = run("a.yml", FileClass::Structured, "a: 1\na: 2\n", &Old::Absent);
        assert!(c.reject.is_none());
        assert_eq!(c.warnings.len(), 1);
        assert_eq!(c.warnings[0].kind, IssueKind::DuplicateKey);
        let bin = check(
            &p("logo.bmp"),
            FileClass::Binary,
            &Bytes::from_static(&[0, 1, 2]),
            &Old::Absent,
            &Limits::DEFAULT,
        );
        assert!(bin.reject.is_none() && bin.diff.starts_with("binary file added"));
    }

    #[test]
    fn issues_are_capped_per_outcome() {
        let mut text = String::new();
        for i in 0..80 {
            let _ = writeln!(text, "k{i}: {}", ["AKIA", "IOSFODNN7EXAMPLE"].concat());
        }
        let c = run("notes.txt", FileClass::Text, &text, &Old::Absent);
        let (_, issues) = c.reject.unwrap();
        assert_eq!(issues.len(), ValidationIssue::MAX_PER_OUTCOME);
    }
}
