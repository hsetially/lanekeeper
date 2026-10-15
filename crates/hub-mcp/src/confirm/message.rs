//! The question the user answers (D52, S15): what is about to happen, in words the server chose.
//!
//! The message is built from server data only: the swimlane (a typed id), the file or files, the change counts of the draft or of
//! the preview, the effect text rendered from fixed templates over the engine's `PickupState` and `PreviewWarning`, and the link to
//! the web UI. Anything the assistant supplied that is not a number or a typed id (a path, a PR title, a service name) is
//! **untrusted**: [`quote`] strips control and format characters (a right-to-left override can reorder a line, a line feed can start
//! a line of the assistant's own), puts the text in quotes with `\` and `"` escaped so it cannot close them, and cuts it, and the
//! field it stands in is labelled "supplied by the assistant". The whole message is at most [`MAX_CHARS`] characters, and the link,
//! which is where the user can read everything, is the last line and is never the part that is cut.

use domain::{
    ChangePreview, ContentHash, Draft, Expected, NfsPath, PickupState, PrRequest, PrTarget, PreviewScope,
    PreviewWarning, RestartRequest, ServiceRef, SwimlaneId,
};

use crate::output::cap::cut_chars;

/// The longest message, in characters.
pub const MAX_CHARS: usize = 1_500;

/// The words that label an untrusted field.
pub const LABEL: &str = "supplied by the assistant";

/// A link longer than this is left out: the message must keep room for what the user is asked about.
const MAX_LINK_CHARS: usize = 600;
/// Files of a pull request that are named in the message; the rest are counted.
const PR_FILES_SHOWN: usize = 3;
/// Warnings of a preview that are told in the message; the rest are counted.
const WARNINGS_SHOWN: usize = 3;
/// Where the longest untrusted texts are cut, in characters.
const PATH_CHARS: usize = 120;
const PR_PATH_CHARS: usize = 60;
const TITLE_CHARS: usize = 80;
const NAME_CHARS: usize = 63;
/// Hex digits of a hash that are shown.
const HASH_SHOWN: usize = 12;

/// A character that has no place in a line the user reads: a control character, or a format character that hides, joins or
/// reorders text (zero-width characters, the bidirectional controls, soft hyphens, variation selectors, language tags).
fn is_stripped(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{ad}'
                | '\u{34f}'
                | '\u{61c}'
                | '\u{115f}'
                | '\u{1160}'
                | '\u{17b4}'
                | '\u{17b5}'
                | '\u{180b}'..='\u{180f}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{3164}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
                | '\u{ffa0}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{e0000}'..='\u{e0fff}'
        )
}

/// A control character that only separates words (a line break, a tab, the next-line and paragraph separators). It becomes one
/// space, so that two words on two lines stay two words; every other character that [`is_stripped`] is removed.
fn is_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\t' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// `text` as an untrusted string: cleaned, escaped, quoted and cut to at most `max_chars` characters inside the quotes. A cut text
/// ends with `…` inside the quotes. An escape (`\\` or `\"`) is two characters that are kept or dropped together, so a cut never
/// leaves a lone backslash in front of the closing quote.
#[must_use]
pub fn quote(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars.min(text.len()) + 4);
    out.push('"');
    let mut used = 0_usize;
    let mut cut = false;
    // A run of breaks is one space, put in front of the next character that is kept; one at either end is nothing.
    let mut pending_space = false;
    for c in text.chars() {
        if is_break(c) {
            pending_space = true;
            continue;
        }
        if is_stripped(c) {
            continue;
        }
        let lead = usize::from(pending_space && used > 0);
        let width = if matches!(c, '"' | '\\') { 2 } else { 1 };
        if used + lead + width > max_chars {
            cut = true;
            break;
        }
        if lead == 1 {
            out.push(' ');
        }
        pending_space = false;
        if width == 2 {
            out.push('\\');
        }
        out.push(c);
        used += lead + width;
    }
    if cut {
        out.push('…');
    }
    out.push('"');
    out
}

/// `n` and the right form of the noun.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The services of an effect list, by what each needs, in the order of the pickup states (D85). Fixed sentences over counts: no
/// service name is in them.
fn effect_of_services(effects: &[(ServiceRef, PickupState)]) -> String {
    if effects.is_empty() {
        return "no consuming service was found.".to_owned();
    }
    let n = |state: PickupState| effects.iter().filter(|(_, s)| *s == state).count();
    let mut parts: Vec<String> = Vec::new();
    for state in [
        PickupState::Live,
        PickupState::LiveWithinTtl,
        PickupState::NeedsNotifyOrRestart,
        PickupState::NeedsConfigServerRestart,
    ] {
        let k = n(state);
        if k == 0 {
            continue;
        }
        let one = k == 1;
        let services = count(k, "service", "services");
        parts.push(match state {
            PickupState::Live => format!("{services} {} it up at once", if one { "takes" } else { "take" }),
            PickupState::LiveWithinTtl => format!(
                "{services} {} it up within {} cache time",
                if one { "picks" } else { "pick" },
                if one { "its" } else { "their" }
            ),
            PickupState::NeedsNotifyOrRestart => format!(
                "{services} {} a notification or a restart",
                if one { "needs" } else { "need" }
            ),
            PickupState::NeedsConfigServerRestart => format!(
                "{services} {} the config-server restarted first",
                if one { "needs" } else { "need" }
            ),
        });
    }
    format!("{}.", parts.join("; "))
}

/// One warning of a preview, from a fixed sentence. A tenant suffix is typed, and is quoted all the same.
fn warning_text(warning: &PreviewWarning) -> String {
    match warning {
        PreviewWarning::FallsBackToBase => "the base file takes over for this tenant".to_owned(),
        PreviewWarning::NoFileLeft => "no file is left, so the service finds nothing".to_owned(),
        PreviewWarning::ForksFromBase => {
            "a new tenant file becomes a copy that can fall behind the base file".to_owned()
        }
        PreviewWarning::NeverRead { suffix } => format!(
            "its suffix {} is not a tenant of this swimlane, so nobody reads it",
            quote(suffix.as_str(), NAME_CHARS)
        ),
        PreviewWarning::NeedsNotifyOrRestart => {
            "a property source changes: services need a notification or a restart".to_owned()
        }
        PreviewWarning::NewFolderNeedsConfigServerRestart => {
            "the folder is new: the config-server needs a restart to see it".to_owned()
        }
        PreviewWarning::ChannelsYmlNeedsConfigServerRestart => {
            "channels.yml changes: the config-server needs a restart to read it".to_owned()
        }
    }
}

/// Who reads the file and what the change brings with it, for an upload or a delete.
fn effect_of_preview(preview: &ChangePreview) -> String {
    let scope = match &preview.scope {
        PreviewScope::EveryService => "every service reads it".to_owned(),
        PreviewScope::Application { app } => {
            format!("the application {} reads it", quote(app.as_str(), NAME_CHARS))
        }
        PreviewScope::Folder { path } => format!(
            "services that use the folder {} read it",
            quote(path.as_str(), TITLE_CHARS)
        ),
    };
    let mut parts = vec![scope];
    parts.extend(preview.warnings.iter().take(WARNINGS_SHOWN).map(warning_text));
    let more = preview.warnings.len().saturating_sub(WARNINGS_SHOWN);
    if more > 0 {
        parts.push(format!("and {more} more to read in the web UI"));
    }
    format!("{}.", parts.join("; "))
}

fn changes_of_preview(preview: &ChangePreview) -> String {
    match preview.changes_total {
        0 => "no setting differs in what the services get.".to_owned(),
        1 => "1 setting differs in what the services get.".to_owned(),
        n => format!("{n} settings differ in what the services get."),
    }
}

/// The last words of a message for an account whose changes need an admin's approval (D37).
const APPROVAL: &str = "Your account needs approval: after you confirm, this goes to an admin as a proposal and is not carried out now.";

/// Join the lines, then the link. The link is the last line and is never cut; if the body is too long for the room that is left,
/// the body is cut with `…`. (The builders keep each field within a budget so that this does not happen; it is the guard.)
fn finish(lines: &[String], approval: bool, link: Option<&str>) -> String {
    let mut lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    if approval {
        lines.push(APPROVAL);
    }
    let link_line = link
        .filter(|l| l.chars().count() <= MAX_LINK_CHARS)
        .map(|l| format!("Review it in the web UI: {l}"));
    let reserved = link_line.as_ref().map_or(0, |l| l.chars().count() + 1);
    let body = lines.join("\n");
    let room = MAX_CHARS.saturating_sub(reserved);
    let body = if body.chars().count() > room {
        let (head, _) = cut_chars(&body, room.saturating_sub(1));
        format!("{head}…")
    } else {
        body
    };
    match link_line {
        Some(link) => format!("{body}\n{link}"),
        None => body,
    }
}

#[must_use]
pub fn apply_change(draft: &Draft, approval: bool, link: Option<&str>) -> String {
    let severity = draft
        .severity
        .map_or_else(|| "not rated".to_owned(), |s| s.as_str().to_owned());
    finish(
        &[
            format!("Apply a draft to a file in swimlane {}?", draft.swimlane),
            format!("File ({LABEL}): {}", quote(draft.path.as_str(), PATH_CHARS)),
            format!(
                "Draft status: {}. Changes: {} in {}. Severity: {severity}.",
                draft.status,
                count(draft.setting_changes.len(), "setting", "settings"),
                count(draft.hunks.len(), "hunk", "hunks"),
            ),
            format!("Effect: {}", effect_of_services(&draft.effects)),
        ],
        approval,
        link,
    )
}

#[must_use]
pub fn upload_file(
    swimlane: &SwimlaneId,
    path: &NfsPath,
    expected: &Expected,
    bytes: usize,
    preview: &ChangePreview,
    approval: bool,
    link: Option<&str>,
) -> String {
    let what = match expected {
        Expected::Absent => "creates",
        Expected::Hash { .. } => "replaces",
    };
    finish(
        &[
            format!("Upload a file to swimlane {swimlane}?"),
            format!(
                "File ({LABEL}): {}; this {what} it and is {}.",
                quote(path.as_str(), PATH_CHARS),
                count(bytes, "byte", "bytes")
            ),
            format!("Changes: {}", changes_of_preview(preview)),
            format!("Effect: {}", effect_of_preview(preview)),
        ],
        approval,
        link,
    )
}

#[must_use]
pub fn delete_file(
    swimlane: &SwimlaneId,
    path: &NfsPath,
    expected: &ContentHash,
    preview: &ChangePreview,
    approval: bool,
    link: Option<&str>,
) -> String {
    let hash = expected.to_string();
    let shown = hash.get(..HASH_SHOWN).unwrap_or(&hash);
    finish(
        &[
            format!("Delete a file from swimlane {swimlane}?"),
            format!(
                "File ({LABEL}): {}; the version to delete starts with {shown}.",
                quote(path.as_str(), PATH_CHARS)
            ),
            format!("Changes: {}", changes_of_preview(preview)),
            format!("Effect: {}", effect_of_preview(preview)),
        ],
        approval,
        link,
    )
}

#[must_use]
pub fn raise_pr(request: &PrRequest, approval: bool, link: Option<&str>) -> String {
    let named: Vec<String> = request
        .paths
        .iter()
        .take(PR_FILES_SHOWN)
        .map(|p| quote(p.as_str(), PR_PATH_CHARS))
        .collect();
    let more = request.paths.len().saturating_sub(PR_FILES_SHOWN);
    let files = if more > 0 {
        format!("{} and {more} more", named.join(", "))
    } else {
        named.join(", ")
    };
    let target = match &request.target {
        Some(PrTarget::Tenant { branch }) => {
            format!("the branch of tenant {}", quote(branch.as_str(), NAME_CHARS))
        }
        Some(PrTarget::Base) => "the base branch".to_owned(),
        None => "chosen from the file paths: a tenant file to its tenant, a base file to the base branch"
            .to_owned(),
    };
    let body = request.body.as_ref().map_or_else(
        || "none".to_owned(),
        |b| {
            format!(
                "{}, not shown here",
                count(b.chars().count(), "character", "characters")
            )
        },
    );
    finish(
        &[
            format!("Raise a pull request from swimlane {}?", request.swimlane),
            format!(
                "Files ({LABEL}), {}: {files}",
                count(request.paths.len(), "file", "files")
            ),
            format!("Title ({LABEL}): {}", quote(request.title.as_str(), TITLE_CHARS)),
            format!("Target: {target}."),
            format!("Description: {body}."),
            "Effect: the pull request is raised with your own GitHub token. No file on NFS changes."
                .to_owned(),
        ],
        approval,
        link,
    )
}

#[must_use]
pub fn restart_service(request: &RestartRequest, approval: bool, link: Option<&str>) -> String {
    let service = format!("{}/{}", request.service.namespace(), request.service.name());
    finish(
        &[
            format!("Restart a service in swimlane {}?", request.swimlane),
            format!("Service ({LABEL}): {}", quote(&service, PATH_CHARS)),
            "Effect: a rolling restart of this one service, so that it reads its configuration again."
                .to_owned(),
        ],
        approval,
        link,
    )
}

#[cfg(test)]
mod tests {
    use domain::{
        AppName, ChangeKind, DraftId, PickupState, PreviewId, PreviewScope, PreviewWarning, ProposalStatus,
        ServiceRef, SettingChange, SettingPath, Severity, ShortText, Timestamp, UserId,
    };
    use proptest::prelude::*;

    use super::*;

    const LINK: &str = "https://hub.example.test/drafts/draft-1";

    fn swimlane() -> SwimlaneId {
        SwimlaneId::parse("sit1").unwrap()
    }

    fn path(text: &str) -> NfsPath {
        NfsPath::parse(text).unwrap()
    }

    fn hash(c: char) -> ContentHash {
        ContentHash::parse(&c.to_string().repeat(64)).unwrap()
    }

    fn change(n: usize) -> SettingChange {
        SettingChange {
            path: SettingPath::parse(&format!("a.k{n}")).unwrap(),
            kind: ChangeKind::Changed,
            left: None,
            right: None,
            left_location: None,
            right_location: None,
        }
    }

    fn draft(changes: usize, effects: Vec<(ServiceRef, PickupState)>) -> Draft {
        Draft {
            id: DraftId::parse("draft-1").unwrap(),
            author: "11111111-2222-4333-8444-555555555555:aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
                .parse::<UserId>()
                .unwrap(),
            swimlane: swimlane(),
            path: path("tx-core/app-sit1.yml"),
            expected: Expected::Hash { hash: hash('a') },
            new_hash: hash('b'),
            hunks: Vec::new(),
            setting_changes: (0..changes).map(change).collect(),
            status: ProposalStatus::Draft,
            expires_at: Timestamp::from_unix_millis(0),
            severity: Some(Severity::High),
            effects,
        }
    }

    fn service(n: usize) -> ServiceRef {
        ServiceRef::new("apps", &format!("svc-{n}")).unwrap()
    }

    fn preview(
        scope: PreviewScope,
        warnings: Vec<PreviewWarning>,
        total: u32,
        effective: bool,
    ) -> ChangePreview {
        ChangePreview {
            preview_id: PreviewId::parse("pv-1").unwrap(),
            expires_at: Timestamp::from_unix_millis(0),
            scope,
            warnings,
            changes: Vec::new(),
            changes_total: total,
            effective_changed: effective,
        }
    }

    #[test]
    fn quote_wraps_escapes_and_labels_nothing_by_itself() {
        assert_eq!(quote("a.yml", 20), "\"a.yml\"");
        assert_eq!(quote("say \"hi\" \\ bye", 40), r#""say \"hi\" \\ bye""#);
        assert_eq!(quote("", 5), "\"\"");
    }

    #[test]
    fn quote_removes_control_and_format_characters() {
        let hostile = "a\nb\rc\td\u{0}e\u{1b}f\u{7f}g\u{85}h\u{202e}i\u{202c}j\u{200b}k\u{200f}l\u{2066}m\u{feff}n\u{2028}o\u{ad}p";
        // Line breaks and tabs are word breaks; every other control or format character is gone.
        assert_eq!(quote(hostile, 100), "\"a b c defg hijklmn op\"");
        assert_eq!(quote("one\r\n\r\ntwo", 100), "\"one two\"");
        assert_eq!(quote("\n\nlead", 100), "\"lead\"");
    }

    #[test]
    fn quote_cuts_at_the_limit_and_never_splits_an_escape() {
        assert_eq!(quote("abcdef", 3), "\"abc…\"");
        assert_eq!(quote("abc", 3), "\"abc\"");
        // The two characters of an escape stand or fall together: a lone backslash would escape the closing quote.
        assert_eq!(quote("ab\"cd", 3), "\"ab…\"");
        assert_eq!(quote("ab\"cd", 4), "\"ab\\\"…\"");
        let cut = quote(&"\\".repeat(10), 5);
        assert_eq!(cut, "\"\\\\\\\\…\"");
        // Characters, not bytes.
        assert_eq!(quote("äöüß", 2), "\"äö…\"");
    }

    proptest! {
        #[test]
        fn quote_is_bounded_and_always_closes_prop(text in "\\PC{0,300}|[\\\\\"a-z\u{202e}\n]{0,200}", max in 0_usize..80) {
            let q = quote(&text, max);
            prop_assert!(q.chars().count() <= max + 3, "{q}");
            prop_assert!(q.starts_with('"') && q.ends_with('"') && q.len() >= 2);
            let inner = &q[1..q.len() - 1];
            // Inside the quotes every `"` is escaped and every `\` starts an escape: the text cannot close its own quotes.
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                prop_assert!(c != '"', "{q}");
                if c == '\\' {
                    prop_assert!(matches!(chars.next(), Some('\\' | '"')), "{q}");
                }
                prop_assert!(!c.is_control(), "{q}");
            }
        }
    }

    #[test]
    fn apply_change_names_the_swimlane_the_file_the_counts_the_effect_and_the_link() {
        let effects = vec![
            (service(1), PickupState::LiveWithinTtl),
            (service(2), PickupState::LiveWithinTtl),
            (service(3), PickupState::NeedsNotifyOrRestart),
        ];
        let text = apply_change(&draft(3, effects), false, Some(LINK));
        assert!(text.contains("swimlane sit1"), "{text}");
        assert!(text.contains("\"tx-core/app-sit1.yml\""), "{text}");
        assert!(text.contains(&format!("({LABEL})")), "{text}");
        assert!(text.contains("3 settings"), "{text}");
        assert!(text.contains("high"), "{text}");
        assert!(text.contains("2 services"), "{text}");
        assert!(text.contains("within"), "{text}");
        assert!(
            text.contains("1 service needs a notification or a restart"),
            "{text}"
        );
        assert!(text.ends_with(LINK), "{text}");
        assert!(!text.contains("approval"), "{text}");
    }

    #[test]
    fn a_user_who_needs_approval_is_told_it_becomes_a_proposal() {
        for text in [
            apply_change(&draft(1, Vec::new()), true, Some(LINK)),
            raise_pr(&pr(&["a.yml"], "t"), true, Some(LINK)),
            restart_service(&restart(), true, Some(LINK)),
        ] {
            assert!(text.contains("approval"), "{text}");
            assert!(text.contains("proposal"), "{text}");
        }
    }

    fn pr(paths: &[&str], title: &str) -> PrRequest {
        PrRequest {
            swimlane: swimlane(),
            paths: paths.iter().map(|p| path(p)).collect(),
            title: ShortText::parse(title).unwrap(),
            body: Some("b".repeat(300)),
            target: None,
        }
    }

    fn restart() -> RestartRequest {
        RestartRequest {
            swimlane: swimlane(),
            service: ServiceRef::new("apps", "core").unwrap(),
        }
    }

    #[test]
    fn upload_says_whether_it_creates_or_replaces_and_what_it_changes() {
        let p = preview(
            PreviewScope::EveryService,
            vec![PreviewWarning::NeedsNotifyOrRestart],
            2,
            true,
        );
        let replace = upload_file(
            &swimlane(),
            &path("a.yml"),
            &Expected::Hash { hash: hash('a') },
            33,
            &p,
            false,
            Some(LINK),
        );
        assert!(replace.contains("replaces"), "{replace}");
        assert!(replace.contains("33 bytes"), "{replace}");
        assert!(replace.contains("2 settings"), "{replace}");
        assert!(replace.contains("every service"), "{replace}");
        assert!(replace.contains("notification or a restart"), "{replace}");
        let create = upload_file(
            &swimlane(),
            &path("a.yml"),
            &Expected::Absent,
            33,
            &p,
            false,
            Some(LINK),
        );
        assert!(create.contains("creates"), "{create}");
        assert!(!create.contains("replaces"), "{create}");
        let unchanged = preview(PreviewScope::EveryService, Vec::new(), 0, false);
        let text = upload_file(
            &swimlane(),
            &path("a.yml"),
            &Expected::Absent,
            1,
            &unchanged,
            false,
            None,
        );
        assert!(text.contains("no setting differs"), "{text}");
        assert!(!text.contains("web UI"), "no link, no mention of one: {text}");
    }

    #[test]
    fn delete_shows_the_start_of_the_hash_it_will_delete() {
        let p = preview(
            PreviewScope::Application {
                app: AppName::parse("tx-core").unwrap(),
            },
            vec![PreviewWarning::FallsBackToBase],
            1,
            true,
        );
        let text = delete_file(&swimlane(), &path("a.yml"), &hash('c'), &p, false, Some(LINK));
        assert!(text.contains("cccccccccccc"), "{text}");
        assert!(!text.contains(&"c".repeat(20)), "{text}");
        assert!(text.contains("base file takes over"), "{text}");
        assert!(text.contains("\"tx-core\""), "{text}");
    }

    #[test]
    fn a_pr_lists_a_few_files_counts_the_rest_and_never_prints_the_body() {
        let many: Vec<String> = (0..40).map(|n| format!("dir/file-{n}.yml")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let text = raise_pr(&pr(&refs, "A title"), false, Some(LINK));
        assert!(text.contains("40 files"), "{text}");
        assert!(text.contains("\"dir/file-0.yml\""), "{text}");
        assert!(text.contains("37 more"), "{text}");
        assert!(!text.contains("file-39"), "{text}");
        assert!(text.contains("300 characters"), "{text}");
        assert!(text.contains("\"A title\""), "{text}");
    }

    #[test]
    fn restart_names_the_service_as_namespace_slash_name() {
        let text = restart_service(&restart(), false, Some(LINK));
        assert!(text.contains("\"apps/core\""), "{text}");
        assert!(text.contains("swimlane sit1"), "{text}");
    }

    #[test]
    fn the_worst_case_of_every_message_fits_and_keeps_its_link() {
        let link = format!("https://hub.example.test/{}", "x".repeat(480));
        assert!(link.chars().count() <= 512);
        let long = "a/".repeat(500);
        let long = &long[..1000];
        let effects: Vec<_> = (0..50)
            .map(|n| {
                (
                    service(n),
                    [
                        PickupState::Live,
                        PickupState::LiveWithinTtl,
                        PickupState::NeedsNotifyOrRestart,
                        PickupState::NeedsConfigServerRestart,
                    ][n % 4],
                )
            })
            .collect();
        let mut big_draft = draft(100_000, effects);
        big_draft.path = path(long);
        let warnings = vec![
            PreviewWarning::FallsBackToBase,
            PreviewWarning::NoFileLeft,
            PreviewWarning::ForksFromBase,
            PreviewWarning::NeverRead {
                suffix: domain::TenantId::parse(&"t".repeat(63)).unwrap(),
            },
            PreviewWarning::NeedsNotifyOrRestart,
            PreviewWarning::NewFolderNeedsConfigServerRestart,
            PreviewWarning::ChannelsYmlNeedsConfigServerRestart,
        ];
        let p = preview(
            PreviewScope::Folder { path: path(long) },
            warnings,
            u32::MAX,
            true,
        );
        let many: Vec<&str> = vec![long; 100];
        let title = "\"".repeat(250);
        let mut messages = vec![
            apply_change(&big_draft, true, Some(&link)),
            upload_file(
                &SwimlaneId::parse(&"s".repeat(63)).unwrap(),
                &path(long),
                &Expected::Hash { hash: hash('f') },
                usize::MAX,
                &p,
                true,
                Some(&link),
            ),
            delete_file(
                &SwimlaneId::parse(&"s".repeat(63)).unwrap(),
                &path(long),
                &hash('f'),
                &p,
                true,
                Some(&link),
            ),
            restart_service(
                &RestartRequest {
                    swimlane: SwimlaneId::parse(&"s".repeat(63)).unwrap(),
                    service: ServiceRef::new(&"n".repeat(253), &"m".repeat(253)).unwrap(),
                },
                true,
                Some(&link),
            ),
        ];
        let mut request = pr(&many, &title);
        request.swimlane = SwimlaneId::parse(&"s".repeat(63)).unwrap();
        request.target = Some(domain::PrTarget::Tenant {
            branch: domain::TenantId::parse(&"b".repeat(63)).unwrap(),
        });
        messages.push(raise_pr(&request, true, Some(&link)));
        for text in messages {
            // Within the budgets of the builders, so that the guard of `finish` that cuts the body is never what keeps it short.
            assert!(
                text.chars().count() <= MAX_CHARS - 100,
                "{} chars:\n{text}",
                text.chars().count()
            );
            assert!(text.ends_with(&link), "the link is cut:\n{text}");
        }
    }
}
