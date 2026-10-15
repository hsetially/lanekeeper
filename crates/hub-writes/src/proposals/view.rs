//! What a draft or a proposal shows an approver: the hunks of the change and the settings it changes (S12, S13, S14b).
//!
//! The facts come from the engine (rule 7). The only thing added here is the mask: a line or a setting that the value-based
//! secret scan flags, on either side of the change, is replaced by its marker before it leaves this module. A file that cannot be
//! scanned is shown as nothing at all (no hunks) and its settings are masked whole, because "might this be a secret" is answered
//! "yes" when the scan could not look.
//!
//! Everything here is CPU-bound and synchronous; callers run it on the blocking pool (rule 4).

use std::collections::BTreeMap;

use bytes::Bytes;
use domain::{DiffHunk, DiffLineKind, FileClass, LineRange, NfsPath, SettingChange};
use engine::{
    DenyMatcher, Limits, ScanConfig, Syntax, classify, decode_text, flatten, parse, secret_scan,
    semantic_diff, text_diff,
};

use crate::pipeline::validate::{Old, flagged_lines};

/// The marker of a file the scan could not read.
const UNSCANNED: &str = "[REDACTED: the file could not be scanned]";

/// The hunks and the setting changes of one change, masked.
#[derive(Debug, Clone, Default)]
pub(crate) struct DiffView {
    pub hunks: Vec<DiffHunk>,
    pub setting_changes: Vec<SettingChange>,
}

/// One side of a change: its text, the markers of its flagged lines, and its settings.
pub(crate) struct Side {
    pub text: String,
    /// The markers of the flagged lines; `None` when the scan could not run.
    pub flags: Option<BTreeMap<u32, String>>,
    pub settings: Vec<engine::FlatSetting>,
}

pub(crate) fn side(path: &NfsPath, class: FileClass, bytes: &[u8], limits: &Limits) -> Option<Side> {
    let decoded = decode_text(bytes, limits).ok()?;
    let flags = secret_scan(&decoded.text, &ScanConfig::default(), limits)
        .ok()
        .map(|found| flagged_lines(&found));
    let mut settings = Vec::new();
    if class == FileClass::Structured {
        if let Some(syntax) = Syntax::of_path(path) {
            // A file that does not parse has no settings to compare.
            if let Ok(parsed) = parse(&decoded.text, syntax, limits) {
                if let Ok(flat) = flatten(&parsed, limits) {
                    settings = flat.settings;
                }
            }
        }
    }
    Some(Side {
        text: decoded.text.into_owned(),
        flags,
        settings,
    })
}

/// The marker for a value written on `lines`, if the scan flagged one of them (or could not scan at all). A value is masked
/// exactly when this is `Some`, and the reveal action (T10) reveals exactly the values for which it is.
pub(crate) fn marker(flags: Option<&BTreeMap<u32, String>>, lines: LineRange) -> Option<String> {
    match flags {
        None => Some(UNSCANNED.to_owned()),
        Some(flags) => flags
            .range(lines.start()..=lines.end())
            .next()
            .map(|(_, marker)| marker.clone()),
    }
}

/// The diff of `new` against `old` as the file `path`, masked. A binary or denied file has none; neither does a change whose old
/// side is not known.
pub(crate) fn diff_view(
    path: &NfsPath,
    old: &Old,
    new: &Bytes,
    deny: &DenyMatcher,
    limits: &Limits,
) -> DiffView {
    let class = classify(path, Some(new), deny);
    if matches!(class, FileClass::Binary | FileClass::Denied) {
        return DiffView::default();
    }
    let old_bytes: &[u8] = match old {
        Old::Absent => &[],
        Old::Known(b) => b,
        Old::Unknown => return DiffView::default(),
    };
    let (Some(before), Some(after)) = (
        side(path, class, old_bytes, limits),
        side(path, class, new, limits),
    ) else {
        return DiffView::default();
    };

    let mut view = DiffView::default();
    // A hunk is shown only when both sides were scanned: a line that might be a secret is never shown.
    if let (Some(old_flags), Some(new_flags)) = (&before.flags, &after.flags) {
        let diff = text_diff(&before.text, &after.text, limits);
        view.hunks = diff.hunks;
        for hunk in &mut view.hunks {
            redact(hunk, old_flags, new_flags);
        }
    }

    view.setting_changes = semantic_diff(&before.settings, &after.settings);
    for change in &mut view.setting_changes {
        if let (Some(value), Some(at)) = (change.left.as_mut(), change.left_location.as_ref()) {
            if let Some(m) = marker(before.flags.as_ref(), at.lines) {
                value.text = m;
            }
        } else if let Some(value) = change.left.as_mut() {
            UNSCANNED.clone_into(&mut value.text);
        }
        if let (Some(value), Some(at)) = (change.right.as_mut(), change.right_location.as_ref()) {
            if let Some(m) = marker(after.flags.as_ref(), at.lines) {
                value.text = m;
            }
        } else if let Some(value) = change.right.as_mut() {
            UNSCANNED.clone_into(&mut value.text);
        }
    }
    view
}

/// Replace every flagged line of a hunk by its marker. The numbers follow the unified diff's: a context line is on both sides.
fn redact(hunk: &mut DiffHunk, old_flags: &BTreeMap<u32, String>, new_flags: &BTreeMap<u32, String>) {
    let (mut o, mut n) = (hunk.old_start, hunk.new_start);
    for line in &mut hunk.lines {
        let flagged = match line.kind {
            DiffLineKind::Context => {
                let f = old_flags.get(&o).or_else(|| new_flags.get(&n));
                o += 1;
                n += 1;
                f
            }
            DiffLineKind::Removed => {
                let f = old_flags.get(&o);
                o += 1;
                f
            }
            DiffLineKind::Added => {
                let f = new_flags.get(&n);
                n += 1;
                f
            }
        };
        if let Some(marker) = flagged {
            line.text.clone_from(marker);
            line.inline_changes.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> String {
        ["AKIA", "IOSFODNN7EXAMPLE"].concat()
    }

    fn view(path: &str, old: &str, new: &str) -> DiffView {
        diff_view(
            &NfsPath::parse(path).unwrap(),
            &Old::Known(Bytes::copy_from_slice(old.as_bytes())),
            &Bytes::copy_from_slice(new.as_bytes()),
            &DenyMatcher::with_globs(std::iter::empty::<&str>()).unwrap(),
            &Limits::DEFAULT,
        )
    }

    #[test]
    fn a_changed_setting_shows_in_the_hunks_and_the_setting_changes() {
        let v = view("a.yml", "x: 1\ny: 2\n", "x: 1\ny: 3\n");
        assert_eq!(v.hunks.len(), 1);
        assert_eq!(v.setting_changes.len(), 1);
        assert_eq!(v.setting_changes[0].left.as_ref().unwrap().text, "2");
        assert_eq!(v.setting_changes[0].right.as_ref().unwrap().text, "3");
    }

    #[test]
    fn a_secret_in_the_old_file_is_masked_in_the_hunks_and_in_the_setting_changes() {
        let v = view("a.yml", &format!("k: {}\nok: 1\n", key()), "k: gone\nok: 1\n");
        let shown = format!("{v:?}");
        assert!(!shown.contains(&key()), "{shown}");
        assert!(shown.contains("[REDACTED: "), "{shown}");
        let k = v
            .setting_changes
            .iter()
            .find(|c| c.left.is_some())
            .expect("the removed value is listed");
        assert!(k.left.as_ref().unwrap().text.starts_with("[REDACTED: "));
    }

    #[test]
    fn no_old_content_means_no_view_and_a_binary_file_has_none() {
        let deny = DenyMatcher::with_globs(std::iter::empty::<&str>()).unwrap();
        let p = NfsPath::parse("a.yml").unwrap();
        let unknown = diff_view(
            &p,
            &Old::Unknown,
            &Bytes::from_static(b"x: 1\n"),
            &deny,
            &Limits::DEFAULT,
        );
        assert!(unknown.hunks.is_empty() && unknown.setting_changes.is_empty());
        let bmp = NfsPath::parse("logo.bmp").unwrap();
        let bin = diff_view(
            &bmp,
            &Old::Absent,
            &Bytes::from_static(&[0, 1, 2]),
            &deny,
            &Limits::DEFAULT,
        );
        assert!(bin.hunks.is_empty());
    }
}
