//! The Adaptive Cards the Teams notifier sends (T7, D49, plan A10): one per kind of notification, built from the notification and
//! nothing else, so a card holds the names, ids and counts the notification holds and never file content or a secret.
//!
//! - **Envelope.** A Workflows webhook takes a message with one attachment of content type
//!   `application/vnd.microsoft.card.adaptive`.
//! - **Text is data.** The summary and title may hold a person's display name. Teams renders a subset of Markdown in a text
//!   block, so every character that would start a link, emphasis, a tag or a list is escaped; a name like `[x](https://evil)`
//!   shows as typed.
//! - **Links.** A notification's link is a path inside the web app. It becomes a button only when the configured web origin is
//!   known, and only when it is a plain path (`/...`), so a link can never send a reader to another site.
//! - **Kinds.** Each kind has its own headline. Drift alerts have their kind; overwritten changes and audit-chain alerts have no
//!   kind of their own and travel as critical findings, so those two are told apart by their title (A10).

use domain::{Severity, ShortText};
use ports::{Notification, NotificationKind};
use serde_json::{Value, json};

/// The most bytes a card may have: Teams refuses a message above 28 KB. Every field of a notification is at most 256 bytes, so a
/// card stays far below it; the unit tests hold the line.
pub(super) const MAX_CARD_BYTES: usize = 28 * 1024;

/// The headline of the card: what kind of event it is.
pub(super) fn headline(n: &Notification) -> &'static str {
    match n.kind {
        NotificationKind::AccessRequested => "Access requested",
        NotificationKind::ProposalPending => "Change awaiting approval",
        NotificationKind::ProposalDecided => "Proposal decided",
        NotificationKind::ServiceRestarted => "Service restarted",
        NotificationKind::PrRaised => "Pull request raised",
        NotificationKind::AgentDisconnected => "Agent disconnected",
        NotificationKind::DriftDetected => "Drift detected",
        NotificationKind::SyncWindowStale => "Sync window open too long",
        // Overwritten changes and audit-chain alerts have no kind of their own (A10): the title says which they are.
        NotificationKind::CriticalFinding => {
            let title = n.title.as_str().to_ascii_lowercase();
            if title.starts_with("overwritten change") {
                "Overwritten change"
            } else if title.starts_with("audit chain") {
                "Audit chain alert"
            } else {
                "Critical finding"
            }
        }
    }
}

/// `text` with every character that Teams would read as Markdown or markup escaped with a backslash.
pub(super) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '`' | '~' | '[' | ']' | '(' | ')' | '<' | '>'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether `link` is a plain path of the web app: it starts with one `/`, holds only the characters of a path and a query, and
/// has no `..` segment. No scheme, host, user, fragment, backslash or space can get through.
fn is_plain_path(link: &str) -> bool {
    link.starts_with('/')
        && !link.starts_with("//")
        && link.len() <= 256
        && link
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.~%?=&+,:".contains(&b))
        && !link.split(['/', '?']).any(|segment| segment == "..")
        && !link.to_ascii_lowercase().contains("%2e")
}

/// The address of the button: the web origin and the notification's path, or `None` when there is no origin or the link is not
/// a plain path of the web app.
pub(super) fn link_url(origin: Option<&str>, link: Option<&str>) -> Option<String> {
    let (origin, link) = (origin?, link?);
    is_plain_path(link).then(|| format!("{origin}{link}"))
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Low => "Low",
        Severity::Medium => "Medium",
        Severity::High => "High",
        Severity::Critical => "Critical",
    }
}

fn severity_colour(s: Severity) -> &'static str {
    match s {
        Severity::Critical | Severity::High => "Attention",
        Severity::Medium => "Warning",
        Severity::Low => "Default",
    }
}

/// The card itself.
pub(super) fn card(n: &Notification, origin: Option<&str>) -> Value {
    let mut facts = vec![json!({ "title": "Severity", "value": severity_word(n.severity) })];
    if let Some(swimlane) = &n.swimlane {
        facts.push(json!({ "title": "Swimlane", "value": swimlane.to_string() }));
    }
    let mut card = json!({
        "type": "AdaptiveCard",
        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
        "version": "1.4",
        "msteams": { "width": "Full" },
        "body": [
            { "type": "TextBlock", "text": headline(n), "weight": "Bolder", "size": "Small", "isSubtle": true, "wrap": true },
            { "type": "TextBlock", "text": escape(n.title.as_str()), "weight": "Bolder", "size": "Medium",
              "color": severity_colour(n.severity), "wrap": true },
            { "type": "TextBlock", "text": escape(n.summary.as_str()), "wrap": true },
            { "type": "FactSet", "facts": facts },
        ],
    });
    if let Some(url) = link_url(origin, n.link.as_ref().map(ShortText::as_str)) {
        card["actions"] = json!([{ "type": "Action.OpenUrl", "title": "Open in Lanekeeper", "url": url }]);
    }
    card
}

/// The message a Workflows webhook takes: the card as its one attachment.
pub(super) fn message(n: &Notification, origin: Option<&str>) -> Value {
    json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "contentUrl": null,
            "content": card(n, origin),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Severity, ShortText, SwimlaneId};

    /// Every kind. The match makes a new kind a compile error here until it has a card test.
    fn every_kind() -> Vec<NotificationKind> {
        let all = [
            NotificationKind::AccessRequested,
            NotificationKind::ProposalPending,
            NotificationKind::ProposalDecided,
            NotificationKind::ServiceRestarted,
            NotificationKind::PrRaised,
            NotificationKind::AgentDisconnected,
            NotificationKind::CriticalFinding,
            NotificationKind::DriftDetected,
            NotificationKind::SyncWindowStale,
        ];
        for k in all {
            match k {
                NotificationKind::AccessRequested
                | NotificationKind::ProposalPending
                | NotificationKind::ProposalDecided
                | NotificationKind::ServiceRestarted
                | NotificationKind::PrRaised
                | NotificationKind::AgentDisconnected
                | NotificationKind::CriticalFinding
                | NotificationKind::DriftDetected
                | NotificationKind::SyncWindowStale => {}
            }
        }
        all.to_vec()
    }

    fn note(kind: NotificationKind, title: &str, summary: &str, link: Option<&str>) -> Notification {
        Notification {
            kind,
            severity: Severity::High,
            swimlane: Some(SwimlaneId::parse("sit1").unwrap()),
            title: ShortText::parse(title).unwrap(),
            summary: ShortText::parse(summary).unwrap(),
            link: link.map(|l| ShortText::parse(l).unwrap()),
        }
    }

    const ORIGIN: &str = "https://lanekeeper.example.test";

    /// Every string in the card, keys excluded.
    fn strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            Value::Object(o) => o.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }

    #[test]
    fn a_card_exists_for_every_kind_and_is_a_valid_adaptive_card() {
        for kind in every_kind() {
            let n = note(kind, "A title", "A summary", Some("/swimlanes/sit1"));
            let c = card(&n, Some(ORIGIN));
            assert_eq!(c["type"], "AdaptiveCard", "{kind:?}");
            assert_eq!(c["version"], "1.4", "{kind:?}");
            assert!(c["body"].as_array().is_some_and(|b| !b.is_empty()), "{kind:?}");
            assert!(!headline(&n).is_empty(), "{kind:?} has a headline");
            let m = message(&n, Some(ORIGIN));
            assert_eq!(m["type"], "message");
            assert_eq!(
                m["attachments"][0]["contentType"],
                "application/vnd.microsoft.card.adaptive"
            );
            assert_eq!(m["attachments"][0]["content"], c);
        }
    }

    #[test]
    fn each_kind_has_its_own_headline() {
        let mut seen = std::collections::BTreeSet::new();
        for kind in every_kind() {
            let n = note(kind, "Neutral title", "s", None);
            assert!(seen.insert(headline(&n)), "{kind:?} repeats a headline");
        }
    }

    #[test]
    fn critical_findings_are_told_apart_by_title() {
        let h = |title: &str| headline(&note(NotificationKind::CriticalFinding, title, "s", None));
        assert_eq!(h("Overwritten change in sit1"), "Overwritten change");
        assert_eq!(h("overwritten change"), "Overwritten change");
        assert_eq!(h("Audit chain broken"), "Audit chain alert");
        assert_eq!(h("Something else entirely"), "Critical finding");
        // The title decides only for critical findings.
        let other = note(NotificationKind::PrRaised, "Audit chain broken", "s", None);
        assert_eq!(headline(&other), "Pull request raised");
    }

    #[test]
    fn a_card_holds_the_notification_and_fixed_words_and_nothing_else() {
        for kind in every_kind() {
            let n = note(kind, "T-marker", "S-marker", Some("/proposals/7"));
            let mut all = Vec::new();
            strings(&card(&n, Some(ORIGIN)), &mut all);
            let allowed_fixed = [
                "AdaptiveCard",
                "http://adaptivecards.io/schemas/adaptive-card.json",
                "1.4",
                "TextBlock",
                "FactSet",
                "Action.OpenUrl",
                "Bolder",
                "Medium",
                "Small",
                "Full",
                "Severity",
                "Swimlane",
                "High",
                "sit1",
                "Open in Lanekeeper",
                "Attention",
                "Default",
                "Accent",
                "Warning",
                "Good",
                "T-marker",
                "S-marker",
                "https://lanekeeper.example.test/proposals/7",
            ];
            for s in all {
                assert!(
                    allowed_fixed.contains(&s.as_str()) || s == headline(&n),
                    "{kind:?}: the card holds an unexpected string {s:?}"
                );
            }
        }
    }

    #[test]
    fn a_card_stays_far_below_the_teams_limit_at_the_largest_input() {
        let big = "x".repeat(256);
        let escaped_worst = "*".repeat(256);
        for kind in every_kind() {
            for text in [&big, &escaped_worst] {
                let n = note(kind, text, text, Some(&format!("/{}", "a".repeat(255))));
                let bytes = serde_json::to_vec(&message(&n, Some(ORIGIN))).unwrap();
                assert!(
                    bytes.len() < MAX_CARD_BYTES / 4,
                    "{kind:?}: {} bytes",
                    bytes.len()
                );
            }
        }
    }

    #[test]
    fn markdown_in_a_name_is_shown_as_typed() {
        let n = note(
            NotificationKind::AccessRequested,
            "Access requested",
            "[click here](https://evil.example/x) <b>bold</b> *a* _b_ `c` ~d~ \\e",
            None,
        );
        let c = card(&n, None);
        let text = c["body"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("](https"), "{text}");
        assert!(!text.contains("<b>"), "{text}");
        assert!(
            text.contains("\\[click here\\]\\(https://evil.example/x\\)"),
            "{text}"
        );
        for raw in ["*a*", "_b_", "`c`", "~d~"] {
            assert!(!text.contains(raw), "{raw} not escaped in {text}");
        }
    }

    #[test]
    fn escape_escapes_the_markdown_characters_and_the_backslash_first() {
        assert_eq!(escape("plain text 1-2."), "plain text 1-2.");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("[x](y)"), "\\[x\\]\\(y\\)");
        assert_eq!(escape("*_`~<>"), "\\*\\_\\`\\~\\<\\>");
        // Escaping twice does not undo itself: the output of one pass is plain text for the next.
        assert_eq!(escape(&escape("a_b")), "a\\\\\\_b");
    }

    #[test]
    fn only_a_plain_path_of_the_web_app_becomes_a_button() {
        let url = |origin: Option<&str>, link: &str| link_url(origin, Some(link));
        assert_eq!(
            url(Some(ORIGIN), "/swimlanes/sit1").as_deref(),
            Some("https://lanekeeper.example.test/swimlanes/sit1")
        );
        assert_eq!(
            url(Some(ORIGIN), "/proposals/7?tab=diff").as_deref(),
            Some("https://lanekeeper.example.test/proposals/7?tab=diff")
        );
        // No origin: no button.
        assert_eq!(url(None, "/swimlanes/sit1"), None);
        assert_eq!(link_url(Some(ORIGIN), None), None);
        for bad in [
            "//evil.example/x",
            "https://evil.example/x",
            "javascript:alert(1)",
            "swimlanes/sit1",
            "",
            "/a\\b",
            "/a b",
            "/a\u{7f}b",
            "/../../x@evil.example",
            "/\\evil.example",
            "/a#frag@evil",
        ] {
            assert_eq!(url(Some(ORIGIN), bad), None, "{bad:?} must not become a button");
        }
    }

    #[test]
    fn the_button_is_present_only_with_a_valid_link_and_an_origin() {
        let with = card(
            &note(NotificationKind::PrRaised, "t", "s", Some("/prs/1")),
            Some(ORIGIN),
        );
        assert_eq!(with["actions"][0]["type"], "Action.OpenUrl");
        assert_eq!(with["actions"][0]["url"], "https://lanekeeper.example.test/prs/1");
        for c in [
            card(&note(NotificationKind::PrRaised, "t", "s", Some("/prs/1")), None),
            card(
                &note(NotificationKind::PrRaised, "t", "s", Some("//evil.example")),
                Some(ORIGIN),
            ),
            card(&note(NotificationKind::PrRaised, "t", "s", None), Some(ORIGIN)),
        ] {
            assert!(c.get("actions").is_none(), "{c}");
        }
    }

    #[test]
    fn severity_sets_the_colour_of_the_title() {
        let colour = |s: Severity| {
            let mut n = note(NotificationKind::DriftDetected, "t", "s", None);
            n.severity = s;
            card(&n, None)["body"][1]["color"].as_str().unwrap().to_owned()
        };
        assert_eq!(colour(Severity::Critical), "Attention");
        assert_eq!(colour(Severity::High), "Attention");
        assert_eq!(colour(Severity::Medium), "Warning");
        assert_eq!(colour(Severity::Low), "Default");
    }

    #[test]
    fn a_notification_without_a_swimlane_has_no_swimlane_fact() {
        let mut n = note(NotificationKind::AccessRequested, "t", "s", None);
        n.swimlane = None;
        let c = card(&n, None);
        let facts = c["body"][3]["facts"].as_array().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["title"], "Severity");
    }

    #[test]
    fn golden_cards() {
        for kind in every_kind() {
            let (title, summary) = match kind {
                NotificationKind::CriticalFinding => {
                    ("Audit chain broken", "The chain does not verify at event 120")
                }
                _ => ("Title of the event", "Two files changed in sit1 by Ada Lovelace"),
            };
            let n = note(kind, title, summary, Some("/swimlanes/sit1"));
            insta::assert_json_snapshot!(format!("card_{kind:?}"), message(&n, Some(ORIGIN)));
        }
        let finding = note(
            NotificationKind::CriticalFinding,
            "Overwritten change in sit1",
            "3 files changed on NFS were overwritten by a sync",
            Some("/swimlanes/sit1/findings"),
        );
        insta::assert_json_snapshot!("card_overwritten_change", message(&finding, Some(ORIGIN)));
    }
}
